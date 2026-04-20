//! Watchpoint engine — fires actions on memory access to watched address ranges.

use std::ops::Range;
#[cfg(feature = "instrumentation")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "instrumentation")]
use crate::state::NativeTriggerState;
use crate::state::WatchpointView;

/// Action to take when a watchpoint fires.
#[derive(Debug, Clone)]
pub enum WatchAction {
    /// Break execution.
    Break,
    /// Log the access.
    Log,
    /// Custom callback (identified by ID for Python bridge).
    Callback(u64),
}

/// What types of access trigger the watchpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchKind {
    Read,
    Write,
    ReadWrite,
}

/// A memory watchpoint.
#[derive(Debug, Clone)]
pub struct Watchpoint {
    pub id: u32,
    pub range: Range<u64>,
    pub kind: WatchKind,
    pub action: WatchAction,
    pub enabled: bool,
}

/// Checkpoint-friendly watchpoint intent without runtime-assigned IDs.
#[derive(Debug, Clone)]
pub struct WatchpointIntent {
    pub start: u64,
    pub size: u64,
    pub kind: WatchKind,
    pub action: WatchAction,
    pub enabled: bool,
}

/// Result of checking a memory access against watchpoints.
#[derive(Debug)]
pub enum WatchResult {
    None,
    Hit {
        watchpoint_id: u32,
        addr: u64,
        size: usize,
        is_store: bool,
        action: WatchAction,
    },
}

/// Engine that manages watchpoints and checks memory accesses.
///
/// Watchpoints are kept sorted by `range.start` for early-exit optimization:
/// once a watchpoint's start exceeds the access end, no further watchpoints
/// can overlap.
pub struct WatchpointEngine {
    watchpoints: Vec<Watchpoint>,
    next_id: u32,
}

impl WatchAction {
    pub fn checkpoint_fields(&self) -> (u64, u64) {
        match self {
            Self::Break => (0, 0),
            Self::Log => (1, 0),
            Self::Callback(id) => (2, *id),
        }
    }

    pub fn from_checkpoint_fields(kind: u64, arg: u64) -> Self {
        match kind {
            1 => Self::Log,
            2 => Self::Callback(arg),
            _ => Self::Break,
        }
    }
}

impl WatchKind {
    pub fn checkpoint_value(self) -> u64 {
        match self {
            Self::Read => 0,
            Self::Write => 1,
            Self::ReadWrite => 2,
        }
    }

    pub fn from_checkpoint_value(value: u64) -> Self {
        match value {
            0 => Self::Read,
            2 => Self::ReadWrite,
            _ => Self::Write,
        }
    }
}

impl WatchpointEngine {
    pub fn new() -> Self {
        Self {
            watchpoints: Vec::new(),
            next_id: 0,
        }
    }

    pub fn add(&mut self, start: u64, size: u64, kind: WatchKind, action: WatchAction) -> u32 {
        self.add_with_state(start, size, kind, action, true)
    }

    pub fn add_with_state(
        &mut self,
        start: u64,
        size: u64,
        kind: WatchKind,
        action: WatchAction,
        enabled: bool,
    ) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let wp = Watchpoint {
            id,
            range: start..start + size,
            kind,
            action,
            enabled,
        };
        let pos = self.watchpoints.partition_point(|w| w.range.start < start);
        self.watchpoints.insert(pos, wp);
        id
    }

    pub fn remove(&mut self, id: u32) -> bool {
        if let Some(pos) = self.watchpoints.iter().position(|w| w.id == id) {
            self.watchpoints.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn set_enabled(&mut self, id: u32, enabled: bool) -> bool {
        if let Some(wp) = self.watchpoints.iter_mut().find(|w| w.id == id) {
            wp.enabled = enabled;
            true
        } else {
            false
        }
    }

    pub fn check(&self, addr: u64, size: usize, is_store: bool) -> WatchResult {
        let access_end = addr + size as u64;
        for wp in &self.watchpoints {
            // Sorted by start: if start >= access_end, no more can overlap.
            if wp.range.start >= access_end {
                break;
            }
            if !wp.enabled {
                continue;
            }
            let kind_match = match wp.kind {
                WatchKind::Read => !is_store,
                WatchKind::Write => is_store,
                WatchKind::ReadWrite => true,
            };
            if !kind_match {
                continue;
            }
            if addr < wp.range.end {
                return WatchResult::Hit {
                    watchpoint_id: wp.id,
                    addr,
                    size,
                    is_store,
                    action: wp.action.clone(),
                };
            }
        }
        WatchResult::None
    }

    pub fn count(&self) -> usize {
        self.watchpoints.len()
    }
    pub fn clear(&mut self) {
        self.watchpoints.clear();
    }
    pub fn list(&self) -> &[Watchpoint] {
        &self.watchpoints
    }
    pub fn get(&self, id: u32) -> Option<&Watchpoint> {
        self.watchpoints.iter().find(|w| w.id == id)
    }

    pub fn snapshot_intent(&self) -> Vec<WatchpointIntent> {
        self.watchpoints
            .iter()
            .map(|wp| WatchpointIntent {
                start: wp.range.start,
                size: wp.range.end - wp.range.start,
                kind: wp.kind,
                action: wp.action.clone(),
                enabled: wp.enabled,
            })
            .collect()
    }

    pub fn views(&self) -> Vec<WatchpointView> {
        self.watchpoints
            .iter()
            .map(|wp| WatchpointView {
                id: wp.id,
                start: wp.range.start,
                size: wp.range.end - wp.range.start,
                kind: match wp.kind {
                    WatchKind::Read => "read",
                    WatchKind::Write => "write",
                    WatchKind::ReadWrite => "rw",
                }
                .to_string(),
                action: match wp.action {
                    WatchAction::Break => "break",
                    WatchAction::Log => "log",
                    WatchAction::Callback(_) => "callback",
                }
                .to_string(),
                enabled: wp.enabled,
            })
            .collect()
    }

    pub fn restore_intent(&mut self, intents: &[WatchpointIntent]) {
        self.clear();
        for intent in intents {
            self.add_with_state(
                intent.start,
                intent.size,
                intent.kind,
                intent.action.clone(),
                intent.enabled,
            );
        }
    }
}

#[cfg(feature = "instrumentation")]
pub fn attach_watchpoint_engine<F>(
    probes: &mut helm_probe::CpuProbes,
    trigger_state: Arc<Mutex<NativeTriggerState>>,
    on_hit: F,
) -> Arc<Mutex<WatchpointEngine>>
where
    F: Fn(WatchResult) + Send + Sync + 'static,
{
    let engine = Arc::new(Mutex::new(WatchpointEngine::new()));
    let probe_engine = Arc::clone(&engine);
    let on_hit = Arc::new(on_hit);
    probes.mem.subscribe(move |event| {
        if let Ok(guard) = probe_engine.lock() {
            match guard.check(event.addr, usize::from(event.size), event.is_store) {
                WatchResult::Hit {
                    watchpoint_id,
                    addr,
                    size,
                    is_store,
                    action,
                } => {
                    if let Ok(mut state) = trigger_state.lock() {
                        state.note_watchpoint_hit(
                            watchpoint_id,
                            addr,
                            size as u64,
                            is_store,
                            match &action {
                                WatchAction::Break => "break",
                                WatchAction::Log => "log",
                                WatchAction::Callback(_) => "callback",
                            },
                        );
                    }
                    on_hit(WatchResult::Hit {
                        watchpoint_id,
                        addr,
                        size,
                        is_store,
                        action,
                    })
                }
                WatchResult::None => {}
            }
        }
    });
    engine
}

impl Default for WatchpointEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_watchpoint_hit() {
        let mut e = WatchpointEngine::new();
        e.add(0x1000, 0x100, WatchKind::Write, WatchAction::Break);
        assert!(matches!(e.check(0x1050, 4, true), WatchResult::Hit { .. }));
        assert!(matches!(e.check(0x1050, 4, false), WatchResult::None));
    }

    #[test]
    fn disabled_skipped() {
        let mut e = WatchpointEngine::new();
        let id = e.add(0x1000, 4, WatchKind::ReadWrite, WatchAction::Break);
        e.set_enabled(id, false);
        assert!(matches!(e.check(0x1000, 4, true), WatchResult::None));
    }

    #[test]
    fn remove_works() {
        let mut e = WatchpointEngine::new();
        let id = e.add(0x1000, 4, WatchKind::ReadWrite, WatchAction::Break);
        assert!(e.remove(id));
        assert_eq!(e.count(), 0);
    }

    #[test]
    fn snapshot_restore_preserves_watchpoint_intent() {
        let mut source = WatchpointEngine::new();
        let id = source.add(0x1000, 16, WatchKind::ReadWrite, WatchAction::Log);
        source.set_enabled(id, false);

        let intents = source.snapshot_intent();
        let mut restored = WatchpointEngine::new();
        restored.restore_intent(&intents);

        let wp = &restored.list()[0];
        assert_eq!(wp.range.start, 0x1000);
        assert_eq!(wp.range.end - wp.range.start, 16);
        assert!(!wp.enabled);
        assert!(matches!(wp.kind, WatchKind::ReadWrite));
        assert!(matches!(wp.action, WatchAction::Log));
    }

    #[cfg(feature = "instrumentation")]
    #[test]
    fn attached_engine_receives_probe_hits() {
        let mut probes = helm_probe::CpuProbes::default();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&hits);
        let engine =
            attach_watchpoint_engine(&mut probes, NativeTriggerState::shared(), move |result| {
                if let WatchResult::Hit { addr, .. } = result {
                    seen.lock().unwrap().push(addr);
                }
            });
        engine
            .lock()
            .unwrap()
            .add(0x2000, 8, WatchKind::Write, WatchAction::Break);

        probes.mem.notify(&helm_probe::MemAccessEvent {
            addr: 0x2000,
            size: 4,
            is_store: true,
            pc: 0x1000,
        });

        assert_eq!(hits.lock().unwrap().as_slice(), &[0x2000]);
    }
}

//! Watchpoint engine — fires actions on memory access to watched address ranges.

use std::ops::Range;

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

impl WatchpointEngine {
    pub fn new() -> Self {
        Self {
            watchpoints: Vec::new(),
            next_id: 0,
        }
    }

    pub fn add(&mut self, start: u64, size: u64, kind: WatchKind, action: WatchAction) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let wp = Watchpoint {
            id,
            range: start..start + size,
            kind,
            action,
            enabled: true,
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
    pub fn list(&self) -> &[Watchpoint] {
        &self.watchpoints
    }
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
}

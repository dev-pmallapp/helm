//! Shared debug-state views for control surfaces.

#[cfg(feature = "instrumentation")]
use std::sync::{Arc, Mutex};

use crate::{BreakpointEngine, BreakpointIntent, WatchpointEngine, WatchpointIntent};

/// A control-surface-friendly breakpoint view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakpointView {
    pub id: u32,
    pub addr: u64,
    pub action: String,
    pub enabled: bool,
    pub hit_count: u64,
}

/// A control-surface-friendly watchpoint view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchpointView {
    pub id: u32,
    pub start: u64,
    pub size: u64,
    pub kind: String,
    pub action: String,
    pub enabled: bool,
}

/// Current native debug trigger state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DebugStateSnapshot {
    pub breakpoints: Vec<BreakpointView>,
    pub watchpoints: Vec<WatchpointView>,
}

/// Last native breakpoint hit details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakpointHitView {
    pub breakpoint_id: u32,
    pub addr: u64,
    pub action: String,
}

/// Last native watchpoint hit details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchpointHitView {
    pub watchpoint_id: u32,
    pub addr: u64,
    pub size: u64,
    pub access: String,
    pub action: String,
}

/// Native debug trigger hit details captured during execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeTriggerHitView {
    Breakpoint(BreakpointHitView),
    Watchpoint(WatchpointHitView),
}

/// Generic runtime stop view for user-facing rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStopView {
    Exit { code: i32 },
    Quantum,
    Exception(String),
    Unsupported,
    JitBreakpoint { pc: Option<u64> },
    ErrorNotInstantiated,
}

/// Structured control-plane stop state for the most recent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeStopState {
    pub stop: RuntimeStopView,
    pub last_native_hit: Option<NativeTriggerHitView>,
}

impl Default for RuntimeStopState {
    fn default() -> Self {
        Self {
            stop: RuntimeStopView::ErrorNotInstantiated,
            last_native_hit: None,
        }
    }
}

/// Shared recorder for native trigger hits that occur during a run.
#[cfg(feature = "instrumentation")]
#[derive(Debug, Default)]
pub struct NativeTriggerState {
    last_hit: Option<NativeTriggerHitView>,
}

impl RuntimeStopView {
    pub fn render(&self) -> String {
        match self {
            Self::Exit { code } => format!("exit:{code}"),
            Self::Quantum => "quantum".to_string(),
            Self::Exception(err) => format!("exception:{err}"),
            Self::Unsupported => "unsupported".to_string(),
            Self::JitBreakpoint { .. } => "breakpoint".to_string(),
            Self::ErrorNotInstantiated => "error:not_instantiated".to_string(),
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Exit { .. } => "exit",
            Self::Quantum => "quantum",
            Self::Exception(_) => "exception",
            Self::Unsupported => "unsupported",
            Self::JitBreakpoint { .. } => "jit_breakpoint",
            Self::ErrorNotInstantiated => "error_not_instantiated",
        }
    }
}

impl DebugStateSnapshot {
    pub fn capture(
        breakpoints: Option<&BreakpointEngine>,
        watchpoints: Option<&WatchpointEngine>,
    ) -> Self {
        Self {
            breakpoints: breakpoints.map(BreakpointEngine::views).unwrap_or_default(),
            watchpoints: watchpoints.map(WatchpointEngine::views).unwrap_or_default(),
        }
    }
}

impl RuntimeStopState {
    pub fn render(&self) -> String {
        self.stop.render()
    }
}

impl NativeTriggerHitView {
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Breakpoint(_) => "native_breakpoint",
            Self::Watchpoint(_) => "native_watchpoint",
        }
    }
}

#[cfg(feature = "instrumentation")]
impl NativeTriggerState {
    pub fn shared() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::default()))
    }

    pub fn clear(&mut self) {
        self.last_hit = None;
    }

    pub fn snapshot(&self) -> Option<NativeTriggerHitView> {
        self.last_hit.clone()
    }

    pub fn note_breakpoint_hit(&mut self, breakpoint_id: u32, addr: u64, action: &str) {
        self.last_hit = Some(NativeTriggerHitView::Breakpoint(BreakpointHitView {
            breakpoint_id,
            addr,
            action: action.to_string(),
        }));
    }

    pub fn note_watchpoint_hit(
        &mut self,
        watchpoint_id: u32,
        addr: u64,
        size: u64,
        is_store: bool,
        action: &str,
    ) {
        self.last_hit = Some(NativeTriggerHitView::Watchpoint(WatchpointHitView {
            watchpoint_id,
            addr,
            size,
            access: if is_store { "write" } else { "read" }.to_string(),
            action: action.to_string(),
        }));
    }
}

impl BreakpointIntent {
    pub fn action_label(&self) -> &'static str {
        match self.action {
            crate::BreakAction::Break => "break",
            crate::BreakAction::Log => "log",
            crate::BreakAction::Callback(_) => "callback",
        }
    }
}

impl WatchpointIntent {
    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            crate::WatchKind::Read => "read",
            crate::WatchKind::Write => "write",
            crate::WatchKind::ReadWrite => "rw",
        }
    }

    pub fn action_label(&self) -> &'static str {
        match self.action {
            crate::WatchAction::Break => "break",
            crate::WatchAction::Log => "log",
            crate::WatchAction::Callback(_) => "callback",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BreakAction, BreakpointEngine, WatchAction, WatchKind, WatchpointEngine};

    #[test]
    fn runtime_stop_view_renders_expected_strings() {
        assert_eq!(RuntimeStopView::Quantum.render(), "quantum");
        assert_eq!(
            RuntimeStopView::JitBreakpoint { pc: Some(0x1000) }.render(),
            "breakpoint"
        );
        assert_eq!(
            RuntimeStopView::Exception("fault".to_string()).render(),
            "exception:fault"
        );
        assert_eq!(RuntimeStopView::Exit { code: 7 }.render(), "exit:7");
    }

    #[test]
    fn debug_state_snapshot_captures_trigger_views() {
        let mut breakpoints = BreakpointEngine::new();
        let id = breakpoints.add(0x1000, BreakAction::Log);
        breakpoints.check(0x1000);
        breakpoints.set_enabled(id, false);

        let mut watchpoints = WatchpointEngine::new();
        watchpoints.add(0x2000, 8, WatchKind::ReadWrite, WatchAction::Break);

        let snapshot = DebugStateSnapshot::capture(Some(&breakpoints), Some(&watchpoints));
        assert_eq!(snapshot.breakpoints.len(), 1);
        assert_eq!(snapshot.breakpoints[0].action, "log");
        assert!(!snapshot.breakpoints[0].enabled);
        assert_eq!(snapshot.breakpoints[0].hit_count, 1);
        assert_eq!(snapshot.watchpoints.len(), 1);
        assert_eq!(snapshot.watchpoints[0].kind, "rw");
        assert_eq!(snapshot.watchpoints[0].action, "break");
    }

    #[cfg(feature = "instrumentation")]
    #[test]
    fn native_trigger_state_tracks_last_hit() {
        let mut state = NativeTriggerState::default();
        state.note_breakpoint_hit(1, 0x1000, "log");
        assert!(matches!(
            state.snapshot(),
            Some(NativeTriggerHitView::Breakpoint(BreakpointHitView {
                breakpoint_id: 1,
                addr: 0x1000,
                ..
            }))
        ));

        state.note_watchpoint_hit(2, 0x2000, 8, true, "break");
        assert!(matches!(
            state.snapshot(),
            Some(NativeTriggerHitView::Watchpoint(WatchpointHitView {
                watchpoint_id: 2,
                addr: 0x2000,
                size: 8,
                ..
            }))
        ));
    }
}

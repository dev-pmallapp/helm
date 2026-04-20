//! Shared debug-state views for control surfaces.

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

/// Generic runtime stop view for user-facing rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStopView {
    Exit { code: i32 },
    Quantum,
    Exception(String),
    Unsupported,
    Breakpoint,
    ErrorNotInstantiated,
}

impl RuntimeStopView {
    pub fn render(&self) -> String {
        match self {
            Self::Exit { code } => format!("exit:{code}"),
            Self::Quantum => "quantum".to_string(),
            Self::Exception(err) => format!("exception:{err}"),
            Self::Unsupported => "unsupported".to_string(),
            Self::Breakpoint => "breakpoint".to_string(),
            Self::ErrorNotInstantiated => "error:not_instantiated".to_string(),
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
        assert_eq!(RuntimeStopView::Breakpoint.render(), "breakpoint");
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
}

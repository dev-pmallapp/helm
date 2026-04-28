//! Backend-agnostic JIT debug/trace controller.
//!
//! Provides the same debug primitives the interpreter path offers (breakpoints,
//! trace windows gated by PC range or instruction count, per-block callbacks)
//! while operating at block granularity.
//!
//! # Integration
//!
//! The controller works at the *dispatch* level -- between the JIT cache lookup
//! and compiled-block execution -- so it is independent of which backend
//! (dynasm, stencil, cranelift, ...) produced the code.
//!
//! It complements the existing `helm-probe` / `helm-plugin` stacks:
//! - **Block-level observability** is handled here via `JitProbes`.
//! - **Per-instruction observability** requires interpreter fallback: when
//!   `force_interpreter` is true or plugin/probe callbacks are active, the
//!   dispatch loop should hand off to the interpreter for the affected block.
//!
//! # Trace window
//!
//! A *trace window* gates event emission to a (start, stop) range defined by
//! guest PC and/or retired instruction count. Outside the window, probe
//! delivery and per-block logging are skipped. This mirrors the interpreter's
//! `pc_start` / `pc_end` / `max` arguments on the `execlog` plugin.

use std::collections::HashSet;

// ── Trace window ────────────────────────────────────────────────────────────

/// Defines when JIT trace/debug events are emitted.
///
/// All fields are optional. An unset field means "no constraint on that axis".
/// The window is *active* when all set start conditions have been met and no
/// stop condition has fired yet.
#[derive(Debug, Clone, Default)]
pub struct JitTraceWindow {
    /// Start emitting events when PC reaches this address.
    pub start_pc: Option<u64>,
    /// Start emitting events after this many guest instructions have retired.
    pub start_insn: Option<u64>,
    /// Stop emitting events when PC reaches this address.
    pub stop_pc: Option<u64>,
    /// Stop emitting events after this many guest instructions have retired.
    pub stop_insn: Option<u64>,
    /// Maximum number of block-execute events to emit (then auto-close).
    pub max_events: Option<u64>,
}

/// Runtime state for an active trace window.
#[derive(Debug, Clone)]
struct WindowState {
    window: JitTraceWindow,
    /// Whether the window has been opened (all start conditions met).
    opened: bool,
    /// Whether the window has been permanently closed (a stop condition fired).
    closed: bool,
    /// Number of block-execute events emitted so far.
    events_emitted: u64,
}

impl WindowState {
    fn new(window: JitTraceWindow) -> Self {
        // If no start conditions, the window is immediately open.
        let opened = window.start_pc.is_none() && window.start_insn.is_none();
        Self {
            window,
            opened,
            closed: false,
            events_emitted: 0,
        }
    }

    /// Check whether the window is currently active for the given state.
    fn is_active(&mut self, pc: u64, insns_retired: u64) -> bool {
        if self.closed {
            return false;
        }
        // Try to open.
        if !self.opened {
            let pc_ok = self.window.start_pc.map_or(true, |spc| pc >= spc);
            let insn_ok = self
                .window
                .start_insn
                .map_or(true, |si| insns_retired >= si);
            if pc_ok && insn_ok {
                self.opened = true;
            }
        }
        if !self.opened {
            return false;
        }
        // Check stop conditions.
        if self.window.stop_pc.is_some_and(|spc| pc >= spc) {
            self.closed = true;
            return false;
        }
        if self.window.stop_insn.is_some_and(|si| insns_retired >= si) {
            self.closed = true;
            return false;
        }
        if self
            .window
            .max_events
            .is_some_and(|m| self.events_emitted >= m)
        {
            self.closed = true;
            return false;
        }
        true
    }

    fn note_event(&mut self) {
        self.events_emitted += 1;
    }
}

// ── Debug controller ────────────────────────────────────────────────────────

/// Outcome of checking whether dispatch should proceed for a given PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchDecision {
    /// Execute the compiled block normally.
    Execute,
    /// A breakpoint matched -- the dispatch loop should stop.
    Breakpoint,
    /// Debug policy requires interpreter fallback for this block (e.g.
    /// per-instruction plugin callbacks are active, or `force_interpreter`
    /// is set).
    FallbackToInterpreter,
}

/// Backend-agnostic controller for JIT-level debug and trace features.
///
/// # Usage
///
/// Created once and stored alongside the JIT cache. The dispatch loop calls
/// `on_block_entry` before executing each compiled block to decide whether
/// to execute, break, or fall back to the interpreter. After execution,
/// `on_block_exit` is called to update counters and window state.
///
/// The controller does not own probes or plugins -- it *advises* the dispatch
/// loop, which is responsible for firing `JitProbes` events and plugin
/// callbacks as appropriate.
pub struct JitDebugController {
    /// PC breakpoints -- O(1) check via `HashSet`.
    breakpoints: HashSet<u64>,
    /// Active trace window (if any).
    trace_window: Option<WindowState>,
    /// When true, every block dispatches to the interpreter instead of
    /// running compiled code. Useful when per-instruction plugin callbacks
    /// are subscribed.
    pub force_interpreter: bool,
    /// When true, each JIT block is verified against the interpreter by
    /// snapshotting state, re-running via interpreter, and comparing.
    pub verify: bool,
    /// Cumulative guest instructions retired through the JIT path.
    /// Updated by `on_block_exit`.
    insns_retired: u64,
    /// Whether the trace window is currently active (cached for fast path).
    window_active: bool,
}

impl JitDebugController {
    /// Create a new controller with no breakpoints and no trace window.
    pub fn new() -> Self {
        Self {
            breakpoints: HashSet::new(),
            trace_window: None,
            force_interpreter: false,
            verify: false,
            insns_retired: 0,
            window_active: false,
        }
    }

    // ── Breakpoints ─────────────────────────────────────────────────────

    /// Add a PC breakpoint. Returns `true` if newly inserted.
    pub fn add_breakpoint(&mut self, pc: u64) -> bool {
        self.breakpoints.insert(pc)
    }

    /// Remove a PC breakpoint. Returns `true` if it existed.
    pub fn remove_breakpoint(&mut self, pc: u64) -> bool {
        self.breakpoints.remove(&pc)
    }

    /// Remove all breakpoints.
    pub fn clear_breakpoints(&mut self) {
        self.breakpoints.clear();
    }

    /// Returns `true` if `pc` is a breakpoint address.
    #[inline]
    pub fn has_breakpoint(&self, pc: u64) -> bool {
        !self.breakpoints.is_empty() && self.breakpoints.contains(&pc)
    }

    /// Number of active breakpoints.
    pub fn breakpoint_count(&self) -> usize {
        self.breakpoints.len()
    }

    /// Iterator over all breakpoint addresses.
    pub fn breakpoints(&self) -> impl Iterator<Item = u64> + '_ {
        self.breakpoints.iter().copied()
    }

    // ── Trace window ────────────────────────────────────────────────────

    /// Set or replace the trace window.
    pub fn set_trace_window(&mut self, window: JitTraceWindow) {
        self.trace_window = Some(WindowState::new(window));
        self.window_active = false;
    }

    /// Remove the trace window (events will always be emitted if probes are
    /// subscribed).
    pub fn clear_trace_window(&mut self) {
        self.trace_window = None;
        self.window_active = false;
    }

    /// Returns `true` if a trace window has been configured.
    pub fn has_trace_window(&self) -> bool {
        self.trace_window.is_some()
    }

    /// Returns `true` if the trace window is currently active (events should
    /// be emitted).
    ///
    /// When no trace window is set, this returns `true` unconditionally
    /// (all events pass through).
    #[inline]
    pub fn is_window_active(&self) -> bool {
        self.window_active
    }

    // ── Instruction counting ────────────────────────────────────────────

    /// Total guest instructions retired through the JIT path.
    pub fn insns_retired(&self) -> u64 {
        self.insns_retired
    }

    /// Reset the instruction counter.
    pub fn reset_insns_retired(&mut self) {
        self.insns_retired = 0;
    }

    // ── Dispatch hooks ──────────────────────────────────────────────────

    /// Called before dispatching a compiled block at `pc`.
    ///
    /// Returns a `DispatchDecision` telling the dispatch loop what to do.
    /// The caller is responsible for acting on the decision.
    #[inline]
    pub fn on_block_entry(&mut self, pc: u64) -> DispatchDecision {
        if self.has_breakpoint(pc) {
            return DispatchDecision::Breakpoint;
        }
        if self.force_interpreter {
            return DispatchDecision::FallbackToInterpreter;
        }
        // Update window state.
        self.window_active = match &mut self.trace_window {
            Some(ws) => ws.is_active(pc, self.insns_retired),
            None => true,
        };
        DispatchDecision::Execute
    }

    /// Called after a compiled block finishes. Updates instruction counter
    /// and trace window state.
    ///
    /// Returns `true` if the trace window is active (the caller should emit
    /// a `JitBlockExecuteEvent`).
    pub fn on_block_exit(&mut self, _pc: u64, insns_retired: u32) -> bool {
        self.insns_retired = self.insns_retired.saturating_add(u64::from(insns_retired));
        if let Some(ws) = &mut self.trace_window {
            if ws.is_active(_pc, self.insns_retired) {
                ws.note_event();
                return true;
            }
            return false;
        }
        true
    }

    /// Maximum number of guest instructions a block starting at `pc` may
    /// contain before it *must* end.
    ///
    /// Used to truncate block compilation at the next breakpoint address
    /// so that the breakpoint can fire at block boundaries.
    ///
    /// Returns `None` if there is no constraint (the backend's default
    /// block-size limit applies).
    pub fn max_block_insns(&self, start_pc: u64) -> Option<u32> {
        if self.breakpoints.is_empty() {
            return None;
        }
        // Find the nearest breakpoint strictly after start_pc.
        // AArch64 instructions are 4-byte aligned.
        let mut nearest: Option<u64> = None;
        for &bp in &self.breakpoints {
            if bp > start_pc {
                nearest = Some(match nearest {
                    Some(prev) => prev.min(bp),
                    None => bp,
                });
            }
        }
        nearest.map(|bp_pc| {
            let delta_bytes = bp_pc.saturating_sub(start_pc);
            // Each AArch64 instruction is 4 bytes.
            (delta_bytes / 4) as u32
        })
    }

    /// Returns `true` if any debug feature is active (breakpoints, trace
    /// window, or force-interpreter). The dispatch loop can skip debug
    /// checks entirely when this is `false`.
    #[inline]
    pub fn is_active(&self) -> bool {
        !self.breakpoints.is_empty() || self.trace_window.is_some() || self.force_interpreter || self.verify
    }
}

impl Default for JitDebugController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_debug_features_returns_execute() {
        let mut ctrl = JitDebugController::new();
        assert!(!ctrl.is_active());
        assert_eq!(ctrl.on_block_entry(0x1000), DispatchDecision::Execute);
        assert!(ctrl.is_window_active());
    }

    #[test]
    fn breakpoint_fires_at_exact_pc() {
        let mut ctrl = JitDebugController::new();
        ctrl.add_breakpoint(0x2000);
        assert!(ctrl.is_active());
        assert_eq!(ctrl.on_block_entry(0x1000), DispatchDecision::Execute);
        assert_eq!(ctrl.on_block_entry(0x2000), DispatchDecision::Breakpoint);
    }

    #[test]
    fn remove_breakpoint() {
        let mut ctrl = JitDebugController::new();
        ctrl.add_breakpoint(0x2000);
        assert!(ctrl.remove_breakpoint(0x2000));
        assert_eq!(ctrl.on_block_entry(0x2000), DispatchDecision::Execute);
    }

    #[test]
    fn force_interpreter_overrides_execute() {
        let mut ctrl = JitDebugController::new();
        ctrl.force_interpreter = true;
        assert_eq!(
            ctrl.on_block_entry(0x1000),
            DispatchDecision::FallbackToInterpreter
        );
    }

    #[test]
    fn breakpoint_takes_priority_over_force_interpreter() {
        let mut ctrl = JitDebugController::new();
        ctrl.force_interpreter = true;
        ctrl.add_breakpoint(0x1000);
        assert_eq!(ctrl.on_block_entry(0x1000), DispatchDecision::Breakpoint);
    }

    #[test]
    fn trace_window_pc_range() {
        let mut ctrl = JitDebugController::new();
        ctrl.set_trace_window(JitTraceWindow {
            start_pc: Some(0x2000),
            stop_pc: Some(0x3000),
            ..Default::default()
        });
        assert!(ctrl.is_active());

        // Before window opens.
        ctrl.on_block_entry(0x1000);
        assert!(!ctrl.is_window_active());

        // Inside window.
        ctrl.on_block_entry(0x2000);
        assert!(ctrl.is_window_active());

        ctrl.on_block_entry(0x2800);
        assert!(ctrl.is_window_active());

        // Past stop PC.
        ctrl.on_block_entry(0x3000);
        assert!(!ctrl.is_window_active());

        // Stays closed.
        ctrl.on_block_entry(0x2500);
        assert!(!ctrl.is_window_active());
    }

    #[test]
    fn trace_window_insn_count_range() {
        let mut ctrl = JitDebugController::new();
        ctrl.set_trace_window(JitTraceWindow {
            start_insn: Some(10),
            stop_insn: Some(20),
            ..Default::default()
        });

        // Before threshold.
        ctrl.on_block_entry(0x1000);
        assert!(!ctrl.is_window_active());

        // Simulate 12 insns retired.
        ctrl.on_block_exit(0x1000, 12);
        ctrl.on_block_entry(0x1030);
        assert!(ctrl.is_window_active());

        // Simulate 10 more (total 22) -- past stop.
        ctrl.on_block_exit(0x1030, 10);
        ctrl.on_block_entry(0x1060);
        assert!(!ctrl.is_window_active());
    }

    #[test]
    fn trace_window_max_events() {
        let mut ctrl = JitDebugController::new();
        ctrl.set_trace_window(JitTraceWindow {
            max_events: Some(3),
            ..Default::default()
        });

        for i in 0..3 {
            ctrl.on_block_entry(0x1000 + i * 4);
            assert!(ctrl.is_window_active());
            assert!(ctrl.on_block_exit(0x1000 + i * 4, 1));
        }

        // 4th event: window closed.
        ctrl.on_block_entry(0x100c);
        assert!(!ctrl.is_window_active());
    }

    #[test]
    fn max_block_insns_truncates_at_breakpoint() {
        let mut ctrl = JitDebugController::new();
        ctrl.add_breakpoint(0x1010);
        ctrl.add_breakpoint(0x1020);

        // Block starting at 0x1000: nearest BP is 0x1010, delta=16 bytes = 4 insns.
        assert_eq!(ctrl.max_block_insns(0x1000), Some(4));

        // Block starting at 0x1010: nearest BP ahead is 0x1020, delta=16 bytes = 4 insns.
        assert_eq!(ctrl.max_block_insns(0x1010), Some(4));

        // No breakpoints ahead of 0x1020.
        assert_eq!(ctrl.max_block_insns(0x1020), None);
    }

    #[test]
    fn max_block_insns_none_without_breakpoints() {
        let ctrl = JitDebugController::new();
        assert_eq!(ctrl.max_block_insns(0x1000), None);
    }

    #[test]
    fn clear_trace_window_makes_all_events_pass() {
        let mut ctrl = JitDebugController::new();
        ctrl.set_trace_window(JitTraceWindow {
            start_pc: Some(0x9000),
            ..Default::default()
        });
        ctrl.on_block_entry(0x1000);
        assert!(!ctrl.is_window_active());

        ctrl.clear_trace_window();
        ctrl.on_block_entry(0x1000);
        assert!(ctrl.is_window_active());
    }

    #[test]
    fn verify_field_activates_controller() {
        let mut ctrl = JitDebugController::new();
        assert!(!ctrl.verify);
        assert!(!ctrl.is_active());
        ctrl.verify = true;
        assert!(ctrl.is_active());
        // verify does not change dispatch decision
        assert_eq!(ctrl.on_block_entry(0x1000), DispatchDecision::Execute);
    }

    #[test]
    fn insn_counter_tracks_across_blocks() {
        let mut ctrl = JitDebugController::new();
        assert_eq!(ctrl.insns_retired(), 0);
        ctrl.on_block_exit(0x1000, 5);
        ctrl.on_block_exit(0x1014, 3);
        assert_eq!(ctrl.insns_retired(), 8);
        ctrl.reset_insns_retired();
        assert_eq!(ctrl.insns_retired(), 0);
    }
}

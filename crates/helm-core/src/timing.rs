//! Timing backend trait — pluggable accuracy levels.

use crate::insn::{DecodedInsn, ExecOutcome};
use serde::{Deserialize, Serialize};

/// Simulation accuracy levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccuracyLevel {
    /// Functional Emulation: IPC=1, no memory modelling, maximum speed.
    FE,
    /// Interval-Timing Emulation: cache latencies, device stalls.
    ITE,
    /// Cycle-Accurate Emulation: full pipeline stages.
    CAE,
}

/// Pluggable timing strategy. The execution loop calls this after
/// every instruction (or after every JIT block for FE mode).
pub trait TimingBackend: Send + Sync {
    fn accuracy(&self) -> AccuracyLevel;

    /// Called after functional execution. Returns stall cycles.
    fn account(&mut self, insn: &DecodedInsn, outcome: &ExecOutcome) -> u64;

    /// End-of-quantum hook for temporal decoupling.
    fn end_of_quantum(&mut self) {}

    /// Reset internal state.
    fn reset(&mut self) {}
}

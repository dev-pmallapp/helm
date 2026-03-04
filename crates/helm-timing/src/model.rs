//! The TimingModel trait and accuracy-level enum.

use helm_core::ir::MicroOp;
use helm_core::types::Addr;

/// Accuracy level — determines which timing effects are modelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AccuracyLevel {
    /// IPC = 1, no memory modelling.
    Functional,
    /// Cache hit/miss latencies; device stall cycles.
    StallAnnotated,
    /// OoO pipeline, branch prediction, detailed caches.
    Microarchitectural,
    /// Cycle-by-cycle pipeline stages, bypass network, store buffer.
    CycleAccurate,
}

/// A pluggable timing model.  Attach one to a core to inject stall
/// cycles into the simulation.  Detach it to go back to functional mode.
pub trait TimingModel: Send + Sync {
    fn accuracy(&self) -> AccuracyLevel;

    /// How many cycles does this instruction cost?
    fn instruction_latency(&mut self, uop: &MicroOp) -> u64;

    /// How many stall cycles for a memory access?
    fn memory_latency(&mut self, addr: Addr, size: usize, is_write: bool) -> u64;

    /// Penalty for a branch misprediction (pipeline flush).
    fn branch_misprediction_penalty(&mut self) -> u64;

    /// Called once per quantum so the model can update internal state.
    fn end_of_quantum(&mut self) {}

    /// Reset model state.
    fn reset(&mut self) {}
}

/// Simplest model: every instruction costs 1 cycle, no stalls.
pub struct FunctionalModel;

impl TimingModel for FunctionalModel {
    fn accuracy(&self) -> AccuracyLevel {
        AccuracyLevel::Functional
    }
    fn instruction_latency(&mut self, _uop: &MicroOp) -> u64 {
        1
    }
    fn memory_latency(&mut self, _addr: Addr, _size: usize, _is_write: bool) -> u64 {
        0
    }
    fn branch_misprediction_penalty(&mut self) -> u64 {
        0
    }
}

/// Stall-annotated model with configurable cache-level latencies.
pub struct StallAnnotatedModel {
    pub l1_latency: u64,
    pub l2_latency: u64,
    pub l3_latency: u64,
    pub dram_latency: u64,
}

impl Default for StallAnnotatedModel {
    fn default() -> Self {
        Self {
            l1_latency: 3,
            l2_latency: 12,
            l3_latency: 40,
            dram_latency: 200,
        }
    }
}

impl TimingModel for StallAnnotatedModel {
    fn accuracy(&self) -> AccuracyLevel {
        AccuracyLevel::StallAnnotated
    }
    fn instruction_latency(&mut self, _uop: &MicroOp) -> u64 {
        1
    }
    fn memory_latency(&mut self, _addr: Addr, _size: usize, _is_write: bool) -> u64 {
        // Stub: always returns L1 hit.  A real implementation plugs
        // into helm-memory's cache hierarchy.
        self.l1_latency
    }
    fn branch_misprediction_penalty(&mut self) -> u64 {
        0 // not modelled at this level
    }
}

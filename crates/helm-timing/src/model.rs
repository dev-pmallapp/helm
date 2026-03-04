//! The TimingModel trait and accuracy-level enum.

use helm_core::ir::MicroOp;
use helm_core::types::Addr;

/// Simulation accuracy levels.
///
/// | Level | Acronym | Speed | What is modelled |
/// |-------|---------|-------|------------------|
/// | L0 | **FE** | 100-1000 MIPS | IPC=1, flat memory, no timing |
/// | L1-L2 | **APE** | 1-100 MIPS | Cache latencies, device stalls, optional pipeline |
/// | L3 | **CAE** | 0.1-1 MIPS | Full pipeline stages, bypass, store buffer |
///
/// **FE** — Functional Emulation.  Execute binaries at maximum speed
/// with no microarchitectural detail.  Like QEMU.
///
/// **APE** — Approximate Emulation.  Add cache-miss latencies, device
/// stalls, and optionally a simplified pipeline model.  Like Simics.
///
/// **CAE** — Cycle-Accurate Emulation.  Model every pipeline stage,
/// dependency, and stall with high fidelity.  Like gem5 O3CPU.
///
/// The execution mode is orthogonal: **SE** (Syscall Emulation) means
/// the binary runs in user-mode with Linux syscalls emulated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AccuracyLevel {
    /// L0 — Functional Emulation: IPC=1, no memory modelling, maximum speed.
    FE,
    /// L1-L2 — Approximate Emulation: cache latencies, device stalls,
    /// optional simplified OoO pipeline and branch prediction.
    APE,
    /// L3 — Cycle-Accurate Emulation: full pipeline stages, bypass
    /// network, store buffer, precise speculation.
    CAE,
}

/// A pluggable timing model.  Attach one to a core to inject stall
/// cycles into the simulation.  Detach it to go back to FE mode.
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

// ---------------------------------------------------------------------------
// Built-in models
// ---------------------------------------------------------------------------

/// **FE** model: every instruction costs 1 cycle, no stalls.
pub struct FeModel;

impl TimingModel for FeModel {
    fn accuracy(&self) -> AccuracyLevel {
        AccuracyLevel::FE
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

/// **APE** model: configurable cache-level latencies.
pub struct ApeModel {
    pub l1_latency: u64,
    pub l2_latency: u64,
    pub l3_latency: u64,
    pub dram_latency: u64,
}

impl Default for ApeModel {
    fn default() -> Self {
        Self {
            l1_latency: 3,
            l2_latency: 12,
            l3_latency: 40,
            dram_latency: 200,
        }
    }
}

impl TimingModel for ApeModel {
    fn accuracy(&self) -> AccuracyLevel {
        AccuracyLevel::APE
    }
    fn instruction_latency(&mut self, _uop: &MicroOp) -> u64 {
        1
    }
    fn memory_latency(&mut self, _addr: Addr, _size: usize, _is_write: bool) -> u64 {
        // Stub: always returns L1 hit.  Real impl plugs into helm-memory.
        self.l1_latency
    }
    fn branch_misprediction_penalty(&mut self) -> u64 {
        0 // not modelled at basic APE level
    }
}

//! The TimingModel trait and accuracy-level enum.

use helm_core::ir::MicroOp;
use helm_core::types::Addr;

/// Simulation accuracy tiers.
///
/// | Tier | Name | Speed | What is modelled |
/// |------|------|-------|------------------|
/// | L0 | **Express** | 100-1000 MIPS | IPC=1, flat memory, no timing |
/// | L1 | **Recon** | 10-100 MIPS | Cache latencies, device stalls |
/// | L2 | **Recon+** | 1-10 MIPS | Simplified OoO, branch pred |
/// | L3 | **Signal** | 0.1-1 MIPS | Full pipeline stages, bypass, store buffer |
///
/// **Express** (L0) — Functional emulation at maximum throughput.
/// Like QEMU: execute binaries fast, no microarchitectural detail.
///
/// **Recon** (L1-L2) — Reconnaissance-grade approximate timing.
/// Like Simics: observe cache behaviour, device interactions, and
/// coarse pipeline effects without modelling every cycle.
///
/// **Signal** (L2-L3) — Signal-accurate cycle-level detail.
/// Like gem5 O3CPU: every pipeline stage, dependency, and stall
/// is modelled with high fidelity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AccuracyLevel {
    /// L0 — **Express**: IPC=1, no memory modelling, maximum speed.
    Express,
    /// L1 — **Recon**: cache hit/miss latencies, device stall cycles.
    Recon,
    /// L2 — **Recon+**: simplified OoO pipeline, branch prediction.
    ReconDetailed,
    /// L3 — **Signal**: cycle-by-cycle pipeline stages, bypass network.
    Signal,
}

/// A pluggable timing model.  Attach one to a core to inject stall
/// cycles into the simulation.  Detach it to go back to Express mode.
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

/// **Express** model: every instruction costs 1 cycle, no stalls.
pub struct ExpressModel;

impl TimingModel for ExpressModel {
    fn accuracy(&self) -> AccuracyLevel {
        AccuracyLevel::Express
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

/// **Recon** model: configurable cache-level latencies.
pub struct ReconModel {
    pub l1_latency: u64,
    pub l2_latency: u64,
    pub l3_latency: u64,
    pub dram_latency: u64,
}

impl Default for ReconModel {
    fn default() -> Self {
        Self {
            l1_latency: 3,
            l2_latency: 12,
            l3_latency: 40,
            dram_latency: 200,
        }
    }
}

impl TimingModel for ReconModel {
    fn accuracy(&self) -> AccuracyLevel {
        AccuracyLevel::Recon
    }
    fn instruction_latency(&mut self, _uop: &MicroOp) -> u64 {
        1
    }
    fn memory_latency(&mut self, _addr: Addr, _size: usize, _is_write: bool) -> u64 {
        // Stub: always returns L1 hit.  Real impl plugs into helm-memory.
        self.l1_latency
    }
    fn branch_misprediction_penalty(&mut self) -> u64 {
        0 // not modelled at Recon level
    }
}

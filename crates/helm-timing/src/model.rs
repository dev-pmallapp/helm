//! The TimingModel trait and accuracy-level enum.

use helm_core::ir::MicroOp;
use helm_core::types::Addr;

// ---------------------------------------------------------------------------
// Instruction classification (ISA-independent)
// ---------------------------------------------------------------------------

/// Lightweight instruction class for timing lookup.
///
/// This avoids a dependency on `helm-core::ir::MicroOp` in places where
/// only the latency category matters (e.g. the SE classify pass).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InsnClass {
    IntAlu,
    IntMul,
    IntDiv,
    FpAlu,
    FpMul,
    FpDiv,
    Load,
    Store,
    Branch,
    CondBranch,
    Syscall,
    Nop,
    Simd,
    Fence,
}

/// Convert from the new unified InsnClass (helm-core) to the legacy timing InsnClass.
impl From<helm_core::insn::InsnClass> for InsnClass {
    fn from(c: helm_core::insn::InsnClass) -> Self {
        use helm_core::insn::InsnClass as New;
        match c {
            New::IntAlu => InsnClass::IntAlu,
            New::IntMul => InsnClass::IntMul,
            New::IntDiv => InsnClass::IntDiv,
            New::FpAlu => InsnClass::FpAlu,
            New::FpMul => InsnClass::FpMul,
            New::FpDiv | New::FpCvt => InsnClass::FpDiv,
            New::SimdAlu | New::SimdFpAlu | New::SimdShuffle => InsnClass::Simd,
            New::SimdMul | New::SimdFpMul => InsnClass::Simd,
            New::Load | New::LoadPair => InsnClass::Load,
            New::Store | New::StorePair => InsnClass::Store,
            New::Atomic => InsnClass::Load, // closest fit
            New::Prefetch => InsnClass::Nop,
            New::Branch | New::IndBranch | New::Call | New::Return => InsnClass::Branch,
            New::CondBranch => InsnClass::CondBranch,
            New::Syscall => InsnClass::Syscall,
            New::Fence => InsnClass::Fence,
            New::Nop | New::CacheMaint | New::SysRegAccess => InsnClass::Nop,
            New::Crypto => InsnClass::Simd,
            New::IoPort | New::Microcode | New::StringOp => InsnClass::IntAlu,
        }
    }
}

/// Simulation accuracy levels.
///
/// | Level | Acronym | Speed | What is modelled |
/// |-------|---------|-------|------------------|
/// | L0 | **FE** | 100-1000 MIPS | IPC=1, flat memory, no timing |
/// | L1-L2 | **ITE** | 1-100 MIPS | Cache latencies, device stalls, optional pipeline |
/// | L3 | **CAE** | 0.1-1 MIPS | Full pipeline stages, bypass, store buffer |
///
/// **FE** — Functional Emulation.  Execute binaries at maximum speed
/// with no microarchitectural detail.  Like QEMU.
///
/// **ITE** — Interval-Timing Emulation.  Add cache-miss latencies, device
/// stalls, and optionally a simplified pipeline model.  Based on the
/// *interval simulation* methodology (Genbrugge et al., HPCA 2010).
/// Comparable to Sniper and Simics timing modes.
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
    /// L1-L2 — Interval-Timing Emulation: cache latencies, device stalls,
    /// optional simplified OoO pipeline and branch prediction.
    ITE,
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

    /// How many cycles does this instruction class cost?
    ///
    /// Default delegates to [`instruction_latency`](TimingModel::instruction_latency)
    /// with a dummy `MicroOp` so existing models work unchanged.
    fn instruction_latency_for_class(&mut self, _class: InsnClass) -> u64 {
        1
    }

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

/// **ITE** model: configurable cache-level latencies.
pub struct IteModel {
    pub l1_latency: u64,
    pub l2_latency: u64,
    pub l3_latency: u64,
    pub dram_latency: u64,
}

impl Default for IteModel {
    fn default() -> Self {
        Self {
            l1_latency: 3,
            l2_latency: 12,
            l3_latency: 40,
            dram_latency: 200,
        }
    }
}

impl TimingModel for IteModel {
    fn accuracy(&self) -> AccuracyLevel {
        AccuracyLevel::ITE
    }
    fn instruction_latency(&mut self, _uop: &MicroOp) -> u64 {
        1
    }
    fn memory_latency(&mut self, _addr: Addr, _size: usize, _is_write: bool) -> u64 {
        // Stub: always returns L1 hit.  Real impl plugs into helm-memory.
        self.l1_latency
    }
    fn branch_misprediction_penalty(&mut self) -> u64 {
        0 // not modelled at basic ITE level
    }
}

// ---------------------------------------------------------------------------
// IteModelDetailed — per-opcode latency table
// ---------------------------------------------------------------------------

/// **ITE** model with per-instruction-class latencies.
///
/// Unlike [`IteModel`] which assigns IPC=1 everywhere, this model
/// returns differentiated latencies for integer multiply/divide,
/// floating-point, loads, stores, and branches.  Memory latencies use
/// a simple probabilistic model (L1/L2/L3/DRAM hit rates) — real
/// cache simulation is layered on top in the engine.
pub struct IteModelDetailed {
    pub int_alu_latency: u64,
    pub int_mul_latency: u64,
    pub int_div_latency: u64,
    pub fp_alu_latency: u64,
    pub fp_mul_latency: u64,
    pub fp_div_latency: u64,
    pub load_latency: u64,
    pub store_latency: u64,
    pub branch_penalty: u64,
    pub l1_latency: u64,
    pub l2_latency: u64,
    pub l3_latency: u64,
    pub dram_latency: u64,
}

impl Default for IteModelDetailed {
    fn default() -> Self {
        Self {
            int_alu_latency: 1,
            int_mul_latency: 3,
            int_div_latency: 12,
            fp_alu_latency: 4,
            fp_mul_latency: 5,
            fp_div_latency: 15,
            load_latency: 4,
            store_latency: 1,
            branch_penalty: 10,
            l1_latency: 3,
            l2_latency: 12,
            l3_latency: 40,
            dram_latency: 200,
        }
    }
}

impl TimingModel for IteModelDetailed {
    fn accuracy(&self) -> AccuracyLevel {
        AccuracyLevel::ITE
    }

    fn instruction_latency(&mut self, _uop: &MicroOp) -> u64 {
        1
    }

    fn instruction_latency_for_class(&mut self, class: InsnClass) -> u64 {
        match class {
            InsnClass::IntAlu => self.int_alu_latency,
            InsnClass::IntMul => self.int_mul_latency,
            InsnClass::IntDiv => self.int_div_latency,
            InsnClass::FpAlu => self.fp_alu_latency,
            InsnClass::FpMul => self.fp_mul_latency,
            InsnClass::FpDiv => self.fp_div_latency,
            InsnClass::Load => self.load_latency,
            InsnClass::Store => self.store_latency,
            InsnClass::Branch | InsnClass::CondBranch => 1,
            InsnClass::Syscall => 1,
            InsnClass::Nop => 1,
            InsnClass::Simd => self.fp_alu_latency,
            InsnClass::Fence => 1,
        }
    }

    fn memory_latency(&mut self, addr: Addr, _size: usize, _is_write: bool) -> u64 {
        // Simple probabilistic model: use address bits to approximate
        // cache-level hit distribution.  Real cache simulation is done
        // in the engine layer via helm-memory::Cache.
        let hash = (addr >> 6) % 100; // pseudo-random via cache-line index
        if hash < 85 {
            self.l1_latency
        } else if hash < 95 {
            self.l2_latency
        } else if hash < 99 {
            self.l3_latency
        } else {
            self.dram_latency
        }
    }

    fn branch_misprediction_penalty(&mut self) -> u64 {
        self.branch_penalty
    }
}

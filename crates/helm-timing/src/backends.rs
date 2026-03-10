//! TimingBackend implementations.
//!
//! These implement the new `helm_core::timing::TimingBackend` trait,
//! bridging to the existing timing models.

use helm_core::insn::{DecodedInsn, ExecOutcome, InsnClass};
use helm_core::timing::{AccuracyLevel, TimingBackend};

/// FE — returns 0 always. Compiles away in monomorphised code.
pub struct NullBackend;

impl TimingBackend for NullBackend {
    #[inline(always)]
    fn accuracy(&self) -> AccuracyLevel {
        AccuracyLevel::FE
    }

    #[inline(always)]
    fn account(&mut self, _insn: &DecodedInsn, _outcome: &ExecOutcome) -> u64 {
        0
    }
}

/// ITE — per-class latencies + probabilistic cache model.
pub struct IntervalBackend {
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

impl Default for IntervalBackend {
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

impl IntervalBackend {
    fn class_latency(&self, class: InsnClass) -> u64 {
        match class {
            InsnClass::IntAlu | InsnClass::Nop => self.int_alu_latency,
            InsnClass::IntMul => self.int_mul_latency,
            InsnClass::IntDiv => self.int_div_latency,
            InsnClass::FpAlu => self.fp_alu_latency,
            InsnClass::FpMul => self.fp_mul_latency,
            InsnClass::FpDiv | InsnClass::FpCvt => self.fp_div_latency,
            InsnClass::SimdAlu | InsnClass::SimdFpAlu => self.fp_alu_latency,
            InsnClass::SimdMul | InsnClass::SimdFpMul => self.fp_mul_latency,
            InsnClass::SimdShuffle => self.int_alu_latency,
            InsnClass::Load | InsnClass::LoadPair => self.load_latency,
            InsnClass::Store | InsnClass::StorePair => self.store_latency,
            InsnClass::Atomic => self.load_latency + self.store_latency,
            InsnClass::Prefetch => 1,
            InsnClass::Branch | InsnClass::IndBranch | InsnClass::Call | InsnClass::Return => 1,
            InsnClass::CondBranch => 1, // penalty added via branch_taken
            InsnClass::Syscall => 1,
            InsnClass::Fence => 1,
            InsnClass::CacheMaint | InsnClass::SysRegAccess => 1,
            InsnClass::Crypto => self.fp_mul_latency,
            InsnClass::IoPort | InsnClass::Microcode | InsnClass::StringOp => self.int_alu_latency,
        }
    }

    fn mem_latency(&self, addr: u64) -> u64 {
        let hash = (addr >> 6) % 100;
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
}

impl TimingBackend for IntervalBackend {
    fn accuracy(&self) -> AccuracyLevel {
        AccuracyLevel::ITE
    }

    fn account(&mut self, insn: &DecodedInsn, outcome: &ExecOutcome) -> u64 {
        let mut stall = self.class_latency(insn.class);

        // Add memory latency for load/store
        for i in 0..outcome.mem_access_count as usize {
            stall += self.mem_latency(outcome.mem_accesses[i].addr);
        }

        // Branch misprediction penalty (simple: always predict not-taken)
        if outcome.branch_taken {
            stall += self.branch_penalty;
        }

        stall
    }
}

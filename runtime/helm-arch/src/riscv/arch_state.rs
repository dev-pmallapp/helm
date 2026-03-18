//! `RiscvArchState` — the RISC-V architectural register file.

use helm_core::{ArchState, AttrRegistry, AttrValue};

/// RISC-V RV64 architectural state.
///
/// Register x0 is hardwired to zero: reads always return 0, writes are ignored.
/// CSRs are stored as a flat array indexed by the 12-bit CSR address.
pub struct RiscvArchState {
    /// Integer registers x0–x31. `iregs[0]` is always 0.
    pub iregs: [u64; 32],
    /// Floating-point registers f0–f31 (stored as raw bits; NaN-boxed for F extension).
    pub fregs: [u64; 32],
    /// Control/status registers, indexed by 12-bit address (0..=0xFFF).
    pub csrs: Box<[u64; 4096]>,
    /// Program counter.
    pub pc: u64,
}

impl Default for RiscvArchState {
    fn default() -> Self {
        Self {
            iregs: [0u64; 32],
            fregs: [0u64; 32],
            csrs: Box::new([0u64; 4096]),
            pc: 0,
        }
    }
}

impl RiscvArchState {
    pub fn new() -> Self { Self::default() }

    pub fn new_with_pc(reset_vector: u64) -> Self {
        Self { pc: reset_vector, ..Self::default() }
    }
}

impl ArchState for RiscvArchState {
    #[inline(always)]
    fn read_int_reg(&self, idx: usize) -> u64 {
        // x0 is hardwired zero — no branch needed because iregs[0] is always 0.
        self.iregs[idx]
    }

    #[inline(always)]
    fn write_int_reg(&mut self, idx: usize, val: u64) {
        if idx != 0 {
            self.iregs[idx] = val;
        }
    }

    #[inline(always)]
    fn read_pc(&self) -> u64 { self.pc }

    #[inline(always)]
    fn write_pc(&mut self, val: u64) { self.pc = val; }

    fn register_attrs(&self, r: &mut AttrRegistry) {
        for i in 0..32usize {
            r.set(format!("x{i}"), AttrValue::U64(self.iregs[i]));
            r.set(format!("f{i}"), AttrValue::U64(self.fregs[i]));
        }
        r.set("pc", AttrValue::U64(self.pc));
    }

    fn reset(&mut self, reset_vector: u64) {
        self.iregs = [0u64; 32];
        self.fregs = [0u64; 32];
        // CSRs: keep mhartid, clear the rest in a real impl; zero all for now.
        *self.csrs = [0u64; 4096];
        self.pc = reset_vector;
    }
}

//! AArch64 architectural state — registers, NZCV flags, system registers.

use helm_core::{ArchState, AttrRegistry, AttrValue};

/// AArch64 architectural register file.
///
/// # Register conventions
/// - `x[0..=30]`: general-purpose 64-bit registers X0–X30
/// - `sp`:        stack pointer (SP_EL0 in SE mode)
/// - `pc`:        program counter
/// - `nzcv`:      condition flags packed into bits [31:28]: N=31, Z=30, C=29, V=28
/// - `v[0..=31]`: 128-bit SIMD/FP registers V0–V31
///
/// # SE mode assumptions
/// - `current_el = 0` (EL0 user mode)
/// - MMU off (identity translation)
/// - `tpidr_el0` carries thread-local storage pointer (set by `set_tls` syscall)
pub struct Aarch64ArchState {
    /// General-purpose registers X0–X30.  X31 is context-dependent (SP or XZR).
    pub x: [u64; 31],
    /// Stack pointer (SP_EL0 in SE mode).
    pub sp: u64,
    /// Program counter.
    pub pc: u64,

    // ── Condition flags ──────────────────────────────────────────────────────
    /// NZCV packed: N=bit31, Z=bit30, C=bit29, V=bit28.
    pub nzcv: u32,

    // ── FP / SIMD ────────────────────────────────────────────────────────────
    /// 128-bit SIMD/FP registers V0–V31 (lane interpretation depends on instruction).
    pub v: [u128; 32],
    /// Floating-point control register.
    pub fpcr: u32,
    /// Floating-point status register.
    pub fpsr: u32,

    // ── User-visible system registers ────────────────────────────────────────
    /// Thread pointer (EL0). Set via `MRS TPIDR_EL0` / `set_tls` prctl.
    pub tpidr_el0: u64,
    /// Counter frequency (default 62.5 MHz = 62_500_000).
    pub cntfrq_el0: u64,
    /// Virtual counter value (monotonically increasing; SE mode: host clock).
    pub cntvct_el0: u64,

    // ── EL1 system registers (needed for exception handling stubs) ───────────
    pub sp_el1: u64,
    pub elr_el1: u64,
    pub spsr_el1: u32,
    pub vbar_el1: u64,
    pub esr_el1: u32,
    pub far_el1: u64,
    pub sctlr_el1: u64,
    pub tcr_el1: u64,
    pub ttbr0_el1: u64,
    pub ttbr1_el1: u64,
    pub mair_el1: u64,
    pub midr_el1: u64,
    pub mpidr_el1: u64,
    pub id_aa64pfr0_el1: u64,
    pub id_aa64isar0_el1: u64,
    pub id_aa64mmfr0_el1: u64,
    pub id_aa64mmfr1_el1: u64,
    pub daif: u32,
    pub current_el: u8,
}

impl Default for Aarch64ArchState {
    fn default() -> Self {
        Self {
            x: [0u64; 31],
            sp: 0,
            pc: 0,
            nzcv: 0,
            v: [0u128; 32],
            fpcr: 0,
            fpsr: 0,
            tpidr_el0: 0,
            cntfrq_el0: 62_500_000,
            cntvct_el0: 0,
            sp_el1: 0,
            elr_el1: 0,
            spsr_el1: 0,
            vbar_el1: 0,
            esr_el1: 0,
            far_el1: 0,
            // RES1 bits; MMU disabled.
            sctlr_el1: 0x0000_0800,
            tcr_el1: 0,
            ttbr0_el1: 0,
            ttbr1_el1: 0,
            mair_el1: 0,
            // Cortex-A53 MIDR
            midr_el1: 0x410F_D034,
            // 4 cores, cluster 0
            mpidr_el1: 0x8000_0000,
            // EL0/1 AArch64 support, FP + AdvSIMD present
            id_aa64pfr0_el1: 0x0000_0000_1122_0000,
            id_aa64isar0_el1: 0x0000_0000_0001_1120,
            id_aa64mmfr0_el1: 0x0000_0000_0000_1122,
            id_aa64mmfr1_el1: 0,
            daif: 0,
            current_el: 0,
        }
    }
}

impl Aarch64ArchState {
    pub fn new() -> Self { Self::default() }

    // ── NZCV helpers ─────────────────────────────────────────────────────────

    pub fn flag_n(&self) -> bool { self.nzcv & (1 << 31) != 0 }
    pub fn flag_z(&self) -> bool { self.nzcv & (1 << 30) != 0 }
    pub fn flag_c(&self) -> bool { self.nzcv & (1 << 29) != 0 }
    pub fn flag_v(&self) -> bool { self.nzcv & (1 << 28) != 0 }

    pub fn set_nzcv(&mut self, n: bool, z: bool, c: bool, v: bool) {
        self.nzcv = ((n as u32) << 31)
            | ((z as u32) << 30)
            | ((c as u32) << 29)
            | ((v as u32) << 28);
    }

    /// Set NZCV from a 64-bit arithmetic result + carry/overflow flags.
    pub fn set_nzcv64(&mut self, result: u64, carry: bool, overflow: bool) {
        self.set_nzcv(result >> 63 != 0, result == 0, carry, overflow);
    }

    // ── Register read/write with X31 convention ───────────────────────────────

    /// Read GPR. X31 = XZR (returns 0) in most contexts.
    #[inline(always)]
    pub fn read_x(&self, idx: u32) -> u64 {
        if idx >= 31 { 0 } else { self.x[idx as usize] }
    }

    /// Write GPR. X31 = XZR (ignored) in most contexts.
    #[inline(always)]
    pub fn write_x(&mut self, idx: u32, val: u64) {
        if idx < 31 { self.x[idx as usize] = val; }
    }

    /// Read GPR as 32-bit (W register).  X31 = WZR (returns 0).
    #[inline(always)]
    pub fn read_w(&self, idx: u32) -> u32 { self.read_x(idx) as u32 }

    /// Write 32-bit W register (zero-extends to 64 bits).
    #[inline(always)]
    pub fn write_w(&mut self, idx: u32, val: u32) { self.write_x(idx, val as u64); }

    /// Read GPR or SP: X31 → SP.
    #[inline(always)]
    pub fn read_xsp(&self, idx: u32) -> u64 {
        if idx == 31 { self.sp } else { self.x[idx as usize] }
    }

    /// Write GPR or SP: X31 → SP.
    #[inline(always)]
    pub fn write_xsp(&mut self, idx: u32, val: u64) {
        if idx == 31 { self.sp = val; } else { self.x[idx as usize] = val; }
    }

    // ── Condition evaluation ──────────────────────────────────────────────────

    /// Evaluate an AArch64 condition code (4-bit `cond` field).
    pub fn eval_cond(&self, cond: u32) -> bool {
        let n = self.flag_n();
        let z = self.flag_z();
        let c = self.flag_c();
        let v = self.flag_v();
        match cond & 0xF {
            0b0000 => z,                          // EQ
            0b0001 => !z,                         // NE
            0b0010 => c,                          // CS/HS
            0b0011 => !c,                         // CC/LO
            0b0100 => n,                          // MI
            0b0101 => !n,                         // PL
            0b0110 => v,                          // VS
            0b0111 => !v,                         // VC
            0b1000 => c && !z,                    // HI
            0b1001 => !c || z,                    // LS
            0b1010 => n == v,                     // GE
            0b1011 => n != v,                     // LT
            0b1100 => !z && (n == v),             // GT
            0b1101 => z || (n != v),              // LE
            0b1110 | 0b1111 => true,              // AL / NV
            _ => unreachable!(),
        }
    }
}

impl ArchState for Aarch64ArchState {
    #[inline(always)]
    fn read_int_reg(&self, idx: usize) -> u64 {
        if idx < 31 { self.x[idx] } else if idx == 31 { 0 } else { self.sp }
    }

    #[inline(always)]
    fn write_int_reg(&mut self, idx: usize, val: u64) {
        if idx < 31 { self.x[idx] = val; }
    }

    fn read_pc(&self) -> u64 { self.pc }
    fn write_pc(&mut self, val: u64) { self.pc = val; }

    fn register_attrs(&self, r: &mut AttrRegistry) {
        for i in 0..31usize {
            r.set(format!("x{i}"), AttrValue::U64(self.x[i]));
        }
        r.set("sp", AttrValue::U64(self.sp));
        r.set("pc", AttrValue::U64(self.pc));
        r.set("nzcv", AttrValue::U64(self.nzcv as u64));
    }

    fn reset(&mut self, reset_vector: u64) {
        *self = Self::default();
        self.pc = reset_vector;
    }
}

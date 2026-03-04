//! ARM register files for AArch64 and AArch32.

/// AArch64 architectural state (EL0, SE mode).
#[derive(Debug, Clone, Default)]
pub struct Aarch64Regs {
    /// General-purpose registers X0-X30.
    pub x: [u64; 31],
    /// Stack pointer (SP_EL0).
    pub sp: u64,
    /// Program counter.
    pub pc: u64,
    /// Condition flags: N, Z, C, V packed into bits [31:28].
    pub nzcv: u32,
    /// SIMD/FP registers V0-V31 (128-bit each).
    pub v: [u128; 32],
    /// Floating-point control register.
    pub fpcr: u32,
    /// Floating-point status register.
    pub fpsr: u32,
    /// Thread-local storage base.
    pub tpidr_el0: u64,
}

impl Aarch64Regs {
    pub fn n(&self) -> bool {
        self.nzcv & (1 << 31) != 0
    }
    pub fn z(&self) -> bool {
        self.nzcv & (1 << 30) != 0
    }
    pub fn c(&self) -> bool {
        self.nzcv & (1 << 29) != 0
    }
    pub fn v(&self) -> bool {
        self.nzcv & (1 << 28) != 0
    }

    pub fn set_nzcv(&mut self, n: bool, z: bool, c: bool, v: bool) {
        self.nzcv =
            ((n as u32) << 31) | ((z as u32) << 30) | ((c as u32) << 29) | ((v as u32) << 28);
    }
}

/// AArch32 architectural state (ARMv7-A) — stage 1.
#[derive(Debug, Clone, Default)]
pub struct Aarch32Regs {
    /// R0-R15 (R13=SP, R14=LR, R15=PC).
    pub r: [u32; 16],
    /// Current program status register.
    pub cpsr: u32,
    /// VFP/NEON double-precision registers D0-D31.
    pub d: [u64; 32],
    /// Floating-point status and control register.
    pub fpscr: u32,
}

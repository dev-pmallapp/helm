//! ARM register files for AArch64 and AArch32.

/// AArch64 architectural state (EL0–EL3).
#[derive(Debug, Clone)]
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
    /// Thread-local storage base (EL0).
    pub tpidr_el0: u64,

    // ── Exception level ──────────────────────────────────────────────
    /// Current exception level (0, 1, 2, or 3).
    pub current_el: u8,
    /// DAIF mask bits (D=9, A=8, I=7, F=6).
    pub daif: u32,
    /// SP selection: 0 = SP_EL0, 1 = SP_ELx.
    pub sp_sel: u8,

    // ── EL1 system registers ─────────────────────────────────────────
    pub sp_el1: u64,
    pub elr_el1: u64,
    pub spsr_el1: u32,
    pub vbar_el1: u64,
    pub sctlr_el1: u64,
    pub tcr_el1: u64,
    pub ttbr0_el1: u64,
    pub ttbr1_el1: u64,
    pub mair_el1: u64,
    pub amair_el1: u64,
    pub contextidr_el1: u64,
    pub cpacr_el1: u64,
    pub esr_el1: u32,
    pub far_el1: u64,
    pub tpidr_el1: u64,
    pub cntkctl_el1: u64,
    pub csselr_el1: u64,
    pub par_el1: u64,
    pub mdscr_el1: u32,
    pub actlr_el1: u64,
    pub afsr0_el1: u64,
    pub afsr1_el1: u64,

    // ── EL2 system registers ────────────────────────────────────────
    pub sp_el2: u64,
    pub elr_el2: u64,
    pub spsr_el2: u32,
    pub vbar_el2: u64,
    pub hcr_el2: u64,
    pub sctlr_el2: u64,
    pub tcr_el2: u64,
    pub ttbr0_el2: u64,
    pub ttbr1_el2: u64,          // VHE (E2H=1) only
    pub mair_el2: u64,
    pub amair_el2: u64,
    pub esr_el2: u32,
    pub far_el2: u64,
    pub hpfar_el2: u64,          // Hypervisor IPA Fault Address
    pub vttbr_el2: u64,
    pub vtcr_el2: u64,           // Virtualization Translation Control
    pub cptr_el2: u64,           // Architectural Feature Trap
    pub vmpidr_el2: u64,         // Virtualization Multiprocessor ID
    pub vpidr_el2: u64,          // Virtualization Processor ID
    pub mdcr_el2: u64,           // Monitor Debug Configuration
    pub hacr_el2: u64,           // Hypervisor Auxiliary Control
    pub cnthctl_el2: u64,        // Counter-timer Hypervisor Control
    pub cnthp_ctl_el2: u64,      // Hypervisor Physical Timer Control
    pub cnthp_cval_el2: u64,     // Hypervisor Physical Timer Compare
    pub cntvoff_el2: u64,
    pub tpidr_el2: u64,
    pub afsr0_el2: u64,
    pub afsr1_el2: u64,
    pub contextidr_el2: u64,     // VHE context ID
    pub actlr_el2: u64,

    // ── EL3 system registers ────────────────────────────────────────
    pub sp_el3: u64,
    pub elr_el3: u64,
    pub spsr_el3: u32,
    pub scr_el3: u64,
    pub sctlr_el3: u64,
    pub tcr_el3: u64,
    pub ttbr0_el3: u64,
    pub mair_el3: u64,
    pub amair_el3: u64,
    pub esr_el3: u32,
    pub far_el3: u64,
    pub vbar_el3: u64,
    pub mdcr_el3: u64,
    pub cptr_el3: u64,
    pub tpidr_el3: u64,
    pub afsr0_el3: u64,
    pub afsr1_el3: u64,
    pub actlr_el3: u64,

    // ── ID registers (read-only) ─────────────────────────────────────
    pub midr_el1: u64,
    pub mpidr_el1: u64,
    pub revidr_el1: u64,
    pub id_aa64pfr0_el1: u64,
    pub id_aa64pfr1_el1: u64,
    pub id_aa64mmfr0_el1: u64,
    pub id_aa64mmfr1_el1: u64,
    pub id_aa64mmfr2_el1: u64,
    pub id_aa64isar0_el1: u64,
    pub id_aa64isar1_el1: u64,
    pub id_aa64isar2_el1: u64,
    pub id_aa64dfr0_el1: u64,
    pub ctr_el0: u64,
    pub dczid_el0: u64,

    // ── Timer registers ──────────────────────────────────────────────
    pub cntfrq_el0: u64,
    pub cntvct_el0: u64,
    pub cntv_ctl_el0: u64,
    pub cntv_cval_el0: u64,
    pub cntp_ctl_el0: u64,
    pub cntp_cval_el0: u64,
}

impl Default for Aarch64Regs {
    fn default() -> Self {
        Self {
            x: [0; 31],
            sp: 0,
            pc: 0,
            nzcv: 0,
            v: [0; 32],
            fpcr: 0,
            fpsr: 0,
            tpidr_el0: 0,
            current_el: 0, // SE mode starts at EL0; FS runner sets EL1
            daif: 0,       // unmasked in SE mode
            sp_sel: 0,     // SP_EL0 in SE mode
            sp_el1: 0, elr_el1: 0, spsr_el1: 0, vbar_el1: 0,
            sctlr_el1: 0x0080_0800, // RES1 bits: EOS, EIS
            tcr_el1: 0, ttbr0_el1: 0, ttbr1_el1: 0,
            mair_el1: 0, amair_el1: 0, contextidr_el1: 0,
            cpacr_el1: 0, esr_el1: 0, far_el1: 0,
            tpidr_el1: 0, cntkctl_el1: 0, csselr_el1: 0,
            par_el1: 0, mdscr_el1: 0, actlr_el1: 0,
            afsr0_el1: 0, afsr1_el1: 0,
            // EL2
            sp_el2: 0, elr_el2: 0, spsr_el2: 0, vbar_el2: 0,
            hcr_el2: 0, sctlr_el2: 0x0080_0800, // RES1 bits
            tcr_el2: 0, ttbr0_el2: 0, ttbr1_el2: 0,
            mair_el2: 0, amair_el2: 0, esr_el2: 0, far_el2: 0, hpfar_el2: 0,
            vttbr_el2: 0, vtcr_el2: 0, cptr_el2: 0,
            vmpidr_el2: 0, vpidr_el2: 0, mdcr_el2: 0, hacr_el2: 0,
            cnthctl_el2: 0, cnthp_ctl_el2: 0, cnthp_cval_el2: 0, cntvoff_el2: 0,
            tpidr_el2: 0, afsr0_el2: 0, afsr1_el2: 0,
            contextidr_el2: 0, actlr_el2: 0,
            // EL3
            sp_el3: 0, elr_el3: 0, spsr_el3: 0, scr_el3: 0,
            sctlr_el3: 0x0080_0800, // RES1 bits
            tcr_el3: 0, ttbr0_el3: 0,
            mair_el3: 0, amair_el3: 0, esr_el3: 0, far_el3: 0, vbar_el3: 0,
            mdcr_el3: 0, cptr_el3: 0, tpidr_el3: 0,
            afsr0_el3: 0, afsr1_el3: 0, actlr_el3: 0,
            // Cortex-A53 ID values
            midr_el1: 0x410F_D034,
            mpidr_el1: 0x8000_0000,
            revidr_el1: 0,
            id_aa64pfr0_el1: 0x1100_0000_0000_1111, // EL0-3 AArch64, FP, AdvSIMD, CSV2+CSV3
            id_aa64pfr1_el1: 0,
            id_aa64mmfr0_el1: 0x0000_0000_0000_1125, // 4K/16K/64K granule, 48-bit PA
            id_aa64mmfr1_el1: 0,
            id_aa64mmfr2_el1: 0,
            id_aa64isar0_el1: 0x0000_0001_0011_0000, // AES, SHA1, SHA256, CRC32
            id_aa64isar1_el1: 0,
            id_aa64isar2_el1: 0,
            id_aa64dfr0_el1: 0x0000_0000_0000_0006, // debug v8
            ctr_el0: 0x8444_C004,  // cache line sizes
            dczid_el0: 0x04,       // 64-byte DC ZVA block
            cntfrq_el0: 62_500_000,
            cntvct_el0: 0,
            cntv_ctl_el0: 0, cntv_cval_el0: 0,
            cntp_ctl_el0: 0, cntp_cval_el0: 0,
        }
    }
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

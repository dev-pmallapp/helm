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
    /// Read-only thread pointer visible at EL0, writable by privileged code.
    pub tpidrro_el0: u64,
    /// Counter frequency (default 62.5 MHz = 62_500_000).
    pub cntfrq_el0: u64,
    /// Virtual counter value (monotonically increasing; SE mode: host clock).
    pub cntvct_el0: u64,

    // ── EL1 system registers (needed for exception handling stubs) ───────────
    pub sp_el1: u64,
    pub sp_el2: u64,
    pub sp_el3: u64,
    pub elr_el1: u64,
    pub elr_el2: u64,
    pub elr_el3: u64,
    pub spsr_el1: u32,
    pub spsr_el2: u32,
    pub spsr_el3: u32,
    pub vbar_el1: u64,
    pub vbar_el2: u64,
    pub vbar_el3: u64,
    pub esr_el1: u32,
    pub esr_el2: u32,
    pub esr_el3: u32,
    pub far_el1: u64,
    pub far_el2: u64,
    pub far_el3: u64,
    pub hpfar_el2: u64,
    pub mdcr_el2: u64,
    pub cptr_el2: u64,
    pub hstr_el2: u64,
    pub sctlr_el1: u64,
    pub sctlr_el2: u64,
    pub sctlr_el3: u64,
    pub tcr_el1: u64,
    pub tcr_el2: u64,
    pub tcr_el3: u64,
    pub ttbr0_el1: u64,
    pub ttbr0_el2: u64,
    pub ttbr0_el3: u64,
    pub ttbr1_el1: u64,
    pub ttbr1_el2: u64,
    pub vttbr_el2: u64,
    pub vtcr_el2: u64,
    pub mair_el1: u64,
    pub mair_el2: u64,
    pub mair_el3: u64,
    pub midr_el1: u64,
    pub mpidr_el1: u64,
    pub id_aa64pfr0_el1: u64,
    pub id_aa64isar0_el1: u64,
    pub id_aa64mmfr0_el1: u64,
    pub id_aa64mmfr1_el1: u64,
    pub daif: u32,
    pub current_el: u8,
    pub hcr_el2: u64,
    pub scr_el3: u64,

    // ── EL1 extension registers ────────────────────────────────────────────
    /// SP selection: false = SP_EL0, true = SP_EL1.
    pub spsel: bool,
    /// EL1 thread pointer.
    pub tpidr_el1: u64,
    /// Context ID register.
    pub contextidr_el1: u64,
    /// Coprocessor access control (FPEN=0b11 enables FP/SIMD).
    pub cpacr_el1: u64,
    /// Physical Address Register.
    pub par_el1: u64,
    /// Auxiliary memory attribute register.
    pub amair_el1: u64,
    /// Monitor Debug System Control.
    pub mdscr_el1: u32,
    /// Counter Kernel Control.
    pub cntkctl_el1: u32,
    /// Counter-timer Hypervisor Control.
    pub cnthctl_el2: u64,
    /// Hypervisor physical timer control.
    pub cnthp_ctl_el2: u32,
    /// Hypervisor physical timer compare value.
    pub cnthp_cval_el2: u64,
    /// Virtual counter offset.
    pub cntvoff_el2: u64,
    /// Physical timer control.
    pub cntp_ctl_el0: u32,
    /// Physical timer compare value.
    pub cntp_cval_el0: u64,
    /// Virtual timer control.
    pub cntv_ctl_el0: u32,
    /// Virtual timer compare value.
    pub cntv_cval_el0: u64,
    /// ISA feature register 1.
    pub id_aa64isar1_el1: u64,
    /// Processor feature register 1.
    pub id_aa64pfr1_el1: u64,

    /// Set by the `SYS` executor (TLBI/DC/IC instructions) to signal that the
    /// software TLB in `FsState` must be flushed before the next translation.
    /// Checked and cleared by `step_aarch64_fs()` after each instruction.
    pub tlb_flush_pending: bool,
    /// Set alongside tlb_flush_pending; cleared by engine after broadcasting
    /// the flush to all other vCPUs. Separate flag so step_aarch64_fs can
    /// clear tlb_flush_pending without losing the broadcast signal.
    pub tlb_flush_broadcast: bool,
    /// When set, the pending TLB flush targets only this VA (page-aligned).
    /// `None` means full flush; `Some(va)` means per-VA invalidation.
    pub tlb_flush_va: Option<u64>,
    /// When set, the pending TLB flush targets only this ASID.
    pub tlb_flush_asid: Option<u16>,

    // ── Exclusive monitor (LDXR/STXR) ────────────────────────────────────────
    /// Address recorded by the last LDXR/LDAXR (None = no active reservation).
    pub exclusive_addr: Option<u64>,
    /// Value read by the last LDXR/LDAXR, for compare on STXR.
    pub exclusive_val: u64,

    /// Route PSCI HVC/SMC calls through the enclosing FS machine instead of
    /// handling them entirely inside the per-hart executor.
    pub psci_via_engine: bool,

    // ── Pointer Authentication keys (ARMv8.3-PAuth) ─────────────────────────
    pub apia_key: [u64; 2],
    pub apib_key: [u64; 2],
    pub apda_key: [u64; 2],
    pub apdb_key: [u64; 2],
    pub apga_key: [u64; 2],
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
            tpidrro_el0: 0,
            cntfrq_el0: 62_500_000,
            cntvct_el0: 0,
            sp_el1: 0,
            sp_el2: 0,
            sp_el3: 0,
            elr_el1: 0,
            elr_el2: 0,
            elr_el3: 0,
            spsr_el1: 0,
            spsr_el2: 0,
            spsr_el3: 0,
            vbar_el1: 0,
            vbar_el2: 0,
            vbar_el3: 0,
            esr_el1: 0,
            esr_el2: 0,
            esr_el3: 0,
            far_el1: 0,
            far_el2: 0,
            far_el3: 0,
            hpfar_el2: 0,
            mdcr_el2: 0,
            cptr_el2: 0,
            hstr_el2: 0,
            // RES1 bits; MMU disabled.
            sctlr_el1: 0x0000_0800,
            sctlr_el2: 0x0000_0800,
            sctlr_el3: 0x0000_0800,
            tcr_el1: 0,
            tcr_el2: 0,
            tcr_el3: 0,
            ttbr0_el1: 0,
            ttbr0_el2: 0,
            ttbr0_el3: 0,
            ttbr1_el1: 0,
            ttbr1_el2: 0,
            vttbr_el2: 0,
            vtcr_el2: 0,
            mair_el1: 0,
            mair_el2: 0,
            mair_el3: 0,
            // Cortex-A55 feature set with non-ARM implementer (0x48='H') to
            // prevent Linux from matching Spectre-BHB vulnerability tables.
            // The simulator has no speculative execution.
            midr_el1: 0x4810_D050,
            // Uniprocessor, cluster 0
            mpidr_el1: 0x8000_0000,
            // EL0/EL1: AArch64-only (no AArch32), EL2/EL3: not impl
            // FP/AdvSIMD: present (no FP16), GIC: 0 (MMIO GICv2 only, no SRE)
            // CSV2=2 (bits 57:56), CSV3=1 (bit 60): no speculative execution.
            id_aa64pfr0_el1: 0x1200_0000_0000_0000,
            // v8.2: SHA1=1, SHA2=1, AES=2, CRC32=1, ATOMIC=2, RDM=1
            id_aa64isar0_el1: 0x0000_0000_0002_1000, // CRC32=1, ATOMIC=2 (LSE+CAS); no SHA/RDM we don't implement
            // PAC: API=1 (IMP DEF address auth), GPI=1 (IMP DEF generic auth).
            // Implemented as identity function (PAC bits = 0, AUT = pass-through).
            id_aa64isar1_el1: 0x0000_0000_1000_0100, // GPI[31:28]=1, API[11:8]=1
            // PARange=5 (48-bit PA), TGran4=0 (4KB supported), TGran16=6 (16KB)
            id_aa64mmfr0_el1: 0x0000_0000_0000_1125,
            id_aa64mmfr1_el1: 0,
            daif: 0,
            current_el: 0,
            hcr_el2: 0,
            scr_el3: 0,
            spsel: false,
            tpidr_el1: 0,
            contextidr_el1: 0,
            cpacr_el1: 0x0030_0000, // FPEN=0b11: FP/SIMD access enabled
            par_el1: 0,
            amair_el1: 0,
            mdscr_el1: 0,
            cntkctl_el1: 0,
            cnthctl_el2: 0,
            cnthp_ctl_el2: 0,
            cnthp_cval_el2: 0,
            cntvoff_el2: 0,
            cntp_ctl_el0: 0,
            cntp_cval_el0: 0,
            cntv_ctl_el0: 0,
            cntv_cval_el0: 0,
            id_aa64pfr1_el1: 0,
            tlb_flush_pending: false,
            tlb_flush_broadcast: false,
            tlb_flush_va: None,
            tlb_flush_asid: None,
            exclusive_addr: None,
            exclusive_val: 0,
            psci_via_engine: false,
            apia_key: [0; 2],
            apib_key: [0; 2],
            apda_key: [0; 2],
            apdb_key: [0; 2],
            apga_key: [0; 2],
        }
    }
}

impl Aarch64ArchState {
    pub fn new() -> Self {
        Self::default()
    }

    // ── NZCV helpers ─────────────────────────────────────────────────────────

    pub fn flag_n(&self) -> bool {
        self.nzcv & (1 << 31) != 0
    }
    pub fn flag_z(&self) -> bool {
        self.nzcv & (1 << 30) != 0
    }
    pub fn flag_c(&self) -> bool {
        self.nzcv & (1 << 29) != 0
    }
    pub fn flag_v(&self) -> bool {
        self.nzcv & (1 << 28) != 0
    }

    pub fn set_nzcv(&mut self, n: bool, z: bool, c: bool, v: bool) {
        self.nzcv =
            ((n as u32) << 31) | ((z as u32) << 30) | ((c as u32) << 29) | ((v as u32) << 28);
    }

    /// Set NZCV from a 64-bit arithmetic result + carry/overflow flags.
    pub fn set_nzcv64(&mut self, result: u64, carry: bool, overflow: bool) {
        self.set_nzcv(result >> 63 != 0, result == 0, carry, overflow);
    }

    // ── Register read/write with X31 convention ───────────────────────────────

    /// Read GPR. X31 = XZR (returns 0) in most contexts.
    #[inline(always)]
    pub fn read_x(&self, idx: u32) -> u64 {
        if idx >= 31 {
            0
        } else {
            self.x[idx as usize]
        }
    }

    /// Write GPR. X31 = XZR (ignored) in most contexts.
    #[inline(always)]
    pub fn write_x(&mut self, idx: u32, val: u64) {
        if idx < 31 {
            self.x[idx as usize] = val;
        }
    }

    /// Read GPR as 32-bit (W register).  X31 = WZR (returns 0).
    #[inline(always)]
    pub fn read_w(&self, idx: u32) -> u32 {
        self.read_x(idx) as u32
    }

    /// Write 32-bit W register (zero-extends to 64 bits).
    #[inline(always)]
    pub fn write_w(&mut self, idx: u32, val: u32) {
        self.write_x(idx, val as u64);
    }

    /// Read GPR or SP: X31 → current stack pointer.
    ///
    /// When `current_el >= 1` and `spsel == true`, X31 maps to SP_ELx.
    /// Otherwise X31 maps to SP_EL0 (`self.sp`).
    #[inline(always)]
    pub fn read_xsp(&self, idx: u32) -> u64 {
        if idx == 31 {
            if self.current_el >= 1 && self.spsel {
                self.current_sp()
            } else {
                self.sp
            }
        } else {
            self.x[idx as usize]
        }
    }

    /// Write GPR or SP: X31 → current stack pointer.
    ///
    /// When `current_el >= 1` and `spsel == true`, X31 maps to SP_ELx.
    /// Otherwise X31 maps to SP_EL0 (`self.sp`).
    #[inline(always)]
    pub fn write_xsp(&mut self, idx: u32, val: u64) {
        if idx == 31 {
            if self.current_el >= 1 && self.spsel {
                match self.current_el {
                    1 => self.sp_el1 = val,
                    2 => self.sp_el2 = val,
                    3 => self.sp_el3 = val,
                    _ => self.sp = val,
                }
            } else {
                self.sp = val;
            }
        } else {
            self.x[idx as usize] = val;
        }
    }

    /// Return the current stack pointer value based on SPSel.
    #[inline(always)]
    pub fn current_sp(&self) -> u64 {
        if self.current_el >= 1 && self.spsel {
            match self.current_el {
                1 => self.sp_el1,
                2 => self.sp_el2,
                3 => self.sp_el3,
                _ => self.sp,
            }
        } else {
            self.sp
        }
    }

    /// Check whether the MMU is enabled (SCTLR_EL1 bit 0).
    #[inline(always)]
    pub fn mmu_enabled(&self) -> bool {
        match self.current_el {
            0 | 1 => (self.sctlr_el1 & 1 != 0) || (self.hcr_el2 & 1 != 0),
            2 => self.sctlr_el2 & 1 != 0,
            3 => self.sctlr_el3 & 1 != 0,
            _ => false,
        }
    }

    // ── Condition evaluation ──────────────────────────────────────────────────

    /// Evaluate an AArch64 condition code (4-bit `cond` field).
    pub fn eval_cond(&self, cond: u32) -> bool {
        let n = self.flag_n();
        let z = self.flag_z();
        let c = self.flag_c();
        let v = self.flag_v();
        match cond & 0xF {
            0b0000 => z,              // EQ
            0b0001 => !z,             // NE
            0b0010 => c,              // CS/HS
            0b0011 => !c,             // CC/LO
            0b0100 => n,              // MI
            0b0101 => !n,             // PL
            0b0110 => v,              // VS
            0b0111 => !v,             // VC
            0b1000 => c && !z,        // HI
            0b1001 => !c || z,        // LS
            0b1010 => n == v,         // GE
            0b1011 => n != v,         // LT
            0b1100 => !z && (n == v), // GT
            0b1101 => z || (n != v),  // LE
            0b1110 | 0b1111 => true,  // AL / NV
            _ => unreachable!(),
        }
    }
}

impl ArchState for Aarch64ArchState {
    #[inline(always)]
    fn read_int_reg(&self, idx: usize) -> u64 {
        if idx < 31 {
            self.x[idx]
        } else if idx == 31 {
            0
        } else {
            self.sp
        }
    }

    #[inline(always)]
    fn write_int_reg(&mut self, idx: usize, val: u64) {
        if idx < 31 {
            self.x[idx] = val;
        }
    }

    fn read_pc(&self) -> u64 {
        self.pc
    }
    fn write_pc(&mut self, val: u64) {
        self.pc = val;
    }

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

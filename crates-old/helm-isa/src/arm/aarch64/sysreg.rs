//! AArch64 system register encoding constants and helpers.
//!
//! System register encoding: `(op0 << 14) | (op1 << 11) | (crn << 7) | (crm << 3) | op2`
//! This matches the 15-bit field extracted from MSR/MRS instruction bits [20:5].

/// Encode a system register from its fields.
pub const fn sysreg(op0: u32, op1: u32, crn: u32, crm: u32, op2: u32) -> u32 {
    (op0 << 14) | (op1 << 11) | (crn << 7) | (crm << 3) | op2
}

// ── EL1 control registers ────────────────────────────────────────────────
pub const SCTLR_EL1: u32 = sysreg(3, 0, 1, 0, 0);
pub const ACTLR_EL1: u32 = sysreg(3, 0, 1, 0, 1);
pub const CPACR_EL1: u32 = sysreg(3, 0, 1, 0, 2);

// ── Translation registers ────────────────────────────────────────────────
pub const TTBR0_EL1: u32 = sysreg(3, 0, 2, 0, 0);
pub const TTBR1_EL1: u32 = sysreg(3, 0, 2, 0, 1);
pub const TCR_EL1: u32 = sysreg(3, 0, 2, 0, 2);

// ── Fault registers ──────────────────────────────────────────────────────
pub const ESR_EL1: u32 = sysreg(3, 0, 5, 2, 0);
pub const AFSR0_EL1: u32 = sysreg(3, 0, 5, 1, 0);
pub const AFSR1_EL1: u32 = sysreg(3, 0, 5, 1, 1);
pub const FAR_EL1: u32 = sysreg(3, 0, 6, 0, 0);
pub const PAR_EL1: u32 = sysreg(3, 0, 7, 4, 0);

// ── Memory attribute registers ───────────────────────────────────────────
pub const MAIR_EL1: u32 = sysreg(3, 0, 10, 2, 0);
pub const AMAIR_EL1: u32 = sysreg(3, 0, 10, 3, 0);

// ── Vector / exception registers ─────────────────────────────────────────
pub const VBAR_EL1: u32 = sysreg(3, 0, 12, 0, 0);
pub const CONTEXTIDR_EL1: u32 = sysreg(3, 0, 13, 0, 1);

// ── Thread ID registers ──────────────────────────────────────────────────
pub const TPIDR_EL0: u32 = sysreg(3, 3, 13, 0, 2);
pub const TPIDR_EL1: u32 = sysreg(3, 0, 13, 0, 4);
pub const TPIDRRO_EL0: u32 = sysreg(3, 3, 13, 0, 3);

// ── EL1 SP / exception state ─────────────────────────────────────────────
pub const SP_EL0: u32 = sysreg(3, 0, 4, 1, 0);
pub const SP_EL1: u32 = sysreg(3, 4, 4, 1, 0);
pub const ELR_EL1: u32 = sysreg(3, 0, 4, 0, 1);
pub const SPSR_EL1: u32 = sysreg(3, 0, 4, 0, 0);
pub const CURRENT_EL: u32 = sysreg(3, 0, 4, 2, 2);
pub const DAIF: u32 = sysreg(3, 3, 4, 2, 1);
pub const NZCV: u32 = sysreg(3, 3, 4, 2, 0);
pub const SPSEL: u32 = sysreg(3, 0, 4, 2, 0);

// ── Debug registers ──────────────────────────────────────────────────────
pub const MDSCR_EL1: u32 = sysreg(2, 0, 0, 2, 2);
pub const MDCCSR_EL0: u32 = sysreg(2, 3, 0, 1, 0);

// ── Cache / TLB ──────────────────────────────────────────────────────────
pub const CSSELR_EL1: u32 = sysreg(3, 2, 0, 0, 0);
pub const CCSIDR_EL1: u32 = sysreg(3, 1, 0, 0, 0);
pub const CLIDR_EL1: u32 = sysreg(3, 1, 0, 0, 1);

// ── Timer registers ──────────────────────────────────────────────────────
pub const CNTFRQ_EL0: u32 = sysreg(3, 3, 14, 0, 0);
pub const CNTVCT_EL0: u32 = sysreg(3, 3, 14, 0, 2);
pub const CNTV_CTL_EL0: u32 = sysreg(3, 3, 14, 3, 1);
pub const CNTV_CVAL_EL0: u32 = sysreg(3, 3, 14, 3, 2);
pub const CNTV_TVAL_EL0: u32 = sysreg(3, 3, 14, 3, 0);
pub const CNTP_CTL_EL0: u32 = sysreg(3, 3, 14, 2, 1);
pub const CNTP_CVAL_EL0: u32 = sysreg(3, 3, 14, 2, 2);
pub const CNTP_TVAL_EL0: u32 = sysreg(3, 3, 14, 2, 0);
pub const CNTKCTL_EL1: u32 = sysreg(3, 0, 14, 1, 0);

// ── Counter (read-only) ──────────────────────────────────────────────────
pub const CTR_EL0: u32 = sysreg(3, 3, 0, 0, 1);
pub const DCZID_EL0: u32 = sysreg(3, 3, 0, 0, 7);

// ── ID registers (read-only) ─────────────────────────────────────────────
pub const MIDR_EL1: u32 = sysreg(3, 0, 0, 0, 0);
pub const MPIDR_EL1: u32 = sysreg(3, 0, 0, 0, 5);
pub const REVIDR_EL1: u32 = sysreg(3, 0, 0, 0, 6);
pub const ID_AA64PFR0_EL1: u32 = sysreg(3, 0, 0, 4, 0);
pub const ID_AA64PFR1_EL1: u32 = sysreg(3, 0, 0, 4, 1);
pub const ID_AA64MMFR0_EL1: u32 = sysreg(3, 0, 0, 7, 0);
pub const ID_AA64MMFR1_EL1: u32 = sysreg(3, 0, 0, 7, 1);
pub const ID_AA64MMFR2_EL1: u32 = sysreg(3, 0, 0, 7, 2);
pub const ID_AA64ISAR0_EL1: u32 = sysreg(3, 0, 0, 6, 0);
pub const ID_AA64ISAR1_EL1: u32 = sysreg(3, 0, 0, 6, 1);
pub const ID_AA64ISAR2_EL1: u32 = sysreg(3, 0, 0, 6, 2);
pub const ID_AA64DFR0_EL1: u32 = sysreg(3, 0, 0, 5, 0);
pub const ID_AA64DFR1_EL1: u32 = sysreg(3, 0, 0, 5, 1);
pub const ID_AA64AFR0_EL1: u32 = sysreg(3, 0, 0, 5, 4);
pub const ID_AA64AFR1_EL1: u32 = sysreg(3, 0, 0, 5, 5);
// Legacy AArch32 ID regs (read as zero)
pub const ID_PFR0_EL1: u32 = sysreg(3, 0, 0, 1, 0);
pub const ID_PFR1_EL1: u32 = sysreg(3, 0, 0, 1, 1);
pub const ID_PFR2_EL1: u32 = sysreg(3, 0, 0, 3, 4);
pub const ID_DFR0_EL1: u32 = sysreg(3, 0, 0, 1, 2);
pub const ID_MMFR0_EL1: u32 = sysreg(3, 0, 0, 1, 4);
pub const ID_MMFR1_EL1: u32 = sysreg(3, 0, 0, 1, 5);
pub const ID_MMFR2_EL1: u32 = sysreg(3, 0, 0, 1, 6);
pub const ID_MMFR3_EL1: u32 = sysreg(3, 0, 0, 1, 7);
pub const ID_MMFR4_EL1: u32 = sysreg(3, 0, 0, 2, 6);
pub const ID_ISAR0_EL1: u32 = sysreg(3, 0, 0, 2, 0);
pub const ID_ISAR1_EL1: u32 = sysreg(3, 0, 0, 2, 1);
pub const ID_ISAR2_EL1: u32 = sysreg(3, 0, 0, 2, 2);
pub const ID_ISAR3_EL1: u32 = sysreg(3, 0, 0, 2, 3);
pub const ID_ISAR4_EL1: u32 = sysreg(3, 0, 0, 2, 4);
pub const ID_ISAR5_EL1: u32 = sysreg(3, 0, 0, 2, 5);
pub const ID_ISAR6_EL1: u32 = sysreg(3, 0, 0, 2, 7);
pub const ID_AFR0_EL1: u32 = sysreg(3, 0, 0, 1, 3);

// ── EL2 registers ────────────────────────────────────────────────────────
pub const HCR_EL2: u32 = sysreg(3, 4, 1, 1, 0);
pub const SCTLR_EL2: u32 = sysreg(3, 4, 1, 0, 0);
pub const ACTLR_EL2: u32 = sysreg(3, 4, 1, 0, 1);
pub const TCR_EL2: u32 = sysreg(3, 4, 2, 0, 2);
pub const TTBR0_EL2: u32 = sysreg(3, 4, 2, 0, 0);
pub const TTBR1_EL2: u32 = sysreg(3, 4, 2, 0, 1); // VHE
pub const VTTBR_EL2: u32 = sysreg(3, 4, 2, 1, 0);
pub const VTCR_EL2: u32 = sysreg(3, 4, 2, 1, 2);
pub const MAIR_EL2: u32 = sysreg(3, 4, 10, 2, 0);
pub const AMAIR_EL2: u32 = sysreg(3, 4, 10, 3, 0);
pub const ESR_EL2: u32 = sysreg(3, 4, 5, 2, 0);
pub const AFSR0_EL2: u32 = sysreg(3, 4, 5, 1, 0);
pub const AFSR1_EL2: u32 = sysreg(3, 4, 5, 1, 1);
pub const FAR_EL2: u32 = sysreg(3, 4, 6, 0, 0);
pub const HPFAR_EL2: u32 = sysreg(3, 4, 6, 0, 4);
pub const VBAR_EL2: u32 = sysreg(3, 4, 12, 0, 0);
pub const ELR_EL2: u32 = sysreg(3, 4, 4, 0, 1);
pub const SPSR_EL2: u32 = sysreg(3, 4, 4, 0, 0);
pub const SP_EL2: u32 = sysreg(3, 6, 4, 1, 0);
pub const CPTR_EL2: u32 = sysreg(3, 4, 1, 1, 2);
pub const VMPIDR_EL2: u32 = sysreg(3, 4, 0, 0, 5);
pub const VPIDR_EL2: u32 = sysreg(3, 4, 0, 0, 0);
pub const MDCR_EL2: u32 = sysreg(3, 4, 1, 1, 1);
pub const HACR_EL2: u32 = sysreg(3, 4, 1, 1, 7);
pub const CONTEXTIDR_EL2: u32 = sysreg(3, 4, 13, 0, 1);
pub const TPIDR_EL2: u32 = sysreg(3, 4, 13, 0, 2);
pub const CNTHCTL_EL2: u32 = sysreg(3, 4, 14, 1, 0);
pub const CNTHP_CTL_EL2: u32 = sysreg(3, 4, 14, 2, 1);
pub const CNTHP_CVAL_EL2: u32 = sysreg(3, 4, 14, 2, 2);
pub const CNTHP_TVAL_EL2: u32 = sysreg(3, 4, 14, 2, 0);
pub const CNTVOFF_EL2: u32 = sysreg(3, 4, 14, 0, 3);

// ── EL3 registers ────────────────────────────────────────────────────────
pub const SCR_EL3: u32 = sysreg(3, 6, 1, 1, 0);
pub const SCTLR_EL3: u32 = sysreg(3, 6, 1, 0, 0);
pub const ACTLR_EL3: u32 = sysreg(3, 6, 1, 0, 1);
pub const TCR_EL3: u32 = sysreg(3, 6, 2, 0, 2);
pub const TTBR0_EL3: u32 = sysreg(3, 6, 2, 0, 0);
pub const MAIR_EL3: u32 = sysreg(3, 6, 10, 2, 0);
pub const AMAIR_EL3: u32 = sysreg(3, 6, 10, 3, 0);
pub const ESR_EL3: u32 = sysreg(3, 6, 5, 2, 0);
pub const AFSR0_EL3: u32 = sysreg(3, 6, 5, 1, 0);
pub const AFSR1_EL3: u32 = sysreg(3, 6, 5, 1, 1);
pub const FAR_EL3: u32 = sysreg(3, 6, 6, 0, 0);
pub const VBAR_EL3: u32 = sysreg(3, 6, 12, 0, 0);
pub const ELR_EL3: u32 = sysreg(3, 6, 4, 0, 1);
pub const SPSR_EL3: u32 = sysreg(3, 6, 4, 0, 0);
pub const SP_EL3: u32 = sysreg(3, 7, 4, 1, 0);
pub const MDCR_EL3: u32 = sysreg(3, 6, 1, 3, 1);
pub const CPTR_EL3: u32 = sysreg(3, 6, 1, 1, 2);
pub const TPIDR_EL3: u32 = sysreg(3, 6, 13, 0, 2);

// ── Performance monitor ──────────────────────────────────────────────────
pub const PMCR_EL0: u32 = sysreg(3, 3, 9, 12, 0);
pub const PMCNTENSET_EL0: u32 = sysreg(3, 3, 9, 12, 1);
pub const PMCNTENCLR_EL0: u32 = sysreg(3, 3, 9, 12, 2);
pub const PMOVSCLR_EL0: u32 = sysreg(3, 3, 9, 12, 3);
pub const PMUSERENR_EL0: u32 = sysreg(3, 3, 9, 14, 0);
pub const PMCCNTR_EL0: u32 = sysreg(3, 3, 9, 13, 0);
pub const PMCCFILTR_EL0: u32 = sysreg(3, 3, 14, 15, 7);
pub const PMSELR_EL0: u32 = sysreg(3, 3, 9, 12, 5);
pub const PMXEVTYPER_EL0: u32 = sysreg(3, 3, 9, 13, 1);
pub const PMXEVCNTR_EL0: u32 = sysreg(3, 3, 9, 13, 2);

// ── Floating-point control/status ─────────────────────────────────────────
pub const FPCR: u32 = sysreg(3, 3, 4, 4, 0);
pub const FPSR: u32 = sysreg(3, 3, 4, 4, 1);

// ── OS lock ──────────────────────────────────────────────────────────────
pub const OSLAR_EL1: u32 = sysreg(2, 0, 1, 0, 4);
pub const OSLSR_EL1: u32 = sysreg(2, 0, 1, 1, 4);
pub const OSDLR_EL1: u32 = sysreg(2, 0, 1, 3, 4);

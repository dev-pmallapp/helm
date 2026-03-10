//! HCR_EL2 and SCR_EL3 bit definitions for exception routing and trap control.

// ── HCR_EL2 — Hypervisor Configuration Register ────────────────────────

/// Virtualization enable (stage-2 translation).
pub const HCR_VM: u64 = 1 << 0;
/// Set/Way Invalidation Override.
pub const HCR_SWIO: u64 = 1 << 1;
/// PTW — Protected Table Walk (stage-2 for page table walks).
pub const HCR_PTW: u64 = 1 << 2;
/// FMO — FIQ Mask Override → route physical FIQ to EL2.
pub const HCR_FMO: u64 = 1 << 3;
/// IMO — IRQ Mask Override → route physical IRQ to EL2.
pub const HCR_IMO: u64 = 1 << 4;
/// AMO — SError Mask Override → route SError to EL2.
pub const HCR_AMO: u64 = 1 << 5;
/// VF — Virtual FIQ pending.
pub const HCR_VF: u64 = 1 << 6;
/// VI — Virtual IRQ pending.
pub const HCR_VI: u64 = 1 << 7;
/// VSE — Virtual SError pending.
pub const HCR_VSE: u64 = 1 << 8;
/// FB — Force Broadcast of TLB/cache maintenance.
pub const HCR_FB: u64 = 1 << 9;
/// BSU — Barrier Shareability Upgrade (bits [11:10]).
pub const HCR_BSU_MASK: u64 = 3 << 10;
/// DC — Default Cacheability (stage-1 disabled: all Normal WB).
pub const HCR_DC: u64 = 1 << 12;
/// TWI — Trap WFI to EL2.
pub const HCR_TWI: u64 = 1 << 13;
/// TWE — Trap WFE to EL2.
pub const HCR_TWE: u64 = 1 << 14;
/// TID0 — Trap ID group 0 (JIDR, REVIDR).
pub const HCR_TID0: u64 = 1 << 15;
/// TID1 — Trap ID group 1 (AIDR, CSSELR).
pub const HCR_TID1: u64 = 1 << 16;
/// TID2 — Trap ID group 2 (CCSIDR, CLIDR, CTR, CSSELR).
pub const HCR_TID2: u64 = 1 << 17;
/// TID3 — Trap ID group 3 (ID_AA64*).
pub const HCR_TID3: u64 = 1 << 18;
/// TSC — Trap SMC → EL2.
pub const HCR_TSC: u64 = 1 << 19;
/// TIDCP — Trap IMPLEMENTATION DEFINED functionality.
pub const HCR_TIDCP: u64 = 1 << 20;
/// TACR — Trap ACTLR accesses.
pub const HCR_TACR: u64 = 1 << 21;
/// TSW — Trap DC by Set/Way.
pub const HCR_TSW: u64 = 1 << 22;
/// TPCP — Trap DC/IC by Point of Coherency/Persistence.
pub const HCR_TPCP: u64 = 1 << 23;
/// TPU — Trap cache maintenance by Point of Unification.
pub const HCR_TPU: u64 = 1 << 24;
/// TTLB — Trap TLB maintenance instructions.
pub const HCR_TTLB: u64 = 1 << 25;
/// TVM — Trap Virtual Memory controls (MSR to SCTLR_EL1, TCR, TTBR, MAIR, etc.).
pub const HCR_TVM: u64 = 1 << 26;
/// TGE — Trap General Exceptions to EL2 (EL0 exceptions → EL2).
pub const HCR_TGE: u64 = 1 << 27;
/// TDZ — Trap DC ZVA.
pub const HCR_TDZ: u64 = 1 << 28;
/// HCD — HVC disable (1 = HVC UNDEFINED at EL1).
pub const HCR_HCD: u64 = 1 << 29;
/// TRVM — Trap reads of Virtual Memory controls.
pub const HCR_TRVM: u64 = 1 << 30;
/// RW — Execution state for EL1 (1 = AArch64).
pub const HCR_RW: u64 = 1 << 31;
/// CD — Stage-2 cacheability disable.
pub const HCR_CD: u64 = 1u64 << 32;
/// ID — Stage-2 instruction cacheability disable.
pub const HCR_ID: u64 = 1u64 << 33;
/// E2H — EL2 Host Enable (Virtualization Host Extensions).
pub const HCR_E2H: u64 = 1u64 << 34;
/// TLOR — Trap LOR registers.
pub const HCR_TLOR: u64 = 1u64 << 35;
/// TERR — Trap Error record accesses.
pub const HCR_TERR: u64 = 1u64 << 36;
/// TEA — Trap External Aborts to EL2.
pub const HCR_TEA: u64 = 1u64 << 37;
/// APK — Trap pointer authentication key accesses.
pub const HCR_APK: u64 = 1u64 << 40;
/// API — Trap pointer authentication instructions.
pub const HCR_API: u64 = 1u64 << 41;

// ── SCR_EL3 — Secure Configuration Register ────────────────────────────

/// NS — Non-Secure bit. When 1, EL0/EL1 are Non-secure.
pub const SCR_NS: u64 = 1 << 0;
/// IRQ — Physical IRQ routing. When 1, IRQs taken to EL3.
pub const SCR_IRQ: u64 = 1 << 1;
/// FIQ — Physical FIQ routing. When 1, FIQs taken to EL3.
pub const SCR_FIQ: u64 = 1 << 2;
/// EA — External Abort / SError routing. When 1, taken to EL3.
pub const SCR_EA: u64 = 1 << 3;
/// SMD — Secure Monitor Disable. When 1, SMC is UNDEFINED at EL1/EL2.
pub const SCR_SMD: u64 = 1 << 7;
/// HCE — Hypervisor Call Enable. When 1, HVC is enabled at EL1/EL2.
pub const SCR_HCE: u64 = 1 << 8;
/// SIF — Secure Instruction Fetch. When 1, prevents Non-secure fetch of Secure memory.
pub const SCR_SIF: u64 = 1 << 9;
/// RW — Execution state for EL2 (1 = AArch64).
pub const SCR_RW: u64 = 1 << 10;
/// ST — Secure Timer access. When 1, Secure EL1 can access physical timer.
pub const SCR_ST: u64 = 1 << 11;
/// TWI — Trap WFI to EL3.
pub const SCR_TWI: u64 = 1 << 12;
/// TWE — Trap WFE to EL3.
pub const SCR_TWE: u64 = 1 << 13;
/// TLOR — Trap LOR registers to EL3.
pub const SCR_TLOR: u64 = 1 << 14;
/// TERR — Trap Error record accesses to EL3.
pub const SCR_TERR: u64 = 1 << 15;
/// APK — Trap pointer authentication key accesses to EL3.
pub const SCR_APK: u64 = 1 << 16;
/// API — Trap pointer authentication instructions to EL3.
pub const SCR_API: u64 = 1 << 17;

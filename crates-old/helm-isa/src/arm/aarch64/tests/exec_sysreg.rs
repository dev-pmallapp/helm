//! Tests for AArch64 system register access, exception entry/return,
//! DAIF masking, SP selection, and ERET.

use crate::arm::aarch64::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;

fn cpu_with_code(insns: &[u32]) -> (Aarch64Cpu, AddressSpace) {
    let mut cpu = Aarch64Cpu::new();
    let mut mem = AddressSpace::new();
    let base = 0x40_0000u64;
    let size = (insns.len() * 4 + 0x1000) as u64;
    mem.map(base, size, (true, true, true));
    for (i, insn) in insns.iter().enumerate() {
        let addr = base + (i as u64 * 4);
        mem.write(addr, &insn.to_le_bytes()).unwrap();
    }
    cpu.regs.pc = base;
    mem.map(0x7FFF_0000, 0x10000, (true, true, false));
    cpu.regs.sp = 0x7FFF_8000;
    (cpu, mem)
}

const NOP: u32 = 0xD503_201F;
const BASE: u64 = 0x40_0000;

// ═══════════════════════════════════════════════════════════════════
//  MSR / MRS system register encoding helpers
// ═══════════════════════════════════════════════════════════════════

/// Encode MRS Xt, <sysreg>: 1101_0101_00_1_1_o0_op1_CRn_CRm_op2_Rt
/// bit 21 = L=1 (read), bit 20 = 1 (op0 >= 2)
fn encode_mrs(rt: u32, o0: u32, op1: u32, crn: u32, crm: u32, op2: u32) -> u32 {
    0xD500_0000
        | (1 << 21)
        | (1 << 20)
        | (o0 << 19)
        | (op1 << 16)
        | (crn << 12)
        | (crm << 8)
        | (op2 << 5)
        | rt
}

/// Encode MSR <sysreg>, Xt: 1101_0101_00_0_1_o0_op1_CRn_CRm_op2_Rt
/// bit 21 = L=0 (write), bit 20 = 1 (op0 >= 2)
fn encode_msr(rt: u32, o0: u32, op1: u32, crn: u32, crm: u32, op2: u32) -> u32 {
    0xD500_0000
        | (0 << 21)
        | (1 << 20)
        | (o0 << 19)
        | (op1 << 16)
        | (crn << 12)
        | (crm << 8)
        | (op2 << 5)
        | rt
}

/// MSR DAIFSet, #imm: 1101_0101_00000_011_0100_imm:4_110_11111
fn encode_msr_daifset(imm: u32) -> u32 {
    0xD503_40DF | ((imm & 0xF) << 8)
}

/// MSR DAIFClr, #imm: 1101_0101_00000_011_0100_imm:4_111_11111
fn encode_msr_daifclr(imm: u32) -> u32 {
    0xD503_40FF | ((imm & 0xF) << 8)
}

/// MSR SPSel, #imm: 1101_0101_00000_000_0100_imm:4_101_11111
fn encode_msr_spsel(imm: u32) -> u32 {
    0xD500_40BF | ((imm & 0xF) << 8)
}

/// ERET: 0xD69F_03E0
const ERET: u32 = 0xD69F_03E0;

// ═══════════════════════════════════════════════════════════════════
//  MRS/MSR to named system registers
// ═══════════════════════════════════════════════════════════════════

#[test]
fn mrs_current_el() {
    // MRS X0, CurrentEL: op0=3(o0=1), op1=0, CRn=4, CRm=2, op2=2
    let (mut c, mut m) = cpu_with_code(&[encode_mrs(0, 1, 0, 4, 2, 2)]);
    c.regs.current_el = 1;
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 1 << 2, "CurrentEL should be EL1 << 2 = 4");
}

#[test]
fn msr_mrs_vbar_el1() {
    // MSR VBAR_EL1, X1: op0=3(o0=1), op1=0, CRn=12, CRm=0, op2=0
    // MRS X2, VBAR_EL1
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 1, 0, 12, 0, 0), encode_mrs(2, 1, 0, 12, 0, 0)]);
    c.set_xn(1, 0xFFFF_0000_1000_0000);
    c.step(&mut m).unwrap();
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(2), 0xFFFF_0000_1000_0000);
    assert_eq!(c.regs.vbar_el1, 0xFFFF_0000_1000_0000);
}

#[test]
fn msr_mrs_sctlr_el1() {
    let (mut c, mut m) = cpu_with_code(&[
        encode_msr(1, 1, 0, 1, 0, 0), // MSR SCTLR_EL1, X1
        encode_mrs(2, 1, 0, 1, 0, 0), // MRS X2, SCTLR_EL1
    ]);
    c.set_xn(1, 0xDEAD_BEEE); // bit 0 clear → MMU stays off
    c.step(&mut m).unwrap();
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(2), 0xDEAD_BEEE);
}

#[test]
fn msr_mrs_ttbr0_el1() {
    let (mut c, mut m) = cpu_with_code(&[
        encode_msr(1, 1, 0, 2, 0, 0), // MSR TTBR0_EL1, X1
        encode_mrs(2, 1, 0, 2, 0, 0), // MRS X2, TTBR0_EL1
    ]);
    c.set_xn(1, 0x4000_0000);
    c.step(&mut m).unwrap();
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(2), 0x4000_0000);
}

#[test]
fn msr_mrs_tcr_el1() {
    let (mut c, mut m) = cpu_with_code(&[
        encode_msr(3, 1, 0, 2, 0, 2), // MSR TCR_EL1, X3
        encode_mrs(4, 1, 0, 2, 0, 2), // MRS X4, TCR_EL1
    ]);
    c.set_xn(3, 0x0000_0000_B510_1510);
    c.step(&mut m).unwrap();
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(4), 0x0000_0000_B510_1510);
}

#[test]
fn msr_mrs_mair_el1() {
    let (mut c, mut m) = cpu_with_code(&[
        encode_msr(1, 1, 0, 10, 2, 0), // MSR MAIR_EL1, X1
        encode_mrs(2, 1, 0, 10, 2, 0), // MRS X2, MAIR_EL1
    ]);
    c.set_xn(1, 0xFF44_00BB_0400_FFCC);
    c.step(&mut m).unwrap();
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(2), 0xFF44_00BB_0400_FFCC);
}

#[test]
fn mrs_midr_el1() {
    // MRS X0, MIDR_EL1: op0=3(o0=1), op1=0, CRn=0, CRm=0, op2=0
    let (mut c, mut m) = cpu_with_code(&[encode_mrs(0, 1, 0, 0, 0, 0)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x410F_D034, "MIDR should be Cortex-A53");
}

#[test]
fn mrs_ctr_el0() {
    // MRS X0, CTR_EL0: op0=3(o0=1), op1=3, CRn=0, CRm=0, op2=1
    let (mut c, mut m) = cpu_with_code(&[encode_mrs(0, 1, 3, 0, 0, 1)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x8444_C004);
}

#[test]
fn mrs_cntfrq_el0() {
    // MRS X0, CNTFRQ_EL0: op0=3(o0=1), op1=3, CRn=14, CRm=0, op2=0
    let (mut c, mut m) = cpu_with_code(&[encode_mrs(0, 1, 3, 14, 0, 0)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 62_500_000);
}

// ═══════════════════════════════════════════════════════════════════
//  DAIF masking
// ═══════════════════════════════════════════════════════════════════

#[test]
fn daifset_masks_interrupts() {
    let (mut c, mut m) = cpu_with_code(&[encode_msr_daifset(0xF)]);
    c.regs.daif = 0;
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.daif, 0x3C0, "DAIFSet #0xF should set bits [9:6]");
}

#[test]
fn daifclr_unmasks_interrupts() {
    let (mut c, mut m) = cpu_with_code(&[encode_msr_daifclr(0xF)]);
    c.regs.daif = 0x3C0;
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.daif, 0, "DAIFClr #0xF should clear bits [9:6]");
}

#[test]
fn daifset_partial() {
    // Set only I and F (bits 1 and 0 of imm → bits 7 and 6 of DAIF)
    let (mut c, mut m) = cpu_with_code(&[encode_msr_daifset(0x3)]);
    c.regs.daif = 0;
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.daif, 0x0C0, "DAIFSet #3 should set I and F only");
}

#[test]
fn daifclr_partial() {
    let (mut c, mut m) = cpu_with_code(&[encode_msr_daifclr(0x3)]);
    c.regs.daif = 0x3C0;
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.daif, 0x300, "DAIFClr #3 should clear I and F only");
}

// ═══════════════════════════════════════════════════════════════════
//  SP selection
// ═══════════════════════════════════════════════════════════════════

#[test]
fn spsel_switches_sp() {
    let (mut c, mut m) = cpu_with_code(&[encode_msr_spsel(1), NOP]);
    c.regs.current_el = 1;
    c.regs.sp = 0x1000; // SP_EL0
    c.regs.sp_el1 = 0x2000; // SP_EL1
    c.regs.sp_sel = 0; // using SP_EL0
    assert_eq!(c.current_sp(), 0x1000);
    c.step(&mut m).unwrap(); // MSR SPSel, #1
    assert_eq!(c.regs.sp_sel, 1);
    assert_eq!(c.current_sp(), 0x2000, "after SPSel=1, SP should be SP_EL1");
}

// ═══════════════════════════════════════════════════════════════════
//  Exception entry and ERET
// ═══════════════════════════════════════════════════════════════════

#[test]
fn exception_entry_saves_state() {
    let (mut c, mut m) = cpu_with_code(&[0xD400_0001]); // SVC #0
    c.regs.vbar_el1 = 0x1_0000;
    c.regs.nzcv = 0xA000_0000; // N and C set
    c.regs.daif = 0x080; // only I masked
    c.regs.sp_sel = 0;
    c.regs.current_el = 0; // from EL0

    c.step(&mut m).unwrap();

    // Should be at VBAR + 0x400 (lower EL, AArch64)
    assert_eq!(c.regs.pc, 0x1_0000 + 0x400);
    assert_eq!(c.regs.current_el, 1);
    assert_eq!(c.regs.sp_sel, 1);
    assert_eq!(c.regs.daif, 0x3C0, "all interrupts masked");
    assert_eq!(c.regs.elr_el1, BASE + 4, "ELR should be PC+4 for SVC");
    // SPSR should contain saved NZCV + DAIF + EL + SP
    let expected_spsr = 0xA000_0000 | 0x080 | (0 << 2) | 0;
    assert_eq!(c.regs.spsr_el1, expected_spsr);
}

#[test]
fn eret_restores_state() {
    // Map exception vector area and ERET landing
    let mut cpu = Aarch64Cpu::new();
    let mut mem = AddressSpace::new();

    // Map code area and ERET landing
    mem.map(0x40_0000, 0x10000, (true, true, true));
    mem.map(0x7FFF_0000, 0x10000, (true, true, false));

    // Write ERET at the vector entry point
    let eret_addr = 0x40_0000u64;
    mem.write(eret_addr, &ERET.to_le_bytes()).unwrap();

    // Set up as if we took an exception and now want to return
    cpu.regs.pc = eret_addr;
    cpu.regs.current_el = 1;
    cpu.regs.sp_sel = 1;
    cpu.regs.daif = 0x3C0;
    cpu.regs.elr_el1 = 0x50_0000;
    // Saved state: EL0, SP_EL0, NZCV=Z, DAIF unmasked
    cpu.regs.spsr_el1 = (1 << 30) | 0; // Z=1, EL0, SP_EL0

    cpu.step(&mut mem).unwrap();

    assert_eq!(cpu.regs.pc, 0x50_0000, "PC restored from ELR_EL1");
    assert_eq!(cpu.regs.current_el, 0, "restored to EL0");
    assert_eq!(cpu.regs.sp_sel, 0, "restored to SP_EL0");
    assert_eq!(cpu.regs.nzcv, 1 << 30, "Z flag restored");
    assert_eq!(cpu.regs.daif, 0, "DAIF restored to unmasked");
}

#[test]
fn exception_roundtrip() {
    // Take exception from EL0 → EL1, then ERET back to EL0
    let mut cpu = Aarch64Cpu::new();
    let mut mem = AddressSpace::new();

    // Map code + stack + vector area
    mem.map(0x40_0000, 0x20000, (true, true, true));
    mem.map(0x7FFF_0000, 0x10000, (true, true, false));

    // SVC at 0x40_0000
    let svc_addr = 0x40_0000u64;
    mem.write(svc_addr, &0xD400_0001u32.to_le_bytes()).unwrap();

    // ERET at vector entry (VBAR + 0x400)
    let vbar = 0x40_1000u64;
    let vector_entry = vbar + 0x400;
    mem.write(vector_entry, &ERET.to_le_bytes()).unwrap();

    // NOP at return point (SVC + 4)
    mem.write(svc_addr + 4, &NOP.to_le_bytes()).unwrap();

    cpu.regs.pc = svc_addr;
    cpu.regs.current_el = 0;
    cpu.regs.sp_sel = 0;
    cpu.regs.sp = 0x7FFF_8000;
    cpu.regs.vbar_el1 = vbar;
    cpu.regs.nzcv = 0x6000_0000; // Z and C
    cpu.regs.daif = 0;

    // Step 1: SVC → takes exception to EL1
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.current_el, 1);
    assert_eq!(cpu.regs.pc, vector_entry);
    assert_eq!(cpu.regs.elr_el1, svc_addr + 4);

    // Step 2: ERET → returns to EL0
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.current_el, 0);
    assert_eq!(cpu.regs.pc, svc_addr + 4, "ERET returns past SVC");
    assert_eq!(cpu.regs.nzcv, 0x6000_0000, "NZCV restored");
    assert_eq!(cpu.regs.daif, 0, "DAIF restored");
}

// ═══════════════════════════════════════════════════════════════════
//  SYS instructions (cache/TLB maintenance) — NOP
// ═══════════════════════════════════════════════════════════════════

#[test]
fn sys_dc_civac_is_nop() {
    // SYS #3, C7, C14, #1, X0  (DC CIVAC)
    // 1101_0101_00_0_01_011_0111_1110_001_00000 = 0xD50B_7E20
    let (mut c, mut m) = cpu_with_code(&[0xD50B_7E20]);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 4, "DC CIVAC should NOP");
}

#[test]
fn sys_tlbi_vmalle1is_is_nop() {
    // TLBI VMALLE1IS: SYS #0, C8, C3, #0, XZR
    // 1101_0101_00_0_01_000_1000_0011_000_11111 = 0xD508_831F
    let (mut c, mut m) = cpu_with_code(&[0xD508_831F]);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 4, "TLBI should NOP");
}

#[test]
fn sys_ic_iallu_is_nop() {
    // IC IALLU: SYS #0, C7, C5, #0, XZR
    // 1101_0101_00_0_01_000_0111_0101_000_11111 = 0xD508_751F
    let (mut c, mut m) = cpu_with_code(&[0xD508_751F]);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 4, "IC IALLU should NOP");
}

// ═══════════════════════════════════════════════════════════════════
//  unimpl() returns Err instead of panic
// ═══════════════════════════════════════════════════════════════════

#[test]
fn unimpl_returns_isa_error() {
    // In SE mode, unimplemented instructions return HelmError::Isa.
    // 0xD400_0000 falls in branch/sys space but matches no pattern.
    let (mut c, mut m) = cpu_with_code(&[0xD400_0000]);
    c.set_se_mode(true);
    let result = c.step(&mut m);
    assert!(result.is_err());
    match result.unwrap_err() {
        helm_core::HelmError::Isa(msg) => assert!(msg.contains("unimplemented")),
        other => panic!("expected Isa error, got {other:?}"),
    }
}

#[test]
fn unimpl_takes_undef_exception_in_fs_mode() {
    let (mut c, mut m) = cpu_with_code(&[0xD400_0000]);
    c.regs.current_el = 1;
    c.regs.sp_sel = 1;
    c.regs.vbar_el1 = 0x1_0000;
    let mut vbar_mem = AddressSpace::new();
    vbar_mem.map(0x1_0000, 0x1000, (true, true, true));
    // Write a NOP at the exception vector so a subsequent step doesn't fault.
    vbar_mem
        .write(0x1_0200, &0xD503201Fu32.to_le_bytes())
        .unwrap();
    // We can't easily merge two AddressSpaces, so just map the vector in m
    m.map(0x1_0000, 0x1000, (true, true, true));
    m.write(0x1_0200, &0xD503201Fu32.to_le_bytes()).unwrap();
    let result = c.step(&mut m);
    // step() catches the Pipeline error from unimpl and returns Ok
    assert!(result.is_ok());
    // PC should now be at VBAR + 0x200 (current EL, SP_ELx)
    assert_eq!(c.regs.pc, 0x1_0200);
    // ESR should have EC=0x00 (unknown reason)
    assert_eq!((c.regs.esr_el1 >> 26) & 0x3F, 0x00);
}

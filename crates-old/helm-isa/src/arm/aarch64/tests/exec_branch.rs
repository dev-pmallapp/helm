//! AArch64 Branch & System instruction tests.
//!
//! Covers: B, BL, B.cond (all 15 conditions), CBZ/CBNZ, TBZ/TBNZ,
//! BR/BLR/RET, SVC, BRK, NOP.

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

fn set_nzcv(cpu: &mut Aarch64Cpu, n: bool, z: bool, c: bool, v: bool) {
    cpu.regs.nzcv =
        ((n as u32) << 31) | ((z as u32) << 30) | ((c as u32) << 29) | ((v as u32) << 28);
}

// Encoding helpers
fn encode_b(imm26: i32) -> u32 {
    let imm = (imm26 as u32) & 0x03FF_FFFF;
    (0b00101 << 26) | imm
}
fn encode_bl(imm26: i32) -> u32 {
    let imm = (imm26 as u32) & 0x03FF_FFFF;
    (0b100101 << 26) | imm
}
fn encode_b_cond(imm19: i32, cond: u32) -> u32 {
    let imm = (imm19 as u32) & 0x7FFFF;
    (0b01010100 << 24) | (imm << 5) | cond
}
fn encode_cbz(sf: u32, imm19: i32, rt: u32) -> u32 {
    let imm = (imm19 as u32) & 0x7FFFF;
    (sf << 31) | (0b011010_0 << 24) | (imm << 5) | rt
}
fn encode_cbnz(sf: u32, imm19: i32, rt: u32) -> u32 {
    let imm = (imm19 as u32) & 0x7FFFF;
    (sf << 31) | (0b011010_1 << 24) | (imm << 5) | rt
}
fn encode_tbz(b5: u32, b40: u32, imm14: i32, rt: u32) -> u32 {
    let imm = (imm14 as u32) & 0x3FFF;
    (b5 << 31) | (0b011011_0 << 24) | (b40 << 19) | (imm << 5) | rt
}
fn encode_tbnz(b5: u32, b40: u32, imm14: i32, rt: u32) -> u32 {
    let imm = (imm14 as u32) & 0x3FFF;
    (b5 << 31) | (0b011011_1 << 24) | (b40 << 19) | (imm << 5) | rt
}
const NOP: u32 = 0xD503_201F;
fn encode_br(rn: u32) -> u32 {
    0xD61F_0000 | (rn << 5)
}
fn encode_blr(rn: u32) -> u32 {
    0xD63F_0000 | (rn << 5)
}
fn encode_ret(rn: u32) -> u32 {
    0xD65F_0000 | (rn << 5)
}

const BASE: u64 = 0x40_0000;

// Condition codes
const EQ: u32 = 0;
const NE: u32 = 1;
const CS: u32 = 2;
const CC: u32 = 3;
const MI: u32 = 4;
const PL: u32 = 5;
const VS: u32 = 6;
const VC: u32 = 7;
const HI: u32 = 8;
const LS: u32 = 9;
const GE: u32 = 10;
const LT: u32 = 11;
const GT: u32 = 12;
const LE: u32 = 13;
const AL: u32 = 14;

// ===================================================================
//  B / BL
// ===================================================================

#[test]
fn b_forward_4() {
    let (mut c, mut m) = cpu_with_code(&[encode_b(1), NOP]);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 4);
}
#[test]
fn b_forward_8() {
    let (mut c, mut m) = cpu_with_code(&[encode_b(2), NOP, NOP]);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 8);
}
#[test]
fn b_forward_1024() {
    let (mut c, mut m) = cpu_with_code(&[encode_b(256)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 1024);
}
#[test]
fn bl_sets_lr() {
    let (mut c, mut m) = cpu_with_code(&[encode_bl(2), NOP, NOP]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(30), BASE + 4, "LR = next insn");
    assert_eq!(c.regs.pc, BASE + 8);
}
#[test]
fn bl_far() {
    let (mut c, mut m) = cpu_with_code(&[encode_bl(0x100)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 0x400);
}

// ===================================================================
//  B.cond — all 15 conditions
// ===================================================================

fn test_bcond(cond: u32, n: bool, z: bool, c: bool, v: bool, expect_taken: bool) {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_b_cond(3, cond), NOP, NOP, NOP]);
    set_nzcv(&mut cpu, n, z, c, v);
    cpu.step(&mut mem).unwrap();
    if expect_taken {
        assert_eq!(cpu.regs.pc, BASE + 12, "B.{cond} should be taken");
    } else {
        assert_eq!(cpu.regs.pc, BASE + 4, "B.{cond} should fall through");
    }
}

#[test]
fn bcond_eq_taken() {
    test_bcond(EQ, false, true, false, false, true);
}
#[test]
fn bcond_eq_not() {
    test_bcond(EQ, false, false, false, false, false);
}
#[test]
fn bcond_ne_taken() {
    test_bcond(NE, false, false, false, false, true);
}
#[test]
fn bcond_ne_not() {
    test_bcond(NE, false, true, false, false, false);
}
#[test]
fn bcond_cs_taken() {
    test_bcond(CS, false, false, true, false, true);
}
#[test]
fn bcond_cs_not() {
    test_bcond(CS, false, false, false, false, false);
}
#[test]
fn bcond_cc_taken() {
    test_bcond(CC, false, false, false, false, true);
}
#[test]
fn bcond_cc_not() {
    test_bcond(CC, false, false, true, false, false);
}
#[test]
fn bcond_mi_taken() {
    test_bcond(MI, true, false, false, false, true);
}
#[test]
fn bcond_mi_not() {
    test_bcond(MI, false, false, false, false, false);
}
#[test]
fn bcond_pl_taken() {
    test_bcond(PL, false, false, false, false, true);
}
#[test]
fn bcond_pl_not() {
    test_bcond(PL, true, false, false, false, false);
}
#[test]
fn bcond_vs_taken() {
    test_bcond(VS, false, false, false, true, true);
}
#[test]
fn bcond_vs_not() {
    test_bcond(VS, false, false, false, false, false);
}
#[test]
fn bcond_vc_taken() {
    test_bcond(VC, false, false, false, false, true);
}
#[test]
fn bcond_vc_not() {
    test_bcond(VC, false, false, false, true, false);
}
#[test]
fn bcond_hi_taken() {
    test_bcond(HI, false, false, true, false, true);
}
#[test]
fn bcond_hi_not_z() {
    test_bcond(HI, false, true, true, false, false);
}
#[test]
fn bcond_hi_not_c() {
    test_bcond(HI, false, false, false, false, false);
}
#[test]
fn bcond_ls_taken_z() {
    test_bcond(LS, false, true, false, false, true);
}
#[test]
fn bcond_ls_taken_nc() {
    test_bcond(LS, false, false, false, false, true);
}
#[test]
fn bcond_ls_not() {
    test_bcond(LS, false, false, true, false, false);
}
#[test]
fn bcond_ge_taken_pp() {
    test_bcond(GE, false, false, false, false, true);
}
#[test]
fn bcond_ge_taken_nn() {
    test_bcond(GE, true, false, false, true, true);
}
#[test]
fn bcond_ge_not() {
    test_bcond(GE, true, false, false, false, false);
}
#[test]
fn bcond_lt_taken() {
    test_bcond(LT, true, false, false, false, true);
}
#[test]
fn bcond_lt_not() {
    test_bcond(LT, false, false, false, false, false);
}
#[test]
fn bcond_gt_taken() {
    test_bcond(GT, false, false, false, false, true);
}
#[test]
fn bcond_gt_not_z() {
    test_bcond(GT, false, true, false, false, false);
}
#[test]
fn bcond_gt_not_lt() {
    test_bcond(GT, true, false, false, false, false);
}
#[test]
fn bcond_le_taken_z() {
    test_bcond(LE, false, true, false, false, true);
}
#[test]
fn bcond_le_taken_lt() {
    test_bcond(LE, true, false, false, false, true);
}
#[test]
fn bcond_le_not() {
    test_bcond(LE, false, false, false, false, false);
}
#[test]
fn bcond_al_taken() {
    test_bcond(AL, false, false, false, false, true);
}
#[test]
fn bcond_al_with_flags() {
    test_bcond(AL, true, true, true, true, true);
}

// ===================================================================
//  CBZ / CBNZ — 32 and 64-bit
// ===================================================================

#[test]
fn cbz_64_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_cbz(1, 2, 0), NOP, NOP]);
    c.set_xn(0, 0);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 8);
}
#[test]
fn cbz_64_not() {
    let (mut c, mut m) = cpu_with_code(&[encode_cbz(1, 2, 0), NOP, NOP]);
    c.set_xn(0, 1);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 4);
}
#[test]
fn cbz_32_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_cbz(0, 2, 0), NOP, NOP]);
    c.set_xn(0, 0x1_0000_0000);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 8, "32-bit CBZ ignores upper bits");
}
#[test]
fn cbz_32_not() {
    let (mut c, mut m) = cpu_with_code(&[encode_cbz(0, 2, 0), NOP, NOP]);
    c.set_xn(0, 1);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 4);
}
#[test]
fn cbnz_64_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_cbnz(1, 2, 0), NOP, NOP]);
    c.set_xn(0, 42);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 8);
}
#[test]
fn cbnz_64_not() {
    let (mut c, mut m) = cpu_with_code(&[encode_cbnz(1, 2, 0), NOP, NOP]);
    c.set_xn(0, 0);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 4);
}
#[test]
fn cbnz_32_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_cbnz(0, 2, 0), NOP, NOP]);
    c.set_xn(0, 0xFF);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 8);
}

// ===================================================================
//  TBZ / TBNZ
// ===================================================================

#[test]
fn tbz_bit0_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_tbz(0, 0, 2, 0), NOP, NOP]);
    c.set_xn(0, 0xFFFE);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 8, "bit 0 clear");
}
#[test]
fn tbz_bit0_not() {
    let (mut c, mut m) = cpu_with_code(&[encode_tbz(0, 0, 2, 0), NOP, NOP]);
    c.set_xn(0, 0xFFFF);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 4, "bit 0 set");
}
#[test]
fn tbz_bit31_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_tbz(0, 31, 2, 0), NOP, NOP]);
    c.set_xn(0, 0x7FFF_FFFF);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 8);
}
#[test]
fn tbz_bit31_not() {
    let (mut c, mut m) = cpu_with_code(&[encode_tbz(0, 31, 2, 0), NOP, NOP]);
    c.set_xn(0, 0x8000_0000);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 4);
}
#[test]
fn tbz_bit63_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_tbz(1, 31, 2, 0), NOP, NOP]);
    c.set_xn(0, 0x7FFF_FFFF_FFFF_FFFF);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 8);
}
#[test]
fn tbz_bit63_not() {
    let (mut c, mut m) = cpu_with_code(&[encode_tbz(1, 31, 2, 0), NOP, NOP]);
    c.set_xn(0, 0x8000_0000_0000_0000);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 4);
}
#[test]
fn tbnz_bit0_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_tbnz(0, 0, 2, 0), NOP, NOP]);
    c.set_xn(0, 1);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 8);
}
#[test]
fn tbnz_bit0_not() {
    let (mut c, mut m) = cpu_with_code(&[encode_tbnz(0, 0, 2, 0), NOP, NOP]);
    c.set_xn(0, 0);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 4);
}
#[test]
fn tbnz_bit16() {
    let (mut c, mut m) = cpu_with_code(&[encode_tbnz(0, 16, 2, 0), NOP, NOP]);
    c.set_xn(0, 1 << 16);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 8);
}

// ===================================================================
//  BR / BLR / RET
// ===================================================================

#[test]
fn br_to_addr() {
    let (mut c, mut m) = cpu_with_code(&[encode_br(1)]);
    c.set_xn(1, 0x50_0000);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, 0x50_0000);
}
#[test]
fn blr_sets_lr() {
    let (mut c, mut m) = cpu_with_code(&[encode_blr(1)]);
    c.set_xn(1, 0x50_0000);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, 0x50_0000);
    assert_eq!(c.xn(30), BASE + 4);
}
#[test]
fn ret_default() {
    let (mut c, mut m) = cpu_with_code(&[encode_ret(30)]);
    c.set_xn(30, 0x60_0000);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, 0x60_0000);
}
#[test]
fn ret_custom_reg() {
    let (mut c, mut m) = cpu_with_code(&[encode_ret(5)]);
    c.set_xn(5, 0x70_0000);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, 0x70_0000);
}

// ===================================================================
//  SVC / BRK
// ===================================================================

#[test]
fn svc_from_el0_takes_exception() {
    let (mut c, mut m) = cpu_with_code(&[0xD400_0001]); // SVC #0
                                                        // EL0 by default — SVC dispatches to VBAR_EL1 + 0x400
    c.regs.vbar_el1 = 0x1000;
    c.step(&mut m).unwrap(); // should succeed (exception taken)
    assert_eq!(c.regs.current_el, 1);
    assert_eq!(c.regs.pc, 0x1000 + 0x400); // lower EL vector
    assert_eq!(c.regs.elr_el1, BASE + 4); // preferred return address (PC+4 for SVC)
}

#[test]
fn svc_from_el1_raises_syscall() {
    let (mut c, mut m) = cpu_with_code(&[0xD400_0001]); // SVC #0
    c.regs.current_el = 1;
    c.set_xn(8, 42); // syscall number
                     // SVC from EL1 always returns Syscall error (for engine handling)
    let err = c.step(&mut m).unwrap_err();
    match err {
        helm_core::HelmError::Syscall { number, .. } => assert_eq!(number, 42),
        other => panic!("expected Syscall, got {other:?}"),
    }
}

#[test]
fn brk_raises_decode_error() {
    let (mut c, mut m) = cpu_with_code(&[0xD420_0000]); // BRK #0
    c.set_se_mode(true); // BRK returns Decode error only in SE mode
    let err = c.step(&mut m).unwrap_err();
    match err {
        helm_core::HelmError::Decode { reason, .. } => assert!(reason.contains("BRK")),
        other => panic!("expected Decode/BRK, got {other:?}"),
    }
}

#[test]
fn nop_advances_pc() {
    let (mut c, mut m) = cpu_with_code(&[NOP, NOP]);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 4);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, BASE + 8);
}

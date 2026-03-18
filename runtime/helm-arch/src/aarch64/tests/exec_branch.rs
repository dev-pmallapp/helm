//! AArch64 Branch & System tests. Ported from exec_branch.rs.
//! Tests using HelmError / set_se_mode are marked #[ignore].
use super::harness::*;

const BASE: u64 = CODE_BASE;
const NOP: u32 = 0xD503_201F;

fn encode_b(imm26: i32) -> u32 { (0b00101 << 26) | ((imm26 as u32) & 0x03FF_FFFF) }
fn encode_bl(imm26: i32) -> u32 { (0b100101 << 26) | ((imm26 as u32) & 0x03FF_FFFF) }
fn encode_b_cond(imm19: i32, cond: u32) -> u32 {
    (0b01010100 << 24) | (((imm19 as u32) & 0x7FFFF) << 5) | cond
}
fn encode_cbz(sf: u32, imm19: i32, rt: u32) -> u32 {
    (sf << 31) | (0b011010_0 << 24) | (((imm19 as u32) & 0x7FFFF) << 5) | rt
}
fn encode_cbnz(sf: u32, imm19: i32, rt: u32) -> u32 {
    (sf << 31) | (0b011010_1 << 24) | (((imm19 as u32) & 0x7FFFF) << 5) | rt
}
fn encode_tbz(b5: u32, b40: u32, imm14: i32, rt: u32) -> u32 {
    (b5 << 31) | (0b011011_0 << 24) | (b40 << 19) | (((imm14 as u32) & 0x3FFF) << 5) | rt
}
fn encode_tbnz(b5: u32, b40: u32, imm14: i32, rt: u32) -> u32 {
    (b5 << 31) | (0b011011_1 << 24) | (b40 << 19) | (((imm14 as u32) & 0x3FFF) << 5) | rt
}
fn encode_br(rn: u32) -> u32 { 0xD61F_0000 | (rn << 5) }
fn encode_blr(rn: u32) -> u32 { 0xD63F_0000 | (rn << 5) }
fn encode_ret(rn: u32) -> u32 { 0xD65F_0000 | (rn << 5) }

const EQ: u32 = 0; const NE: u32 = 1; const CS: u32 = 2; const CC: u32 = 3;
const MI: u32 = 4; const PL: u32 = 5; const VS: u32 = 6; const VC: u32 = 7;
const HI: u32 = 8; const LS: u32 = 9; const GE: u32 = 10; const LT: u32 = 11;
const GT: u32 = 12; const LE: u32 = 13; const AL: u32 = 14;

#[test]
fn b_forward_4() {
    let (mut c, mut m) = cpu_with_code(&[encode_b(1), NOP]);
    step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, BASE + 4);
}
#[test]
fn b_forward_8() {
    let (mut c, mut m) = cpu_with_code(&[encode_b(2), NOP, NOP]);
    step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, BASE + 8);
}
#[test]
fn bl_sets_lr() {
    let (mut c, mut m) = cpu_with_code(&[encode_bl(2), NOP, NOP]);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[30], BASE + 4); assert_eq!(c.pc, BASE + 8);
}

fn test_bcond(cond: u32, n: bool, z: bool, cv: bool, v: bool, expect_taken: bool) {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_b_cond(3, cond), NOP, NOP, NOP]);
    set_nzcv(&mut cpu, n, z, cv, v);
    step(&mut cpu, &mut mem).unwrap();
    if expect_taken {
        assert_eq!(cpu.pc, BASE + 12, "B.{cond} should be taken");
    } else {
        assert_eq!(cpu.pc, BASE + 4, "B.{cond} should fall through");
    }
}

#[test] fn bcond_eq_taken() { test_bcond(EQ, false, true, false, false, true); }
#[test] fn bcond_eq_not() { test_bcond(EQ, false, false, false, false, false); }
#[test] fn bcond_ne_taken() { test_bcond(NE, false, false, false, false, true); }
#[test] fn bcond_ne_not() { test_bcond(NE, false, true, false, false, false); }
#[test] fn bcond_cs_taken() { test_bcond(CS, false, false, true, false, true); }
#[test] fn bcond_cs_not() { test_bcond(CS, false, false, false, false, false); }
#[test] fn bcond_cc_taken() { test_bcond(CC, false, false, false, false, true); }
#[test] fn bcond_cc_not() { test_bcond(CC, false, false, true, false, false); }
#[test] fn bcond_mi_taken() { test_bcond(MI, true, false, false, false, true); }
#[test] fn bcond_mi_not() { test_bcond(MI, false, false, false, false, false); }
#[test] fn bcond_pl_taken() { test_bcond(PL, false, false, false, false, true); }
#[test] fn bcond_pl_not() { test_bcond(PL, true, false, false, false, false); }
#[test] fn bcond_vs_taken() { test_bcond(VS, false, false, false, true, true); }
#[test] fn bcond_vs_not() { test_bcond(VS, false, false, false, false, false); }
#[test] fn bcond_vc_taken() { test_bcond(VC, false, false, false, false, true); }
#[test] fn bcond_vc_not() { test_bcond(VC, false, false, false, true, false); }
#[test] fn bcond_hi_taken() { test_bcond(HI, false, false, true, false, true); }
#[test] fn bcond_hi_not_z() { test_bcond(HI, false, true, true, false, false); }
#[test] fn bcond_hi_not_c() { test_bcond(HI, false, false, false, false, false); }
#[test] fn bcond_ls_taken_z() { test_bcond(LS, false, true, false, false, true); }
#[test] fn bcond_ls_taken_nc() { test_bcond(LS, false, false, false, false, true); }
#[test] fn bcond_ls_not() { test_bcond(LS, false, false, true, false, false); }
#[test] fn bcond_ge_taken_pp() { test_bcond(GE, false, false, false, false, true); }
#[test] fn bcond_ge_taken_nn() { test_bcond(GE, true, false, false, true, true); }
#[test] fn bcond_ge_not() { test_bcond(GE, true, false, false, false, false); }
#[test] fn bcond_lt_taken() { test_bcond(LT, true, false, false, false, true); }
#[test] fn bcond_lt_not() { test_bcond(LT, false, false, false, false, false); }
#[test] fn bcond_gt_taken() { test_bcond(GT, false, false, false, false, true); }
#[test] fn bcond_gt_not_z() { test_bcond(GT, false, true, false, false, false); }
#[test] fn bcond_gt_not_lt() { test_bcond(GT, true, false, false, false, false); }
#[test] fn bcond_le_taken_z() { test_bcond(LE, false, true, false, false, true); }
#[test] fn bcond_le_taken_lt() { test_bcond(LE, true, false, false, false, true); }
#[test] fn bcond_le_not() { test_bcond(LE, false, false, false, false, false); }
#[test] fn bcond_al_taken() { test_bcond(AL, false, false, false, false, true); }
#[test] fn bcond_al_with_flags() { test_bcond(AL, true, true, true, true, true); }

#[test]
fn cbz_64_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_cbz(1, 2, 0), NOP, NOP]);
    c.x[0] = 0; step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, BASE + 8);
}
#[test]
fn cbz_64_not() {
    let (mut c, mut m) = cpu_with_code(&[encode_cbz(1, 2, 0), NOP, NOP]);
    c.x[0] = 1; step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, BASE + 4);
}
#[test]
fn cbz_32_taken_ignores_upper() {
    let (mut c, mut m) = cpu_with_code(&[encode_cbz(0, 2, 0), NOP, NOP]);
    c.x[0] = 0x1_0000_0000; step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, BASE + 8);
}
#[test]
fn cbnz_64_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_cbnz(1, 2, 0), NOP, NOP]);
    c.x[0] = 42; step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, BASE + 8);
}
#[test]
fn cbnz_64_not() {
    let (mut c, mut m) = cpu_with_code(&[encode_cbnz(1, 2, 0), NOP, NOP]);
    c.x[0] = 0; step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, BASE + 4);
}
#[test]
fn tbz_bit0_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_tbz(0, 0, 2, 0), NOP, NOP]);
    c.x[0] = 0xFFFE; step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, BASE + 8);
}
#[test]
fn tbz_bit0_not() {
    let (mut c, mut m) = cpu_with_code(&[encode_tbz(0, 0, 2, 0), NOP, NOP]);
    c.x[0] = 0xFFFF; step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, BASE + 4);
}
#[test]
fn tbz_bit63_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_tbz(1, 31, 2, 0), NOP, NOP]);
    c.x[0] = 0x7FFF_FFFF_FFFF_FFFF; step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, BASE + 8);
}
#[test]
fn tbz_bit63_not() {
    let (mut c, mut m) = cpu_with_code(&[encode_tbz(1, 31, 2, 0), NOP, NOP]);
    c.x[0] = 0x8000_0000_0000_0000; step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, BASE + 4);
}
#[test]
fn tbnz_bit0_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_tbnz(0, 0, 2, 0), NOP, NOP]);
    c.x[0] = 1; step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, BASE + 8);
}
#[test]
fn tbnz_bit0_not() {
    let (mut c, mut m) = cpu_with_code(&[encode_tbnz(0, 0, 2, 0), NOP, NOP]);
    c.x[0] = 0; step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, BASE + 4);
}
#[test]
fn br_to_addr() {
    let (mut c, mut m) = cpu_with_code(&[encode_br(1)]);
    c.x[1] = 0x50_0000; step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, 0x50_0000);
}
#[test]
fn blr_sets_lr() {
    let (mut c, mut m) = cpu_with_code(&[encode_blr(1)]);
    c.x[1] = 0x50_0000; step(&mut c, &mut m).unwrap();
    assert_eq!(c.pc, 0x50_0000); assert_eq!(c.x[30], BASE + 4);
}
#[test]
fn ret_default() {
    let (mut c, mut m) = cpu_with_code(&[encode_ret(30)]);
    c.x[30] = 0x60_0000; step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, 0x60_0000);
}
#[test]
fn ret_custom_reg() {
    let (mut c, mut m) = cpu_with_code(&[encode_ret(5)]);
    c.x[5] = 0x70_0000; step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, 0x70_0000);
}
#[test]
fn nop_advances_pc() {
    let (mut c, mut m) = cpu_with_code(&[NOP, NOP]);
    step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, BASE + 4);
    step(&mut c, &mut m).unwrap(); assert_eq!(c.pc, BASE + 8);
}
#[test]
#[ignore = "SVC from EL0 returns EnvironmentCall, not exception entry; SE vs FS mode difference"]
fn svc_from_el0_takes_exception() { }
#[test]
#[ignore = "requires HelmError::Syscall which is not in this codebase"]
fn svc_from_el1_raises_syscall() { }
#[test]
#[ignore = "requires HelmError::Decode and set_se_mode() which are not in this codebase"]
fn brk_raises_decode_error() { }

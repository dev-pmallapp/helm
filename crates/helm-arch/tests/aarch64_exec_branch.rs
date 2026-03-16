//! AArch64 Branch & System instruction tests.
//!
//! Ported from the reference helm.git `exec_branch.rs` test suite.
//! Covers: B, BL, B.cond (all 15 conditions), CBZ/CBNZ 32/64,
//! TBZ/TBNZ, BR/BLR/RET, SVC, BRK, NOP.

use helm_arch::aarch64::arch_state::Aarch64ArchState;
use helm_core::{AccessType, HartException, MemFault, MemInterface};

// ── Test memory ────────────────────────────────────────────────────────────────

struct TestMem {
    data: Vec<u8>,
}

impl TestMem {
    fn new() -> Self {
        Self { data: vec![0u8; 16 * 1024 * 1024] }
    }
}

impl MemInterface for TestMem {
    fn read(&mut self, addr: u64, size: usize, _ty: AccessType) -> Result<u64, MemFault> {
        let off = (addr & 0xFF_FFFF) as usize;
        if off + size > self.data.len() {
            return Err(MemFault::AccessFault { addr });
        }
        let mut buf = [0u8; 8];
        buf[..size].copy_from_slice(&self.data[off..off + size]);
        Ok(u64::from_le_bytes(buf))
    }
    fn write(&mut self, addr: u64, size: usize, val: u64, _ty: AccessType) -> Result<(), MemFault> {
        let off = (addr & 0xFF_FFFF) as usize;
        if off + size > self.data.len() {
            return Err(MemFault::AccessFault { addr });
        }
        self.data[off..off + size].copy_from_slice(&val.to_le_bytes()[..size]);
        Ok(())
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

const BASE: u64 = 0x40_0000;

fn setup() -> (Aarch64ArchState, TestMem) {
    let mut a = Aarch64ArchState::new();
    let m = TestMem::new();
    a.pc = BASE;
    a.sp = 0x7F_8000;
    (a, m)
}

fn step(a: &mut Aarch64ArchState, mem: &mut TestMem, raw: u32) {
    let insn = helm_arch::aarch64::decode::decode(raw, a.pc).expect("decode");
    let pc_written = helm_arch::aarch64::execute::execute(&insn, a, mem).expect("execute");
    if !pc_written {
        a.pc += 4;
    }
}

fn set_nzcv(a: &mut Aarch64ArchState, n: bool, z: bool, c: bool, v: bool) {
    a.nzcv =
        ((n as u32) << 31) | ((z as u32) << 30) | ((c as u32) << 29) | ((v as u32) << 28);
}

// ── Encoding helpers ───────────────────────────────────────────────────────────

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

#[allow(dead_code)]
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
    let (mut a, mut m) = setup();
    step(&mut a, &mut m, encode_b(1));
    assert_eq!(a.pc, BASE + 4);
}

#[test]
fn b_forward_8() {
    let (mut a, mut m) = setup();
    step(&mut a, &mut m, encode_b(2));
    assert_eq!(a.pc, BASE + 8);
}

#[test]
fn b_forward_1024() {
    let (mut a, mut m) = setup();
    step(&mut a, &mut m, encode_b(256));
    assert_eq!(a.pc, BASE + 1024);
}

#[test]
fn bl_sets_lr() {
    let (mut a, mut m) = setup();
    step(&mut a, &mut m, encode_bl(2));
    assert_eq!(a.x[30], BASE + 4, "LR = next insn");
    assert_eq!(a.pc, BASE + 8);
}

#[test]
fn bl_far() {
    let (mut a, mut m) = setup();
    step(&mut a, &mut m, encode_bl(0x100));
    assert_eq!(a.pc, BASE + 0x400);
}

// ===================================================================
//  B.cond -- all 15 conditions
// ===================================================================

fn test_bcond(cond: u32, n: bool, z: bool, c: bool, v: bool, expect_taken: bool) {
    let (mut a, mut m) = setup();
    set_nzcv(&mut a, n, z, c, v);
    step(&mut a, &mut m, encode_b_cond(3, cond));
    if expect_taken {
        assert_eq!(a.pc, BASE + 12, "B.cond({cond}) should be taken");
    } else {
        assert_eq!(a.pc, BASE + 4, "B.cond({cond}) should fall through");
    }
}

#[test]
fn bcond_eq_taken() { test_bcond(EQ, false, true, false, false, true); }
#[test]
fn bcond_eq_not() { test_bcond(EQ, false, false, false, false, false); }
#[test]
fn bcond_ne_taken() { test_bcond(NE, false, false, false, false, true); }
#[test]
fn bcond_ne_not() { test_bcond(NE, false, true, false, false, false); }
#[test]
fn bcond_cs_taken() { test_bcond(CS, false, false, true, false, true); }
#[test]
fn bcond_cs_not() { test_bcond(CS, false, false, false, false, false); }
#[test]
fn bcond_cc_taken() { test_bcond(CC, false, false, false, false, true); }
#[test]
fn bcond_cc_not() { test_bcond(CC, false, false, true, false, false); }
#[test]
fn bcond_mi_taken() { test_bcond(MI, true, false, false, false, true); }
#[test]
fn bcond_mi_not() { test_bcond(MI, false, false, false, false, false); }
#[test]
fn bcond_pl_taken() { test_bcond(PL, false, false, false, false, true); }
#[test]
fn bcond_pl_not() { test_bcond(PL, true, false, false, false, false); }
#[test]
fn bcond_vs_taken() { test_bcond(VS, false, false, false, true, true); }
#[test]
fn bcond_vs_not() { test_bcond(VS, false, false, false, false, false); }
#[test]
fn bcond_vc_taken() { test_bcond(VC, false, false, false, false, true); }
#[test]
fn bcond_vc_not() { test_bcond(VC, false, false, false, true, false); }
#[test]
fn bcond_hi_taken() { test_bcond(HI, false, false, true, false, true); }
#[test]
fn bcond_hi_not_z() { test_bcond(HI, false, true, true, false, false); }
#[test]
fn bcond_hi_not_c() { test_bcond(HI, false, false, false, false, false); }
#[test]
fn bcond_ls_taken_z() { test_bcond(LS, false, true, false, false, true); }
#[test]
fn bcond_ls_taken_nc() { test_bcond(LS, false, false, false, false, true); }
#[test]
fn bcond_ls_not() { test_bcond(LS, false, false, true, false, false); }
#[test]
fn bcond_ge_taken_pp() { test_bcond(GE, false, false, false, false, true); }
#[test]
fn bcond_ge_taken_nn() { test_bcond(GE, true, false, false, true, true); }
#[test]
fn bcond_ge_not() { test_bcond(GE, true, false, false, false, false); }
#[test]
fn bcond_lt_taken() { test_bcond(LT, true, false, false, false, true); }
#[test]
fn bcond_lt_not() { test_bcond(LT, false, false, false, false, false); }
#[test]
fn bcond_gt_taken() { test_bcond(GT, false, false, false, false, true); }
#[test]
fn bcond_gt_not_z() { test_bcond(GT, false, true, false, false, false); }
#[test]
fn bcond_gt_not_lt() { test_bcond(GT, true, false, false, false, false); }
#[test]
fn bcond_le_taken_z() { test_bcond(LE, false, true, false, false, true); }
#[test]
fn bcond_le_taken_lt() { test_bcond(LE, true, false, false, false, true); }
#[test]
fn bcond_le_not() { test_bcond(LE, false, false, false, false, false); }
#[test]
fn bcond_al_taken() { test_bcond(AL, false, false, false, false, true); }
#[test]
fn bcond_al_with_flags() { test_bcond(AL, true, true, true, true, true); }

// ===================================================================
//  CBZ / CBNZ -- 32 and 64-bit
// ===================================================================

#[test]
fn cbz_64_taken() {
    let (mut a, mut m) = setup();
    a.x[0] = 0;
    step(&mut a, &mut m, encode_cbz(1, 2, 0));
    assert_eq!(a.pc, BASE + 8);
}

#[test]
fn cbz_64_not() {
    let (mut a, mut m) = setup();
    a.x[0] = 1;
    step(&mut a, &mut m, encode_cbz(1, 2, 0));
    assert_eq!(a.pc, BASE + 4);
}

#[test]
fn cbz_32_taken() {
    let (mut a, mut m) = setup();
    a.x[0] = 0x1_0000_0000; // upper bits set, low 32 = 0
    step(&mut a, &mut m, encode_cbz(0, 2, 0));
    assert_eq!(a.pc, BASE + 8, "32-bit CBZ ignores upper bits");
}

#[test]
fn cbz_32_not() {
    let (mut a, mut m) = setup();
    a.x[0] = 1;
    step(&mut a, &mut m, encode_cbz(0, 2, 0));
    assert_eq!(a.pc, BASE + 4);
}

#[test]
fn cbnz_64_taken() {
    let (mut a, mut m) = setup();
    a.x[0] = 42;
    step(&mut a, &mut m, encode_cbnz(1, 2, 0));
    assert_eq!(a.pc, BASE + 8);
}

#[test]
fn cbnz_64_not() {
    let (mut a, mut m) = setup();
    a.x[0] = 0;
    step(&mut a, &mut m, encode_cbnz(1, 2, 0));
    assert_eq!(a.pc, BASE + 4);
}

#[test]
fn cbnz_32_taken() {
    let (mut a, mut m) = setup();
    a.x[0] = 0xFF;
    step(&mut a, &mut m, encode_cbnz(0, 2, 0));
    assert_eq!(a.pc, BASE + 8);
}

// ===================================================================
//  TBZ / TBNZ
// ===================================================================

#[test]
fn tbz_bit0_taken() {
    let (mut a, mut m) = setup();
    a.x[0] = 0xFFFE; // bit 0 clear
    step(&mut a, &mut m, encode_tbz(0, 0, 2, 0));
    assert_eq!(a.pc, BASE + 8, "bit 0 clear");
}

#[test]
fn tbz_bit0_not() {
    let (mut a, mut m) = setup();
    a.x[0] = 0xFFFF; // bit 0 set
    step(&mut a, &mut m, encode_tbz(0, 0, 2, 0));
    assert_eq!(a.pc, BASE + 4, "bit 0 set");
}

#[test]
fn tbz_bit31_taken() {
    let (mut a, mut m) = setup();
    a.x[0] = 0x7FFF_FFFF;
    step(&mut a, &mut m, encode_tbz(0, 31, 2, 0));
    assert_eq!(a.pc, BASE + 8);
}

#[test]
fn tbz_bit31_not() {
    let (mut a, mut m) = setup();
    a.x[0] = 0x8000_0000;
    step(&mut a, &mut m, encode_tbz(0, 31, 2, 0));
    assert_eq!(a.pc, BASE + 4);
}

#[test]
fn tbz_bit63_taken() {
    let (mut a, mut m) = setup();
    a.x[0] = 0x7FFF_FFFF_FFFF_FFFF;
    step(&mut a, &mut m, encode_tbz(1, 31, 2, 0));
    assert_eq!(a.pc, BASE + 8);
}

#[test]
fn tbz_bit63_not() {
    let (mut a, mut m) = setup();
    a.x[0] = 0x8000_0000_0000_0000;
    step(&mut a, &mut m, encode_tbz(1, 31, 2, 0));
    assert_eq!(a.pc, BASE + 4);
}

#[test]
fn tbnz_bit0_taken() {
    let (mut a, mut m) = setup();
    a.x[0] = 1;
    step(&mut a, &mut m, encode_tbnz(0, 0, 2, 0));
    assert_eq!(a.pc, BASE + 8);
}

#[test]
fn tbnz_bit0_not() {
    let (mut a, mut m) = setup();
    a.x[0] = 0;
    step(&mut a, &mut m, encode_tbnz(0, 0, 2, 0));
    assert_eq!(a.pc, BASE + 4);
}

#[test]
fn tbnz_bit16() {
    let (mut a, mut m) = setup();
    a.x[0] = 1 << 16;
    step(&mut a, &mut m, encode_tbnz(0, 16, 2, 0));
    assert_eq!(a.pc, BASE + 8);
}

// ===================================================================
//  BR / BLR / RET
// ===================================================================

#[test]
fn br_to_addr() {
    let (mut a, mut m) = setup();
    a.x[1] = 0x50_0000;
    step(&mut a, &mut m, encode_br(1));
    assert_eq!(a.pc, 0x50_0000);
}

#[test]
fn blr_sets_lr() {
    let (mut a, mut m) = setup();
    a.x[1] = 0x50_0000;
    step(&mut a, &mut m, encode_blr(1));
    assert_eq!(a.pc, 0x50_0000);
    assert_eq!(a.x[30], BASE + 4);
}

#[test]
fn ret_default() {
    let (mut a, mut m) = setup();
    a.x[30] = 0x60_0000;
    step(&mut a, &mut m, encode_ret(30));
    assert_eq!(a.pc, 0x60_0000);
}

#[test]
fn ret_custom_reg() {
    let (mut a, mut m) = setup();
    a.x[5] = 0x70_0000;
    step(&mut a, &mut m, encode_ret(5));
    assert_eq!(a.pc, 0x70_0000);
}

// ===================================================================
//  SVC / BRK
// ===================================================================

#[test]
fn svc_raises_environment_call() {
    let (mut a, mut m) = setup();
    a.x[8] = 42; // syscall number
    let insn = helm_arch::aarch64::decode::decode(0xD400_0001, a.pc).expect("decode");
    let err = helm_arch::aarch64::execute::execute(&insn, &mut a, &mut m).unwrap_err();
    match err {
        HartException::EnvironmentCall { nr, .. } => assert_eq!(nr, 42),
        other => panic!("expected EnvironmentCall, got {other:?}"),
    }
}

#[test]
fn brk_raises_breakpoint() {
    let (mut a, mut m) = setup();
    let insn = helm_arch::aarch64::decode::decode(0xD420_0000, a.pc).expect("decode");
    let err = helm_arch::aarch64::execute::execute(&insn, &mut a, &mut m).unwrap_err();
    match err {
        HartException::Breakpoint { .. } => {}
        other => panic!("expected Breakpoint, got {other:?}"),
    }
}

#[test]
fn nop_does_not_write_pc() {
    let (mut a, mut m) = setup();
    step(&mut a, &mut m, NOP);
    assert_eq!(a.pc, BASE + 4);
    step(&mut a, &mut m, NOP);
    assert_eq!(a.pc, BASE + 8);
}

//! Exhaustive NZCV flag tests for flag-setting instructions.
//!
//! Ported from the reference helm.git `exec_flags.rs` test suite.
//! Tests every combination of N, Z, C, V for ADDS/SUBS (register),
//! ADCS/SBCS, ANDS, and CCMP/CCMN.

use helm_arch::aarch64::arch_state::Aarch64ArchState;
use helm_arch::aarch64::decode::decode;
use helm_arch::aarch64::execute::execute;
use helm_core::{AccessType, MemFault, MemInterface};

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
    let insn = decode(raw, a.pc).expect("decode failed");
    let pc_written = execute(&insn, a, mem).expect("execute failed");
    if !pc_written {
        a.pc += 4;
    }
}

fn set_flags(a: &mut Aarch64ArchState, n: bool, z: bool, c: bool, v: bool) {
    a.nzcv =
        ((n as u32) << 31) | ((z as u32) << 30) | ((c as u32) << 29) | ((v as u32) << 28);
}

// ── Encoding helpers ───────────────────────────────────────────────────────────

fn adds_reg(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b01011 << 24) | (1 << 29) | (rm << 16) | (rn << 5) | rd
}

fn subs_reg(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (1 << 30) | (1 << 29) | (0b01011 << 24) | (rm << 16) | (rn << 5) | rd
}

fn adcs(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0 << 30) | (1 << 29) | (0b11010000 << 21) | (rm << 16) | (rn << 5) | rd
}

fn sbcs(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (1 << 30) | (1 << 29) | (0b11010000 << 21) | (rm << 16) | (rn << 5) | rd
}

fn ands_reg_enc(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b11 << 29) | (0b01010 << 24) | (rm << 16) | (rn << 5) | rd
}

fn ccmp_reg(sf: u32, rm: u32, cond: u32, rn: u32, nzcv: u32) -> u32 {
    (sf << 31)
        | (1 << 30)
        | (1 << 29)
        | (0b11010010 << 21)
        | (rm << 16)
        | (cond << 12)
        | (rn << 5)
        | nzcv
}

fn ccmn_reg(sf: u32, rm: u32, cond: u32, rn: u32, nzcv: u32) -> u32 {
    (sf << 31)
        | (0 << 30)
        | (1 << 29)
        | (0b11010010 << 21)
        | (rm << 16)
        | (cond << 12)
        | (rn << 5)
        | nzcv
}

// ── Macros ─────────────────────────────────────────────────────────────────────

macro_rules! flag_test {
    ($name:ident, $insn:expr, $rn_val:expr, $rm_val:expr, $n:expr, $z:expr, $c:expr, $v:expr) => {
        #[test]
        fn $name() {
            let (mut a, mut m) = setup();
            a.x[1] = $rn_val;
            a.x[2] = $rm_val;
            step(&mut a, &mut m, $insn);
            assert_eq!(a.flag_n(), $n, "N");
            assert_eq!(a.flag_z(), $z, "Z");
            assert_eq!(a.flag_c(), $c, "C");
            assert_eq!(a.flag_v(), $v, "V");
        }
    };
}

macro_rules! flag_test_carry {
    ($name:ident, $insn:expr, $rn_val:expr, $rm_val:expr, $cin:expr, $n:expr, $z:expr, $c:expr, $v:expr) => {
        #[test]
        fn $name() {
            let (mut a, mut m) = setup();
            a.x[1] = $rn_val;
            a.x[2] = $rm_val;
            set_flags(&mut a, false, false, $cin, false);
            step(&mut a, &mut m, $insn);
            assert_eq!(a.flag_n(), $n, "N");
            assert_eq!(a.flag_z(), $z, "Z");
            assert_eq!(a.flag_c(), $c, "C");
            assert_eq!(a.flag_v(), $v, "V");
        }
    };
}

// ===================================================================
//  ADDS (register, 64-bit) -- flags
// ===================================================================
flag_test!(adds64_0_0, adds_reg(1, 2, 1, 31), 0u64, 0u64, false, true, false, false);
flag_test!(adds64_1_neg1, adds_reg(1, 2, 1, 31), 1u64, u64::MAX, false, true, true, false);
flag_test!(adds64_max_1, adds_reg(1, 2, 1, 31), i64::MAX as u64, 1u64, true, false, false, true);
flag_test!(adds64_min_neg1, adds_reg(1, 2, 1, 31), i64::MIN as u64, u64::MAX, false, false, true, true);
flag_test!(adds64_neg_neg, adds_reg(1, 2, 1, 31), u64::MAX, u64::MAX, true, false, true, false);
flag_test!(adds64_pos_pos, adds_reg(1, 2, 1, 31), 100u64, 200u64, false, false, false, false);
flag_test!(adds64_max_0, adds_reg(1, 2, 1, 31), u64::MAX, 0u64, true, false, false, false);
flag_test!(adds64_half_half, adds_reg(1, 2, 1, 31), 1u64 << 63, 1u64 << 63, false, true, true, true);

// ===================================================================
//  ADDS (register, 32-bit) -- flags
// ===================================================================
flag_test!(adds32_0_0, adds_reg(0, 2, 1, 31), 0u64, 0u64, false, true, false, false);
flag_test!(adds32_max_1, adds_reg(0, 2, 1, 31), 0x7FFF_FFFFu64, 1u64, true, false, false, true);
flag_test!(adds32_ff_1, adds_reg(0, 2, 1, 31), 0xFFFF_FFFFu64, 1u64, false, true, true, false);
flag_test!(adds32_neg_neg, adds_reg(0, 2, 1, 31), 0xFFFF_FFFFu64, 0xFFFF_FFFFu64, true, false, true, false);
flag_test!(adds32_80_80, adds_reg(0, 2, 1, 31), 0x8000_0000u64, 0x8000_0000u64, false, true, true, true);

// ===================================================================
//  SUBS (register, 64-bit) -- flags
// ===================================================================
flag_test!(subs64_eq, subs_reg(1, 2, 1, 31), 42u64, 42u64, false, true, true, false);
flag_test!(subs64_gt, subs_reg(1, 2, 1, 31), 100u64, 50u64, false, false, true, false);
flag_test!(subs64_lt, subs_reg(1, 2, 1, 31), 50u64, 100u64, true, false, false, false);
flag_test!(subs64_0_0, subs_reg(1, 2, 1, 31), 0u64, 0u64, false, true, true, false);
flag_test!(subs64_max_max, subs_reg(1, 2, 1, 31), u64::MAX, u64::MAX, false, true, true, false);
flag_test!(subs64_0_1, subs_reg(1, 2, 1, 31), 0u64, 1u64, true, false, false, false);
flag_test!(subs64_min_1, subs_reg(1, 2, 1, 31), i64::MIN as u64, 1u64, false, false, true, true);
flag_test!(subs64_max_neg1, subs_reg(1, 2, 1, 31), i64::MAX as u64, u64::MAX, true, false, false, true);

// ===================================================================
//  SUBS (register, 32-bit) -- flags
// ===================================================================
flag_test!(subs32_eq, subs_reg(0, 2, 1, 31), 42u64, 42u64, false, true, true, false);
flag_test!(subs32_gt, subs_reg(0, 2, 1, 31), 100u64, 50u64, false, false, true, false);
flag_test!(subs32_lt, subs_reg(0, 2, 1, 31), 50u64, 100u64, true, false, false, false);
flag_test!(subs32_0_1, subs_reg(0, 2, 1, 31), 0u64, 1u64, true, false, false, false);
flag_test!(subs32_min_1, subs_reg(0, 2, 1, 31), 0x8000_0000u64, 1u64, false, false, true, true);

// ===================================================================
//  ADCS -- carry-in affects result and flags
// ===================================================================
flag_test_carry!(adcs64_0_0_nc, adcs(1, 2, 1, 31), 0u64, 0u64, false, false, true, false, false);
flag_test_carry!(adcs64_0_0_c, adcs(1, 2, 1, 31), 0u64, 0u64, true, false, false, false, false);
flag_test_carry!(adcs64_max_0_c, adcs(1, 2, 1, 31), u64::MAX, 0u64, true, false, true, true, false);
flag_test_carry!(adcs64_max_max_c, adcs(1, 2, 1, 31), u64::MAX, u64::MAX, true, true, false, true, false);
flag_test_carry!(adcs64_half_nc, adcs(1, 2, 1, 31), i64::MAX as u64, 1u64, false, true, false, false, true);
flag_test_carry!(adcs32_ff_0_c, adcs(0, 2, 1, 31), 0xFFFF_FFFFu64, 0u64, true, false, true, true, false);
flag_test_carry!(adcs32_7f_0_c, adcs(0, 2, 1, 31), 0x7FFF_FFFFu64, 0u64, true, true, false, false, true);

// ===================================================================
//  SBCS -- borrow-in affects result and flags
// ===================================================================
flag_test_carry!(sbcs64_100_50_c, sbcs(1, 2, 1, 31), 100u64, 50u64, true, false, false, true, false);
flag_test_carry!(sbcs64_100_50_nc, sbcs(1, 2, 1, 31), 100u64, 50u64, false, false, false, true, false);
flag_test_carry!(sbcs64_50_100_c, sbcs(1, 2, 1, 31), 50u64, 100u64, true, true, false, false, false);
flag_test_carry!(sbcs64_0_0_c, sbcs(1, 2, 1, 31), 0u64, 0u64, true, false, true, true, false);
flag_test_carry!(sbcs64_0_0_nc, sbcs(1, 2, 1, 31), 0u64, 0u64, false, true, false, false, false);
flag_test_carry!(sbcs32_eq_c, sbcs(0, 2, 1, 31), 42u64, 42u64, true, false, true, true, false);
flag_test_carry!(sbcs32_eq_nc, sbcs(0, 2, 1, 31), 42u64, 42u64, false, true, false, false, false);

// ===================================================================
//  ANDS (register) -- N and Z flags
// ===================================================================
flag_test!(ands64_zero, ands_reg_enc(1, 2, 1, 31), 0xFF00u64, 0x00FFu64, false, true, false, false);
flag_test!(ands64_nonzero, ands_reg_enc(1, 2, 1, 31), 0xFF00u64, 0x0FF0u64, false, false, false, false);
flag_test!(ands64_msb, ands_reg_enc(1, 2, 1, 31), u64::MAX, 1u64 << 63, true, false, false, false);
flag_test!(ands64_all_ones, ands_reg_enc(1, 2, 1, 31), u64::MAX, u64::MAX, true, false, false, false);
flag_test!(ands32_zero, ands_reg_enc(0, 2, 1, 31), 0xFF00u64, 0x00FFu64, false, true, false, false);
flag_test!(ands32_msb, ands_reg_enc(0, 2, 1, 31), 0xFFFF_FFFFu64, 0x8000_0000u64, true, false, false, false);

// ===================================================================
//  CCMP -- condition true does compare, false sets nzcv
// ===================================================================

macro_rules! ccmp_test {
    ($name:ident, $sf:expr, $rn_val:expr, $rm_val:expr, $cond:expr, $nzcv_imm:expr,
     $in_n:expr, $in_z:expr, $in_c:expr, $in_v:expr,
     $out_n:expr, $out_z:expr, $out_c:expr, $out_v:expr) => {
        #[test]
        fn $name() {
            let (mut a, mut m) = setup();
            a.x[1] = $rn_val;
            a.x[2] = $rm_val;
            set_flags(&mut a, $in_n, $in_z, $in_c, $in_v);
            step(&mut a, &mut m, ccmp_reg($sf, 2, $cond, 1, $nzcv_imm));
            assert_eq!(a.flag_n(), $out_n, "N");
            assert_eq!(a.flag_z(), $out_z, "Z");
            assert_eq!(a.flag_c(), $out_c, "C");
            assert_eq!(a.flag_v(), $out_v, "V");
        }
    };
}

// EQ=0 cond true (Z=1): does CMP
ccmp_test!(ccmp64_eq_t_equal, 1, 42, 42, 0, 0b0000, false, true, false, false, false, true, true, false);
ccmp_test!(ccmp64_eq_t_gt, 1, 100, 50, 0, 0b0000, false, true, false, false, false, false, true, false);
ccmp_test!(ccmp64_eq_t_lt, 1, 50, 100, 0, 0b0000, false, true, false, false, true, false, false, false);
// EQ cond false (Z=0): sets nzcv imm
ccmp_test!(ccmp64_eq_f_0000, 1, 42, 42, 0, 0b0000, false, false, false, false, false, false, false, false);
ccmp_test!(ccmp64_eq_f_1111, 1, 42, 42, 0, 0b1111, false, false, false, false, true, true, true, true);
ccmp_test!(ccmp64_eq_f_1010, 1, 42, 42, 0, 0b1010, false, false, false, false, true, false, true, false);
ccmp_test!(ccmp64_eq_f_0101, 1, 42, 42, 0, 0b0101, false, false, false, false, false, true, false, true);
// NE=1 cond true (Z=0)
ccmp_test!(ccmp64_ne_t, 1, 10, 10, 1, 0b1111, false, false, false, false, false, true, true, false);
// Various conditions
ccmp_test!(ccmp64_cs_t, 1, 100, 50, 2, 0b0000, false, false, true, false, false, false, true, false);
ccmp_test!(ccmp64_cs_f, 1, 100, 50, 2, 0b1010, false, false, false, false, true, false, true, false);
ccmp_test!(ccmp64_lt_t, 1, 50, 100, 11, 0b0000, true, false, false, false, true, false, false, false);
ccmp_test!(ccmp64_lt_f, 1, 50, 100, 11, 0b0100, false, false, false, false, false, true, false, false);
// 32-bit
ccmp_test!(ccmp32_eq_t, 0, 42, 42, 0, 0b0000, false, true, false, false, false, true, true, false);
ccmp_test!(ccmp32_eq_f, 0, 42, 42, 0, 0b1111, false, false, false, false, true, true, true, true);

// ===================================================================
//  CCMN -- condition true does CMN (add), false sets nzcv
// ===================================================================

macro_rules! ccmn_test {
    ($name:ident, $sf:expr, $rn_val:expr, $rm_val:expr, $cond:expr, $nzcv_imm:expr,
     $in_n:expr, $in_z:expr, $in_c:expr, $in_v:expr,
     $out_n:expr, $out_z:expr, $out_c:expr, $out_v:expr) => {
        #[test]
        fn $name() {
            let (mut a, mut m) = setup();
            a.x[1] = $rn_val;
            a.x[2] = $rm_val;
            set_flags(&mut a, $in_n, $in_z, $in_c, $in_v);
            step(&mut a, &mut m, ccmn_reg($sf, 2, $cond, 1, $nzcv_imm));
            assert_eq!(a.flag_n(), $out_n, "N");
            assert_eq!(a.flag_z(), $out_z, "Z");
            assert_eq!(a.flag_c(), $out_c, "C");
            assert_eq!(a.flag_v(), $out_v, "V");
        }
    };
}

ccmn_test!(ccmn64_eq_t_zero, 1, 0, 0, 0, 0b0000, false, true, false, false, false, true, false, false);
ccmn_test!(ccmn64_eq_t_carry, 1, u64::MAX, 1, 0, 0b0000, false, true, false, false, false, true, true, false);
ccmn_test!(ccmn64_eq_f, 1, 0, 0, 0, 0b1010, false, false, false, false, true, false, true, false);
ccmn_test!(ccmn32_eq_t, 0, 0, 0, 0, 0b0000, false, true, false, false, false, true, false, false);

// ===================================================================
//  Exhaustive ADDS 64-bit flag sweep -- boundary values
// ===================================================================
flag_test!(adds64_1_0, adds_reg(1, 2, 1, 31), 1u64, 0u64, false, false, false, false);
flag_test!(adds64_1_1, adds_reg(1, 2, 1, 31), 1u64, 1u64, false, false, false, false);
flag_test!(adds64_ff_1, adds_reg(1, 2, 1, 31), 0xFFu64, 1u64, false, false, false, false);
flag_test!(adds64_7f_7f, adds_reg(1, 2, 1, 31), 0x7Fu64, 0x7Fu64, false, false, false, false);
flag_test!(adds64_neg1_0, adds_reg(1, 2, 1, 31), u64::MAX, 0u64, true, false, false, false);
flag_test!(adds64_neg1_1, adds_reg(1, 2, 1, 31), u64::MAX, 1u64, false, true, true, false);
flag_test!(adds64_neg1_2, adds_reg(1, 2, 1, 31), u64::MAX, 2u64, false, false, true, false);
flag_test!(adds64_min_0, adds_reg(1, 2, 1, 31), i64::MIN as u64, 0u64, true, false, false, false);
flag_test!(adds64_min_min, adds_reg(1, 2, 1, 31), i64::MIN as u64, i64::MIN as u64, false, true, true, true);
flag_test!(adds64_max_max, adds_reg(1, 2, 1, 31), i64::MAX as u64, i64::MAX as u64, true, false, false, true);

// ===================================================================
//  Exhaustive SUBS 64-bit flag sweep
// ===================================================================
flag_test!(subs64_1_0, subs_reg(1, 2, 1, 31), 1u64, 0u64, false, false, true, false);
flag_test!(subs64_1_1, subs_reg(1, 2, 1, 31), 1u64, 1u64, false, true, true, false);
flag_test!(subs64_0_max, subs_reg(1, 2, 1, 31), 0u64, u64::MAX, false, false, false, false);
flag_test!(subs64_min_min, subs_reg(1, 2, 1, 31), i64::MIN as u64, i64::MIN as u64, false, true, true, false);
flag_test!(subs64_max_0, subs_reg(1, 2, 1, 31), i64::MAX as u64, 0u64, false, false, true, false);
flag_test!(subs64_neg1_neg1, subs_reg(1, 2, 1, 31), u64::MAX, u64::MAX, false, true, true, false);
flag_test!(subs64_1_2, subs_reg(1, 2, 1, 31), 1u64, 2u64, true, false, false, false);
flag_test!(subs64_2_1, subs_reg(1, 2, 1, 31), 2u64, 1u64, false, false, true, false);
flag_test!(subs64_min_max, subs_reg(1, 2, 1, 31), i64::MIN as u64, i64::MAX as u64, false, false, true, true);

// ===================================================================
//  Exhaustive ADDS 32-bit flag sweep
// ===================================================================
flag_test!(adds32_1_0, adds_reg(0, 2, 1, 31), 1u64, 0u64, false, false, false, false);
flag_test!(adds32_1_1, adds_reg(0, 2, 1, 31), 1u64, 1u64, false, false, false, false);
flag_test!(adds32_ff_1b, adds_reg(0, 2, 1, 31), 0xFFu64, 1u64, false, false, false, false);
flag_test!(adds32_7fffffff_0, adds_reg(0, 2, 1, 31), 0x7FFF_FFFFu64, 0u64, false, false, false, false);
flag_test!(adds32_80000000_0, adds_reg(0, 2, 1, 31), 0x8000_0000u64, 0u64, true, false, false, false);
flag_test!(adds32_80000000_ff, adds_reg(0, 2, 1, 31), 0x8000_0000u64, 0x8000_0000u64, false, true, true, true);
flag_test!(adds32_7fff_1b, adds_reg(0, 2, 1, 31), 0x7FFF_FFFFu64, 0x7FFF_FFFFu64, true, false, false, true);

// ===================================================================
//  Exhaustive SUBS 32-bit flag sweep
// ===================================================================
flag_test!(subs32_1_0, subs_reg(0, 2, 1, 31), 1u64, 0u64, false, false, true, false);
flag_test!(subs32_1_1b, subs_reg(0, 2, 1, 31), 1u64, 1u64, false, true, true, false);
flag_test!(subs32_0_1b, subs_reg(0, 2, 1, 31), 0u64, 1u64, true, false, false, false);
flag_test!(subs32_ff_ff, subs_reg(0, 2, 1, 31), 0xFFFF_FFFFu64, 0xFFFF_FFFFu64, false, true, true, false);
flag_test!(subs32_80_1, subs_reg(0, 2, 1, 31), 0x8000_0000u64, 1u64, false, false, true, true);
flag_test!(subs32_7f_ff, subs_reg(0, 2, 1, 31), 0x7FFF_FFFFu64, 0xFFFF_FFFFu64, true, false, false, true);

// ===================================================================
//  More ADCS flag sweep
// ===================================================================
flag_test_carry!(adcs64_1_1_nc, adcs(1, 2, 1, 31), 1u64, 1u64, false, false, false, false, false);
flag_test_carry!(adcs64_1_1_c, adcs(1, 2, 1, 31), 1u64, 1u64, true, false, false, false, false);
flag_test_carry!(adcs64_ff_0_nc, adcs(1, 2, 1, 31), u64::MAX, 0u64, false, true, false, false, false);
flag_test_carry!(adcs64_max_1_nc, adcs(1, 2, 1, 31), i64::MAX as u64, 1u64, false, true, false, false, true);
flag_test_carry!(adcs64_max_0_nc, adcs(1, 2, 1, 31), i64::MAX as u64, 0u64, false, false, false, false, false);
flag_test_carry!(adcs32_ff_1_nc, adcs(0, 2, 1, 31), 0xFFFF_FFFFu64, 1u64, false, false, true, true, false);
flag_test_carry!(adcs32_ff_1_c, adcs(0, 2, 1, 31), 0xFFFF_FFFFu64, 1u64, true, false, false, true, false);
flag_test_carry!(adcs32_0_0_nc, adcs(0, 2, 1, 31), 0u64, 0u64, false, false, true, false, false);
flag_test_carry!(adcs32_0_0_c, adcs(0, 2, 1, 31), 0u64, 0u64, true, false, false, false, false);

// ===================================================================
//  More SBCS flag sweep
// ===================================================================
flag_test_carry!(sbcs64_1_0_c, sbcs(1, 2, 1, 31), 1u64, 0u64, true, false, false, true, false);
flag_test_carry!(sbcs64_1_0_nc, sbcs(1, 2, 1, 31), 1u64, 0u64, false, false, true, true, false);
flag_test_carry!(sbcs64_0_1_c, sbcs(1, 2, 1, 31), 0u64, 1u64, true, true, false, false, false);
flag_test_carry!(sbcs64_0_1_nc, sbcs(1, 2, 1, 31), 0u64, 1u64, false, true, false, false, false);
flag_test_carry!(sbcs64_max_max_c, sbcs(1, 2, 1, 31), u64::MAX, u64::MAX, true, false, true, true, false);
flag_test_carry!(sbcs64_max_0_c, sbcs(1, 2, 1, 31), u64::MAX, 0u64, true, true, false, true, false);
flag_test_carry!(sbcs32_1_0_c, sbcs(0, 2, 1, 31), 1u64, 0u64, true, false, false, true, false);
flag_test_carry!(sbcs32_1_0_nc, sbcs(0, 2, 1, 31), 1u64, 0u64, false, false, true, true, false);
flag_test_carry!(sbcs32_0_1_c, sbcs(0, 2, 1, 31), 0u64, 1u64, true, true, false, false, false);
flag_test_carry!(sbcs32_0_1_nc, sbcs(0, 2, 1, 31), 0u64, 1u64, false, true, false, false, false);

// ===================================================================
//  More ANDS flag tests
// ===================================================================
flag_test!(ands64_1_1, ands_reg_enc(1, 2, 1, 31), 1u64, 1u64, false, false, false, false);
flag_test!(ands64_0_0, ands_reg_enc(1, 2, 1, 31), 0u64, 0u64, false, true, false, false);
flag_test!(ands64_ff_0, ands_reg_enc(1, 2, 1, 31), 0xFFu64, 0u64, false, true, false, false);
flag_test!(ands64_neg_neg, ands_reg_enc(1, 2, 1, 31), 0x8000_0000_0000_0001u64, 0x8000_0000_0000_0002u64, true, false, false, false);
flag_test!(ands32_1_1, ands_reg_enc(0, 2, 1, 31), 1u64, 1u64, false, false, false, false);
flag_test!(ands32_0_0, ands_reg_enc(0, 2, 1, 31), 0u64, 0u64, false, true, false, false);
flag_test!(ands32_neg_neg, ands_reg_enc(0, 2, 1, 31), 0x8000_0001u64, 0x8000_0002u64, true, false, false, false);

// ===================================================================
//  More CCMP with all conditions
// ===================================================================
ccmp_test!(ccmp64_mi_t, 1, 10, 5, 4, 0b0000, true, false, false, false, false, false, true, false);
ccmp_test!(ccmp64_mi_f, 1, 10, 5, 4, 0b1111, false, false, false, false, true, true, true, true);
ccmp_test!(ccmp64_pl_t, 1, 10, 5, 5, 0b0000, false, false, false, false, false, false, true, false);
ccmp_test!(ccmp64_pl_f, 1, 10, 5, 5, 0b0101, true, false, false, false, false, true, false, true);
ccmp_test!(ccmp64_vs_t, 1, 10, 5, 6, 0b0000, false, false, false, true, false, false, true, false);
ccmp_test!(ccmp64_vs_f, 1, 10, 5, 6, 0b1010, false, false, false, false, true, false, true, false);
ccmp_test!(ccmp64_vc_t, 1, 10, 5, 7, 0b0000, false, false, false, false, false, false, true, false);
ccmp_test!(ccmp64_vc_f, 1, 10, 5, 7, 0b0101, false, false, false, true, false, true, false, true);
ccmp_test!(ccmp64_hi_t, 1, 10, 5, 8, 0b0000, false, false, true, false, false, false, true, false);
ccmp_test!(ccmp64_hi_f_z, 1, 10, 5, 8, 0b1010, false, true, true, false, true, false, true, false);
ccmp_test!(ccmp64_hi_f_nc, 1, 10, 5, 8, 0b0101, false, false, false, false, false, true, false, true);
ccmp_test!(ccmp64_ls_t_z, 1, 10, 5, 9, 0b0000, false, true, false, false, false, false, true, false);
ccmp_test!(ccmp64_ls_t_nc, 1, 10, 5, 9, 0b0000, false, false, false, false, false, false, true, false);
ccmp_test!(ccmp64_ls_f, 1, 10, 5, 9, 0b1111, false, false, true, false, true, true, true, true);
ccmp_test!(ccmp64_ge_t, 1, 10, 5, 10, 0b0000, false, false, false, false, false, false, true, false);
ccmp_test!(ccmp64_ge_f, 1, 10, 5, 10, 0b1010, true, false, false, false, true, false, true, false);
ccmp_test!(ccmp64_gt_t, 1, 10, 5, 12, 0b0000, false, false, false, false, false, false, true, false);
ccmp_test!(ccmp64_gt_f_z, 1, 10, 5, 12, 0b0101, false, true, false, false, false, true, false, true);
ccmp_test!(ccmp64_le_t_z, 1, 10, 10, 13, 0b0000, false, true, false, false, false, true, true, false);
ccmp_test!(ccmp64_le_f, 1, 10, 5, 13, 0b1010, false, false, false, false, true, false, true, false);
ccmp_test!(ccmp64_al_t, 1, 10, 5, 14, 0b0000, false, false, false, false, false, false, true, false);
ccmp_test!(ccmp64_al_t2, 1, 10, 5, 14, 0b1111, true, true, true, true, false, false, true, false);

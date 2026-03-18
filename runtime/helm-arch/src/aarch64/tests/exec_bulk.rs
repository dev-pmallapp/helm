//! Bulk parametric tests. Ported from exec_bulk.rs.
use super::harness::*;

const D: u64 = DATA_BASE;

fn dp2(sf: u32, op: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b0011010110 << 21) | (rm << 16) | (op << 10) | (rn << 5) | rd
}
fn dp1(sf: u32, op: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b1011010110 << 21) | (op << 10) | (rn << 5) | rd
}
fn csel_fam(sf: u32, inv: u32, inc: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (inv << 30) | (0b011010100 << 21) | (rm << 16) | (cond << 12) | (inc << 10) | (rn << 5) | rd
}
fn add_sub_imm(sf: u32, op: u32, s: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s << 29) | (0b10001 << 24) | (sh << 22) | (imm12 << 10) | (rn << 5) | rd
}
fn adc_fam(sf: u32, op: u32, s: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s << 29) | (0b11010000 << 21) | (rm << 16) | (rn << 5) | rd
}
fn add_sub_reg(sf: u32, op: u32, s: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s << 29) | (0b01011 << 24) | (shift << 22) | (rm << 16) | (imm6 << 10) | (rn << 5) | rd
}
fn log_reg(sf: u32, opc: u32, n: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b01010 << 24) | (shift << 22) | (n << 21) | (rm << 16) | (imm6 << 10) | (rn << 5) | rd
}
fn bitfield(sf: u32, opc: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b100110 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
fn mov_wide(sf: u32, opc: u32, hw: u32, imm16: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b100101 << 23) | (hw << 21) | (imm16 << 5) | rd
}
fn str_x(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b11111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn ldr_x(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b11111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt
}

macro_rules! test_dp2 {
    ($name:ident, $sf:expr, $op:expr, $a:expr, $b:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let (mut c, mut m) = cpu_with_code(&[dp2($sf, $op, 2, 1, 0)]);
            c.x[1] = $a; c.x[2] = $b;
            step(&mut c, &mut m).unwrap();
            assert_eq!(c.x[0], $expected);
        }
    };
}
macro_rules! test_dp1 {
    ($name:ident, $sf:expr, $op:expr, $a:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let (mut c, mut m) = cpu_with_code(&[dp1($sf, $op, 1, 0)]);
            c.x[1] = $a;
            step(&mut c, &mut m).unwrap();
            assert_eq!(c.x[0], $expected);
        }
    };
}
macro_rules! test_csel {
    ($name:ident, $sf:expr, $inv:expr, $inc:expr, $cond:expr, $rn_val:expr, $rm_val:expr,
     $n:expr, $z:expr, $c:expr, $v:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let (mut c, mut m) = cpu_with_code(&[csel_fam($sf, $inv, $inc, 2, $cond, 1, 0)]);
            c.x[1] = $rn_val; c.x[2] = $rm_val;
            set_nzcv(&mut c, $n, $z, $c, $v);
            step(&mut c, &mut m).unwrap();
            assert_eq!(c.x[0], $expected);
        }
    };
}

// UDIV
test_dp2!(udiv_64_1, 1, 0b000010, 100, 10, 10);
test_dp2!(udiv_64_max, 1, 0b000010, u64::MAX, 1, u64::MAX);
test_dp2!(udiv_64_by0, 1, 0b000010, 42, 0, 0);
test_dp2!(udiv_64_eq, 1, 0b000010, 7, 7, 1);
test_dp2!(udiv_64_lt, 1, 0b000010, 3, 7, 0);
test_dp2!(udiv_32_1, 0, 0b000010, 100, 10, 10);
test_dp2!(udiv_32_by0, 0, 0b000010, 42, 0, 0);
test_dp2!(udiv_32_max, 0, 0b000010, 0xFFFF_FFFF, 1, 0xFFFF_FFFF);

// SDIV
test_dp2!(sdiv_64_pos, 1, 0b000011, 100, 7, 14);
test_dp2!(sdiv_64_neg, 1, 0b000011, (-100i64) as u64, 7, (-14i64) as u64);
test_dp2!(sdiv_64_neg_neg, 1, 0b000011, (-100i64) as u64, (-7i64) as u64, 14);
test_dp2!(sdiv_64_by0, 1, 0b000011, 42, 0, 0);
test_dp2!(sdiv_32_pos, 0, 0b000011, 100, 7, 14);
test_dp2!(sdiv_32_by0, 0, 0b000011, 42, 0, 0);

// LSLV / LSRV / ASRV / RORV
test_dp2!(lslv_64_by0, 1, 0b001000, 0xFF, 0, 0xFF);
test_dp2!(lslv_64_by1, 1, 0b001000, 1, 1, 2);
test_dp2!(lslv_64_by63, 1, 0b001000, 1, 63, 0x8000_0000_0000_0000);
test_dp2!(lslv_64_by64, 1, 0b001000, 1, 64, 1);
test_dp2!(lslv_32_by31, 0, 0b001000, 1, 31, 0x8000_0000);
test_dp2!(lslv_32_by32, 0, 0b001000, 1, 32, 1);
test_dp2!(lsrv_64_msb, 1, 0b001001, 0x8000_0000_0000_0000, 63, 1);
test_dp2!(lsrv_32_msb, 0, 0b001001, 0x8000_0000, 31, 1);
test_dp2!(asrv_64_neg, 1, 0b001010, 0x8000_0000_0000_0000, 4, 0xF800_0000_0000_0000);
test_dp2!(asrv_64_neg63, 1, 0b001010, 0x8000_0000_0000_0000, 63, u64::MAX);
test_dp2!(asrv_32_neg, 0, 0b001010, 0x8000_0000, 4, 0xF800_0000);
test_dp2!(rorv_64_by1, 1, 0b001011, 1, 1, 0x8000_0000_0000_0000);
test_dp2!(rorv_64_by32, 1, 0b001011, 0xDEAD_BEEF_0000_0000, 32, 0x0000_0000_DEAD_BEEF);

// CLZ / REV / RBIT
test_dp1!(clz_64_zero, 1, 0b000100, 0, 64);
test_dp1!(clz_64_one, 1, 0b000100, 1, 63);
test_dp1!(clz_64_msb, 1, 0b000100, 0x8000_0000_0000_0000, 0);
test_dp1!(clz_64_mid, 1, 0b000100, 0x00FF_0000, 40);
test_dp1!(clz_32_zero, 0, 0b000100, 0, 32);
test_dp1!(clz_32_byte, 0, 0b000100, 0xFF, 24);
test_dp1!(rev_64_swap, 1, 0b000011, 0x0102030405060708, 0x0807060504030201);
test_dp1!(rev_32_swap, 0, 0b000011, 0x01020304, 0x04030201);
test_dp1!(rev16_64_swap, 1, 0b000001, 0x0102030405060708, 0x0201040306050807);
test_dp1!(rbit_64_one, 1, 0b000000, 1, 0x8000_0000_0000_0000);
test_dp1!(rbit_32_one, 0, 0b000000, 1, 0x8000_0000);
test_dp1!(rbit_64_zero, 1, 0b000000, 0, 0);
test_dp1!(rbit_64_max, 1, 0b000000, u64::MAX, u64::MAX);

// CSEL
test_csel!(csel64_eq_t, 1, 0, 0, 0, 10, 20, false, true, false, false, 10);
test_csel!(csel64_eq_f, 1, 0, 0, 0, 10, 20, false, false, false, false, 20);
test_csel!(csel64_ne_t, 1, 0, 0, 1, 10, 20, false, false, false, false, 10);
test_csel!(csel64_ne_f, 1, 0, 0, 1, 10, 20, false, true, false, false, 20);
test_csel!(csinc64_false, 1, 0, 1, 0, 10, 5, false, false, false, false, 6);
test_csel!(csinv64_false, 1, 1, 0, 0, 10, 0, false, false, false, false, u64::MAX);
test_csel!(csneg64_false, 1, 1, 1, 0, 10, 5, false, false, false, false, (-5i64) as u64);

// ── MOVZ/MOVK/MOVN systematic ────────────────────────────────────────────

fn mov_wide_b(sf: u32, opc: u32, hw: u32, imm16: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b100101 << 23) | (hw << 21) | (imm16 << 5) | rd
}

#[test]
fn movz_32_hw0() {
    let (mut a, mut m) = cpu_with_code(&[mov_wide_b(0, 0b10, 0, 0x1234, 0)]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x1234);
}
#[test]
fn movz_32_hw1() {
    let (mut a, mut m) = cpu_with_code(&[mov_wide_b(0, 0b10, 1, 0x1234, 0)]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x1234_0000);
}
#[test]
fn movz_64_hw0() {
    let (mut a, mut m) = cpu_with_code(&[mov_wide_b(1, 0b10, 0, 0xABCD, 0)]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xABCD);
}
#[test]
fn movz_64_hw1() {
    let (mut a, mut m) = cpu_with_code(&[mov_wide_b(1, 0b10, 1, 0xABCD, 0)]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xABCD_0000);
}
#[test]
fn movz_64_hw2() {
    let (mut a, mut m) = cpu_with_code(&[mov_wide_b(1, 0b10, 2, 0xABCD, 0)]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xABCD_0000_0000);
}
#[test]
fn movz_64_hw3() {
    let (mut a, mut m) = cpu_with_code(&[mov_wide_b(1, 0b10, 3, 0xABCD, 0)]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xABCD_0000_0000_0000);
}
#[test]
fn movz_clears_prev() {
    let (mut a, mut m) = cpu_with_code(&[mov_wide_b(1, 0b10, 0, 0x42, 0)]);
    a.x[0] = 0xDEAD_BEEF_CAFE_BABE;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x42, "MOVZ zeroes all other bits");
}
#[test]
fn movk_hw1_preserve() {
    let (mut a, mut m) = cpu_with_code(&[mov_wide_b(1, 0b11, 1, 0xBEEF, 0)]);
    a.x[0] = 0x1234_5678_9ABC_DEF0;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x1234_5678_BEEF_DEF0);
}
#[test]
fn movk_hw2_preserve() {
    let (mut a, mut m) = cpu_with_code(&[mov_wide_b(1, 0b11, 2, 0xCAFE, 0)]);
    a.x[0] = 0x1234_5678_9ABC_DEF0;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x1234_CAFE_9ABC_DEF0);
}
#[test]
fn movn_32_1() {
    let (mut a, mut m) = cpu_with_code(&[mov_wide_b(0, 0b00, 0, 1, 0)]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xFFFF_FFFE);
}
#[test]
fn movn_64_hw0() {
    let (mut a, mut m) = cpu_with_code(&[mov_wide_b(1, 0b00, 0, 0, 0)]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], u64::MAX);
}
#[test]
fn movn_64_hw1() {
    let (mut a, mut m) = cpu_with_code(&[mov_wide_b(1, 0b00, 1, 1, 0)]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], !0x0001_0000u64);
}
#[test]
fn movn_64_hw2() {
    let (mut a, mut m) = cpu_with_code(&[mov_wide_b(1, 0b00, 2, 0xFFFF, 0)]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], !0xFFFF_0000_0000u64);
}

// ── ADD/SUB register shift variants ─────────────────────────────────────

fn add_sub_reg_b(sf: u32, op: u32, s: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s << 29) | (0b01011 << 24) | (shift << 22)
        | (rm << 16) | (imm6 << 10) | (rn << 5) | rd
}

#[test]
fn add_reg_64_lsl0() {
    let (mut a, mut m) = cpu_with_code(&[add_sub_reg_b(1, 0, 0, 0, 2, 0, 1, 0)]);
    a.x[1] = 100; a.x[2] = 200;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 300);
}
#[test]
fn add_reg_64_lsl2() {
    let (mut a, mut m) = cpu_with_code(&[add_sub_reg_b(1, 0, 0, 0, 2, 2, 1, 0)]);
    a.x[1] = 0; a.x[2] = 3;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 12);
}
#[test]
fn add_reg_64_lsl4() {
    let (mut a, mut m) = cpu_with_code(&[add_sub_reg_b(1, 0, 0, 0, 2, 4, 1, 0)]);
    a.x[1] = 0; a.x[2] = 1;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 16);
}
#[test]
fn add_reg_64_lsl8() {
    let (mut a, mut m) = cpu_with_code(&[add_sub_reg_b(1, 0, 0, 0, 2, 8, 1, 0)]);
    a.x[1] = 0; a.x[2] = 1;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 256);
}
#[test]
fn add_reg_64_lsr4() {
    let (mut a, mut m) = cpu_with_code(&[add_sub_reg_b(1, 0, 0, 1, 2, 4, 1, 0)]);
    a.x[1] = 0; a.x[2] = 0x10;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 1);
}
#[test]
fn add_reg_64_lsr8() {
    let (mut a, mut m) = cpu_with_code(&[add_sub_reg_b(1, 0, 0, 1, 2, 8, 1, 0)]);
    a.x[1] = 0; a.x[2] = 0x100;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 1);
}
#[test]
fn add_reg_64_asr4() {
    let (mut a, mut m) = cpu_with_code(&[add_sub_reg_b(1, 0, 0, 2, 2, 4, 1, 0)]);
    a.x[1] = 0; a.x[2] = (-16i64) as u64;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] as i64, -1);
}
#[test]
fn add_reg_32_lsl4() {
    let (mut a, mut m) = cpu_with_code(&[add_sub_reg_b(0, 0, 0, 0, 2, 4, 1, 0)]);
    a.x[1] = 0; a.x[2] = 1;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 16);
}
#[test]
fn sub_reg_64_lsl4() {
    let (mut a, mut m) = cpu_with_code(&[add_sub_reg_b(1, 1, 0, 0, 2, 4, 1, 0)]);
    a.x[1] = 0x100; a.x[2] = 1;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x100 - 16);
}
#[test]
fn sub_reg_32_lsl4() {
    let (mut a, mut m) = cpu_with_code(&[add_sub_reg_b(0, 1, 0, 0, 2, 4, 1, 0)]);
    a.x[1] = 0x100; a.x[2] = 1;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x100 - 16);
}
#[test]
fn sub_reg_64_lsr4() {
    let (mut a, mut m) = cpu_with_code(&[add_sub_reg_b(1, 1, 0, 1, 2, 4, 1, 0)]);
    a.x[1] = 0x100; a.x[2] = 0x10;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xFF);
}
#[test]
fn adds_reg_64_lsl4() {
    let (mut a, mut m) = cpu_with_code(&[add_sub_reg_b(1, 0, 1, 0, 2, 4, 1, 0)]);
    a.x[1] = 0; a.x[2] = 0;
    step(&mut a, &mut m).unwrap();
    assert!(flag_z(&a));
}
#[test]
fn subs_reg_64_lsr4() {
    let (mut a, mut m) = cpu_with_code(&[add_sub_reg_b(1, 1, 1, 1, 2, 4, 1, 0)]);
    a.x[1] = 1; a.x[2] = 0x10;
    step(&mut a, &mut m).unwrap();
    assert!(flag_z(&a), "1 - (0x10 >> 4) = 0");
}

// ── Logical immediate ─────────────────────────────────────────────────────

fn log_imm_b(sf: u32, opc: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b100100 << 23) | (n << 22) | (immr << 16)
        | (imms << 10) | (rn << 5) | rd
}

#[test]
fn and_imm_32_low8() {
    // AND W0, W1, #0xFF: immr=0, imms=7, N=0, sf=0
    let (mut a, mut m) = cpu_with_code(&[log_imm_b(0, 0, 0, 0, 7, 1, 0)]);
    a.x[1] = 0xABCD;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xCD);
}
#[test]
fn and_imm_64_low32() {
    // AND X0, X1, #0xFFFFFFFF: N=1, immr=0, imms=31
    let (mut a, mut m) = cpu_with_code(&[log_imm_b(1, 0, 1, 0, 31, 1, 0)]);
    a.x[1] = 0xDEAD_BEEF_CAFE_BABE;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xCAFE_BABE);
}
#[test]
fn orr_imm_64_bit0() {
    // ORR X0, X1, #1: N=1, immr=0, imms=0
    let (mut a, mut m) = cpu_with_code(&[log_imm_b(1, 1, 1, 0, 0, 1, 0)]);
    a.x[1] = 0xFFFF_FFFF_FFFF_FFFE;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], u64::MAX);
}
#[test]
fn eor_imm_64_low8() {
    // EOR X0, X1, #0xFF: N=1, immr=0, imms=7
    let (mut a, mut m) = cpu_with_code(&[log_imm_b(1, 2, 1, 0, 7, 1, 0)]);
    a.x[1] = 0xFF;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0, "0xFF XOR 0xFF = 0");
}
#[test]
fn ands_imm_64_z() {
    // ANDS X0, X1, #0xFF sets Z
    let (mut a, mut m) = cpu_with_code(&[log_imm_b(1, 3, 1, 0, 7, 1, 0)]);
    a.x[1] = 0xFFFF_FF00;
    step(&mut a, &mut m).unwrap();
    assert!(flag_z(&a));
}
#[test]
fn ands_imm_64_nz() {
    let (mut a, mut m) = cpu_with_code(&[log_imm_b(1, 3, 1, 0, 7, 1, 0)]);
    a.x[1] = 0xFF;
    step(&mut a, &mut m).unwrap();
    assert!(!flag_z(&a));
}

// ── UBFM / SBFM systematic ──────────────────────────────────────────────

fn bitfield_b(sf: u32, opc: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b100110 << 23) | (n << 22) | (immr << 16)
        | (imms << 10) | (rn << 5) | rd
}

#[test]
fn ubfm_lsr_64_by1() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(1, 2, 1, 1, 63, 1, 0)]);
    a.x[1] = 0x8000_0000_0000_0000;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x4000_0000_0000_0000);
}
#[test]
fn ubfm_lsr_64_by4() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(1, 2, 1, 4, 63, 1, 0)]);
    a.x[1] = 0x10;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 1);
}
#[test]
fn ubfm_lsr_64_by32() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(1, 2, 1, 32, 63, 1, 0)]);
    a.x[1] = 0xABCD_0000_0000;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xABCD);
}
#[test]
fn ubfm_lsr_64_by48() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(1, 2, 1, 48, 63, 1, 0)]);
    a.x[1] = 0xABCD_0000_0000_0000;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xABCD);
}
#[test]
fn ubfm_lsr_32_by1() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(0, 2, 0, 1, 31, 1, 0)]);
    a.x[1] = 0x100;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x80);
}
#[test]
fn ubfm_lsr_32_by16() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(0, 2, 0, 16, 31, 1, 0)]);
    a.x[1] = 0xABCD_0000;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xABCD);
}
#[test]
fn ubfm_lsl_64_by1() {
    // LSL X0, X1, #1 = UBFM X0, X1, #63, #62
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(1, 2, 1, 63, 62, 1, 0)]);
    a.x[1] = 1;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 2);
}
#[test]
fn ubfm_lsl_64_by8() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(1, 2, 1, 56, 55, 1, 0)]);
    a.x[1] = 1;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x100);
}
#[test]
fn ubfm_lsl_64_by32() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(1, 2, 1, 32, 31, 1, 0)]);
    a.x[1] = 1;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x1_0000_0000);
}
#[test]
fn ubfm_lsl_32_by1() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(0, 2, 0, 31, 30, 1, 0)]);
    a.x[1] = 1;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 2);
}
#[test]
fn ubfm_lsl_32_by8() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(0, 2, 0, 24, 23, 1, 0)]);
    a.x[1] = 1;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x100);
}
#[test]
fn ubfm_uxtb_32() {
    // UXTB W0, W1 = UBFM W0, W1, #0, #7
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(0, 2, 0, 0, 7, 1, 0)]);
    a.x[1] = 0xABCD_FF42;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x42);
}
#[test]
fn sbfm_asr_64_by1() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(1, 0, 1, 1, 63, 1, 0)]);
    a.x[1] = 0x8000_0000_0000_0000;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] as i64, i64::MIN >> 1);
}
#[test]
fn sbfm_asr_64_by32() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(1, 0, 1, 32, 63, 1, 0)]);
    a.x[1] = (-1i64 as u64) << 32;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] as i64, -1);
}
#[test]
fn sbfm_asr_32_by1() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(0, 0, 0, 1, 31, 1, 0)]);
    a.x[1] = 0x8000_0000;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] as i64, (0x8000_0000u32 as i32 >> 1) as i64 & 0xFFFF_FFFF);
}
#[test]
fn sbfm_asr_32_by16() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(0, 0, 0, 16, 31, 1, 0)]);
    a.x[1] = 0xFFFF_0000u64;
    step(&mut a, &mut m).unwrap();
    // (-65536) >> 16 = -1 (sign extended)
    assert_eq!(a.x[0], 0xFFFF_FFFF);
}
#[test]
fn sbfm_sxtb_neg() {
    // SXTB X0, W1 = SBFM X0, X1, #0, #7
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(1, 0, 1, 0, 7, 1, 0)]);
    a.x[1] = 0x80; // -128 signed byte
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] as i64, -128);
}
#[test]
fn sbfm_sxtb_pos() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(1, 0, 1, 0, 7, 1, 0)]);
    a.x[1] = 0x7F;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 127);
}
#[test]
fn sbfm_sxth_neg() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(1, 0, 1, 0, 15, 1, 0)]);
    a.x[1] = 0x8000;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] as i64, -32768);
}
#[test]
fn sbfm_sxtw_neg() {
    let (mut a, mut m) = cpu_with_code(&[bitfield_b(1, 0, 1, 0, 31, 1, 0)]);
    a.x[1] = 0x8000_0000;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] as i64, i32::MIN as i64);
}

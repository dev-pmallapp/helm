//! Exhaustive NZCV flag tests. Ported from exec_flags.rs.
use super::harness::*;

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
fn ands_reg(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b11 << 29) | (0b01010 << 24) | (rm << 16) | (rn << 5) | rd
}
fn ccmp_reg(sf: u32, rm: u32, cond: u32, rn: u32, nzcv: u32) -> u32 {
    (sf << 31) | (1 << 30) | (1 << 29) | (0b11010010 << 21) | (rm << 16) | (cond << 12) | (rn << 5) | nzcv
}
fn ccmn_reg(sf: u32, rm: u32, cond: u32, rn: u32, nzcv: u32) -> u32 {
    (sf << 31) | (0 << 30) | (1 << 29) | (0b11010010 << 21) | (rm << 16) | (cond << 12) | (rn << 5) | nzcv
}

macro_rules! flag_test {
    ($name:ident, $insn:expr, $rn_val:expr, $rm_val:expr, $n:expr, $z:expr, $c:expr, $v:expr) => {
        #[test]
        fn $name() {
            let (mut c, mut m) = cpu_with_code(&[$insn]);
            c.x[1] = $rn_val; c.x[2] = $rm_val;
            step(&mut c, &mut m).unwrap();
            assert_eq!(flag_n(&c), $n, "N");
            assert_eq!(flag_z(&c), $z, "Z");
            assert_eq!(flag_c(&c), $c, "C");
            assert_eq!(flag_v(&c), $v, "V");
        }
    };
}

macro_rules! flag_test_carry {
    ($name:ident, $insn:expr, $rn_val:expr, $rm_val:expr, $cin:expr,
     $n:expr, $z:expr, $c:expr, $v:expr) => {
        #[test]
        fn $name() {
            let (mut c, mut m) = cpu_with_code(&[$insn]);
            c.x[1] = $rn_val; c.x[2] = $rm_val;
            set_nzcv(&mut c, false, false, $cin, false);
            step(&mut c, &mut m).unwrap();
            assert_eq!(flag_n(&c), $n, "N");
            assert_eq!(flag_z(&c), $z, "Z");
            assert_eq!(flag_c(&c), $c, "C");
            assert_eq!(flag_v(&c), $v, "V");
        }
    };
}

macro_rules! ccmp_test {
    ($name:ident, $sf:expr, $rn_val:expr, $rm_val:expr, $cond:expr, $nzcv_imm:expr,
     $in_n:expr, $in_z:expr, $in_c:expr, $in_v:expr,
     $out_n:expr, $out_z:expr, $out_c:expr, $out_v:expr) => {
        #[test]
        fn $name() {
            let (mut c, mut m) = cpu_with_code(&[ccmp_reg($sf, 2, $cond, 1, $nzcv_imm)]);
            c.x[1] = $rn_val; c.x[2] = $rm_val;
            set_nzcv(&mut c, $in_n, $in_z, $in_c, $in_v);
            step(&mut c, &mut m).unwrap();
            assert_eq!(flag_n(&c), $out_n, "N");
            assert_eq!(flag_z(&c), $out_z, "Z");
            assert_eq!(flag_c(&c), $out_c, "C");
            assert_eq!(flag_v(&c), $out_v, "V");
        }
    };
}

macro_rules! ccmn_test {
    ($name:ident, $sf:expr, $rn_val:expr, $rm_val:expr, $cond:expr, $nzcv_imm:expr,
     $in_n:expr, $in_z:expr, $in_c:expr, $in_v:expr,
     $out_n:expr, $out_z:expr, $out_c:expr, $out_v:expr) => {
        #[test]
        fn $name() {
            let (mut c, mut m) = cpu_with_code(&[ccmn_reg($sf, 2, $cond, 1, $nzcv_imm)]);
            c.x[1] = $rn_val; c.x[2] = $rm_val;
            set_nzcv(&mut c, $in_n, $in_z, $in_c, $in_v);
            step(&mut c, &mut m).unwrap();
            assert_eq!(flag_n(&c), $out_n, "N");
            assert_eq!(flag_z(&c), $out_z, "Z");
            assert_eq!(flag_c(&c), $out_c, "C");
            assert_eq!(flag_v(&c), $out_v, "V");
        }
    };
}

// ADDS 64-bit
flag_test!(adds64_0_0, adds_reg(1, 2, 1, 31), 0u64, 0u64, false, true, false, false);
flag_test!(adds64_1_neg1, adds_reg(1, 2, 1, 31), 1u64, u64::MAX, false, true, true, false);
flag_test!(adds64_max_1, adds_reg(1, 2, 1, 31), i64::MAX as u64, 1u64, true, false, false, true);
flag_test!(adds64_min_neg1, adds_reg(1, 2, 1, 31), i64::MIN as u64, u64::MAX, false, false, true, true);
flag_test!(adds64_neg_neg, adds_reg(1, 2, 1, 31), u64::MAX, u64::MAX, true, false, true, false);
flag_test!(adds64_pos_pos, adds_reg(1, 2, 1, 31), 100u64, 200u64, false, false, false, false);
flag_test!(adds64_max_0, adds_reg(1, 2, 1, 31), u64::MAX, 0u64, true, false, false, false);
flag_test!(adds64_half_half, adds_reg(1, 2, 1, 31), 1u64 << 63, 1u64 << 63, false, true, true, true);
flag_test!(adds64_1_0, adds_reg(1, 2, 1, 31), 1u64, 0u64, false, false, false, false);
flag_test!(adds64_neg1_0, adds_reg(1, 2, 1, 31), u64::MAX, 0u64, true, false, false, false);
flag_test!(adds64_neg1_1, adds_reg(1, 2, 1, 31), u64::MAX, 1u64, false, true, true, false);
flag_test!(adds64_min_0, adds_reg(1, 2, 1, 31), i64::MIN as u64, 0u64, true, false, false, false);
flag_test!(adds64_min_min, adds_reg(1, 2, 1, 31), i64::MIN as u64, i64::MIN as u64, false, true, true, true);
flag_test!(adds64_max_max, adds_reg(1, 2, 1, 31), i64::MAX as u64, i64::MAX as u64, true, false, false, true);

// ADDS 32-bit
flag_test!(adds32_0_0, adds_reg(0, 2, 1, 31), 0u64, 0u64, false, true, false, false);
flag_test!(adds32_max_1, adds_reg(0, 2, 1, 31), 0x7FFF_FFFFu64, 1u64, true, false, false, true);
flag_test!(adds32_ff_1, adds_reg(0, 2, 1, 31), 0xFFFF_FFFFu64, 1u64, false, true, true, false);
flag_test!(adds32_neg_neg, adds_reg(0, 2, 1, 31), 0xFFFF_FFFFu64, 0xFFFF_FFFFu64, true, false, true, false);
flag_test!(adds32_80_80, adds_reg(0, 2, 1, 31), 0x8000_0000u64, 0x8000_0000u64, false, true, true, true);
flag_test!(adds32_80000000_0, adds_reg(0, 2, 1, 31), 0x8000_0000u64, 0u64, true, false, false, false);
flag_test!(adds32_7fff_1b, adds_reg(0, 2, 1, 31), 0x7FFF_FFFFu64, 0x7FFF_FFFFu64, true, false, false, true);

// SUBS 64-bit
flag_test!(subs64_eq, subs_reg(1, 2, 1, 31), 42u64, 42u64, false, true, true, false);
flag_test!(subs64_gt, subs_reg(1, 2, 1, 31), 100u64, 50u64, false, false, true, false);
flag_test!(subs64_lt, subs_reg(1, 2, 1, 31), 50u64, 100u64, true, false, false, false);
flag_test!(subs64_0_0, subs_reg(1, 2, 1, 31), 0u64, 0u64, false, true, true, false);
flag_test!(subs64_0_1, subs_reg(1, 2, 1, 31), 0u64, 1u64, true, false, false, false);
flag_test!(subs64_min_1, subs_reg(1, 2, 1, 31), i64::MIN as u64, 1u64, false, false, true, true);
flag_test!(subs64_max_neg1, subs_reg(1, 2, 1, 31), i64::MAX as u64, u64::MAX, true, false, false, true);
flag_test!(subs64_1_0, subs_reg(1, 2, 1, 31), 1u64, 0u64, false, false, true, false);
flag_test!(subs64_1_1, subs_reg(1, 2, 1, 31), 1u64, 1u64, false, true, true, false);
flag_test!(subs64_min_max, subs_reg(1, 2, 1, 31), i64::MIN as u64, i64::MAX as u64, false, false, true, true);

// SUBS 32-bit
flag_test!(subs32_eq, subs_reg(0, 2, 1, 31), 42u64, 42u64, false, true, true, false);
flag_test!(subs32_gt, subs_reg(0, 2, 1, 31), 100u64, 50u64, false, false, true, false);
flag_test!(subs32_lt, subs_reg(0, 2, 1, 31), 50u64, 100u64, true, false, false, false);
flag_test!(subs32_0_1, subs_reg(0, 2, 1, 31), 0u64, 1u64, true, false, false, false);
flag_test!(subs32_min_1, subs_reg(0, 2, 1, 31), 0x8000_0000u64, 1u64, false, false, true, true);
flag_test!(subs32_1_0, subs_reg(0, 2, 1, 31), 1u64, 0u64, false, false, true, false);
flag_test!(subs32_1_1b, subs_reg(0, 2, 1, 31), 1u64, 1u64, false, true, true, false);
flag_test!(subs32_ff_ff, subs_reg(0, 2, 1, 31), 0xFFFF_FFFFu64, 0xFFFF_FFFFu64, false, true, true, false);

// ADCS
flag_test_carry!(adcs64_0_0_nc, adcs(1, 2, 1, 31), 0u64, 0u64, false, false, true, false, false);
flag_test_carry!(adcs64_0_0_c, adcs(1, 2, 1, 31), 0u64, 0u64, true, false, false, false, false);
flag_test_carry!(adcs64_max_0_c, adcs(1, 2, 1, 31), u64::MAX, 0u64, true, false, true, true, false);
flag_test_carry!(adcs64_half_nc, adcs(1, 2, 1, 31), i64::MAX as u64, 1u64, false, true, false, false, true);
flag_test_carry!(adcs32_ff_0_c, adcs(0, 2, 1, 31), 0xFFFF_FFFFu64, 0u64, true, false, true, true, false);
flag_test_carry!(adcs32_7f_0_c, adcs(0, 2, 1, 31), 0x7FFF_FFFFu64, 0u64, true, true, false, false, true);
flag_test_carry!(adcs64_1_1_nc, adcs(1, 2, 1, 31), 1u64, 1u64, false, false, false, false, false);
flag_test_carry!(adcs64_max_1_nc, adcs(1, 2, 1, 31), i64::MAX as u64, 1u64, false, true, false, false, true);
flag_test_carry!(adcs32_ff_1_nc, adcs(0, 2, 1, 31), 0xFFFF_FFFFu64, 1u64, false, false, true, true, false);
flag_test_carry!(adcs32_0_0_nc, adcs(0, 2, 1, 31), 0u64, 0u64, false, false, true, false, false);

// SBCS
flag_test_carry!(sbcs64_100_50_c, sbcs(1, 2, 1, 31), 100u64, 50u64, true, false, false, true, false);
flag_test_carry!(sbcs64_100_50_nc, sbcs(1, 2, 1, 31), 100u64, 50u64, false, false, false, true, false);
flag_test_carry!(sbcs64_50_100_c, sbcs(1, 2, 1, 31), 50u64, 100u64, true, true, false, false, false);
flag_test_carry!(sbcs64_0_0_c, sbcs(1, 2, 1, 31), 0u64, 0u64, true, false, true, true, false);
flag_test_carry!(sbcs64_0_0_nc, sbcs(1, 2, 1, 31), 0u64, 0u64, false, true, false, false, false);
flag_test_carry!(sbcs32_eq_c, sbcs(0, 2, 1, 31), 42u64, 42u64, true, false, true, true, false);
flag_test_carry!(sbcs32_eq_nc, sbcs(0, 2, 1, 31), 42u64, 42u64, false, true, false, false, false);
flag_test_carry!(sbcs64_1_0_c, sbcs(1, 2, 1, 31), 1u64, 0u64, true, false, false, true, false);
flag_test_carry!(sbcs64_0_1_c, sbcs(1, 2, 1, 31), 0u64, 1u64, true, true, false, false, false);

// ANDS
flag_test!(ands64_zero, ands_reg(1, 2, 1, 31), 0xFF00u64, 0x00FFu64, false, true, false, false);
flag_test!(ands64_nonzero, ands_reg(1, 2, 1, 31), 0xFF00u64, 0x0FF0u64, false, false, false, false);
flag_test!(ands64_msb, ands_reg(1, 2, 1, 31), u64::MAX, 1u64 << 63, true, false, false, false);
flag_test!(ands64_all_ones, ands_reg(1, 2, 1, 31), u64::MAX, u64::MAX, true, false, false, false);
flag_test!(ands32_zero, ands_reg(0, 2, 1, 31), 0xFF00u64, 0x00FFu64, false, true, false, false);
flag_test!(ands32_msb, ands_reg(0, 2, 1, 31), 0xFFFF_FFFFu64, 0x8000_0000u64, true, false, false, false);

// CCMP
ccmp_test!(ccmp64_eq_t_equal, 1, 42, 42, 0, 0b0000, false, true, false, false, false, true, true, false);
ccmp_test!(ccmp64_eq_t_gt, 1, 100, 50, 0, 0b0000, false, true, false, false, false, false, true, false);
ccmp_test!(ccmp64_eq_t_lt, 1, 50, 100, 0, 0b0000, false, true, false, false, true, false, false, false);
ccmp_test!(ccmp64_eq_f_0000, 1, 42, 42, 0, 0b0000, false, false, false, false, false, false, false, false);
ccmp_test!(ccmp64_eq_f_1111, 1, 42, 42, 0, 0b1111, false, false, false, false, true, true, true, true);
ccmp_test!(ccmp64_eq_f_1010, 1, 42, 42, 0, 0b1010, false, false, false, false, true, false, true, false);
ccmp_test!(ccmp64_ne_t, 1, 10, 10, 1, 0b1111, false, false, false, false, false, true, true, false);
ccmp_test!(ccmp64_lt_t, 1, 50, 100, 11, 0b0000, true, false, false, false, true, false, false, false);
ccmp_test!(ccmp64_lt_f, 1, 50, 100, 11, 0b0100, false, false, false, false, false, true, false, false);
ccmp_test!(ccmp32_eq_t, 0, 42, 42, 0, 0b0000, false, true, false, false, false, true, true, false);
ccmp_test!(ccmp32_eq_f, 0, 42, 42, 0, 0b1111, false, false, false, false, true, true, true, true);

// CCMN
ccmn_test!(ccmn64_eq_t_zero, 1, 0, 0, 0, 0b0000, false, true, false, false, false, true, false, false);
ccmn_test!(ccmn64_eq_f, 1, 0, 0, 0, 0b1010, false, false, false, false, true, false, true, false);
ccmn_test!(ccmn32_eq_t, 0, 0, 0, 0, 0b0000, false, true, false, false, false, true, false, false);

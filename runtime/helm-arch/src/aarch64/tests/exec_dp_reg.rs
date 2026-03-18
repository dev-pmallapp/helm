//! AArch64 Data Processing — Register instruction tests.
//! Ported from exec_dp_reg.rs.
use super::harness::*;

fn encode_csel_family(sf: u32, else_inv: u32, else_inc: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (else_inv << 30) | (0b011010100 << 21) | (rm << 16) | (cond << 12) | (else_inc << 10) | (rn << 5) | rd
}
fn encode_csel(sf: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 { encode_csel_family(sf, 0, 0, rm, cond, rn, rd) }
fn encode_csinc(sf: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 { encode_csel_family(sf, 0, 1, rm, cond, rn, rd) }
fn encode_csinv(sf: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 { encode_csel_family(sf, 1, 0, rm, cond, rn, rd) }
fn encode_csneg(sf: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 { encode_csel_family(sf, 1, 1, rm, cond, rn, rd) }
fn encode_dp2(sf: u32, opcode: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b0011010110 << 21) | (rm << 16) | (opcode << 10) | (rn << 5) | rd
}
fn encode_dp1(sf: u32, opcode: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b1011010110 << 21) | (opcode << 10) | (rn << 5) | rd
}
fn encode_adc_family(sf: u32, op: u32, s_flag: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s_flag << 29) | (0b11010000 << 21) | (rm << 16) | (rn << 5) | rd
}
fn encode_adc(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 { encode_adc_family(sf, 0, 0, rm, rn, rd) }
fn encode_adcs(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 { encode_adc_family(sf, 0, 1, rm, rn, rd) }
fn encode_sbc(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 { encode_adc_family(sf, 1, 0, rm, rn, rd) }
fn encode_sbcs(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 { encode_adc_family(sf, 1, 1, rm, rn, rd) }
fn encode_ccmp(sf: u32, rm: u32, cond: u32, rn: u32, nzcv: u32) -> u32 {
    (sf << 31) | (1 << 30) | (1 << 29) | (0b11010010 << 21) | (rm << 16) | (cond << 12) | (rn << 5) | nzcv
}
fn encode_ubfm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b10 << 29) | (0b100110 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
fn encode_sbfm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b00 << 29) | (0b100110 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
fn encode_bfm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b01 << 29) | (0b100110 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}

const EQ: u32 = 0; const NE: u32 = 1; const CS: u32 = 2; const CC: u32 = 3;
const MI: u32 = 4; const LT: u32 = 11; const LE: u32 = 13;

#[test]
fn csel_64_eq_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_csel(1, 2, EQ, 1, 0)]);
    c.x[1] = 0xAAAA; c.x[2] = 0xBBBB;
    set_nzcv(&mut c, false, true, false, false);
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0xAAAA);
}
#[test]
fn csel_64_eq_not_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_csel(1, 2, EQ, 1, 0)]);
    c.x[1] = 0xAAAA; c.x[2] = 0xBBBB;
    set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0xBBBB);
}
#[test]
fn csel_32_truncates() {
    let (mut c, mut m) = cpu_with_code(&[encode_csel(0, 2, NE, 1, 0)]);
    c.x[1] = 0x1_FFFF_FFFF;
    set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0xFFFF_FFFF);
}
#[test]
fn csinc_64_false_increments() {
    let (mut c, mut m) = cpu_with_code(&[encode_csinc(1, 2, EQ, 1, 0)]);
    c.x[2] = 10;
    set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 11);
}
#[test]
fn csinv_64_false_inverts() {
    let (mut c, mut m) = cpu_with_code(&[encode_csinv(1, 2, EQ, 1, 0)]);
    c.x[2] = 0;
    set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], u64::MAX);
}
#[test]
fn csneg_64_false_negates() {
    let (mut c, mut m) = cpu_with_code(&[encode_csneg(1, 2, EQ, 1, 0)]);
    c.x[2] = 5;
    set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], (-5i64) as u64);
}
#[test]
fn csel_lt_condition() {
    let (mut c, mut m) = cpu_with_code(&[encode_csel(1, 2, LT, 1, 0)]);
    c.x[1] = 0xAAAA; c.x[2] = 0xBBBB;
    set_nzcv(&mut c, true, false, false, false);
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0xAAAA);
}
#[test]
fn udiv_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b000010, 2, 1, 0)]);
    c.x[1] = 100; c.x[2] = 7; step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 14);
}
#[test]
fn udiv_by_zero() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b000010, 2, 1, 0)]);
    c.x[1] = 42; c.x[2] = 0; step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0);
}
#[test]
fn sdiv_64_negative() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b000011, 2, 1, 0)]);
    c.x[1] = (-100i64) as u64; c.x[2] = 7;
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], (-14i64) as u64);
}
#[test]
fn lslv_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b001000, 2, 1, 0)]);
    c.x[1] = 1; c.x[2] = 40; step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 1u64 << 40);
}
#[test]
fn lsrv_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b001001, 2, 1, 0)]);
    c.x[1] = 0x8000_0000_0000_0000; c.x[2] = 63;
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 1);
}
#[test]
fn asrv_64_sign_extends() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b001010, 2, 1, 0)]);
    c.x[1] = 0x8000_0000_0000_0000; c.x[2] = 4;
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0xF800_0000_0000_0000);
}
#[test]
fn rorv_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b001011, 2, 1, 0)]);
    c.x[1] = 1; c.x[2] = 1; step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0x8000_0000_0000_0000);
}
#[test]
fn rbit_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp1(1, 0b000000, 1, 0)]);
    c.x[1] = 0x8000_0000_0000_0001; step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0x8000_0000_0000_0001);
}
#[test]
fn rev_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp1(1, 0b000011, 1, 0)]);
    c.x[1] = 0x01_02_03_04_05_06_07_08; step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x08_07_06_05_04_03_02_01);
}
#[test]
fn rev_32() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp1(0, 0b000011, 1, 0)]);
    c.x[1] = 0x01_02_03_04; step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0x04_03_02_01);
}
#[test]
fn clz_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp1(1, 0b000100, 1, 0)]);
    c.x[1] = 0x0000_0000_0000_0100; step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 55);
}
#[test]
fn clz_64_zero_input() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp1(1, 0b000100, 1, 0)]);
    c.x[1] = 0; step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 64);
}
#[test]
fn clz_32_zero_input() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp1(0, 0b000100, 1, 0)]);
    c.x[1] = 0; step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 32);
}
#[test]
fn adc_64_with_carry() {
    let (mut c, mut m) = cpu_with_code(&[encode_adc(1, 2, 1, 0)]);
    c.x[1] = 100; c.x[2] = 50; set_nzcv(&mut c, false, false, true, false);
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 151);
}
#[test]
fn adc_64_without_carry() {
    let (mut c, mut m) = cpu_with_code(&[encode_adc(1, 2, 1, 0)]);
    c.x[1] = 100; c.x[2] = 50; set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 150);
}
#[test]
fn sbc_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbc(1, 2, 1, 0)]);
    c.x[1] = 100; c.x[2] = 30; set_nzcv(&mut c, false, false, true, false);
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 70);
}
#[test]
fn sbc_64_with_borrow() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbc(1, 2, 1, 0)]);
    c.x[1] = 100; c.x[2] = 30; set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 69);
}
#[test]
fn adcs_64_sets_zero_flag() {
    let (mut c, mut m) = cpu_with_code(&[encode_adcs(1, 2, 1, 0)]);
    c.x[1] = 0; c.x[2] = 0; set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap(); assert!(flag_z(&c)); assert!(!flag_n(&c));
}
#[test]
fn sbcs_64_sets_negative() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbcs(1, 2, 1, 0)]);
    c.x[1] = 5; c.x[2] = 10; set_nzcv(&mut c, false, false, true, false);
    step(&mut c, &mut m).unwrap(); assert!(flag_n(&c));
}
#[test]
fn ccmp_64_cond_true_performs_compare() {
    let (mut c, mut m) = cpu_with_code(&[encode_ccmp(1, 2, EQ, 1, 0)]);
    c.x[1] = 10; c.x[2] = 10; set_nzcv(&mut c, false, true, false, false);
    step(&mut c, &mut m).unwrap(); assert!(flag_z(&c)); assert!(flag_c(&c));
}
#[test]
fn ccmp_64_cond_false_uses_nzcv_imm() {
    let (mut c, mut m) = cpu_with_code(&[encode_ccmp(1, 2, EQ, 1, 0b1010)]);
    c.x[1] = 10; c.x[2] = 5; set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap();
    assert!(flag_n(&c)); assert!(!flag_z(&c)); assert!(flag_c(&c)); assert!(!flag_v(&c));
}
#[test]
fn ubfm_lsl_alias() {
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(1, 1, 60, 59, 1, 0)]);
    c.x[1] = 0xF; step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0xF0);
}
#[test]
fn ubfm_lsr_alias() {
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(1, 1, 4, 63, 1, 0)]);
    c.x[1] = 0xF0; step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0xF);
}
#[test]
fn sbfm_sxtw() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(1, 1, 0, 31, 1, 0)]);
    c.x[1] = 0xFFFF_FFFF; step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0xFFFF_FFFF_FFFF_FFFF);
}
#[test]
fn ubfx_extracts_bits() {
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(0, 0, 4, 11, 1, 0)]);
    c.x[1] = 0xABCD; step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0xBC);
}
#[test]
fn bfi_64_insert_at_bit12() {
    let (mut c, mut m) = cpu_with_code(&[encode_bfm(1, 1, 52, 51, 1, 0)]);
    c.x[0] = 0; c.x[1] = 2; step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0x2000);
}

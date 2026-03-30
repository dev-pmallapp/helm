//! AArch64 Data Processing — Register instruction tests.
//! Ported from exec_dp_reg.rs.
use super::harness::*;

fn encode_csel_family(
    sf: u32,
    else_inv: u32,
    else_inc: u32,
    rm: u32,
    cond: u32,
    rn: u32,
    rd: u32,
) -> u32 {
    (sf << 31)
        | (else_inv << 30)
        | (0b011010100 << 21)
        | (rm << 16)
        | (cond << 12)
        | (else_inc << 10)
        | (rn << 5)
        | rd
}
fn encode_csel(sf: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 {
    encode_csel_family(sf, 0, 0, rm, cond, rn, rd)
}
fn encode_csinc(sf: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 {
    encode_csel_family(sf, 0, 1, rm, cond, rn, rd)
}
fn encode_csinv(sf: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 {
    encode_csel_family(sf, 1, 0, rm, cond, rn, rd)
}
fn encode_csneg(sf: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 {
    encode_csel_family(sf, 1, 1, rm, cond, rn, rd)
}
fn encode_dp2(sf: u32, opcode: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b0011010110 << 21) | (rm << 16) | (opcode << 10) | (rn << 5) | rd
}
fn encode_dp1(sf: u32, opcode: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b1011010110 << 21) | (opcode << 10) | (rn << 5) | rd
}
fn encode_adc_family(sf: u32, op: u32, s_flag: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s_flag << 29) | (0b11010000 << 21) | (rm << 16) | (rn << 5) | rd
}
fn encode_adc(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    encode_adc_family(sf, 0, 0, rm, rn, rd)
}
fn encode_adcs(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    encode_adc_family(sf, 0, 1, rm, rn, rd)
}
fn encode_sbc(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    encode_adc_family(sf, 1, 0, rm, rn, rd)
}
fn encode_sbcs(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    encode_adc_family(sf, 1, 1, rm, rn, rd)
}
fn encode_ccmp(sf: u32, rm: u32, cond: u32, rn: u32, nzcv: u32) -> u32 {
    (sf << 31)
        | (1 << 30)
        | (1 << 29)
        | (0b11010010 << 21)
        | (rm << 16)
        | (cond << 12)
        | (rn << 5)
        | nzcv
}
fn encode_ubfm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (0b10 << 29)
        | (0b100110 << 23)
        | (n << 22)
        | (immr << 16)
        | (imms << 10)
        | (rn << 5)
        | rd
}
fn encode_sbfm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (0b00 << 29)
        | (0b100110 << 23)
        | (n << 22)
        | (immr << 16)
        | (imms << 10)
        | (rn << 5)
        | rd
}
fn encode_bfm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (0b01 << 29)
        | (0b100110 << 23)
        | (n << 22)
        | (immr << 16)
        | (imms << 10)
        | (rn << 5)
        | rd
}

const EQ: u32 = 0;
const NE: u32 = 1;
const CS: u32 = 2;
const CC: u32 = 3;
const MI: u32 = 4;
const LT: u32 = 11;
const LE: u32 = 13;

#[test]
fn csel_64_eq_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_csel(1, 2, EQ, 1, 0)]);
    c.x[1] = 0xAAAA;
    c.x[2] = 0xBBBB;
    set_nzcv(&mut c, false, true, false, false);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xAAAA);
}
#[test]
fn csel_64_eq_not_taken() {
    let (mut c, mut m) = cpu_with_code(&[encode_csel(1, 2, EQ, 1, 0)]);
    c.x[1] = 0xAAAA;
    c.x[2] = 0xBBBB;
    set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xBBBB);
}
#[test]
fn csel_32_truncates() {
    let (mut c, mut m) = cpu_with_code(&[encode_csel(0, 2, NE, 1, 0)]);
    c.x[1] = 0x1_FFFF_FFFF;
    set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_FFFF);
}
#[test]
fn csinc_64_false_increments() {
    let (mut c, mut m) = cpu_with_code(&[encode_csinc(1, 2, EQ, 1, 0)]);
    c.x[2] = 10;
    set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 11);
}
#[test]
fn csinv_64_false_inverts() {
    let (mut c, mut m) = cpu_with_code(&[encode_csinv(1, 2, EQ, 1, 0)]);
    c.x[2] = 0;
    set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], u64::MAX);
}
#[test]
fn csneg_64_false_negates() {
    let (mut c, mut m) = cpu_with_code(&[encode_csneg(1, 2, EQ, 1, 0)]);
    c.x[2] = 5;
    set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], (-5i64) as u64);
}
#[test]
fn csel_lt_condition() {
    let (mut c, mut m) = cpu_with_code(&[encode_csel(1, 2, LT, 1, 0)]);
    c.x[1] = 0xAAAA;
    c.x[2] = 0xBBBB;
    set_nzcv(&mut c, true, false, false, false);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xAAAA);
}
#[test]
fn udiv_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b000010, 2, 1, 0)]);
    c.x[1] = 100;
    c.x[2] = 7;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 14);
}
#[test]
fn udiv_by_zero() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b000010, 2, 1, 0)]);
    c.x[1] = 42;
    c.x[2] = 0;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0);
}
#[test]
fn sdiv_64_negative() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b000011, 2, 1, 0)]);
    c.x[1] = (-100i64) as u64;
    c.x[2] = 7;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], (-14i64) as u64);
}
#[test]
fn lslv_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b001000, 2, 1, 0)]);
    c.x[1] = 1;
    c.x[2] = 40;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 1u64 << 40);
}
#[test]
fn lsrv_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b001001, 2, 1, 0)]);
    c.x[1] = 0x8000_0000_0000_0000;
    c.x[2] = 63;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 1);
}
#[test]
fn asrv_64_sign_extends() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b001010, 2, 1, 0)]);
    c.x[1] = 0x8000_0000_0000_0000;
    c.x[2] = 4;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xF800_0000_0000_0000);
}
#[test]
fn rorv_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b001011, 2, 1, 0)]);
    c.x[1] = 1;
    c.x[2] = 1;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x8000_0000_0000_0000);
}
#[test]
fn rbit_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp1(1, 0b000000, 1, 0)]);
    c.x[1] = 0x8000_0000_0000_0001;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x8000_0000_0000_0001);
}
#[test]
fn rev_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp1(1, 0b000011, 1, 0)]);
    c.x[1] = 0x01_02_03_04_05_06_07_08;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x08_07_06_05_04_03_02_01);
}
#[test]
fn rev_32() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp1(0, 0b000011, 1, 0)]);
    c.x[1] = 0x01_02_03_04;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x04_03_02_01);
}
#[test]
fn clz_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp1(1, 0b000100, 1, 0)]);
    c.x[1] = 0x0000_0000_0000_0100;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 55);
}
#[test]
fn clz_64_zero_input() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp1(1, 0b000100, 1, 0)]);
    c.x[1] = 0;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 64);
}
#[test]
fn clz_32_zero_input() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp1(0, 0b000100, 1, 0)]);
    c.x[1] = 0;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 32);
}
#[test]
fn adc_64_with_carry() {
    let (mut c, mut m) = cpu_with_code(&[encode_adc(1, 2, 1, 0)]);
    c.x[1] = 100;
    c.x[2] = 50;
    set_nzcv(&mut c, false, false, true, false);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 151);
}
#[test]
fn adc_64_without_carry() {
    let (mut c, mut m) = cpu_with_code(&[encode_adc(1, 2, 1, 0)]);
    c.x[1] = 100;
    c.x[2] = 50;
    set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 150);
}
#[test]
fn sbc_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbc(1, 2, 1, 0)]);
    c.x[1] = 100;
    c.x[2] = 30;
    set_nzcv(&mut c, false, false, true, false);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 70);
}
#[test]
fn sbc_64_with_borrow() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbc(1, 2, 1, 0)]);
    c.x[1] = 100;
    c.x[2] = 30;
    set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 69);
}
#[test]
fn adcs_64_sets_zero_flag() {
    let (mut c, mut m) = cpu_with_code(&[encode_adcs(1, 2, 1, 0)]);
    c.x[1] = 0;
    c.x[2] = 0;
    set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap();
    assert!(flag_z(&c));
    assert!(!flag_n(&c));
}
#[test]
fn sbcs_64_sets_negative() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbcs(1, 2, 1, 0)]);
    c.x[1] = 5;
    c.x[2] = 10;
    set_nzcv(&mut c, false, false, true, false);
    step(&mut c, &mut m).unwrap();
    assert!(flag_n(&c));
}
#[test]
fn ccmp_64_cond_true_performs_compare() {
    let (mut c, mut m) = cpu_with_code(&[encode_ccmp(1, 2, EQ, 1, 0)]);
    c.x[1] = 10;
    c.x[2] = 10;
    set_nzcv(&mut c, false, true, false, false);
    step(&mut c, &mut m).unwrap();
    assert!(flag_z(&c));
    assert!(flag_c(&c));
}
#[test]
fn ccmp_64_cond_false_uses_nzcv_imm() {
    let (mut c, mut m) = cpu_with_code(&[encode_ccmp(1, 2, EQ, 1, 0b1010)]);
    c.x[1] = 10;
    c.x[2] = 5;
    set_nzcv(&mut c, false, false, false, false);
    step(&mut c, &mut m).unwrap();
    assert!(flag_n(&c));
    assert!(!flag_z(&c));
    assert!(flag_c(&c));
    assert!(!flag_v(&c));
}
#[test]
fn ubfm_lsl_alias() {
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(1, 1, 60, 59, 1, 0)]);
    c.x[1] = 0xF;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xF0);
}
#[test]
fn ubfm_lsr_alias() {
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(1, 1, 4, 63, 1, 0)]);
    c.x[1] = 0xF0;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xF);
}
#[test]
fn sbfm_sxtw() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(1, 1, 0, 31, 1, 0)]);
    c.x[1] = 0xFFFF_FFFF;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_FFFF_FFFF_FFFF);
}
#[test]
fn ubfx_extracts_bits() {
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(0, 0, 4, 11, 1, 0)]);
    c.x[1] = 0xABCD;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xBC);
}
#[test]
fn bfi_64_insert_at_bit12() {
    let (mut c, mut m) = cpu_with_code(&[encode_bfm(1, 1, 52, 51, 1, 0)]);
    c.x[0] = 0;
    c.x[1] = 2;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x2000);
}

// ── Additional encoding helpers ───────────────────────────────────────────

fn encode_lslv(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    encode_dp2(sf, 0b001000, rm, rn, rd)
}
fn encode_lsrv(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    encode_dp2(sf, 0b001001, rm, rn, rd)
}
fn encode_asrv(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    encode_dp2(sf, 0b001010, rm, rn, rd)
}
fn encode_rorv(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    encode_dp2(sf, 0b001011, rm, rn, rd)
}
fn encode_udiv(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    encode_dp2(sf, 0b000010, rm, rn, rd)
}
fn encode_sdiv(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    encode_dp2(sf, 0b000011, rm, rn, rd)
}
fn encode_rbit(sf: u32, rn: u32, rd: u32) -> u32 {
    encode_dp1(sf, 0b000000, rn, rd)
}
fn encode_rev(sf: u32, rn: u32, rd: u32) -> u32 {
    encode_dp1(sf, 0b000010, rn, rd)
}
fn encode_rev16(sf: u32, rn: u32, rd: u32) -> u32 {
    encode_dp1(sf, 0b000001, rn, rd)
}
fn encode_clz(sf: u32, rn: u32, rd: u32) -> u32 {
    encode_dp1(sf, 0b000100, rn, rd)
}
fn encode_cls(sf: u32, rn: u32, rd: u32) -> u32 {
    encode_dp1(sf, 0b000101, rn, rd)
}
fn encode_and_imm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (0b00 << 29)
        | (0b100100 << 23)
        | (n << 22)
        | (immr << 16)
        | (imms << 10)
        | (rn << 5)
        | rd
}
fn encode_orr_imm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (0b01 << 29)
        | (0b100100 << 23)
        | (n << 22)
        | (immr << 16)
        | (imms << 10)
        | (rn << 5)
        | rd
}

// ── ADC 32-bit ────────────────────────────────────────────────────────────
#[test]
fn adc_32() {
    let (mut c, mut m) = cpu_with_code(&[encode_adc(0, 2, 1, 0)]);
    c.x[1] = 0xFFFF_FFFF;
    c.x[2] = 1;
    set_nzcv(&mut c, false, false, true, false); // C=1
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 1, "ADC 32-bit: 0xFFFFFFFF + 1 + 1 wraps to 1");
}

// ── CSEL conditions ───────────────────────────────────────────────────────
#[test]
fn csel_cc_condition() {
    let (mut c, mut m) = cpu_with_code(&[encode_csel(1, 2, CC, 1, 0)]);
    c.x[1] = 0xAAAA;
    c.x[2] = 0xBBBB;
    set_nzcv(&mut c, false, false, true, false); // C=1 → CC is false
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xBBBB, "CC false when C=1");
}

#[test]
fn csinc_32_false_increments() {
    let (mut c, mut m) = cpu_with_code(&[encode_csinc(0, 2, EQ, 1, 0)]);
    c.x[2] = 0xFFFF_FFFF;
    set_nzcv(&mut c, false, false, false, false); // EQ false
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0, "32-bit CSINC wraps at 32 bits");
}

// ── UDIV / SDIV 32-bit ────────────────────────────────────────────────────
#[test]
fn udiv_32() {
    let (mut c, mut m) = cpu_with_code(&[encode_udiv(0, 2, 1, 0)]);
    c.x[1] = 0x1_0000_0064;
    c.x[2] = 10; // upper 32 bits ignored
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 10, "UDIV 32-bit: 0x64/10=10");
}

#[test]
fn sdiv_32() {
    let (mut c, mut m) = cpu_with_code(&[encode_sdiv(0, 2, 1, 0)]);
    c.x[1] = (-100i32) as u32 as u64;
    c.x[2] = 7;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], (-14i32 as u32) as u64, "SDIV 32-bit: -100/7=-14");
}

// ── LSLV / LSRV / ASRV / RORV 32-bit ────────────────────────────────────
#[test]
fn lslv_32() {
    let (mut c, mut m) = cpu_with_code(&[encode_lslv(0, 2, 1, 0)]);
    c.x[1] = 1;
    c.x[2] = 16;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x10000, "LSLV 32-bit: 1 << 16");
}

#[test]
fn lsrv_32() {
    let (mut c, mut m) = cpu_with_code(&[encode_lsrv(0, 2, 1, 0)]);
    c.x[1] = 0x8000_0000;
    c.x[2] = 31;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 1, "LSRV 32-bit: logical shift right");
}

#[test]
fn asrv_32_sign_extends() {
    let (mut c, mut m) = cpu_with_code(&[encode_asrv(0, 2, 1, 0)]);
    c.x[1] = 0x8000_0000;
    c.x[2] = 4;
    step(&mut c, &mut m).unwrap();
    assert_eq!(
        c.x[0], 0xF800_0000,
        "ASRV 32-bit: arithmetic shift preserves sign"
    );
}

// ── RBIT / REV / REV16 32-bit / CLZ / CLS ───────────────────────────────
#[test]
fn rbit_32() {
    let (mut c, mut m) = cpu_with_code(&[encode_rbit(0, 1, 0)]);
    c.x[1] = 0x80000001;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x80000001, "RBIT 32-bit palindrome");
}

#[test]
fn rev16_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_rev16(1, 1, 0)]);
    c.x[1] = 0x0102_0304_0506_0708;
    step(&mut c, &mut m).unwrap();
    assert_eq!(
        c.x[0], 0x0201_0403_0605_0807,
        "REV16 64-bit: swap bytes in halfwords"
    );
}

#[test]
fn clz_32() {
    let (mut c, mut m) = cpu_with_code(&[encode_clz(0, 1, 0)]);
    c.x[1] = 0x0000_0100;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 23, "CLZ 32-bit: 23 leading zeros");
}

#[test]
fn cls_64_positive() {
    let (mut c, mut m) = cpu_with_code(&[encode_cls(1, 1, 0)]);
    c.x[1] = 0x0FFF_FFFF_FFFF_FFFF;
    step(&mut c, &mut m).unwrap();
    assert_eq!(
        c.x[0], 3,
        "CLS 64-bit positive: 3 leading sign bits after MSB"
    );
}

#[test]
fn cls_64_negative() {
    let (mut c, mut m) = cpu_with_code(&[encode_cls(1, 1, 0)]);
    c.x[1] = 0xF000_0000_0000_0000;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 3, "CLS 64-bit negative: 3 leading 1s after MSB");
}

// ── Logical immediate corner cases ────────────────────────────────────────
#[test]
fn and_imm_32_masks_low_bits() {
    // AND W0, W0, #0xFFFFF000: N=0, immr=20, imms=19
    let (mut c, mut m) = cpu_with_code(&[encode_and_imm(0, 0, 20, 19, 0, 0)]);
    c.x[0] = 0x123;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0, "AND W0, #0xFFFFF000 clears low 12 bits");
}

#[test]
fn and_imm_32_sub_element_rotation() {
    let (mut c, mut m) = cpu_with_code(&[encode_and_imm(0, 0, 20, 19, 0, 0)]);
    c.x[0] = 0x2000;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x2000, "AND W0, #0xFFFFF000 preserves 0x2000");
}

// ── SBFM (SXTH) ──────────────────────────────────────────────────────────
#[test]
fn sbfm_sxth() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(1, 1, 0, 15, 1, 0)]);
    c.x[1] = 0x8000;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_FFFF_FFFF_8000, "SXTH sign-extends -32768");
}

// ── 32-bit shifted register operations (apply_shift correctness) ─────────

/// Encode ADD/SUB (shifted register).
/// sf op S 01011 shift 0 Rm imm6 Rn Rd
fn encode_addsub_shift(
    sf: u32,
    op: u32,
    s: u32,
    shift: u32,
    rm: u32,
    imm6: u32,
    rn: u32,
    rd: u32,
) -> u32 {
    (sf << 31)
        | (op << 30)
        | (s << 29)
        | (0b01011 << 24)
        | (shift << 22)
        | (0 << 21)
        | (rm << 16)
        | (imm6 << 10)
        | (rn << 5)
        | rd
}

/// Encode ORN (shifted register) = MVN when Rn=XZR.
/// sf 01 01010 shift 1 Rm imm6 Rn Rd
fn encode_orn_shift(sf: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (0b01 << 29)
        | (0b01010 << 24)
        | (shift << 22)
        | (1 << 21)
        | (rm << 16)
        | (imm6 << 10)
        | (rn << 5)
        | rd
}

#[test]
fn subs_w_lsr_shifted_reg() {
    // SUBS WZR, WZR, W3, LSR #26  (CMP wzr, w3, lsr #26)
    // W3 = 0xFFFFFFDC (= -36 as i32), upper X3 bits = 0
    // W3 >> 26 should be 0x3F (using 32-bit shift), NOT 3 (64-bit shift)
    let raw = encode_addsub_shift(0, 1, 1, /*LSR*/ 1, 3, 26, 31, 31);
    let (mut c, mut m) = cpu_with_code(&[raw]);
    c.x[3] = 0x00000000_FFFFFFDC; // w3=-36, upper bits=0
    step(&mut c, &mut m).unwrap();
    // WZR - (W3 LSR 26) = 0 - 0x3F = wraps to 0xFFFFFFC1
    // Z should be 0 (result non-zero), N should be 1
    assert!(
        c.flag_n(),
        "SUBS WZR, WZR, W3 LSR #26: N=1 (negative result)"
    );
    assert!(!c.flag_z(), "SUBS WZR, WZR, W3 LSR #26: Z=0 (non-zero)");
}

#[test]
fn mvn_w_asr_shifted_reg() {
    // MVN W0, W3, ASR #26 = ORN W0, WZR, W3, ASR #26
    // W3 = 0xFFFFFFDC (= -36 as i32)
    // 32-bit ASR: W3 >> 26 = -1 = 0xFFFFFFFF, then NOT = 0x00000000
    let raw = encode_orn_shift(0, /*ASR*/ 2, 3, 26, 31, 0);
    let (mut c, mut m) = cpu_with_code(&[raw]);
    c.x[3] = 0x00000000_FFFFFFDC;
    step(&mut c, &mut m).unwrap();
    assert_eq!(
        c.x[0], 0,
        "MVN W0, W3 ASR #26: ~(-1) = 0 for small negative w3"
    );
}

#[test]
fn mvn_w_asr_positive() {
    // MVN W0, W3, ASR #26 with positive W3
    // W3 = 0x10000000. W3 ASR 26 = 0x10000000 >> 26 = 4. NOT(4) = 0xFFFFFFFB
    let raw = encode_orn_shift(0, /*ASR*/ 2, 3, 26, 31, 0);
    let (mut c, mut m) = cpu_with_code(&[raw]);
    c.x[3] = 0x10000000;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFFFFFB, "MVN W0, W3 ASR #26: ~4 = 0xFFFFFFFB");
}

#[test]
fn sub_w_lsr_no_upper_contamination() {
    // SUB W0, W1, W2, LSR #4
    // X2 = 0x00000001_000000F0 (upper bits set)
    // W2 = 0x000000F0, W2 LSR 4 = 0x0F
    // W1 = 0x10, result = 0x10 - 0x0F = 1
    let raw = encode_addsub_shift(0, 1, 0, /*LSR*/ 1, 2, 4, 1, 0);
    let (mut c, mut m) = cpu_with_code(&[raw]);
    c.x[1] = 0x10;
    c.x[2] = 0x00000001_000000F0;
    step(&mut c, &mut m).unwrap();
    assert_eq!(
        c.x[0], 1,
        "SUB W0, W1, W2 LSR #4: upper X2 bits must not contaminate"
    );
}

// ── CRC32 / CRC32C ──────────────────────────────────────────────────────────

#[test]
fn crc32b_zero_init() {
    // CRC32B W0, W1, W2  — sf=0, opcode=0b010000
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(0, 0b010000, 2, 1, 0)]);
    c.x[1] = 0;
    c.x[2] = 0x41;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x01db7106);
}

#[test]
fn crc32b_nonzero_init() {
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(0, 0b010000, 2, 1, 0)]);
    c.x[1] = 0xDEADBEEF;
    c.x[2] = 0x41;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x31b02351);
}

#[test]
fn crc32h_zero_init() {
    // CRC32H W0, W1, W2  — sf=0, opcode=0b010001
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(0, 0b010001, 2, 1, 0)]);
    c.x[1] = 0;
    c.x[2] = 0x4142;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xc3945c81);
}

#[test]
fn crc32w_zero_init() {
    // CRC32W W0, W1, W2  — sf=0, opcode=0b010010
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(0, 0b010010, 2, 1, 0)]);
    c.x[1] = 0;
    c.x[2] = 0x41424344;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xa53ea072);
}

#[test]
fn crc32x_zero_init() {
    // CRC32X W0, W1, X2  — sf=1, opcode=0b010011
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b010011, 2, 1, 0)]);
    c.x[1] = 0;
    c.x[2] = 0x4142434445464748;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x97f80c95);
}

#[test]
fn crc32cb_zero_init() {
    // CRC32CB W0, W1, W2  — sf=0, opcode=0b010100
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(0, 0b010100, 2, 1, 0)]);
    c.x[1] = 0;
    c.x[2] = 0x41;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xb3109ebf);
}

#[test]
fn crc32cx_zero_init() {
    // CRC32CX W0, W1, X2  — sf=1, opcode=0b010111
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b010111, 2, 1, 0)]);
    c.x[1] = 0;
    c.x[2] = 0x4142434445464748;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x219c491d);
}

#[test]
fn crc32x_result_is_32bit() {
    // CRC32X writes Wd (32-bit zero-extended), upper 32 bits of Xd must be 0
    let (mut c, mut m) = cpu_with_code(&[encode_dp2(1, 0b010011, 2, 1, 0)]);
    c.x[0] = 0xFFFF_FFFF_FFFF_FFFF; // pre-fill with 1s
    c.x[1] = 0;
    c.x[2] = 0x4142434445464748;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x97f80c95, "upper 32 bits must be cleared");
}

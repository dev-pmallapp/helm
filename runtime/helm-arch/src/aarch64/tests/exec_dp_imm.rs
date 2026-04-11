//! AArch64 Data Processing — Immediate instruction tests.
//! Ported from exec_dp_imm.rs in the old implementation.
use super::harness::*;

fn encode_add_imm(sf: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b10001 << 24) | (sh << 22) | (imm12 << 10) | (rn << 5) | rd
}
fn encode_adds_imm(sf: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (1 << 29) | (0b10001 << 24) | (sh << 22) | (imm12 << 10) | (rn << 5) | rd
}
fn encode_sub_imm(sf: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (1 << 30) | (0b10001 << 24) | (sh << 22) | (imm12 << 10) | (rn << 5) | rd
}
fn encode_subs_imm(sf: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (1 << 30)
        | (1 << 29)
        | (0b10001 << 24)
        | (sh << 22)
        | (imm12 << 10)
        | (rn << 5)
        | rd
}
fn encode_movz(sf: u32, hw: u32, imm16: u32, rd: u32) -> u32 {
    (sf << 31) | (0b10 << 29) | (0b100101 << 23) | (hw << 21) | (imm16 << 5) | rd
}
fn encode_movn(sf: u32, hw: u32, imm16: u32, rd: u32) -> u32 {
    (sf << 31) | (0b00 << 29) | (0b100101 << 23) | (hw << 21) | (imm16 << 5) | rd
}
fn encode_movk(sf: u32, hw: u32, imm16: u32, rd: u32) -> u32 {
    (sf << 31) | (0b11 << 29) | (0b100101 << 23) | (hw << 21) | (imm16 << 5) | rd
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
fn encode_eor_imm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (0b10 << 29)
        | (0b100100 << 23)
        | (n << 22)
        | (immr << 16)
        | (imms << 10)
        | (rn << 5)
        | rd
}
fn encode_ands_imm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (0b11 << 29)
        | (0b100100 << 23)
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

#[test]
fn sbfiz_64_sign_extends_from_inserted_top_bit() {
    // SBFIZ X8, X4, #16, #32 => SBFM X8, X4, #48, #31
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(1, 1, 48, 31, 4, 8)]);
    c.x[4] = 0xFFFF_FFFE;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[8], 0xFFFF_FFFF_FFFE_0000);
}

fn encode_extr(sf: u32, n: u32, rm: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (0b00 << 29)
        | (0b100111 << 23)
        | (n << 22)
        | (rm << 16)
        | (imms << 10)
        | (rn << 5)
        | rd
}
fn encode_adr(immhi: u32, immlo: u32, rd: u32) -> u32 {
    (immlo << 29) | (0b10000 << 24) | (immhi << 5) | rd
}
fn encode_adrp(immhi: u32, immlo: u32, rd: u32) -> u32 {
    (1 << 31) | (immlo << 29) | (0b10000 << 24) | (immhi << 5) | rd
}

#[test]
fn add_imm_64_basic() {
    let (mut c, mut m) = cpu_with_code(&[encode_add_imm(1, 0, 42, 1, 0)]);
    c.x[1] = 100;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 142);
}
#[test]
fn add_imm_32_basic() {
    let (mut c, mut m) = cpu_with_code(&[encode_add_imm(0, 0, 42, 1, 0)]);
    c.x[1] = 100;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 142);
}
#[test]
fn add_imm_64_shifted() {
    let (mut c, mut m) = cpu_with_code(&[encode_add_imm(1, 1, 1, 1, 0)]);
    c.x[1] = 0;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x1000);
}
#[test]
fn add_imm_32_wraps() {
    let (mut c, mut m) = cpu_with_code(&[encode_add_imm(0, 0, 1, 1, 0)]);
    c.x[1] = 0xFFFF_FFFF;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0);
}
#[test]
fn add_imm_64_max() {
    let (mut c, mut m) = cpu_with_code(&[encode_add_imm(1, 0, 0xFFF, 1, 0)]);
    c.x[1] = 0;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFF);
}
#[test]
fn sub_imm_64_basic() {
    let (mut c, mut m) = cpu_with_code(&[encode_sub_imm(1, 0, 10, 1, 0)]);
    c.x[1] = 50;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 40);
}
#[test]
fn sub_imm_32_basic() {
    let (mut c, mut m) = cpu_with_code(&[encode_sub_imm(0, 0, 10, 1, 0)]);
    c.x[1] = 50;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 40);
}
#[test]
fn sub_imm_32_wraps() {
    let (mut c, mut m) = cpu_with_code(&[encode_sub_imm(0, 0, 1, 1, 0)]);
    c.x[1] = 0;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_FFFF);
}
#[test]
fn sub_imm_64_shifted() {
    let (mut c, mut m) = cpu_with_code(&[encode_sub_imm(1, 1, 1, 1, 0)]);
    c.x[1] = 0x2000;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x1000);
}
#[test]
fn adds_imm_64_zero_flag() {
    let (mut c, mut m) = cpu_with_code(&[encode_adds_imm(1, 0, 0, 1, 31)]);
    c.x[1] = 0;
    step(&mut c, &mut m).unwrap();
    assert!(flag_z(&c));
    assert!(!flag_n(&c));
}
#[test]
fn adds_imm_64_negative_flag() {
    let (mut c, mut m) = cpu_with_code(&[encode_adds_imm(1, 0, 1, 1, 31)]);
    c.x[1] = u64::MAX;
    step(&mut c, &mut m).unwrap();
    assert!(flag_c(&c));
    assert!(flag_z(&c));
}
#[test]
fn adds_imm_32_carry() {
    let (mut c, mut m) = cpu_with_code(&[encode_adds_imm(0, 0, 1, 1, 31)]);
    c.x[1] = 0xFFFF_FFFF;
    step(&mut c, &mut m).unwrap();
    assert!(flag_c(&c));
    assert!(flag_z(&c));
}
#[test]
fn subs_imm_64_equal() {
    let (mut c, mut m) = cpu_with_code(&[encode_subs_imm(1, 0, 42, 1, 31)]);
    c.x[1] = 42;
    step(&mut c, &mut m).unwrap();
    assert!(flag_z(&c));
    assert!(flag_c(&c));
}
#[test]
fn subs_imm_64_less() {
    let (mut c, mut m) = cpu_with_code(&[encode_subs_imm(1, 0, 100, 1, 31)]);
    c.x[1] = 50;
    step(&mut c, &mut m).unwrap();
    assert!(flag_n(&c));
    assert!(!flag_c(&c));
}
#[test]
fn subs_imm_32_flags() {
    let (mut c, mut m) = cpu_with_code(&[encode_subs_imm(0, 0, 1, 1, 31)]);
    c.x[1] = 1;
    step(&mut c, &mut m).unwrap();
    assert!(flag_z(&c));
    assert!(flag_c(&c));
}
#[test]
fn subs_imm_64_overflow() {
    let (mut c, mut m) = cpu_with_code(&[encode_subs_imm(1, 0, 1, 1, 0)]);
    c.x[1] = 0x8000_0000_0000_0000;
    step(&mut c, &mut m).unwrap();
    assert!(flag_v(&c));
}
#[test]
fn adds_imm_64_signed_overflow() {
    let (mut c, mut m) = cpu_with_code(&[encode_adds_imm(1, 0, 1, 1, 0)]);
    c.x[1] = 0x7FFF_FFFF_FFFF_FFFF;
    step(&mut c, &mut m).unwrap();
    assert!(flag_v(&c));
    assert!(flag_n(&c));
}
#[test]
fn movz_64_basic() {
    let (mut c, mut m) = cpu_with_code(&[encode_movz(1, 0, 0x1234, 0)]);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x1234);
}
#[test]
fn movz_64_hw1() {
    let (mut c, mut m) = cpu_with_code(&[encode_movz(1, 1, 0xABCD, 0)]);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xABCD_0000);
}
#[test]
fn movz_64_hw2() {
    let (mut c, mut m) = cpu_with_code(&[encode_movz(1, 2, 0xFFFF, 0)]);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_0000_0000);
}
#[test]
fn movz_64_hw3() {
    let (mut c, mut m) = cpu_with_code(&[encode_movz(1, 3, 1, 0)]);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 1u64 << 48);
}
#[test]
fn movz_32_clears_upper() {
    let (mut c, mut m) = cpu_with_code(&[encode_movz(0, 0, 0xFFFF, 0)]);
    c.x[0] = u64::MAX;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF);
}
#[test]
fn movn_64_basic() {
    let (mut c, mut m) = cpu_with_code(&[encode_movn(1, 0, 0, 0)]);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], u64::MAX);
}
#[test]
fn movn_32_basic() {
    let (mut c, mut m) = cpu_with_code(&[encode_movn(0, 0, 0, 0)]);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0] & 0xFFFF_FFFF, 0xFFFF_FFFF);
}
#[test]
fn movk_preserves_other_hw() {
    let (mut c, mut m) = cpu_with_code(&[encode_movk(1, 0, 0x5678, 0)]);
    c.x[0] = 0xAAAA_0000_0000_0000;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xAAAA_0000_0000_5678);
}
#[test]
fn movk_hw2() {
    let (mut c, mut m) = cpu_with_code(&[encode_movk(1, 2, 0x1111, 0)]);
    c.x[0] = 0xFFFF_FFFF_FFFF_FFFF;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_1111_FFFF_FFFF);
}
#[test]
fn movz_movk_chain() {
    let (mut c, mut m) = cpu_with_code(&[
        encode_movz(1, 3, 0x0001, 0),
        encode_movk(1, 2, 0x0002, 0),
        encode_movk(1, 1, 0x0003, 0),
        encode_movk(1, 0, 0x0004, 0),
    ]);
    for _ in 0..4 {
        step(&mut c, &mut m).unwrap();
    }
    assert_eq!(c.x[0], 0x0001_0002_0003_0004);
}
#[test]
fn and_imm_64_all_ones() {
    let (mut c, mut m) = cpu_with_code(&[encode_and_imm(1, 1, 0, 63, 1, 0)]);
    c.x[1] = 0xDEAD_BEEF;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xDEAD_BEEF);
}
#[test]
fn and_imm_32_low_byte() {
    let (mut c, mut m) = cpu_with_code(&[encode_and_imm(0, 0, 0, 7, 1, 0)]);
    c.x[1] = 0x1234_5678;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x78);
}
#[test]
fn orr_imm_64_set_bits() {
    let (mut c, mut m) = cpu_with_code(&[encode_orr_imm(1, 1, 0, 7, 31, 0)]);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFF);
}
#[test]
fn eor_imm_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_eor_imm(1, 1, 0, 7, 1, 0)]);
    c.x[1] = 0xAA;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x55);
}
#[test]
fn ands_imm_64_sets_zero() {
    let (mut c, mut m) = cpu_with_code(&[encode_ands_imm(1, 1, 0, 7, 1, 31)]);
    c.x[1] = 0x100;
    step(&mut c, &mut m).unwrap();
    assert!(flag_z(&c));
}
#[test]
fn ands_imm_64_sets_negative() {
    let (mut c, mut m) = cpu_with_code(&[encode_ands_imm(1, 1, 0, 63, 1, 0)]);
    c.x[1] = 0x8000_0000_0000_0000;
    step(&mut c, &mut m).unwrap();
    assert!(flag_n(&c));
}
#[test]
fn sbfm_sxtb() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(1, 1, 0, 7, 1, 0)]);
    c.x[1] = 0x80;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_FFFF_FFFF_FF80);
}
#[test]
fn sbfm_sxtb_positive() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(1, 1, 0, 7, 1, 0)]);
    c.x[1] = 0x7F;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x7F);
}
#[test]
fn sbfm_sxth() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(1, 1, 0, 15, 1, 0)]);
    c.x[1] = 0xFFFF;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_FFFF_FFFF_FFFF);
}
#[test]
fn sbfm_sxtw() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(1, 1, 0, 31, 1, 0)]);
    c.x[1] = 0x8000_0000;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_FFFF_8000_0000);
}
#[test]
fn sbfm_asr_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(1, 1, 4, 63, 1, 0)]);
    c.x[1] = 0x8000_0000_0000_0000;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xF800_0000_0000_0000);
}
#[test]
fn sbfm_asr_32() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(0, 0, 4, 31, 1, 0)]);
    c.x[1] = 0x8000_0000;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xF800_0000);
}
#[test]
fn sbfm_sbfx() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(1, 1, 8, 15, 1, 0)]);
    c.x[1] = 0xFF00;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_FFFF_FFFF_FFFF);
}
#[test]
fn ubfm_lsl_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(1, 1, 60, 59, 1, 0)]);
    c.x[1] = 0xF;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xF0);
}
#[test]
fn ubfm_lsr_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(1, 1, 4, 63, 1, 0)]);
    c.x[1] = 0xF0;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xF);
}
#[test]
fn ubfm_lsl_32() {
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(0, 0, 28, 27, 1, 0)]);
    c.x[1] = 0xF;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xF0);
}
#[test]
fn ubfm_lsr_32() {
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(0, 0, 4, 31, 1, 0)]);
    c.x[1] = 0xF0;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xF);
}
#[test]
fn ubfm_uxtb() {
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(0, 0, 0, 7, 1, 0)]);
    c.x[1] = 0x1234_5680;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x80);
}
#[test]
fn ubfm_uxth() {
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(0, 0, 0, 15, 1, 0)]);
    c.x[1] = 0xFFFF_ABCD;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xABCD);
}
#[test]
fn ubfm_ubfx() {
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(0, 0, 4, 11, 1, 0)]);
    c.x[1] = 0xABCD;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xBC);
}
#[test]
fn bfm_bfi_64() {
    let (mut c, mut m) = cpu_with_code(&[encode_bfm(1, 1, 52, 51, 1, 0)]);
    c.x[0] = 0;
    c.x[1] = 2;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x2000);
}
#[test]
fn bfm_bfi_32() {
    let (mut c, mut m) = cpu_with_code(&[encode_bfm(0, 0, 24, 7, 1, 0)]);
    c.x[0] = 0xFF;
    c.x[1] = 0xAB;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xABFF);
}
#[test]
fn extr_64_ror() {
    let (mut c, mut m) = cpu_with_code(&[encode_extr(1, 1, 1, 4, 1, 0)]);
    c.x[1] = 0xF;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xF000_0000_0000_0000);
}
#[test]
fn extr_32_ror() {
    let (mut c, mut m) = cpu_with_code(&[encode_extr(0, 0, 1, 4, 1, 0)]);
    c.x[1] = 0xF;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xF000_0000);
}
#[test]
fn extr_64_concat() {
    let (mut c, mut m) = cpu_with_code(&[encode_extr(1, 1, 2, 8, 1, 0)]);
    c.x[1] = 0xFF;
    c.x[2] = 0xAB00_0000_0000_0000;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFAB_0000_0000_0000);
}
#[test]
fn adr_forward() {
    let (mut c, mut m) = cpu_with_code(&[encode_adr(0x20, 0, 0)]);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], CODE_BASE + 0x80);
}
#[test]
fn adrp_page_aligned() {
    let (mut c, mut m) = cpu_with_code(&[encode_adrp(1, 0, 0)]);
    step(&mut c, &mut m).unwrap();
    let expected = (CODE_BASE & !0xFFF) + 0x4000;
    assert_eq!(c.x[0], expected);
}

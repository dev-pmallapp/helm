//! AArch64 Load/Store tests. Ported from exec_ldst.rs.
use super::harness::*;

const D: u64 = DATA_BASE;

fn str_x_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b11111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn ldr_x_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b11111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn str_w_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b10111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn ldr_w_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b10111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn strb_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b00111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn ldrb_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b00111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn strh_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b01111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn ldrh_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b01111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn ldrsw_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b10111001_10 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn ldrsb_w_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b00111001_11 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn ldrsh_w_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b01111001_11 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn stp_x(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b10_101_0_0_10_0u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn ldp_x(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b10_101_0_0_10_1u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn stp_x_pre(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b10_101_0_0_11_0u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn ldp_x_post(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b10_101_0_0_01_1u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn ldxp_x(rt2: u32, rn: u32, rt: u32) -> u32 {
    0xC87F_0000 | (rt2 << 10) | (rn << 5) | rt
}
fn stxp_x(rs: u32, rt2: u32, rn: u32, rt: u32) -> u32 {
    0xC820_0000 | (rs << 16) | (rt2 << 10) | (rn << 5) | rt
}

#[test]
fn str_ldr_x64_roundtrip() {
    let (mut c, mut m) = cpu_with_code(&[str_x_uimm(0, 2, 0), ldr_x_uimm(0, 2, 1)]);
    c.x[0] = 0xDEAD_BEEF_CAFE_1234; c.x[2] = D;
    step(&mut c, &mut m).unwrap(); step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[1], 0xDEAD_BEEF_CAFE_1234);
}
#[test]
fn str_ldr_w32_roundtrip() {
    let (mut c, mut m) = cpu_with_code(&[str_w_uimm(0, 2, 0), ldr_w_uimm(0, 2, 1)]);
    c.x[0] = 0x1_FFFF_FFFF; c.x[2] = D;
    step(&mut c, &mut m).unwrap(); step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[1], 0xFFFF_FFFF);
}
#[test]
fn strb_ldrb_roundtrip() {
    let (mut c, mut m) = cpu_with_code(&[strb_uimm(0, 2, 0), ldrb_uimm(0, 2, 1)]);
    c.x[0] = 0xFF; c.x[2] = D;
    step(&mut c, &mut m).unwrap(); step(&mut c, &mut m).unwrap(); assert_eq!(c.x[1], 0xFF);
}
#[test]
fn strb_ldrb_zero_extends() {
    let (mut c, mut m) = cpu_with_code(&[strb_uimm(0, 2, 0), ldrb_uimm(0, 2, 1)]);
    c.x[0] = 0x1FF; c.x[2] = D;
    step(&mut c, &mut m).unwrap(); step(&mut c, &mut m).unwrap(); assert_eq!(c.x[1], 0xFF);
}
#[test]
fn strh_ldrh_roundtrip() {
    let (mut c, mut m) = cpu_with_code(&[strh_uimm(0, 2, 0), ldrh_uimm(0, 2, 1)]);
    c.x[0] = 0xABCD; c.x[2] = D;
    step(&mut c, &mut m).unwrap(); step(&mut c, &mut m).unwrap(); assert_eq!(c.x[1], 0xABCD);
}
#[test]
fn ldrsw_negative() {
    let (mut c, mut m) = cpu_with_code(&[ldrsw_uimm(0, 2, 0)]);
    m.load_u32(D, 0x8000_0000); c.x[2] = D;
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0xFFFF_FFFF_8000_0000);
}
#[test]
fn ldrsw_positive() {
    let (mut c, mut m) = cpu_with_code(&[ldrsw_uimm(0, 2, 0)]);
    m.load_u32(D, 0x7FFF_FFFF); c.x[2] = D;
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0x7FFF_FFFF);
}
#[test]
fn ldrsb_negative() {
    let (mut c, mut m) = cpu_with_code(&[ldrsb_w_uimm(0, 2, 0)]);
    m.load_u8(D, 0x80); c.x[2] = D;
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0xFFFF_FF80);
}
#[test]
fn ldrsh_negative() {
    let (mut c, mut m) = cpu_with_code(&[ldrsh_w_uimm(0, 2, 0)]);
    m.load_u16(D, 0x8000); c.x[2] = D;
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 0xFFFF_8000);
}
#[test]
fn stp_ldp_x_roundtrip() {
    let (mut c, mut m) = cpu_with_code(&[stp_x(0, 1, 2, 0), ldp_x(0, 3, 2, 4)]);
    c.x[0] = 0xAAAA; c.x[1] = 0xBBBB; c.x[2] = D;
    step(&mut c, &mut m).unwrap(); step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[4], 0xAAAA); assert_eq!(c.x[3], 0xBBBB);
}
#[test]
fn stp_x_pre_index() {
    let (mut c, mut m) = cpu_with_code(&[stp_x_pre(-2, 1, 2, 0)]);
    c.x[0] = 0xAA; c.x[1] = 0xBB; c.x[2] = D + 32;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[2], D + 16);
    assert_eq!(m.read_u64(D + 16), 0xAA); assert_eq!(m.read_u64(D + 24), 0xBB);
}
#[test]
fn ldp_x_post_index() {
    let (mut c, mut m) = cpu_with_code(&[ldp_x_post(2, 3, 2, 4)]);
    m.load_u64(D, 0x1111); m.load_u64(D + 8, 0x2222); c.x[2] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[4], 0x1111); assert_eq!(c.x[3], 0x2222); assert_eq!(c.x[2], D + 16);
}
#[test]
fn ldxr_stxr_success() {
    let (mut c, mut m) = cpu_with_code(&[0xC85F_7C20, 0xC803_7C23]);
    m.load_u64(D, 42); c.x[1] = D; c.x[3] = 99;
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[0], 42);
    step(&mut c, &mut m).unwrap(); assert_eq!(c.x[2], 0);
    assert_eq!(m.read_u64(D), 99);
}
#[test]
fn ldxp_stxp_success() {
    let (mut c, mut m) = cpu_with_code(&[ldxp_x(1, 2, 0), stxp_x(3, 1, 2, 0)]);
    m.load_u64(D, 0x1111);
    m.load_u64(D + 8, 0x2222);
    c.x[2] = D;

    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x1111);
    assert_eq!(c.x[1], 0x2222);

    c.x[0] = 0xAAAA;
    c.x[1] = 0xBBBB;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[3], 0);
    assert_eq!(m.read_u64(D), 0xAAAA);
    assert_eq!(m.read_u64(D + 8), 0xBBBB);
}
#[ignore] // SWP atomic: not yet implemented in execute.rs
#[test]
fn swp_x() {
    // SWP X0, X1, [X2]: correct encoding with opc=100
    let (mut c, mut m) = cpu_with_code(&[0xF820_4041]);
    m.load_u64(D, 100); c.x[0] = 200; c.x[2] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[1], 100); assert_eq!(m.read_u64(D), 200);
}
#[test]
fn str_ldr_via_sp() {
    let (mut c, mut m) = cpu_with_code(&[str_x_uimm(0, 31, 0), ldr_x_uimm(0, 31, 1)]);
    c.x[0] = 0xCAFE_BABE;
    step(&mut c, &mut m).unwrap(); step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[1], 0xCAFE_BABE);
}

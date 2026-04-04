//! Bulk load/store tests. Ported from exec_ldst_bulk.rs.
#![allow(dead_code)]
use super::harness::*;

const D: u64 = DATA_BASE;

fn str_uoff(sz: u32, opc: u32, imm12: u32, rn: u32, rt: u32) -> u32 {
    (sz << 30) | (0b111001 << 24) | (opc << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn ldr_x(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b11, 0b01, i, rn, rt)
}
fn str_x(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b11, 0b00, i, rn, rt)
}
fn ldr_w(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b10, 0b01, i, rn, rt)
}
fn str_w(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b10, 0b00, i, rn, rt)
}
fn ldrh(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b01, 0b01, i, rn, rt)
}
fn strh(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b01, 0b00, i, rn, rt)
}
fn ldrb(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b00, 0b01, i, rn, rt)
}
fn strb(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b00, 0b00, i, rn, rt)
}
fn ldrsw(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b10, 0b10, i, rn, rt)
}
fn ldrsb_w(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b00, 0b11, i, rn, rt)
}
fn ldrsb_x(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b00, 0b10, i, rn, rt)
}
fn ldrsh_w(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b01, 0b11, i, rn, rt)
}
fn ldrsh_x(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b01, 0b10, i, rn, rt)
}
fn stp_x(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b10_101_0_0_10_0u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn ldp_x(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b10_101_0_0_10_1u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn stp_x_pre(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b10_101_0_0_11_0u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn ldp_x_pre(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b10_101_0_0_11_1u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn stp_x_post(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b10_101_0_0_01_0u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn ldp_x_post(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b10_101_0_0_01_1u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn stp_w(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b00_101_0_0_10_0u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn ldp_w(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b00_101_0_0_10_1u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}

macro_rules! test_str_ldr {
    ($name:ident, $str_fn:ident, $ldr_fn:ident, $write_val:expr, $expected:expr, $offset:expr) => {
        #[test]
        fn $name() {
            let (mut c, mut m) = cpu_with_code(&[$str_fn($offset, 3, 0), $ldr_fn($offset, 3, 1)]);
            c.x[0] = $write_val;
            c.x[3] = D;
            step(&mut c, &mut m).unwrap();
            step(&mut c, &mut m).unwrap();
            assert_eq!(c.x[1], $expected);
        }
    };
}
test_str_ldr!(
    ldr_x_off0,
    str_x,
    ldr_x,
    0xDEAD_BEEF_CAFE_1234u64,
    0xDEAD_BEEF_CAFE_1234u64,
    0
);
test_str_ldr!(ldr_x_off1, str_x, ldr_x, 0x42u64, 0x42u64, 1);
test_str_ldr!(
    ldr_w_off0,
    str_w,
    ldr_w,
    0x1_FFFF_FFFFu64,
    0xFFFF_FFFFu64,
    0
);
test_str_ldr!(ldrh_off0, strh, ldrh, 0xABCDu64, 0xABCDu64, 0);
test_str_ldr!(ldrb_off0, strb, ldrb, 0xFFu64, 0xFFu64, 0);
test_str_ldr!(ldrb_off1, strb, ldrb, 0xAAu64, 0xAAu64, 1);

#[test]
fn ldrsw_neg() {
    let (mut c, mut m) = cpu_with_code(&[ldrsw(0, 3, 0)]);
    m.load_u32(D, 0x8000_0000);
    c.x[3] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_FFFF_8000_0000u64);
}
#[test]
fn ldrsw_pos() {
    let (mut c, mut m) = cpu_with_code(&[ldrsw(0, 3, 0)]);
    m.load_u32(D, 0x7FFF_FFFF);
    c.x[3] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x7FFF_FFFFu64);
}
#[test]
fn ldrsb_neg() {
    let (mut c, mut m) = cpu_with_code(&[ldrsb_w(0, 3, 0)]);
    m.load_u8(D, 0x80);
    c.x[3] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_FF80u64);
}
#[test]
fn ldrsb_pos() {
    let (mut c, mut m) = cpu_with_code(&[ldrsb_w(0, 3, 0)]);
    m.load_u8(D, 0x7F);
    c.x[3] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x7Fu64);
}
#[test]
fn ldrsb_x_neg() {
    let (mut c, mut m) = cpu_with_code(&[ldrsb_x(0, 3, 0)]);
    m.load_u8(D, 0x80);
    c.x[3] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_FFFF_FFFF_FF80u64);
}
#[test]
fn ldrsh_neg() {
    let (mut c, mut m) = cpu_with_code(&[ldrsh_w(0, 3, 0)]);
    m.load_u16(D, 0x8000);
    c.x[3] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_8000u64);
}
#[test]
fn ldrsh_x_neg() {
    let (mut c, mut m) = cpu_with_code(&[ldrsh_x(0, 3, 0)]);
    m.load_u16(D, 0x8000);
    c.x[3] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_FFFF_FFFF_8000u64);
}

#[test]
fn stp_ldp_x_off() {
    let (mut c, mut m) = cpu_with_code(&[stp_x(2, 1, 3, 0), ldp_x(2, 4, 3, 5)]);
    c.x[0] = 0x1111;
    c.x[1] = 0x2222;
    c.x[3] = D;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[5], 0x1111);
    assert_eq!(c.x[4], 0x2222);
}
#[test]
fn stp_x_pre_index_bulk() {
    let (mut c, mut m) = cpu_with_code(&[stp_x_pre(-2, 1, 3, 0)]);
    c.x[0] = 0xAA;
    c.x[1] = 0xBB;
    c.x[3] = D + 32;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[3], D + 16);
    assert_eq!(m.read_u64(D + 16), 0xAA);
    assert_eq!(m.read_u64(D + 24), 0xBB);
}
#[test]
fn ldp_x_post_index_bulk() {
    let (mut c, mut m) = cpu_with_code(&[ldp_x_post(2, 4, 3, 5)]);
    m.load_u64(D, 0x1111);
    m.load_u64(D + 8, 0x2222);
    c.x[3] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[5], 0x1111);
    assert_eq!(c.x[4], 0x2222);
    assert_eq!(c.x[3], D + 16);
}
#[test]
fn ldp_x_pre_index_bulk() {
    let (mut c, mut m) = cpu_with_code(&[ldp_x_pre(2, 4, 3, 5)]);
    m.load_u64(D + 16, 0xAAAA);
    m.load_u64(D + 24, 0xBBBB);
    c.x[3] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[5], 0xAAAA);
    assert_eq!(c.x[4], 0xBBBB);
    assert_eq!(c.x[3], D + 16);
}
#[test]
fn stp_ldp_w() {
    let (mut c, mut m) = cpu_with_code(&[stp_w(0, 1, 3, 0), ldp_w(0, 4, 3, 5)]);
    c.x[0] = 0x1111;
    c.x[1] = 0x2222;
    c.x[3] = D;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[5], 0x1111);
    assert_eq!(c.x[4], 0x2222);
}

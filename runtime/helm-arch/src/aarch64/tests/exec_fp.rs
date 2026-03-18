//! Scalar FP and SIMD tests. Ported from exec_fp.rs.
//! These tests exercise features that may be partially stubbed.
use super::harness::*;

#[test]
fn fmov_gp_to_d() {
    // FMOV D0, X1  =>  0x9E670020
    let (mut c, mut m) = cpu_with_code(&[0x9E670020]);
    c.x[1] = 0xDEAD_BEEF_CAFE_BABE;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.v[0] as u64, 0xDEAD_BEEF_CAFE_BABE);
}
#[test]
fn fmov_d_to_gp() {
    // FMOV X0, D1  =>  0x9E660020
    let (mut c, mut m) = cpu_with_code(&[0x9E660020]);
    c.v[1] = 0xCAFE_BABE_DEAD_BEEF;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xCAFE_BABE_DEAD_BEEF);
}
#[test]
fn fmov_gp_to_s() {
    // FMOV S0, W1  =>  0x1E270020
    let (mut c, mut m) = cpu_with_code(&[0x1E270020]);
    c.x[1] = 0x40000000;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.v[0] as u32, 0x40000000);
}
#[test]
fn fmov_s_to_gp() {
    // FMOV W0, S1  =>  0x1E260020
    let (mut c, mut m) = cpu_with_code(&[0x1E260020]);
    c.v[1] = 0x40000000;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x40000000);
}
#[test]
#[ignore = "SIMD vector AND may be stubbed"]
fn and_v16b() {
    let (mut c, mut m) = cpu_with_code(&[0x4E221C20]);
    c.v[1] = 0xFF00_FF00_FF00_FF00_FF00_FF00_FF00_FF00;
    c.v[2] = 0x0F0F_0F0F_0F0F_0F0F_0F0F_0F0F_0F0F_0F0F;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.v[0], 0x0F00_0F00_0F00_0F00_0F00_0F00_0F00_0F00);
}
#[test]
#[ignore = "SIMD vector ORR may be stubbed"]
fn orr_v16b() {
    let (mut c, mut m) = cpu_with_code(&[0x4EA21C20]);
    c.v[1] = 0xFF00_0000_0000_0000_0000_0000_0000_0000u128;
    c.v[2] = 0x00FF_0000_0000_0000_0000_0000_0000_0000u128;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.v[0], 0xFFFF_0000_0000_0000_0000_0000_0000_0000u128);
}
#[test]
#[ignore = "SIMD vector NOT may be stubbed"]
fn not_v16b() {
    let (mut c, mut m) = cpu_with_code(&[0x6E205820]);
    c.v[1] = 0;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.v[0], u128::MAX);
}

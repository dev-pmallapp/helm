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

// ── FP<->integer conversion decode + execute tests ──────────────────────────

#[test]
fn scvtf_d0_w1() {
    // SCVTF D0, W1 => 0x1E620020
    let (mut c, mut m) = cpu_with_code(&[0x1E620020]);
    c.x[1] = 505; // W1 = 505
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.v[0] as u64, 505.0f64.to_bits());
}

#[test]
fn scvtf_d0_x1() {
    // SCVTF D0, X1 => 0x9E620020
    let (mut c, mut m) = cpu_with_code(&[0x9E620020]);
    c.x[1] = 505;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.v[0] as u64, 505.0f64.to_bits());
}

#[test]
fn scvtf_d0_w1_negative() {
    // SCVTF D0, W1 => 0x1E620020, W1 = -1 (0xFFFFFFFF)
    let (mut c, mut m) = cpu_with_code(&[0x1E620020]);
    c.x[1] = 0x00000000_FFFFFFFF; // W1 = -1 as u32
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.v[0] as u64, (-1.0f64).to_bits());
}

#[test]
fn scvtf_s0_w1() {
    // SCVTF S0, W1 => 0x1E220020
    let (mut c, mut m) = cpu_with_code(&[0x1E220020]);
    c.x[1] = 42;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.v[0] as u32, 42.0f32.to_bits());
}

#[test]
fn fcvtzs_w0_d1() {
    // FCVTZS W0, D1 => 0x1E780020
    let (mut c, mut m) = cpu_with_code(&[0x1E780020]);
    c.v[1] = 42.7f64.to_bits() as u128;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 42); // truncated toward zero
}

#[test]
fn fcvtzs_x0_d1() {
    // FCVTZS X0, D1 => 0x9E780020
    let (mut c, mut m) = cpu_with_code(&[0x9E780020]);
    c.v[1] = (-99.9f64).to_bits() as u128;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0] as i64, -99); // truncated toward zero
}

#[test]
fn ucvtf_d0_w1() {
    // UCVTF D0, W1 => 0x1E630020
    let (mut c, mut m) = cpu_with_code(&[0x1E630020]);
    c.x[1] = 1000;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.v[0] as u64, 1000.0f64.to_bits());
}

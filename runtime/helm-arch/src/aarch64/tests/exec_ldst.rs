//! AArch64 Load/Store tests. Ported from exec_ldst.rs.
use super::harness::*;

const D: u64 = DATA_BASE;

fn str_x_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b11111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn ldr_x_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b11111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn str_w_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b10111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn ldr_w_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b10111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn strb_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b00111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn ldrb_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b00111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn strh_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b01111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn ldrh_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b01111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn ldrsw_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b10111001_10 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn ldrsb_w_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b00111001_11 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn ldrsh_w_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b01111001_11 << 22) | (imm12 << 10) | (rn << 5) | rt
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
    c.x[0] = 0xDEAD_BEEF_CAFE_1234;
    c.x[2] = D;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[1], 0xDEAD_BEEF_CAFE_1234);
}
#[test]
fn str_ldr_w32_roundtrip() {
    let (mut c, mut m) = cpu_with_code(&[str_w_uimm(0, 2, 0), ldr_w_uimm(0, 2, 1)]);
    c.x[0] = 0x1_FFFF_FFFF;
    c.x[2] = D;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[1], 0xFFFF_FFFF);
}
#[test]
fn strb_ldrb_roundtrip() {
    let (mut c, mut m) = cpu_with_code(&[strb_uimm(0, 2, 0), ldrb_uimm(0, 2, 1)]);
    c.x[0] = 0xFF;
    c.x[2] = D;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[1], 0xFF);
}
#[test]
fn strb_ldrb_zero_extends() {
    let (mut c, mut m) = cpu_with_code(&[strb_uimm(0, 2, 0), ldrb_uimm(0, 2, 1)]);
    c.x[0] = 0x1FF;
    c.x[2] = D;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[1], 0xFF);
}
#[test]
fn strh_ldrh_roundtrip() {
    let (mut c, mut m) = cpu_with_code(&[strh_uimm(0, 2, 0), ldrh_uimm(0, 2, 1)]);
    c.x[0] = 0xABCD;
    c.x[2] = D;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[1], 0xABCD);
}
#[test]
fn ldrsw_negative() {
    let (mut c, mut m) = cpu_with_code(&[ldrsw_uimm(0, 2, 0)]);
    m.load_u32(D, 0x8000_0000);
    c.x[2] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_FFFF_8000_0000);
}
#[test]
fn ldrsw_positive() {
    let (mut c, mut m) = cpu_with_code(&[ldrsw_uimm(0, 2, 0)]);
    m.load_u32(D, 0x7FFF_FFFF);
    c.x[2] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x7FFF_FFFF);
}
#[test]
fn ldrsb_negative() {
    let (mut c, mut m) = cpu_with_code(&[ldrsb_w_uimm(0, 2, 0)]);
    m.load_u8(D, 0x80);
    c.x[2] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_FF80);
}
#[test]
fn ldrsh_negative() {
    let (mut c, mut m) = cpu_with_code(&[ldrsh_w_uimm(0, 2, 0)]);
    m.load_u16(D, 0x8000);
    c.x[2] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_8000);
}
#[test]
fn stp_ldp_x_roundtrip() {
    let (mut c, mut m) = cpu_with_code(&[stp_x(0, 1, 2, 0), ldp_x(0, 3, 2, 4)]);
    c.x[0] = 0xAAAA;
    c.x[1] = 0xBBBB;
    c.x[2] = D;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[4], 0xAAAA);
    assert_eq!(c.x[3], 0xBBBB);
}
#[test]
fn stp_x_pre_index() {
    let (mut c, mut m) = cpu_with_code(&[stp_x_pre(-2, 1, 2, 0)]);
    c.x[0] = 0xAA;
    c.x[1] = 0xBB;
    c.x[2] = D + 32;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[2], D + 16);
    assert_eq!(m.read_u64(D + 16), 0xAA);
    assert_eq!(m.read_u64(D + 24), 0xBB);
}
#[test]
fn ldp_x_post_index() {
    let (mut c, mut m) = cpu_with_code(&[ldp_x_post(2, 3, 2, 4)]);
    m.load_u64(D, 0x1111);
    m.load_u64(D + 8, 0x2222);
    c.x[2] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[4], 0x1111);
    assert_eq!(c.x[3], 0x2222);
    assert_eq!(c.x[2], D + 16);
}
#[test]
fn ldxr_stxr_success() {
    let (mut c, mut m) = cpu_with_code(&[0xC85F_7C20, 0xC803_7C23]);
    m.load_u64(D, 42);
    c.x[1] = D;
    c.x[3] = 99;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 42);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[2], 0);
    assert_eq!(m.read_u64(D), 99);
}

#[test]
fn stxr_fails_after_plain_store_to_reserved_word() {
    let (mut c, mut m) = cpu_with_code(&[
        0xC85F_7C20,       // LDXR X0, [X1]
        str_x_uimm(0, 1, 2), // STR X2, [X1]
        0xC803_7C24,       // STXR W3, X4, [X1]
    ]);
    m.load_u64(D, 42);
    c.x[1] = D;
    c.x[2] = 42; // Plain store writes the same value back.
    c.x[4] = 99;

    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 42);

    step(&mut c, &mut m).unwrap();
    assert_eq!(m.read_u64(D), 42);

    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[3], 1, "STXR must fail after an overlapping plain store");
    assert_eq!(m.read_u64(D), 42, "failed STXR must not update memory");
}

#[test]
fn stxr_fails_after_plain_store_to_adjacent_word_in_same_granule() {
    let (mut c, mut m) = cpu_with_code(&[
        0xC85F_7C20,        // LDXR X0, [X1]
        str_x_uimm(1, 1, 2), // STR X2, [X1, #8]
        0xC803_7C24,        // STXR W3, X4, [X1]
    ]);
    m.load_u64(D, 42);
    m.load_u64(D + 8, 7);
    c.x[1] = D;
    c.x[2] = 77;
    c.x[4] = 99;

    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 42);

    step(&mut c, &mut m).unwrap();
    assert_eq!(m.read_u64(D), 42);
    assert_eq!(m.read_u64(D + 8), 77);

    step(&mut c, &mut m).unwrap();
    assert_eq!(
        c.x[3], 1,
        "STXR must fail after a store to the same reservation granule"
    );
    assert_eq!(m.read_u64(D), 42, "failed STXR must not update memory");
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
    m.load_u64(D, 100);
    c.x[0] = 200;
    c.x[2] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[1], 100);
    assert_eq!(m.read_u64(D), 200);
}
#[test]
fn str_ldr_via_sp() {
    let (mut c, mut m) = cpu_with_code(&[str_x_uimm(0, 31, 0), ldr_x_uimm(0, 31, 1)]);
    c.x[0] = 0xCAFE_BABE;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[1], 0xCAFE_BABE);
}

// ── LDXRB / STXRB (byte exclusive) ──────────────────────────────────────────

// LDXRB Wt, [Xn]: size=00, o2=0, L=1, o1=0, Rs=11111, o0=0, Rt2=11111
// Encoding: 00 001000 0 1 0 11111 0 11111 Rn Rt
fn ldxrb(rn: u32, rt: u32) -> u32 {
    0x085F_7C00 | (rn << 5) | rt
}
// STXRB Ws, Wt, [Xn]: size=00, o2=0, L=0, o1=0, Rs, o0=0, Rt2=11111
// Encoding: 00 001000 0 0 0 Rs 0 11111 Rn Rt
fn stxrb(rs: u32, rn: u32, rt: u32) -> u32 {
    0x0800_7C00 | (rs << 16) | (rn << 5) | rt
}

#[test]
fn ldxrb_stxrb_byte_only() {
    // Place 0xDEADBEEF_CAFEBABE at D so we can verify only 1 byte is touched
    let (mut c, mut m) = cpu_with_code(&[ldxrb(1, 0), stxrb(3, 1, 2)]);
    m.load_u64(D, 0xDEAD_BEEF_CAFE_BABE);
    c.x[1] = D;
    c.x[2] = 0xFF;

    // LDXRB: must load only 1 byte (0xBE), not 4 bytes
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xBE, "LDXRB must load only 1 byte");

    // STXRB: must store only 1 byte (0xFF), not clobber adjacent bytes
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[3], 0, "STXRB status must be 0 (success)");
    let after = m.read_u64(D);
    assert_eq!(
        after, 0xDEAD_BEEF_CAFE_BAFF,
        "STXRB must write only 1 byte; got {after:#018x}"
    );
}

// ── LDXRH / STXRH (halfword exclusive) ──────────────────────────────────────

// LDXRH Wt, [Xn]: size=01
fn ldxrh(rn: u32, rt: u32) -> u32 {
    0x485F_7C00 | (rn << 5) | rt
}
// STXRH Ws, Wt, [Xn]: size=01
fn stxrh(rs: u32, rn: u32, rt: u32) -> u32 {
    0x4800_7C00 | (rs << 16) | (rn << 5) | rt
}

#[test]
fn ldxrh_stxrh_halfword_only() {
    let (mut c, mut m) = cpu_with_code(&[ldxrh(1, 0), stxrh(3, 1, 2)]);
    m.load_u64(D, 0xDEAD_BEEF_CAFE_BABE);
    c.x[1] = D;
    c.x[2] = 0x1234;

    // LDXRH: must load only 2 bytes (0xBABE), not 8
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xBABE, "LDXRH must load only 2 bytes");

    // STXRH: must store only 2 bytes
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[3], 0, "STXRH status must be 0 (success)");
    let after = m.read_u64(D);
    assert_eq!(
        after, 0xDEAD_BEEF_CAFE_1234,
        "STXRH must write only 2 bytes; got {after:#018x}"
    );
}

#[test]
fn ldp_q_pair_writes_both_registers() {
    // LDP Q27, Q30, [X1, #0x20]  =>  ad41783b
    // Regression: pair_second (q30) was not being written.
    let (mut a, mut m) = cpu_with_code(&[0xad41783b]);

    let src = DATA_BASE + 0x20;
    m.load_u64(src, 0x3000);
    m.load_u64(src + 8, 0x2000);
    m.load_u64(src + 16, 0x7000);
    m.load_u64(src + 24, 0xFFFFFFFFFFFFF800);

    a.x[1] = DATA_BASE;
    a.v[30] = 0xDEAD_BEEF_u128;

    step(&mut a, &mut m).unwrap();

    let q27_lo = a.v[27] as u64;
    let q27_hi = (a.v[27] >> 64) as u64;
    assert_eq!(q27_lo, 0x3000, "q27 lo (first reg lo)");
    assert_eq!(q27_hi, 0x2000, "q27 hi (first reg hi)");

    let q30_lo = a.v[30] as u64;
    let q30_hi = (a.v[30] >> 64) as u64;
    assert_eq!(q30_lo, 0x7000, "q30 lo (pair_second lo)");
    assert_eq!(q30_hi, 0xFFFFFFFFFFFFF800, "q30 hi (pair_second hi)");
}

#[test]
fn stp_q_pair_stores_both_registers() {
    // STP Q30, Q29, [X1, #0x30]  =>  ad01f43e
    let (mut a, mut m) = cpu_with_code(&[0xad01f43e]);
    m.map_zeroed(DATA_BASE, 0x100);
    a.x[1] = DATA_BASE;
    a.v[30] = (0xFFFFFFFFFFFFF800_u128 << 64) | 0x7000_u128;
    a.v[29] = (0x41D_u128 << 64) | 0xFFFFFFFFFFFFF800_u128;
    step(&mut a, &mut m).unwrap();
    assert_eq!(m.read_u64(DATA_BASE + 0x30), 0x7000, "q30 lo stored");
    assert_eq!(m.read_u64(DATA_BASE + 0x38), 0xFFFFFFFFFFFFF800, "q30 hi stored");
    assert_eq!(m.read_u64(DATA_BASE + 0x40), 0xFFFFFFFFFFFFF800, "q29 lo stored");
    assert_eq!(m.read_u64(DATA_BASE + 0x48), 0x41D, "q29 hi stored");
}

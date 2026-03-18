//! AArch64 Load/Store instruction tests.
//!
//! Ported from the reference helm.git `exec_ldst.rs` test suite.
//! Covers: LDR/STR (B/H/W/X, unsigned offset, pre/post, register),
//! LDRS* sign-extending loads, LDP/STP (pre/post/offset),
//! LDXR/STXR exclusive, SWP atomics, load/store via SP.

use helm_arch::aarch64::arch_state::Aarch64ArchState;
use helm_core::{AccessType, MemFault, MemInterface};

// ── Test memory ────────────────────────────────────────────────────────────────

struct TestMem {
    data: Vec<u8>,
}

impl TestMem {
    fn new() -> Self {
        Self { data: vec![0u8; 16 * 1024 * 1024] }
    }
}

impl MemInterface for TestMem {
    fn read(&mut self, addr: u64, size: usize, _ty: AccessType) -> Result<u64, MemFault> {
        let off = (addr & 0xFF_FFFF) as usize;
        if off + size > self.data.len() {
            return Err(MemFault::AccessFault { addr });
        }
        let mut buf = [0u8; 8];
        buf[..size].copy_from_slice(&self.data[off..off + size]);
        Ok(u64::from_le_bytes(buf))
    }
    fn write(&mut self, addr: u64, size: usize, val: u64, _ty: AccessType) -> Result<(), MemFault> {
        let off = (addr & 0xFF_FFFF) as usize;
        if off + size > self.data.len() {
            return Err(MemFault::AccessFault { addr });
        }
        self.data[off..off + size].copy_from_slice(&val.to_le_bytes()[..size]);
        Ok(())
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

const BASE: u64 = 0x40_0000;
const DATA: u64 = 0x10_0000;

fn setup() -> (Aarch64ArchState, TestMem) {
    let mut a = Aarch64ArchState::new();
    let m = TestMem::new();
    a.pc = BASE;
    a.sp = 0x7F_8000;
    (a, m)
}

fn step(a: &mut Aarch64ArchState, mem: &mut TestMem, raw: u32) {
    let insn = helm_arch::aarch64::decode::decode(raw, a.pc).expect("decode");
    let pc_written = helm_arch::aarch64::execute::execute(&insn, a, mem).expect("execute");
    if !pc_written {
        a.pc += 4;
    }
}

fn write_u64(mem: &mut TestMem, addr: u64, val: u64) {
    mem.write(addr, 8, val, AccessType::Store).unwrap();
}

fn write_u32(mem: &mut TestMem, addr: u64, val: u32) {
    mem.write(addr, 4, val as u64, AccessType::Store).unwrap();
}

fn write_u16(mem: &mut TestMem, addr: u64, val: u16) {
    mem.write(addr, 2, val as u64, AccessType::Store).unwrap();
}

fn write_u8(mem: &mut TestMem, addr: u64, val: u8) {
    mem.write(addr, 1, val as u64, AccessType::Store).unwrap();
}

fn read_u64(mem: &mut TestMem, addr: u64) -> u64 {
    mem.read(addr, 8, AccessType::Load).unwrap()
}

fn read_u32(mem: &mut TestMem, addr: u64) -> u32 {
    mem.read(addr, 4, AccessType::Load).unwrap() as u32
}

fn read_u16(mem: &mut TestMem, addr: u64) -> u16 {
    mem.read(addr, 2, AccessType::Load).unwrap() as u16
}

#[allow(dead_code)]
fn read_u8(mem: &mut TestMem, addr: u64) -> u8 {
    mem.read(addr, 1, AccessType::Load).unwrap() as u8
}

// ── Encoding helpers ───────────────────────────────────────────────────────────

fn encode_str_x_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b11111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn encode_ldr_x_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b11111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn encode_str_w_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b10111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn encode_ldr_w_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b10111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn encode_strb_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b00111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn encode_ldrb_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b00111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn encode_strh_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b01111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn encode_ldrh_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b01111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn encode_ldrsw_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b10111001_10 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn encode_ldrsb_w_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b00111001_11 << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn encode_ldrsh_w_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b01111001_11 << 22) | (imm12 << 10) | (rn << 5) | rt
}

// STP/LDP signed offset
fn encode_stp_x(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    let i = (imm7 as u32) & 0x7F;
    (0b10_101_0_0_10_0 << 22) | (i << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn encode_ldp_x(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    let i = (imm7 as u32) & 0x7F;
    (0b10_101_0_0_10_1 << 22) | (i << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn encode_stp_x_pre(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    let i = (imm7 as u32) & 0x7F;
    (0b10_101_0_0_11_0 << 22) | (i << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn encode_ldp_x_post(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    let i = (imm7 as u32) & 0x7F;
    (0b10_101_0_0_01_1 << 22) | (i << 15) | (rt2 << 10) | (rn << 5) | rt
}

// ===================================================================
//  STR / LDR -- unsigned offset, all sizes
// ===================================================================

#[test]
fn str_ldr_x64_roundtrip() {
    let (mut a, mut m) = setup();
    a.x[0] = 0xDEAD_BEEF_CAFE_1234;
    a.x[2] = DATA;
    step(&mut a, &mut m, encode_str_x_uimm(0, 2, 0));
    step(&mut a, &mut m, encode_ldr_x_uimm(0, 2, 1));
    assert_eq!(a.x[1], 0xDEAD_BEEF_CAFE_1234);
}

#[test]
fn str_ldr_x64_offset() {
    let (mut a, mut m) = setup();
    a.x[0] = 0x42;
    a.x[2] = DATA;
    step(&mut a, &mut m, encode_str_x_uimm(1, 2, 0));
    step(&mut a, &mut m, encode_ldr_x_uimm(1, 2, 1));
    assert_eq!(a.x[1], 0x42, "offset 1 = 8 bytes");
    assert_eq!(read_u64(&mut m, DATA + 8), 0x42);
}

#[test]
fn str_ldr_w32_roundtrip() {
    let (mut a, mut m) = setup();
    a.x[0] = 0x1_FFFF_FFFF;
    a.x[2] = DATA;
    step(&mut a, &mut m, encode_str_w_uimm(0, 2, 0));
    step(&mut a, &mut m, encode_ldr_w_uimm(0, 2, 1));
    assert_eq!(a.x[1], 0xFFFF_FFFF, "LDR W zero-extends to 64");
}

#[test]
fn strb_ldrb_roundtrip() {
    let (mut a, mut m) = setup();
    a.x[0] = 0xFF;
    a.x[2] = DATA;
    step(&mut a, &mut m, encode_strb_uimm(0, 2, 0));
    step(&mut a, &mut m, encode_ldrb_uimm(0, 2, 1));
    assert_eq!(a.x[1], 0xFF);
}

#[test]
fn strb_ldrb_zero_extends() {
    let (mut a, mut m) = setup();
    a.x[0] = 0x1FF;
    a.x[2] = DATA;
    step(&mut a, &mut m, encode_strb_uimm(0, 2, 0));
    step(&mut a, &mut m, encode_ldrb_uimm(0, 2, 1));
    assert_eq!(a.x[1], 0xFF, "STRB truncates, LDRB zero-extends");
}

#[test]
fn strh_ldrh_roundtrip() {
    let (mut a, mut m) = setup();
    a.x[0] = 0xABCD;
    a.x[2] = DATA;
    step(&mut a, &mut m, encode_strh_uimm(0, 2, 0));
    step(&mut a, &mut m, encode_ldrh_uimm(0, 2, 1));
    assert_eq!(a.x[1], 0xABCD);
}

#[test]
fn strh_truncates() {
    let (mut a, mut m) = setup();
    a.x[0] = 0x1_ABCD;
    a.x[2] = DATA;
    step(&mut a, &mut m, encode_strh_uimm(0, 2, 0));
    assert_eq!(read_u16(&mut m, DATA), 0xABCD);
}

// ===================================================================
//  Sign-extending loads -- LDRSW, LDRSB, LDRSH
// ===================================================================

#[test]
fn ldrsw_positive() {
    let (mut a, mut m) = setup();
    write_u32(&mut m, DATA, 0x7FFF_FFFF);
    a.x[2] = DATA;
    step(&mut a, &mut m, encode_ldrsw_uimm(0, 2, 0));
    assert_eq!(a.x[0], 0x7FFF_FFFF, "positive sign-extends to same");
}

#[test]
fn ldrsw_negative() {
    let (mut a, mut m) = setup();
    write_u32(&mut m, DATA, 0x8000_0000);
    a.x[2] = DATA;
    step(&mut a, &mut m, encode_ldrsw_uimm(0, 2, 0));
    assert_eq!(a.x[0], 0xFFFF_FFFF_8000_0000, "LDRSW sign-extends");
}

#[test]
fn ldrsb_positive() {
    let (mut a, mut m) = setup();
    write_u8(&mut m, DATA, 0x7F);
    a.x[2] = DATA;
    step(&mut a, &mut m, encode_ldrsb_w_uimm(0, 2, 0));
    assert_eq!(a.x[0], 0x7F);
}

#[test]
fn ldrsb_negative() {
    let (mut a, mut m) = setup();
    write_u8(&mut m, DATA, 0x80);
    a.x[2] = DATA;
    step(&mut a, &mut m, encode_ldrsb_w_uimm(0, 2, 0));
    assert_eq!(
        a.x[0],
        0xFFFF_FF80,
        "LDRSB to W sign-extends within 32-bit"
    );
}

#[test]
fn ldrsh_positive() {
    let (mut a, mut m) = setup();
    write_u16(&mut m, DATA, 0x7FFF);
    a.x[2] = DATA;
    step(&mut a, &mut m, encode_ldrsh_w_uimm(0, 2, 0));
    assert_eq!(a.x[0], 0x7FFF);
}

#[test]
fn ldrsh_negative() {
    let (mut a, mut m) = setup();
    write_u16(&mut m, DATA, 0x8000);
    a.x[2] = DATA;
    step(&mut a, &mut m, encode_ldrsh_w_uimm(0, 2, 0));
    assert_eq!(
        a.x[0],
        0xFFFF_8000,
        "LDRSH to W sign-extends within 32-bit"
    );
}

// ===================================================================
//  STP / LDP -- signed offset, pre-index, post-index
// ===================================================================

#[test]
fn stp_ldp_x_roundtrip() {
    let (mut a, mut m) = setup();
    a.x[0] = 0xAAAA;
    a.x[1] = 0xBBBB;
    a.x[2] = DATA;
    step(&mut a, &mut m, encode_stp_x(0, 1, 2, 0));
    step(&mut a, &mut m, encode_ldp_x(0, 3, 2, 4));
    assert_eq!(a.x[4], 0xAAAA);
    assert_eq!(a.x[3], 0xBBBB);
}

#[test]
fn stp_x_positive_offset() {
    let (mut a, mut m) = setup();
    a.x[0] = 0x1111;
    a.x[1] = 0x2222;
    a.x[2] = DATA;
    step(&mut a, &mut m, encode_stp_x(2, 1, 2, 0));
    assert_eq!(read_u64(&mut m, DATA + 16), 0x1111);
    assert_eq!(read_u64(&mut m, DATA + 24), 0x2222);
}

#[test]
fn stp_x_pre_index() {
    let (mut a, mut m) = setup();
    a.x[0] = 0xAA;
    a.x[1] = 0xBB;
    a.x[2] = DATA + 32;
    step(&mut a, &mut m, encode_stp_x_pre(-2, 1, 2, 0)); // #-16
    assert_eq!(a.x[2], DATA + 16, "pre-index decrements base");
    assert_eq!(read_u64(&mut m, DATA + 16), 0xAA);
    assert_eq!(read_u64(&mut m, DATA + 24), 0xBB);
}

#[test]
fn ldp_x_post_index() {
    let (mut a, mut m) = setup();
    write_u64(&mut m, DATA, 0x1111);
    write_u64(&mut m, DATA + 8, 0x2222);
    a.x[2] = DATA;
    step(&mut a, &mut m, encode_ldp_x_post(2, 3, 2, 4)); // #16
    assert_eq!(a.x[4], 0x1111);
    assert_eq!(a.x[3], 0x2222);
    assert_eq!(a.x[2], DATA + 16, "post-index increments after load");
}

// ===================================================================
//  Exclusive loads/stores
// ===================================================================

#[test]
fn ldxr_stxr_success() {
    // LDXR X0, [X1] ; STXR W2, X3, [X1]
    let (mut a, mut m) = setup();
    write_u64(&mut m, DATA, 42);
    a.x[1] = DATA;
    a.x[3] = 99;
    step(&mut a, &mut m, 0xC85F_7C20); // LDXR
    assert_eq!(a.x[0], 42);
    step(&mut a, &mut m, 0xC803_7C23); // STXR
    assert_eq!(a.x[2], 0, "STXR success");
    assert_eq!(read_u64(&mut m, DATA), 99);
}

// ===================================================================
//  Atomics -- SWP
// ===================================================================

#[test]
fn swp_x() {
    let (mut a, mut m) = setup();
    write_u64(&mut m, DATA, 100);
    a.x[0] = 200;
    a.x[2] = DATA;
    step(&mut a, &mut m, 0xF820_4041); // SWP X0, X1, [X2]
    assert_eq!(a.x[1], 100, "old value returned");
    assert_eq!(read_u64(&mut m, DATA), 200, "new value stored");
}

// ===================================================================
//  Store/load to SP (Rn=31)
// ===================================================================

#[test]
fn str_ldr_via_sp() {
    let (mut a, mut m) = setup();
    a.x[0] = 0xCAFE_BABE;
    step(&mut a, &mut m, encode_str_x_uimm(0, 31, 0));
    step(&mut a, &mut m, encode_ldr_x_uimm(0, 31, 1));
    assert_eq!(a.x[1], 0xCAFE_BABE, "load/store via SP");
}

#[test]
fn stp_q_offset_stores_at_base_plus_offset() {
    let (mut a, mut m) = setup();
    let base = a.sp;
    a.v[0] = 0x1111_2222_3333_4444u128 | (0x5555_6666_7777_8888u128 << 64);
    a.v[1] = 0x9999_AAAA_BBBB_CCCCu128 | (0xDDDD_EEEE_FFFF_0001u128 << 64);

    step(&mut a, &mut m, 0xAD01_07E0); // STP Q0, Q1, [SP, #0x20]

    assert_eq!(read_u64(&mut m, base + 0x20), 0x1111_2222_3333_4444);
    assert_eq!(read_u64(&mut m, base + 0x28), 0x5555_6666_7777_8888);
    assert_eq!(read_u64(&mut m, base + 0x30), 0x9999_AAAA_BBBB_CCCC);
    assert_eq!(read_u64(&mut m, base + 0x38), 0xDDDD_EEEE_FFFF_0001);
}

#[test]
fn ldp_q_offset_loads_from_base_plus_offset() {
    let (mut a, mut m) = setup();
    let base = a.sp;
    write_u64(&mut m, base + 0x20, 0x1111_2222_3333_4444);
    write_u64(&mut m, base + 0x28, 0x5555_6666_7777_8888);
    write_u64(&mut m, base + 0x30, 0x9999_AAAA_BBBB_CCCC);
    write_u64(&mut m, base + 0x38, 0xDDDD_EEEE_FFFF_0001);

    step(&mut a, &mut m, 0xAD41_0FE2); // LDP Q2, Q3, [SP, #0x20]

    assert_eq!(a.v[2], 0x1111_2222_3333_4444u128 | (0x5555_6666_7777_8888u128 << 64));
    assert_eq!(a.v[3], 0x9999_AAAA_BBBB_CCCCu128 | (0xDDDD_EEEE_FFFF_0001u128 << 64));
}

//! AArch64 Load/Store instruction tests.
//!
//! Covers: LDR/STR (B/H/W/X, unsigned offset, pre/post, register),
//! LDRS* sign-extending loads, LDP/STP (pre/post/offset),
//! LDXR/STXR exclusive, SWP/LDADD atomics.

use crate::arm::aarch64::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;

fn cpu_with_code(insns: &[u32]) -> (Aarch64Cpu, AddressSpace) {
    let mut cpu = Aarch64Cpu::new();
    let mut mem = AddressSpace::new();
    let base = 0x40_0000u64;
    let size = (insns.len() * 4 + 0x1000) as u64;
    mem.map(base, size, (true, true, true));
    for (i, insn) in insns.iter().enumerate() {
        let addr = base + (i as u64 * 4);
        mem.write(addr, &insn.to_le_bytes()).unwrap();
    }
    cpu.regs.pc = base;
    mem.map(0x7FFF_0000, 0x10000, (true, true, false));
    cpu.regs.sp = 0x7FFF_8000;
    mem.map(0x10_0000, 0x2000, (true, true, false));
    (cpu, mem)
}

// Encoding helpers from aarch64-ldst.decode
fn encode_str_x_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b11111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn encode_ldr_x_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b11111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn encode_str_w_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b10111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn encode_ldr_w_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b10111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn encode_strb_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b00111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn encode_ldrb_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b00111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn encode_strh_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b01111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn encode_ldrh_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b01111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn encode_ldrsw_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b10111001_10 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn encode_ldrsb_w_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b00111001_11 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn encode_ldrsh_w_uimm(imm12: u32, rn: u32, rt: u32) -> u32 { (0b01111001_11 << 22) | (imm12 << 10) | (rn << 5) | rt }

// STP/LDP signed offset: opc 10100 V=0 10 L imm7 Rt2 Rn Rt
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

const DATA: u64 = 0x10_0000;

fn write_u64(mem: &mut AddressSpace, addr: u64, val: u64) { mem.write(addr, &val.to_le_bytes()).unwrap(); }
fn write_u32(mem: &mut AddressSpace, addr: u64, val: u32) { mem.write(addr, &val.to_le_bytes()).unwrap(); }
fn write_u16(mem: &mut AddressSpace, addr: u64, val: u16) { mem.write(addr, &val.to_le_bytes()).unwrap(); }
fn write_u8(mem: &mut AddressSpace, addr: u64, val: u8) { mem.write(addr, &[val]).unwrap(); }
fn read_u64(mem: &AddressSpace, addr: u64) -> u64 { let mut b = [0u8; 8]; mem.read(addr, &mut b).unwrap(); u64::from_le_bytes(b) }
fn read_u32(mem: &AddressSpace, addr: u64) -> u32 { let mut b = [0u8; 4]; mem.read(addr, &mut b).unwrap(); u32::from_le_bytes(b) }
fn read_u16(mem: &AddressSpace, addr: u64) -> u16 { let mut b = [0u8; 2]; mem.read(addr, &mut b).unwrap(); u16::from_le_bytes(b) }
fn read_u8(mem: &AddressSpace, addr: u64) -> u8 { let mut b = [0u8; 1]; mem.read(addr, &mut b).unwrap(); b[0] }

// ===================================================================
//  STR / LDR — unsigned offset, all sizes
// ===================================================================

#[test] fn str_ldr_x64_roundtrip() {
    let (mut c, mut m) = cpu_with_code(&[encode_str_x_uimm(0, 2, 0), encode_ldr_x_uimm(0, 2, 1)]);
    c.set_xn(0, 0xDEAD_BEEF_CAFE_1234); c.set_xn(2, DATA);
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0xDEAD_BEEF_CAFE_1234);
}
#[test] fn str_ldr_x64_offset() {
    let (mut c, mut m) = cpu_with_code(&[encode_str_x_uimm(1, 2, 0), encode_ldr_x_uimm(1, 2, 1)]);
    c.set_xn(0, 0x42); c.set_xn(2, DATA);
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0x42, "offset 1 = 8 bytes");
    assert_eq!(read_u64(&m, DATA + 8), 0x42);
}
#[test] fn str_ldr_w32_roundtrip() {
    let (mut c, mut m) = cpu_with_code(&[encode_str_w_uimm(0, 2, 0), encode_ldr_w_uimm(0, 2, 1)]);
    c.set_xn(0, 0x1_FFFF_FFFF); c.set_xn(2, DATA);
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0xFFFF_FFFF, "LDR W zero-extends to 64");
}
#[test] fn strb_ldrb_roundtrip() {
    let (mut c, mut m) = cpu_with_code(&[encode_strb_uimm(0, 2, 0), encode_ldrb_uimm(0, 2, 1)]);
    c.set_xn(0, 0xFF); c.set_xn(2, DATA);
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0xFF);
}
#[test] fn strb_ldrb_zero_extends() {
    let (mut c, mut m) = cpu_with_code(&[encode_strb_uimm(0, 2, 0), encode_ldrb_uimm(0, 2, 1)]);
    c.set_xn(0, 0x1FF); c.set_xn(2, DATA);
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0xFF, "STRB truncates, LDRB zero-extends");
}
#[test] fn strh_ldrh_roundtrip() {
    let (mut c, mut m) = cpu_with_code(&[encode_strh_uimm(0, 2, 0), encode_ldrh_uimm(0, 2, 1)]);
    c.set_xn(0, 0xABCD); c.set_xn(2, DATA);
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0xABCD);
}
#[test] fn strh_truncates() {
    let (mut c, mut m) = cpu_with_code(&[encode_strh_uimm(0, 2, 0)]);
    c.set_xn(0, 0x1_ABCD); c.set_xn(2, DATA);
    c.step(&mut m).unwrap();
    assert_eq!(read_u16(&m, DATA), 0xABCD);
}

// ===================================================================
//  Sign-extending loads — LDRSW, LDRSB, LDRSH
// ===================================================================

#[test] fn ldrsw_positive() {
    let (mut c, mut m) = cpu_with_code(&[encode_ldrsw_uimm(0, 2, 0)]);
    write_u32(&mut m, DATA, 0x7FFF_FFFF); c.set_xn(2, DATA);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x7FFF_FFFF, "positive sign-extends to same");
}
#[test] fn ldrsw_negative() {
    let (mut c, mut m) = cpu_with_code(&[encode_ldrsw_uimm(0, 2, 0)]);
    write_u32(&mut m, DATA, 0x8000_0000); c.set_xn(2, DATA);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FFFF_8000_0000, "LDRSW sign-extends");
}
#[test] fn ldrsb_positive() {
    let (mut c, mut m) = cpu_with_code(&[encode_ldrsb_w_uimm(0, 2, 0)]);
    write_u8(&mut m, DATA, 0x7F); c.set_xn(2, DATA);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x7F);
}
#[test] fn ldrsb_negative() {
    let (mut c, mut m) = cpu_with_code(&[encode_ldrsb_w_uimm(0, 2, 0)]);
    write_u8(&mut m, DATA, 0x80); c.set_xn(2, DATA);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FF80, "LDRSB to W sign-extends within 32-bit");
}
#[test] fn ldrsh_positive() {
    let (mut c, mut m) = cpu_with_code(&[encode_ldrsh_w_uimm(0, 2, 0)]);
    write_u16(&mut m, DATA, 0x7FFF); c.set_xn(2, DATA);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x7FFF);
}
#[test] fn ldrsh_negative() {
    let (mut c, mut m) = cpu_with_code(&[encode_ldrsh_w_uimm(0, 2, 0)]);
    write_u16(&mut m, DATA, 0x8000); c.set_xn(2, DATA);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_8000, "LDRSH to W sign-extends within 32-bit");
}

// ===================================================================
//  STP / LDP — signed offset, pre-index, post-index
// ===================================================================

#[test] fn stp_ldp_x_roundtrip() {
    let (mut c, mut m) = cpu_with_code(&[encode_stp_x(0, 1, 2, 0), encode_ldp_x(0, 3, 2, 4)]);
    c.set_xn(0, 0xAAAA); c.set_xn(1, 0xBBBB); c.set_xn(2, DATA);
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    assert_eq!(c.xn(4), 0xAAAA); assert_eq!(c.xn(3), 0xBBBB);
}
#[test] fn stp_x_positive_offset() {
    let (mut c, mut m) = cpu_with_code(&[encode_stp_x(2, 1, 2, 0)]);
    c.set_xn(0, 0x1111); c.set_xn(1, 0x2222); c.set_xn(2, DATA);
    c.step(&mut m).unwrap();
    assert_eq!(read_u64(&m, DATA + 16), 0x1111);
    assert_eq!(read_u64(&m, DATA + 24), 0x2222);
}
#[test] fn stp_x_pre_index() {
    let (mut c, mut m) = cpu_with_code(&[encode_stp_x_pre(-2, 1, 2, 0)]); // #-16
    c.set_xn(0, 0xAA); c.set_xn(1, 0xBB); c.set_xn(2, DATA + 32);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(2), DATA + 16, "pre-index decrements base");
    assert_eq!(read_u64(&m, DATA + 16), 0xAA);
    assert_eq!(read_u64(&m, DATA + 24), 0xBB);
}
#[test] fn ldp_x_post_index() {
    let (mut c, mut m) = cpu_with_code(&[encode_ldp_x_post(2, 3, 2, 4)]); // #16
    write_u64(&mut m, DATA, 0x1111); write_u64(&mut m, DATA + 8, 0x2222);
    c.set_xn(2, DATA);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(4), 0x1111); assert_eq!(c.xn(3), 0x2222);
    assert_eq!(c.xn(2), DATA + 16, "post-index increments after load");
}

// ===================================================================
//  Exclusive loads/stores
// ===================================================================

#[test] fn ldxr_stxr_success() {
    // LDXR X0, [X1] ; STXR W2, X3, [X1]
    let (mut c, mut m) = cpu_with_code(&[0xC85F_7C20, 0xC803_7C23]);
    write_u64(&mut m, DATA, 42); c.set_xn(1, DATA); c.set_xn(3, 99);
    c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 42);
    c.step(&mut m).unwrap(); assert_eq!(c.xn(2), 0, "STXR success");
    assert_eq!(read_u64(&m, DATA), 99);
}

// ===================================================================
//  Atomics — SWP, LDADD
// ===================================================================

#[test] fn swp_x() {
    let (mut c, mut m) = cpu_with_code(&[0xF820_8041]); // SWP X0, X1, [X2]
    write_u64(&mut m, DATA, 100); c.set_xn(0, 200); c.set_xn(2, DATA);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 100, "old value returned");
    assert_eq!(read_u64(&m, DATA), 200, "new value stored");
}

// ===================================================================
//  Store/load to SP (Rn=31)
// ===================================================================

#[test] fn str_ldr_via_sp() {
    let (mut c, mut m) = cpu_with_code(&[encode_str_x_uimm(0, 31, 0), encode_ldr_x_uimm(0, 31, 1)]);
    c.set_xn(0, 0xCAFE_BABE);
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0xCAFE_BABE, "load/store via SP");
}

//! Corner-case tests targeting code paths with ZERO prior coverage.
//!
//! Each test here is designed to catch a specific class of potential bug:
//! - SP vs XZR disambiguation
//! - 32-bit truncation / zero-extension
//! - Flag preservation by non-flag-setting instructions
//! - Extended register addressing
//! - LDUR/STUR negative offsets
//! - Register-offset loads with shifts
//! - Pre/post indexed single loads
//! - MOVN 32-bit truncation

use crate::arm::aarch64::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;

fn cpu_exec(insns: &[u32]) -> (Aarch64Cpu, AddressSpace) {
    let mut cpu = Aarch64Cpu::new();
    let mut mem = AddressSpace::new();
    let base = 0x40_0000u64;
    mem.map(base, (insns.len() * 4 + 0x1000) as u64, (true, true, true));
    for (i, insn) in insns.iter().enumerate() {
        mem.write(base + (i as u64 * 4), &insn.to_le_bytes()).unwrap();
    }
    cpu.regs.pc = base;
    mem.map(0x7FFF_0000, 0x10000, (true, true, false));
    cpu.regs.sp = 0x7FFF_8000;
    mem.map(0x10_0000, 0x4000, (true, true, false));
    (cpu, mem)
}

fn set_flags(cpu: &mut Aarch64Cpu, n: bool, z: bool, c: bool, v: bool) {
    cpu.regs.nzcv = ((n as u32) << 31) | ((z as u32) << 30) | ((c as u32) << 29) | ((v as u32) << 28);
}
fn wr64(m: &mut AddressSpace, a: u64, v: u64) { m.write(a, &v.to_le_bytes()).unwrap(); }
fn wr32(m: &mut AddressSpace, a: u64, v: u32) { m.write(a, &v.to_le_bytes()).unwrap(); }
fn wr16(m: &mut AddressSpace, a: u64, v: u16) { m.write(a, &v.to_le_bytes()).unwrap(); }
fn wr8(m: &mut AddressSpace, a: u64, v: u8)   { m.write(a, &[v]).unwrap(); }
fn rd64(m: &AddressSpace, a: u64) -> u64 { let mut b=[0u8;8]; m.read(a, &mut b).unwrap(); u64::from_le_bytes(b) }

const D: u64 = 0x10_0000;

// Encoding helpers
fn add_sub_imm(sf: u32, op: u32, s: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s << 29) | (0b10001 << 24) | (sh << 22) | (imm12 << 10) | (rn << 5) | rd
}
fn mov_wide(sf: u32, opc: u32, hw: u32, imm16: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b100101 << 23) | (hw << 21) | (imm16 << 5) | rd
}
fn add_sub_ext(sf: u32, op: u32, s: u32, rm: u32, option: u32, imm3: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s << 29) | (0b01011 << 24) | (1 << 21) | (rm << 16) | (option << 13) | (imm3 << 10) | (rn << 5) | rd
}
fn log_reg(sf: u32, opc: u32, n: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b01010 << 24) | (shift << 22) | (n << 21) | (rm << 16) | (imm6 << 10) | (rn << 5) | rd
}
fn add_sub_reg(sf: u32, op: u32, s: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s << 29) | (0b01011 << 24) | (shift << 22) | (rm << 16) | (imm6 << 10) | (rn << 5) | rd
}
// LDUR/STUR: size 111000 opc 0 imm9 type=00 rn rt (unscaled)
fn stur_x(imm9: i32, rn: u32, rt: u32) -> u32 {
    let i = (imm9 as u32) & 0x1FF;
    (0b11 << 30) | (0b111000 << 24) | (0b00 << 22) | (0 << 21) | (i << 12) | (0b00 << 10) | (rn << 5) | rt
}
fn ldur_x(imm9: i32, rn: u32, rt: u32) -> u32 {
    let i = (imm9 as u32) & 0x1FF;
    (0b11 << 30) | (0b111000 << 24) | (0b01 << 22) | (0 << 21) | (i << 12) | (0b00 << 10) | (rn << 5) | rt
}
fn stur_w(imm9: i32, rn: u32, rt: u32) -> u32 {
    let i = (imm9 as u32) & 0x1FF;
    (0b10 << 30) | (0b111000 << 24) | (0b00 << 22) | (0 << 21) | (i << 12) | (0b00 << 10) | (rn << 5) | rt
}
fn ldur_w(imm9: i32, rn: u32, rt: u32) -> u32 {
    let i = (imm9 as u32) & 0x1FF;
    (0b10 << 30) | (0b111000 << 24) | (0b01 << 22) | (0 << 21) | (i << 12) | (0b00 << 10) | (rn << 5) | rt
}
fn sturb(imm9: i32, rn: u32, rt: u32) -> u32 {
    let i = (imm9 as u32) & 0x1FF;
    (0b00 << 30) | (0b111000 << 24) | (0b00 << 22) | (0 << 21) | (i << 12) | (0b00 << 10) | (rn << 5) | rt
}
fn ldurb(imm9: i32, rn: u32, rt: u32) -> u32 {
    let i = (imm9 as u32) & 0x1FF;
    (0b00 << 30) | (0b111000 << 24) | (0b01 << 22) | (0 << 21) | (i << 12) | (0b00 << 10) | (rn << 5) | rt
}
fn ldursb_x(imm9: i32, rn: u32, rt: u32) -> u32 {
    let i = (imm9 as u32) & 0x1FF;
    (0b00 << 30) | (0b111000 << 24) | (0b10 << 22) | (0 << 21) | (i << 12) | (0b00 << 10) | (rn << 5) | rt
}
fn ldursw(imm9: i32, rn: u32, rt: u32) -> u32 {
    let i = (imm9 as u32) & 0x1FF;
    (0b10 << 30) | (0b111000 << 24) | (0b10 << 22) | (0 << 21) | (i << 12) | (0b00 << 10) | (rn << 5) | rt
}
// LDR/STR pre-indexed: size 111000 opc 0 imm9 11 rn rt
fn str_x_pre(imm9: i32, rn: u32, rt: u32) -> u32 {
    let i = (imm9 as u32) & 0x1FF;
    (0b11 << 30) | (0b111000 << 24) | (0b00 << 22) | (0 << 21) | (i << 12) | (0b11 << 10) | (rn << 5) | rt
}
fn ldr_x_pre(imm9: i32, rn: u32, rt: u32) -> u32 {
    let i = (imm9 as u32) & 0x1FF;
    (0b11 << 30) | (0b111000 << 24) | (0b01 << 22) | (0 << 21) | (i << 12) | (0b11 << 10) | (rn << 5) | rt
}
// LDR/STR post-indexed: size 111000 opc 0 imm9 01 rn rt
fn str_x_post(imm9: i32, rn: u32, rt: u32) -> u32 {
    let i = (imm9 as u32) & 0x1FF;
    (0b11 << 30) | (0b111000 << 24) | (0b00 << 22) | (0 << 21) | (i << 12) | (0b01 << 10) | (rn << 5) | rt
}
fn ldr_x_post(imm9: i32, rn: u32, rt: u32) -> u32 {
    let i = (imm9 as u32) & 0x1FF;
    (0b11 << 30) | (0b111000 << 24) | (0b01 << 22) | (0 << 21) | (i << 12) | (0b01 << 10) | (rn << 5) | rt
}
// LDR register offset: size 111000 opc 1 rm option S 10 rn rt
fn ldr_x_reg(rm: u32, option: u32, s_flag: u32, rn: u32, rt: u32) -> u32 {
    (0b11 << 30) | (0b111000 << 24) | (0b01 << 22) | (1 << 21) | (rm << 16) | (option << 13) | (s_flag << 12) | (0b10 << 10) | (rn << 5) | rt
}
fn str_x_reg(rm: u32, option: u32, s_flag: u32, rn: u32, rt: u32) -> u32 {
    (0b11 << 30) | (0b111000 << 24) | (0b00 << 22) | (1 << 21) | (rm << 16) | (option << 13) | (s_flag << 12) | (0b10 << 10) | (rn << 5) | rt
}
fn ldr_w_reg(rm: u32, option: u32, s_flag: u32, rn: u32, rt: u32) -> u32 {
    (0b10 << 30) | (0b111000 << 24) | (0b01 << 22) | (1 << 21) | (rm << 16) | (option << 13) | (s_flag << 12) | (0b10 << 10) | (rn << 5) | rt
}
fn ldrb_reg(rm: u32, option: u32, s_flag: u32, rn: u32, rt: u32) -> u32 {
    (0b00 << 30) | (0b111000 << 24) | (0b01 << 22) | (1 << 21) | (rm << 16) | (option << 13) | (s_flag << 12) | (0b10 << 10) | (rn << 5) | rt
}

// ===================================================================
//  MOVN 32-bit truncation — Bug #8 candidate
// ===================================================================

#[test]
fn movn_w_zero_must_be_32bit() {
    let (mut c, mut m) = cpu_exec(&[mov_wide(0, 0b00, 0, 0, 0)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FFFF, "MOVN W0, #0 must produce 0xFFFFFFFF not u64::MAX");
}

#[test]
fn movn_w_1_must_be_32bit() {
    let (mut c, mut m) = cpu_exec(&[mov_wide(0, 0b00, 0, 1, 0)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FFFE, "MOVN W0, #1 = ~1 truncated to 32-bit");
}

#[test]
fn movn_w_ffff_must_be_32bit() {
    let (mut c, mut m) = cpu_exec(&[mov_wide(0, 0b00, 0, 0xFFFF, 0)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_0000, "MOVN W0, #0xFFFF = ~0xFFFF & 0xFFFFFFFF");
}

#[test]
fn movn_w_hw1_must_be_32bit() {
    let (mut c, mut m) = cpu_exec(&[mov_wide(0, 0b00, 1, 0xFFFF, 0)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x0000_FFFF, "MOVN W0, #0xFFFF, LSL #16 = ~0xFFFF0000 & 0xFFFFFFFF");
}

// ===================================================================
//  SP vs XZR disambiguation
//  ADD/SUB imm with Rd=31: non-S → SP, S → XZR (CMP/CMN)
// ===================================================================

#[test]
fn add_imm_to_sp() {
    // ADD SP, SP, #0x10 — should modify SP
    let (mut c, mut m) = cpu_exec(&[add_sub_imm(1, 0, 0, 0, 0x10, 31, 31)]);
    let old_sp = c.regs.sp;
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.sp, old_sp + 0x10, "ADD to SP should update SP");
}

#[test]
fn sub_imm_from_sp() {
    // SUB SP, SP, #0x20
    let (mut c, mut m) = cpu_exec(&[add_sub_imm(1, 1, 0, 0, 0x20, 31, 31)]);
    let old_sp = c.regs.sp;
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.sp, old_sp - 0x20, "SUB from SP should update SP");
}

#[test]
fn adds_imm_rd31_is_xzr_not_sp() {
    // ADDS XZR, X1, #42 = CMN X1, #42 — Rd=31 with S=1 means XZR, not SP
    let (mut c, mut m) = cpu_exec(&[add_sub_imm(1, 0, 1, 0, 42, 1, 31)]);
    let old_sp = c.regs.sp;
    c.set_xn(1, 0);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.sp, old_sp, "CMN should NOT modify SP");
}

#[test]
fn mov_to_sp_via_add() {
    // MOV SP, X1 = ADD SP, X1, #0
    let (mut c, mut m) = cpu_exec(&[add_sub_imm(1, 0, 0, 0, 0, 1, 31)]);
    c.set_xn(1, 0x7FFF_4000);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.sp, 0x7FFF_4000, "MOV SP, X1 via ADD");
}

// ===================================================================
//  Flag preservation — non-S instructions must not touch NZCV
// ===================================================================

#[test]
fn add_reg_preserves_flags() {
    let (mut c, mut m) = cpu_exec(&[add_sub_reg(1, 0, 0, 0, 2, 0, 1, 0)]);
    c.set_xn(1, 100); c.set_xn(2, 200);
    set_flags(&mut c, true, true, true, true);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 300);
    assert!(c.regs.n() && c.regs.z() && c.regs.c() && c.regs.v(),
            "ADD (no S) must preserve NZCV");
}

#[test]
fn sub_reg_preserves_flags() {
    let (mut c, mut m) = cpu_exec(&[add_sub_reg(1, 1, 0, 0, 2, 0, 1, 0)]);
    c.set_xn(1, 100); c.set_xn(2, 30);
    set_flags(&mut c, true, false, true, false);
    c.step(&mut m).unwrap();
    assert!(c.regs.n() && !c.regs.z() && c.regs.c() && !c.regs.v(),
            "SUB (no S) must preserve NZCV");
}

#[test]
fn and_reg_preserves_flags() {
    let (mut c, mut m) = cpu_exec(&[log_reg(1, 0b00, 0, 0, 2, 0, 1, 0)]);
    c.set_xn(1, 0xFF); c.set_xn(2, 0x0F);
    set_flags(&mut c, true, true, true, true);
    c.step(&mut m).unwrap();
    assert!(c.regs.n() && c.regs.z() && c.regs.c() && c.regs.v(),
            "AND (no S) must preserve NZCV");
}

#[test]
fn orr_reg_preserves_flags() {
    let (mut c, mut m) = cpu_exec(&[log_reg(1, 0b01, 0, 0, 2, 0, 1, 0)]);
    c.set_xn(1, 0); c.set_xn(2, 42);
    set_flags(&mut c, false, true, false, true);
    c.step(&mut m).unwrap();
    assert!(!c.regs.n() && c.regs.z() && !c.regs.c() && c.regs.v(),
            "ORR must preserve NZCV");
}

#[test]
fn ldr_preserves_flags() {
    let str_insn = (0b11u32 << 30) | (0b111001 << 24) | (0b00 << 22) | (3 << 5);
    let ldr_insn = (0b11u32 << 30) | (0b111001 << 24) | (0b01 << 22) | (3 << 5) | 1;
    let (mut c, mut m) = cpu_exec(&[str_insn, ldr_insn]);
    c.set_xn(0, 42); c.set_xn(3, D);
    set_flags(&mut c, true, true, true, true);
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    assert!(c.regs.n() && c.regs.z() && c.regs.c() && c.regs.v(),
            "LDR must preserve NZCV");
}

// ===================================================================
//  LDUR / STUR — unscaled with negative offsets
// ===================================================================

#[test]
fn stur_ldur_x_positive() {
    let (mut c, mut m) = cpu_exec(&[stur_x(8, 3, 0), ldur_x(8, 3, 1)]);
    c.set_xn(0, 0xDEAD_BEEF); c.set_xn(3, D);
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0xDEAD_BEEF);
}

#[test]
fn stur_ldur_x_negative() {
    let (mut c, mut m) = cpu_exec(&[stur_x(-8, 3, 0), ldur_x(-8, 3, 1)]);
    c.set_xn(0, 0xCAFE); c.set_xn(3, D + 0x100);
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0xCAFE);
    assert_eq!(rd64(&m, D + 0xF8), 0xCAFE, "stored at base-8");
}

#[test]
fn stur_ldur_x_zero() {
    let (mut c, mut m) = cpu_exec(&[stur_x(0, 3, 0), ldur_x(0, 3, 1)]);
    c.set_xn(0, 0x1234); c.set_xn(3, D);
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0x1234);
}

#[test]
fn stur_ldur_w() {
    let (mut c, mut m) = cpu_exec(&[stur_w(4, 3, 0), ldur_w(4, 3, 1)]);
    c.set_xn(0, 0x1_ABCD_EF01); c.set_xn(3, D);
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0xABCD_EF01, "LDUR W zero-extends");
}

#[test]
fn sturb_ldurb() {
    let (mut c, mut m) = cpu_exec(&[sturb(-1, 3, 0), ldurb(-1, 3, 1)]);
    c.set_xn(0, 0xAB); c.set_xn(3, D + 0x10);
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0xAB);
}

#[test]
fn ldursb_x_negative_offset() {
    let (mut c, mut m) = cpu_exec(&[ldursb_x(-3, 3, 0)]);
    wr8(&mut m, D + 0x100 - 3, 0x80);
    c.set_xn(3, D + 0x100);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FFFF_FFFF_FF80, "LDURSB X sign-extends");
}

#[test]
fn ldursw_negative_offset() {
    let (mut c, mut m) = cpu_exec(&[ldursw(-4, 3, 0)]);
    wr32(&mut m, D + 0x100 - 4, 0x8000_0000);
    c.set_xn(3, D + 0x100);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FFFF_8000_0000, "LDURSW sign-extends");
}

// ===================================================================
//  LDR/STR pre-indexed (single register, not pair)
// ===================================================================

#[test]
fn str_x_pre_index() {
    let (mut c, mut m) = cpu_exec(&[str_x_pre(-16, 3, 0)]);
    c.set_xn(0, 0xAAAA); c.set_xn(3, D + 0x100);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(3), D + 0xF0, "pre-index decrements base");
    assert_eq!(rd64(&m, D + 0xF0), 0xAAAA);
}

#[test]
fn ldr_x_pre_index() {
    let (mut c, mut m) = cpu_exec(&[ldr_x_pre(16, 3, 0)]);
    wr64(&mut m, D + 0x110, 0xBBBB);
    c.set_xn(3, D + 0x100);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(3), D + 0x110, "pre-index increments base");
    assert_eq!(c.xn(0), 0xBBBB);
}

// ===================================================================
//  LDR/STR post-indexed (single register)
// ===================================================================

#[test]
fn str_x_post_index() {
    let (mut c, mut m) = cpu_exec(&[str_x_post(16, 3, 0)]);
    c.set_xn(0, 0xCCCC); c.set_xn(3, D + 0x100);
    c.step(&mut m).unwrap();
    assert_eq!(rd64(&m, D + 0x100), 0xCCCC, "stores at original base");
    assert_eq!(c.xn(3), D + 0x110, "post-index increments after");
}

#[test]
fn ldr_x_post_index() {
    let (mut c, mut m) = cpu_exec(&[ldr_x_post(-8, 3, 0)]);
    wr64(&mut m, D + 0x100, 0xDDDD);
    c.set_xn(3, D + 0x100);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xDDDD);
    assert_eq!(c.xn(3), D + 0xF8, "post-index decrements");
}

// ===================================================================
//  LDR/STR register offset — [Xn, Xm, LSL #3] etc.
// ===================================================================

#[test]
fn ldr_x_reg_lsl3() {
    // LDR X0, [X3, X2, LSL #3] — option=011 (LSL), S=1 (shift by 3)
    let (mut c, mut m) = cpu_exec(&[ldr_x_reg(2, 0b011, 1, 3, 0)]);
    wr64(&mut m, D + 5 * 8, 0xAAAA_BBBB);
    c.set_xn(3, D); c.set_xn(2, 5);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xAAAA_BBBB, "LDR X0, [X3, X2, LSL #3]");
}

#[test]
fn ldr_x_reg_no_shift() {
    // LDR X0, [X3, X2] — option=011, S=0 (no shift)
    let (mut c, mut m) = cpu_exec(&[ldr_x_reg(2, 0b011, 0, 3, 0)]);
    wr64(&mut m, D + 16, 0x1234_5678);
    c.set_xn(3, D); c.set_xn(2, 16);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x1234_5678, "LDR X0, [X3, X2]");
}

#[test]
fn str_x_reg_lsl3() {
    let (mut c, mut m) = cpu_exec(&[str_x_reg(2, 0b011, 1, 3, 0)]);
    c.set_xn(0, 0xFEED); c.set_xn(3, D); c.set_xn(2, 3);
    c.step(&mut m).unwrap();
    assert_eq!(rd64(&m, D + 24), 0xFEED, "STR X0, [X3, X2, LSL #3]");
}

#[test]
fn ldr_x_reg_sxtw() {
    // LDR X0, [X3, W2, SXTW #3] — option=110, S=1
    let (mut c, mut m) = cpu_exec(&[ldr_x_reg(2, 0b110, 1, 3, 0)]);
    wr64(&mut m, D + 0x100 - 8, 0xBEEF);
    c.set_xn(3, D + 0x100);
    c.set_xn(2, (-1i32) as u32 as u64); // W2 = -1
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xBEEF, "LDR with SXTW negative index");
}

#[test]
fn ldr_w_reg_lsl2() {
    // LDR W0, [X3, X2, LSL #2] — option=011, S=1
    let (mut c, mut m) = cpu_exec(&[ldr_w_reg(2, 0b011, 1, 3, 0)]);
    wr32(&mut m, D + 3 * 4, 0xDEAD_BEEF);
    c.set_xn(3, D); c.set_xn(2, 3);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xDEAD_BEEF, "LDR W0, [X3, X2, LSL #2]");
}

#[test]
fn ldrb_reg_no_shift() {
    let (mut c, mut m) = cpu_exec(&[ldrb_reg(2, 0b011, 0, 3, 0)]);
    wr8(&mut m, D + 5, 0xAB);
    c.set_xn(3, D); c.set_xn(2, 5);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xAB, "LDRB [X3, X2]");
}

// ===================================================================
//  Extended register ADD/SUB — UXTB, UXTH, UXTW, SXTB, SXTH, SXTW
// ===================================================================

#[test]
fn add_ext_uxtb() {
    // ADD X0, X1, W2, UXTB — option=000
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 0, 0, 2, 0b000, 0, 1, 0)]);
    c.set_xn(1, 100); c.set_xn(2, 0x1FF); // UXTB: only low 8 bits = 0xFF
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 100 + 0xFF, "ADD with UXTB");
}

#[test]
fn add_ext_uxth() {
    // ADD X0, X1, W2, UXTH — option=001
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 0, 0, 2, 0b001, 0, 1, 0)]);
    c.set_xn(1, 100); c.set_xn(2, 0x1FFFF); // UXTH: low 16 bits = 0xFFFF
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 100 + 0xFFFF, "ADD with UXTH");
}

#[test]
fn add_ext_uxtw() {
    // ADD X0, X1, W2, UXTW — option=010
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 0, 0, 2, 0b010, 0, 1, 0)]);
    c.set_xn(1, 0); c.set_xn(2, 0x1_FFFF_FFFF); // UXTW: low 32 bits
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FFFF, "ADD with UXTW");
}

#[test]
fn add_ext_sxtb() {
    // ADD X0, X1, W2, SXTB — option=100
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 0, 0, 2, 0b100, 0, 1, 0)]);
    c.set_xn(1, 100); c.set_xn(2, 0x80); // SXTB: 0x80 → -128
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 100u64.wrapping_add((-128i64) as u64), "ADD with SXTB");
}

#[test]
fn add_ext_sxth() {
    // ADD X0, X1, W2, SXTH — option=101
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 0, 0, 2, 0b101, 0, 1, 0)]);
    c.set_xn(1, 1000); c.set_xn(2, 0x8000); // SXTH: 0x8000 → -32768
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 1000u64.wrapping_add((-32768i64) as u64), "ADD with SXTH");
}

#[test]
fn add_ext_sxtw() {
    // ADD X0, X1, W2, SXTW — option=110
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 0, 0, 2, 0b110, 0, 1, 0)]);
    c.set_xn(1, 100); c.set_xn(2, 0x8000_0000); // SXTW: MIN_i32
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 100u64.wrapping_add((-0x8000_0000i64) as u64), "ADD with SXTW");
}

#[test]
fn add_ext_uxtb_lsl2() {
    // ADD X0, X1, W2, UXTB #2 — extend then shift left by 2
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 0, 0, 2, 0b000, 2, 1, 0)]);
    c.set_xn(1, 0); c.set_xn(2, 0x10);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x40, "UXTB then LSL #2: 0x10 << 2 = 0x40");
}

#[test]
fn sub_ext_sxtw() {
    // SUB X0, X1, W2, SXTW
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 1, 0, 2, 0b110, 0, 1, 0)]);
    c.set_xn(1, 100); c.set_xn(2, 0xFFFF_FFFF); // SXTW: -1
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 101, "SUB with SXTW(-1) = ADD 1");
}

#[test]
fn subs_ext_sxtw_flags() {
    // CMP X1, W2, SXTW
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 1, 1, 2, 0b110, 0, 1, 31)]);
    c.set_xn(1, 0); c.set_xn(2, 0xFFFF_FFFF); // SXTW: -1
    c.step(&mut m).unwrap();
    // 0 - (-1) = 1
    assert!(!c.regs.n() && !c.regs.z(), "CMP 0, SXTW(-1)");
}

// ===================================================================
//  32-bit operations must zero-extend to 64 bits
// ===================================================================

#[test]
fn add_w_clears_upper32() {
    let (mut c, mut m) = cpu_exec(&[add_sub_reg(0, 0, 0, 0, 2, 0, 1, 0)]);
    c.set_xn(0, u64::MAX); // pre-fill with ones
    c.set_xn(1, 1); c.set_xn(2, 2);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 3, "32-bit ADD must clear upper 32 bits");
}

#[test]
fn sub_w_clears_upper32() {
    let (mut c, mut m) = cpu_exec(&[add_sub_reg(0, 1, 0, 0, 2, 0, 1, 0)]);
    c.set_xn(0, u64::MAX);
    c.set_xn(1, 10); c.set_xn(2, 3);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 7, "32-bit SUB must clear upper 32 bits");
}

#[test]
fn and_w_clears_upper32() {
    let (mut c, mut m) = cpu_exec(&[log_reg(0, 0b00, 0, 0, 2, 0, 1, 0)]);
    c.set_xn(0, u64::MAX);
    c.set_xn(1, 0x1_FFFF_FFFF); c.set_xn(2, 0x1_FFFF_FFFF);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FFFF, "32-bit AND must clear upper 32");
}

#[test]
fn orr_w_clears_upper32() {
    let (mut c, mut m) = cpu_exec(&[log_reg(0, 0b01, 0, 0, 2, 0, 1, 0)]);
    c.set_xn(0, u64::MAX);
    c.set_xn(1, 0); c.set_xn(2, 0x1_0000_0001);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 1, "32-bit ORR must clear upper 32");
}

#[test]
fn eor_w_clears_upper32() {
    let (mut c, mut m) = cpu_exec(&[log_reg(0, 0b10, 0, 0, 2, 0, 1, 0)]);
    c.set_xn(0, u64::MAX);
    c.set_xn(1, 0xAA); c.set_xn(2, 0xFF);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x55, "32-bit EOR must clear upper 32");
}

#[test]
fn ldr_w_clears_upper32() {
    let str_insn = (0b10u32 << 30) | (0b111001 << 24) | (0b00 << 22) | (3 << 5);
    let ldr_insn = (0b10u32 << 30) | (0b111001 << 24) | (0b01 << 22) | (3 << 5) | 1;
    let (mut c, mut m) = cpu_exec(&[str_insn, ldr_insn]);
    c.set_xn(0, 0xFFFF_FFFF); c.set_xn(1, u64::MAX); c.set_xn(3, D);
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0xFFFF_FFFF, "LDR W must zero-extend (upper 32 = 0)");
}

// ===================================================================
//  Rd=XZR should discard result (for all DP instructions)
// ===================================================================

#[test]
fn add_to_xzr_discards() {
    let (mut c, mut m) = cpu_exec(&[add_sub_reg(1, 0, 0, 0, 2, 0, 1, 31)]);
    c.set_xn(1, 100); c.set_xn(2, 200);
    let old_sp = c.regs.sp;
    c.step(&mut m).unwrap();
    // Non-S ADD with Rd=31 should write to SP (it's the ADD SP alias)
    // Actually for shifted register ADD, Rd=31 is XZR not SP
    // Only ADD immediate with Rd=31 uses SP
    assert_eq!(c.regs.sp, old_sp, "ADD reg Rd=31 should NOT touch SP");
}

#[test]
fn orr_to_xzr_discards() {
    let (mut c, mut m) = cpu_exec(&[log_reg(1, 0b01, 0, 0, 2, 0, 1, 31)]);
    c.set_xn(1, 42); c.set_xn(2, 99);
    let old_sp = c.regs.sp;
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.sp, old_sp, "ORR Rd=31 discards result");
}

// ===================================================================
//  MOVK 32-bit — must truncate result
// ===================================================================

#[test]
fn movk_w_must_truncate() {
    // MOVK W0, #0x1234 — result must be 32-bit zero-extended
    let (mut c, mut m) = cpu_exec(&[mov_wide(0, 0b11, 0, 0x1234, 0)]);
    c.set_xn(0, 0xFFFF_FFFF_FFFF_0000);
    c.step(&mut m).unwrap();
    // W0 should be (0xFFFF_0000 & mask) | 0x1234 = 0xFFFF_1234, zero-extended
    assert!(c.xn(0) <= 0xFFFF_FFFF, "MOVK W must produce 32-bit result");
}

#[test]
fn movk_w_hw0() {
    let (mut c, mut m) = cpu_exec(&[mov_wide(0, 0b11, 0, 0xABCD, 0)]);
    c.set_xn(0, 0x1234_0000);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x1234_ABCD, "MOVK W0, #0xABCD, hw=0");
}

#[test]
fn movk_w_hw1() {
    let (mut c, mut m) = cpu_exec(&[mov_wide(0, 0b11, 1, 0x5678, 0)]);
    c.set_xn(0, 0x0000_ABCD);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x5678_ABCD, "MOVK W0, #0x5678, LSL #16");
}

// ===================================================================
//  LDRSB pre-indexed (sign-extending, not just unscaled)
// ===================================================================

#[test]
fn ldrsb_x_pre_index_neg() {
    // LDRSB X0, [X3, #-1]! — pre-indexed sign-extending byte load
    // size=00 opc=10 0 imm9=-1 11 rn rt
    let insn = (0b00 << 30) | (0b111000 << 24) | (0b10 << 22) | (0 << 21)
             | (((-1i32 as u32) & 0x1FF) << 12) | (0b11 << 10) | (3 << 5) | 0;
    let (mut c, mut m) = cpu_exec(&[insn]);
    wr8(&mut m, D + 0xFF, 0x80);
    c.set_xn(3, D + 0x100);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FFFF_FFFF_FF80, "LDRSB X pre-indexed sign-extends");
    assert_eq!(c.xn(3), D + 0xFF, "base updated");
}

#[test]
fn ldrsw_post_index() {
    // LDRSW X0, [X3], #4 — post-indexed sign-extending word load
    let insn = (0b10 << 30) | (0b111000 << 24) | (0b10 << 22) | (0 << 21)
             | (4 << 12) | (0b01 << 10) | (3 << 5) | 0;
    let (mut c, mut m) = cpu_exec(&[insn]);
    wr32(&mut m, D, 0x8000_0001);
    c.set_xn(3, D);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FFFF_8000_0001, "LDRSW post-indexed sign-extends");
    assert_eq!(c.xn(3), D + 4, "base updated by post-index");
}

#[test]
fn ldrsh_x_unscaled_neg() {
    // LDURSH X0, [X3, #-2]
    let insn = (0b01 << 30) | (0b111000 << 24) | (0b10 << 22) | (0 << 21)
             | (((-2i32 as u32) & 0x1FF) << 12) | (0b00 << 10) | (3 << 5) | 0;
    let (mut c, mut m) = cpu_exec(&[insn]);
    wr16(&mut m, D + 0x100 - 2, 0xFFFE);
    c.set_xn(3, D + 0x100);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FFFF_FFFF_FFFE, "LDURSH X sign-extends -2");
}

// ===================================================================
//  Register-offset loads with SXTW for all sizes
// ===================================================================

#[test]
fn ldrb_reg_sxtw() {
    // LDRB W0, [X3, W2, SXTW] — option=110, S=0
    let insn = (0b00 << 30) | (0b111000 << 24) | (0b01 << 22) | (1 << 21)
             | (2 << 16) | (0b110 << 13) | (0 << 12) | (0b10 << 10) | (3 << 5) | 0;
    let (mut c, mut m) = cpu_exec(&[insn]);
    wr8(&mut m, D + 0x100 - 1, 0xAB);
    c.set_xn(3, D + 0x100);
    c.set_xn(2, (-1i32) as u32 as u64);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xAB, "LDRB with SXTW negative index");
}

#[test]
fn ldrh_reg_lsl1() {
    // LDRH W0, [X3, X2, LSL #1] — option=011, S=1
    let insn = (0b01 << 30) | (0b111000 << 24) | (0b01 << 22) | (1 << 21)
             | (2 << 16) | (0b011 << 13) | (1 << 12) | (0b10 << 10) | (3 << 5) | 0;
    let (mut c, mut m) = cpu_exec(&[insn]);
    wr16(&mut m, D + 3 * 2, 0xBEEF);
    c.set_xn(3, D); c.set_xn(2, 3);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xBEEF, "LDRH with LSL #1");
}

#[test]
fn strh_reg_uxtw() {
    // STRH W0, [X3, W2, UXTW] — option=010, S=0
    let insn = (0b01 << 30) | (0b111000 << 24) | (0b00 << 22) | (1 << 21)
             | (2 << 16) | (0b010 << 13) | (0 << 12) | (0b10 << 10) | (3 << 5) | 0;
    let (mut c, mut m) = cpu_exec(&[insn]);
    c.set_xn(0, 0x1234); c.set_xn(3, D); c.set_xn(2, 10);
    c.step(&mut m).unwrap();
    let mut b = [0u8; 2];
    m.read(D + 10, &mut b).unwrap();
    assert_eq!(u16::from_le_bytes(b), 0x1234);
}

// ===================================================================
//  Extended register ADD/SUB with shift amounts 1-4
// ===================================================================

#[test]
fn add_ext_uxtb_lsl1() {
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 0, 0, 2, 0b000, 1, 1, 0)]);
    c.set_xn(1, 0); c.set_xn(2, 0x80);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x100, "UXTB then LSL #1");
}

#[test]
fn add_ext_sxtw_lsl3() {
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 0, 0, 2, 0b110, 3, 1, 0)]);
    c.set_xn(1, 0x1000); c.set_xn(2, (-1i32) as u32 as u64);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x1000u64.wrapping_add((-8i64) as u64), "SXTW(-1) << 3 = -8");
}

#[test]
fn sub_ext_uxtb_lsl0() {
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 1, 0, 2, 0b000, 0, 1, 0)]);
    c.set_xn(1, 0x200); c.set_xn(2, 0x1FF); // UXTB → 0xFF
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x200 - 0xFF);
}

#[test]
fn subs_ext_uxtw_flags_eq() {
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 1, 1, 2, 0b010, 0, 1, 31)]);
    c.set_xn(1, 42); c.set_xn(2, 42);
    c.step(&mut m).unwrap();
    assert!(c.regs.z(), "CMP X1, W2, UXTW → Z when equal");
    assert!(c.regs.c(), "CMP no borrow → C");
}

#[test]
fn adds_ext_sxtb_overflow() {
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 0, 1, 2, 0b100, 0, 1, 0)]);
    c.set_xn(1, i64::MAX as u64); c.set_xn(2, 1); // SXTB(1) = 1
    c.step(&mut m).unwrap();
    assert!(c.regs.v(), "ADDS overflow: MAX + 1");
    assert!(c.regs.n(), "result is negative");
}

// ===================================================================
//  32-bit EXTR (ROR alias) — verify truncation
// ===================================================================

#[test]
fn extr_32_ror_4() {
    // ROR W0, W1, #4 = EXTR W0, W1, W1, #4
    let insn = (0 << 31) | (0b00 << 29) | (0b100111 << 23) | (0 << 22) | (1 << 16) | (4 << 10) | (1 << 5) | 0;
    let (mut c, mut m) = cpu_exec(&[insn]);
    c.set_xn(1, 0x1_0000_000F); // upper bits should be ignored for W
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xF000_0000, "EXTR W (ROR) truncates to 32-bit");
}

#[test]
fn extr_32_ror_16() {
    let insn = (0 << 31) | (0b00 << 29) | (0b100111 << 23) | (0 << 22) | (1 << 16) | (16 << 10) | (1 << 5) | 0;
    let (mut c, mut m) = cpu_exec(&[insn]);
    c.set_xn(1, 0xAABB_CCDD);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xCCDD_AABB);
}

// ===================================================================
//  Logical immediate with SP as destination (AND/ORR Rd=31 → SP)
// ===================================================================

#[test]
fn orr_imm_to_sp() {
    // ORR SP, X1, #0xFF (non-ANDS, so Rd=31 means SP)
    let insn = (1 << 31) | (0b01 << 29) | (0b100100 << 23) | (1 << 22) | (0 << 16) | (7 << 10) | (1 << 5) | 31;
    let (mut c, mut m) = cpu_exec(&[insn]);
    c.set_xn(1, 0x7FFF_0000);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.sp, 0x7FFF_00FF, "ORR to SP: Rd=31 is SP not XZR");
}

#[test]
fn and_imm_to_sp() {
    // AND SP, X1, #0xFFFFF000 (N=1, immr=0, imms=51)
    let insn = (1 << 31) | (0b00 << 29) | (0b100100 << 23) | (1 << 22) | (52 << 16) | (51 << 10) | (1 << 5) | 31;
    let (mut c, mut m) = cpu_exec(&[insn]);
    c.set_xn(1, 0x7FFF_8ABC);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.sp, 0x7FFF_8000, "AND to SP aligns address");
}

// ===================================================================
//  Conditional branch backward (negative offset)
// ===================================================================

#[test]
fn b_cond_backward() {
    // B.AL .-8 (always taken, backward by 2 insns)
    // imm19 for offset -8: (-8/4) = -2 → 0x7FFFE as 19-bit
    let imm19 = ((-2i32) as u32) & 0x7FFFF;
    let insn = (0b01010100 << 24) | (imm19 << 5) | 14; // AL=14
    let (mut c, mut m) = cpu_exec(&[0xD503201F, 0xD503201F, insn]); // NOP, NOP, B.AL .-8
    c.regs.pc = 0x40_0008; // start at 3rd insn
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, 0x40_0000, "B.AL backward");
}

// ===================================================================
//  TBZ/TBNZ with high bit numbers (bit 32-63 via b5=1)
// ===================================================================

#[test]
fn tbz_bit32_taken() {
    // TBZ X0, #32, +8 → b5=1, b40=0
    let insn = (1 << 31) | (0b011011_0 << 24) | (0 << 19) | (2 << 5) | 0;
    let (mut c, mut m) = cpu_exec(&[insn, 0xD503201F, 0xD503201F]);
    c.set_xn(0, 0xFFFF_FFFE_FFFF_FFFF); // bit 32 is 0
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, 0x40_0008, "TBZ bit 32 taken when clear");
}

#[test]
fn tbnz_bit48() {
    let insn = (1 << 31) | (0b011011_1 << 24) | (16 << 19) | (2 << 5) | 0;
    let (mut c, mut m) = cpu_exec(&[insn, 0xD503201F, 0xD503201F]);
    c.set_xn(0, 1u64 << 48);
    c.step(&mut m).unwrap();
    assert_eq!(c.regs.pc, 0x40_0008, "TBNZ bit 48 taken when set");
}

// ===================================================================
//  CMP extended register (commonly used in musl)
// ===================================================================

#[test]
fn cmp_ext_sxtw_equal() {
    // CMP X1, W2, SXTW = SUBS XZR, X1, W2, SXTW
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 1, 1, 2, 0b110, 0, 1, 31)]);
    c.set_xn(1, 0xFFFF_FFFF_FFFF_FFFF); // -1 as i64
    c.set_xn(2, 0xFFFF_FFFF); // -1 as i32, SXTW → -1 as i64
    c.step(&mut m).unwrap();
    assert!(c.regs.z(), "CMP X, W SXTW: -1 == -1 → Z");
}

#[test]
fn cmp_ext_sxtw_less() {
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 1, 1, 2, 0b110, 0, 1, 31)]);
    c.set_xn(1, 0); c.set_xn(2, 1);
    c.step(&mut m).unwrap();
    assert!(c.regs.n(), "CMP 0, SXTW(1): 0-1 is negative");
    assert!(!c.regs.c(), "CMP 0, 1: borrow → C=0");
}

#[test]
fn cmp_ext_uxtb() {
    let (mut c, mut m) = cpu_exec(&[add_sub_ext(1, 1, 1, 2, 0b000, 0, 1, 31)]);
    c.set_xn(1, 0xFF); c.set_xn(2, 0x1FF); // UXTB(0x1FF) = 0xFF
    c.step(&mut m).unwrap();
    assert!(c.regs.z(), "CMP 0xFF, UXTB(0x1FF) → equal");
}

// ===================================================================
//  MADD/MSUB with ra=rd (accumulate into destination)
// ===================================================================

#[test]
fn madd_ra_eq_rd() {
    // MADD X0, X1, X2, X0 — X0 = X0 + X1*X2
    let insn = (1 << 31) | (0b0011011 << 24) | (2 << 16) | (0 << 15) | (0 << 10) | (1 << 5) | 0;
    let (mut c, mut m) = cpu_exec(&[insn]);
    c.set_xn(0, 100); c.set_xn(1, 5); c.set_xn(2, 6);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 130, "MADD X0, X1, X2, X0: 100 + 5*6 = 130");
}

// ===================================================================
//  Shift by register width (mod behavior)
// ===================================================================

#[test]
fn lslv_64_by65_is_mod64() {
    let insn = (1 << 31) | (0b0011010110 << 21) | (2 << 16) | (0b001000 << 10) | (1 << 5) | 0;
    let (mut c, mut m) = cpu_exec(&[insn]);
    c.set_xn(1, 1); c.set_xn(2, 65);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 2, "LSLV shift 65 mod 64 = shift 1");
}

#[test]
fn lslv_32_by33_is_mod32() {
    let insn = (0 << 31) | (0b0011010110 << 21) | (2 << 16) | (0b001000 << 10) | (1 << 5) | 0;
    let (mut c, mut m) = cpu_exec(&[insn]);
    c.set_xn(1, 1); c.set_xn(2, 33);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 2, "LSLV W shift 33 mod 32 = shift 1");
}

#[test]
fn lsrv_64_by128_is_mod64() {
    let insn = (1 << 31) | (0b0011010110 << 21) | (2 << 16) | (0b001001 << 10) | (1 << 5) | 0;
    let (mut c, mut m) = cpu_exec(&[insn]);
    c.set_xn(1, 0x100); c.set_xn(2, 128);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x100, "LSRV shift 128 mod 64 = shift 0 → unchanged");
}

// ===================================================================
//  CLREX — should be a NOP in SE mode
// ===================================================================

#[test]
fn clrex_is_nop() {
    let (mut c, mut m) = cpu_exec(&[0xD503_305F]); // CLREX
    c.set_xn(0, 42);
    set_flags(&mut c, true, false, true, false);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 42, "CLREX doesn't modify registers");
    assert!(c.regs.n() && !c.regs.z() && c.regs.c() && !c.regs.v(), "CLREX preserves flags");
}

// ===================================================================
//  Stores to XZR read as 0
// ===================================================================

#[test]
fn str_xzr_stores_zero() {
    let insn = (0b11 << 30) | (0b111001 << 24) | (0b00 << 22) | (3 << 5) | 31;
    let (mut c, mut m) = cpu_exec(&[insn]);
    wr64(&mut m, D, 0xFFFF_FFFF_FFFF_FFFF);
    c.set_xn(3, D);
    c.step(&mut m).unwrap();
    assert_eq!(rd64(&m, D), 0, "STR XZR stores 0");
}

// ===================================================================
//  ADDS/SUBS with shifted register at large shift amounts
// ===================================================================

#[test]
fn adds_reg_lsl63() {
    let (mut c, mut m) = cpu_exec(&[add_sub_reg(1, 0, 1, 0b00, 2, 63, 1, 0)]);
    c.set_xn(1, 0); c.set_xn(2, 1);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 1u64 << 63, "ADD with LSL #63");
    assert!(c.regs.n(), "result has MSB set → N");
}

#[test]
fn subs_reg_asr_63() {
    let (mut c, mut m) = cpu_exec(&[add_sub_reg(1, 1, 1, 0b10, 2, 63, 1, 31)]);
    c.set_xn(1, 0); c.set_xn(2, 0x8000_0000_0000_0000);
    c.step(&mut m).unwrap();
    // ASR 63 of negative → -1, so CMP 0, -1 → 0-(-1) = 1
    assert!(!c.regs.n(), "0 - (-1) = 1, positive");
    assert!(!c.regs.z());
}

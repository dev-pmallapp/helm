//! TDD Stage 0 — AArch64 decoder tests.
//!
//! Each test encodes a real A64 instruction as a u32 and verifies
//! the decoder produces the correct MicroOp(s).
//!
//! Tests marked `#[ignore]` are the TDD "red" tests — they define
//! the behaviour we need to implement.  Remove `#[ignore]` as each
//! instruction is implemented.

use crate::arm::aarch64::Aarch64Decoder;
use helm_core::ir::Opcode;

fn decode_one(insn: u32) -> Vec<helm_core::ir::MicroOp> {
    let dec = Aarch64Decoder::new();
    dec.decode_insn(0x1000, insn).unwrap()
}

// -- Data processing (immediate) -----------------------------------------

#[test]
#[ignore] // TDD red: implement ADD (immediate)
fn decode_add_imm() {
    // ADD X0, X1, #42  =>  0x91_00A8_20
    let uops = decode_one(0x9100A820);
    assert_eq!(uops[0].opcode, Opcode::IntAlu);
    assert_eq!(uops[0].immediate, Some(42));
}

#[test]
#[ignore] // TDD red
fn decode_sub_imm() {
    // SUB X0, X1, #1  =>  0xD1_0004_20
    let uops = decode_one(0xD1000420);
    assert_eq!(uops[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_movz() {
    // MOVZ X0, #0x1234  =>  0xD2_8246_80
    let uops = decode_one(0xD2824680);
    assert_eq!(uops[0].opcode, Opcode::IntAlu);
    assert_eq!(uops[0].immediate, Some(0x1234));
}

#[test]
#[ignore]
fn decode_movk() {
    // MOVK X0, #0x5678, LSL #16  =>  0xF2_A0AC_F0
    let uops = decode_one(0xF2A0ACF0);
    assert_eq!(uops[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_cmp_imm() {
    // CMP X1, #0  (alias: SUBS XZR, X1, #0)  =>  0xF1_0000_3F
    let uops = decode_one(0xF100003F);
    assert_eq!(uops[0].opcode, Opcode::IntAlu);
}

// -- Branches ------------------------------------------------------------

#[test]
#[ignore]
fn decode_b_imm() {
    // B #0x100  =>  0x14_0000_40
    let uops = decode_one(0x14000040);
    assert_eq!(uops[0].opcode, Opcode::Branch);
    assert!(uops[0].flags.is_branch);
}

#[test]
#[ignore]
fn decode_bl() {
    // BL #0x100  =>  0x94_0000_40
    let uops = decode_one(0x94000040);
    assert_eq!(uops[0].opcode, Opcode::Branch);
    assert!(uops[0].flags.is_call);
}

#[test]
#[ignore]
fn decode_ret() {
    // RET (BR X30)  =>  0xD6_5F03_C0
    let uops = decode_one(0xD65F03C0);
    assert_eq!(uops[0].opcode, Opcode::Branch);
    assert!(uops[0].flags.is_return);
}

#[test]
#[ignore]
fn decode_b_cond_eq() {
    // B.EQ #0x10  =>  0x54_0000_80
    let uops = decode_one(0x54000080);
    assert_eq!(uops[0].opcode, Opcode::CondBranch);
}

#[test]
#[ignore]
fn decode_cbz() {
    // CBZ X0, #0x10  =>  0xB4_0000_80
    let uops = decode_one(0xB4000080);
    assert_eq!(uops[0].opcode, Opcode::CondBranch);
}

// -- Loads and stores ----------------------------------------------------

#[test]
#[ignore]
fn decode_ldr_imm() {
    // LDR X0, [X1, #8]  =>  0xF9_4004_20
    let uops = decode_one(0xF9400420);
    assert_eq!(uops[0].opcode, Opcode::Load);
}

#[test]
#[ignore]
fn decode_str_imm() {
    // STR X0, [X1, #8]  =>  0xF9_0004_20
    let uops = decode_one(0xF9000420);
    assert_eq!(uops[0].opcode, Opcode::Store);
}

#[test]
#[ignore]
fn decode_ldp() {
    // LDP X0, X1, [SP, #16]  =>  0xA9_4107_E0
    let uops = decode_one(0xA94107E0);
    assert_eq!(uops[0].opcode, Opcode::Load);
}

#[test]
#[ignore]
fn decode_stp() {
    // STP X0, X1, [SP, #-16]!  =>  0xA9_BF07_E0
    let uops = decode_one(0xA9BF07E0);
    assert_eq!(uops[0].opcode, Opcode::Store);
}

#[test]
#[ignore]
fn decode_adrp() {
    // ADRP X0, #0x1000  =>  0x90_0000_00  (page-relative)
    let uops = decode_one(0x90000000);
    assert_eq!(uops[0].opcode, Opcode::IntAlu);
}

// -- System --------------------------------------------------------------

#[test]
#[ignore]
fn decode_svc() {
    // SVC #0  =>  0xD4_0000_01
    let uops = decode_one(0xD4000001);
    assert_eq!(uops[0].opcode, Opcode::Syscall);
}

#[test]
#[ignore]
fn decode_nop() {
    // NOP  =>  0xD5_0320_1F
    let uops = decode_one(0xD503201F);
    assert_eq!(uops[0].opcode, Opcode::Nop);
}

#[test]
#[ignore]
fn decode_mrs_tpidr() {
    // MRS X0, TPIDR_EL0  =>  0xD5_3BD0_40
    let uops = decode_one(0xD53BD040);
    // Should produce an ALU-like op that reads the system register.
    assert!(!uops.is_empty());
}

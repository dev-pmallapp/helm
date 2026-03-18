//! AArch64 decoder tests.
//!
//! Each test encodes a known instruction and verifies the decoder produces
//! the correct `Opcode` and operand fields.

use helm_arch::aarch64::decode::decode;
use helm_arch::aarch64::insn::Opcode;

fn dec(raw: u32) -> helm_arch::aarch64::insn::Instruction {
    decode(raw, 0x1000).expect(&format!("decode failed for {raw:#010x}"))
}

// ── Data processing immediate ──────────────────────────────────────────────────

#[test]
fn decode_movz() {
    // MOVZ X0, #0x1234
    let i = dec(0xD2824680);
    assert_eq!(i.opcode, Opcode::Movz);
    assert_eq!(i.rd, 0);
}

#[test]
fn decode_movk() {
    // MOVK X0, #0x5678, LSL#16
    let i = dec(0xF2AACF00);
    assert_eq!(i.opcode, Opcode::Movk);
    assert_eq!(i.rd, 0);
}

#[test]
fn decode_add_imm() {
    // ADD X1, X0, #4
    let i = dec(0x91001001);
    assert_eq!(i.opcode, Opcode::AddImm);
    assert_eq!(i.rd, 1);
    assert_eq!(i.rn, 0);
    assert_eq!(i.imm, 4);
}

#[test]
fn decode_sub_imm() {
    // SUB SP, SP, #0x10
    let i = dec(0xD10043FF);
    assert_eq!(i.opcode, Opcode::SubImm);
    assert_eq!(i.rd, 31); // SP
    assert_eq!(i.rn, 31);
    assert_eq!(i.imm, 0x10);
}

#[test]
fn decode_adr() {
    // ADR X0, #0
    let i = dec(0x10000000);
    assert_eq!(i.opcode, Opcode::Adr);
    assert_eq!(i.rd, 0);
}

#[test]
fn decode_adrp() {
    // ADRP X0, #0
    let i = dec(0x90000000);
    assert_eq!(i.opcode, Opcode::Adrp);
    assert_eq!(i.rd, 0);
}

#[test]
fn decode_and_imm() {
    // AND X0, X1, #0xFF
    let i = dec(0x92401C20);
    assert_eq!(i.opcode, Opcode::AndImm);
    assert_eq!(i.rd, 0);
    assert_eq!(i.rn, 1);
}

#[test]
fn decode_sbfm_asr() {
    // ASR X0, X1, #3 → SBFM X0, X1, #3, #63
    let i = dec(0x9343FC20);
    assert_eq!(i.opcode, Opcode::Sbfm);
    assert_eq!(i.rd, 0);
    assert_eq!(i.rn, 1);
}

// ── Branches ───────────────────────────────────────────────────────────────────

#[test]
fn decode_b() {
    // B #0 (branch to self)
    let i = dec(0x14000000);
    assert_eq!(i.opcode, Opcode::B);
    assert_eq!(i.imm, 0);
}

#[test]
fn decode_bl() {
    // BL #4
    let i = dec(0x94000001);
    assert_eq!(i.opcode, Opcode::Bl);
    assert_eq!(i.imm, 4);
}

#[test]
fn decode_b_cond() {
    // B.EQ #8
    let i = dec(0x54000040);
    assert_eq!(i.opcode, Opcode::BCond);
    assert_eq!(i.cond, 0); // EQ
    assert_eq!(i.imm, 8);
}

#[test]
fn decode_cbz() {
    // CBZ X0, #0
    let i = dec(0xB4000000);
    assert_eq!(i.opcode, Opcode::Cbz);
    assert_eq!(i.rd, 0);
}

#[test]
fn decode_ret() {
    // RET (X30)
    let i = dec(0xD65F03C0);
    assert_eq!(i.opcode, Opcode::Ret);
    assert_eq!(i.rn, 30);
}

#[test]
fn decode_svc() {
    // SVC #0
    let i = dec(0xD4000001);
    assert_eq!(i.opcode, Opcode::Svc);
}

// ── Load/Store ─────────────────────────────────────────────────────────────────

#[test]
fn decode_ldr_imm() {
    // LDR X0, [X1, #8]
    let i = dec(0xF9400420);
    assert_eq!(i.opcode, Opcode::Ldr);
    assert_eq!(i.rd, 0);
    assert_eq!(i.rn, 1);
    assert_eq!(i.imm, 8);
}

#[test]
fn decode_str_imm() {
    // STR X0, [SP, #0]
    let i = dec(0xF90003E0);
    assert_eq!(i.opcode, Opcode::Str);
    assert_eq!(i.rd, 0);
    assert_eq!(i.rn, 31);
}

#[test]
fn decode_ldp() {
    // LDP X0, X1, [SP]
    let i = dec(0xA94007E0);
    assert_eq!(i.opcode, Opcode::Ldp);
    assert_eq!(i.rd, 0);
    assert_eq!(i.pair_second, 1);
}

#[test]
fn decode_stp_pre_index() {
    // STP X29, X30, [SP, #-16]!
    let i = dec(0xA9BF7BFD);
    assert_eq!(i.opcode, Opcode::Stp);
    assert!(i.pre_index);
    assert_eq!(i.rd, 29);
    assert_eq!(i.pair_second, 30);
}

#[test]
fn decode_ldr_literal() {
    // LDR X0, label (imm19 = 1 → offset = 4)
    let i = dec(0x18000020);
    assert_eq!(i.opcode, Opcode::LdrLit);
    assert_eq!(i.imm, 4);
}

#[test]
fn decode_ldr_reg_offset() {
    // LDR X3, [X19, X2, LSL #3]
    let i = dec(0xF8627A63);
    assert_eq!(i.opcode, Opcode::Ldr);
    assert_eq!(i.rd, 3);
    assert_eq!(i.rn, 19);
    assert_eq!(i.rm, 2);
}

#[test]
fn decode_prfm() {
    // PRFM PLDL1KEEP, [X0]
    let i = dec(0xF9800000);
    assert_eq!(i.opcode, Opcode::Prfm);
}

// ── SIMD / FP load/store ───────────────────────────────────────────────────────

#[test]
fn decode_str_q0() {
    // STR Q0, [X0] (128-bit SIMD store, unsigned offset)
    let i = dec(0x3D800000);
    assert_eq!(i.opcode, Opcode::StrSimd);
}

#[test]
fn decode_ldr_q() {
    // LDR Q0, [X0, #16]
    let i = dec(0x3DC00400);
    assert_eq!(i.opcode, Opcode::LdrSimd);
}

#[test]
fn decode_stp_q() {
    // STP Q0, Q1, [SP]
    let i = dec(0xAD0007E0);
    assert_eq!(i.opcode, Opcode::StpSimd);
}

// ── Data processing register ───────────────────────────────────────────────────

#[test]
fn decode_add_reg() {
    // ADD X0, X1, X2
    let i = dec(0x8B020020);
    assert_eq!(i.opcode, Opcode::AddReg);
    assert_eq!(i.rd, 0);
    assert_eq!(i.rn, 1);
    assert_eq!(i.rm, 2);
}

#[test]
fn decode_madd() {
    // MADD X0, X1, X2, X3 → MUL X0, X1, X2 when Ra=XZR
    let i = dec(0x9B020C20);
    assert_eq!(i.opcode, Opcode::Madd);
    assert_eq!(i.rd, 0);
    assert_eq!(i.rn, 1);
    assert_eq!(i.rm, 2);
}

#[test]
fn decode_udiv() {
    // UDIV X0, X1, X2
    let i = dec(0x9AC20820);
    assert_eq!(i.opcode, Opcode::Udiv);
}

#[test]
fn decode_csel() {
    // CSEL X0, X1, X2, EQ
    let i = dec(0x9A820020);
    assert_eq!(i.opcode, Opcode::Csel);
    assert_eq!(i.cond, 0); // EQ
}

// ── SIMD ───────────────────────────────────────────────────────────────────────

#[test]
fn decode_dup_16b() {
    // DUP V0.16B, W1
    let i = dec(0x4E010C20);
    assert_eq!(i.opcode, Opcode::SimdDup);
}

// ── System ─────────────────────────────────────────────────────────────────────

#[test]
fn decode_nop() {
    let i = dec(0xD503201F);
    assert_eq!(i.opcode, Opcode::Nop);
}

#[test]
fn decode_mrs_tpidr() {
    // MRS X0, TPIDR_EL0
    let i = dec(0xD53BD040);
    assert_eq!(i.opcode, Opcode::Mrs);
    assert_eq!(i.rd, 0);
}

// ── Undefined ──────────────────────────────────────────────────────────────────

#[test]
fn decode_undefined() {
    // An encoding that should not be valid
    assert!(decode(0x00000000, 0).is_err());
}

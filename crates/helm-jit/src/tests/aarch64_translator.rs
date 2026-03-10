//! Tests for A64JitTranslator.

use crate::aarch64_translator::A64JitTranslator;
use helm_core::insn::{DecodedInsn, InsnClass, InsnFlags};
use helm_core::jit::JitTranslator;

fn make_insn(pc: u64, encoding: u32) -> DecodedInsn {
    let mut insn = DecodedInsn {
        pc,
        len: 4,
        class: InsnClass::IntAlu,
        ..DecodedInsn::default()
    };
    insn.encoding_bytes[..4].copy_from_slice(&encoding.to_le_bytes());
    insn
}

#[test]
fn translate_single_add() {
    let mut t = A64JitTranslator::new();
    // ADD X0, X1, X2 = 0x8B020020
    let insn = make_insn(0x1000, 0x8B020020);
    let ends = t.translate_one(&insn);
    // ADD doesn't end a block
    assert!(!ends);
    // Context should have ops
    assert!(!t.context().ops().is_empty());
}

#[test]
fn translate_branch_ends_block() {
    let mut t = A64JitTranslator::new();
    // B #4 = 0x14000001
    let insn = make_insn(0x1000, 0x14000001);
    let ends = t.translate_one(&insn);
    assert!(ends, "branch should end block");
}

#[test]
fn translate_block_multiple_insns() {
    let mut t = A64JitTranslator::new();
    let insns = vec![
        make_insn(0x1000, 0x8B020020), // ADD X0, X1, X2
        make_insn(0x1004, 0xCB030041), // SUB X1, X2, X3
        make_insn(0x1008, 0x14000001), // B #4 (ends block)
    ];

    let block = t.translate_block(&insns, 0x1000);
    assert_eq!(block.pc, 0x1000);
    assert_eq!(block.insn_count, 3);
    assert_eq!(block.end_pc, 0x100C);
}

#[test]
fn translate_block_stops_at_branch() {
    let mut t = A64JitTranslator::new();
    let insns = vec![
        make_insn(0x1000, 0x8B020020), // ADD
        make_insn(0x1004, 0x14000001), // B (ends block here)
        make_insn(0x1008, 0x8B020020), // ADD (should not be translated)
    ];

    let block = t.translate_block(&insns, 0x1000);
    assert_eq!(block.insn_count, 2, "should stop at branch");
    assert_eq!(block.end_pc, 0x1008);
}

#[test]
fn translate_movz() {
    let mut t = A64JitTranslator::new();
    // MOVZ X0, #42 = 0xD2800540
    let insn = make_insn(0x1000, 0xD2800540);
    let ends = t.translate_one(&insn);
    assert!(!ends);
}

#[test]
fn translate_ldr_str() {
    let mut t = A64JitTranslator::new();
    // STR X0, [X1] = 0xF9000020
    let insn = make_insn(0x1000, 0xF9000020);
    let ends = t.translate_one(&insn);
    assert!(!ends);

    // LDR X0, [X1] = 0xF9400020
    let insn = make_insn(0x1004, 0xF9400020);
    let ends = t.translate_one(&insn);
    assert!(!ends);
}

#[test]
fn translate_ret_ends_block() {
    let mut t = A64JitTranslator::new();
    // RET = 0xD65F03C0
    let insn = make_insn(0x1000, 0xD65F03C0);
    let ends = t.translate_one(&insn);
    assert!(ends, "RET should end block");
}

#[test]
fn translate_svc_ends_block() {
    let mut t = A64JitTranslator::new();
    // SVC #0 = 0xD4000001
    let insn = make_insn(0x1000, 0xD4000001);
    let ends = t.translate_one(&insn);
    assert!(ends, "SVC should end block");
}

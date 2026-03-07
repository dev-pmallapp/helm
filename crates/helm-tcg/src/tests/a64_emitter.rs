use crate::a64_emitter::{A64TcgEmitter, TranslateAction};
use crate::context::TcgContext;

fn translate(insn: u32) -> TranslateAction {
    let mut ctx = TcgContext::new();
    let mut emitter = A64TcgEmitter::new(&mut ctx, 0x1000);
    emitter.translate_insn(insn)
}

#[test]
fn translate_nop_ends_or_continues() {
    let action = translate(0xD503201F);
    assert!(
        matches!(action, TranslateAction::Continue | TranslateAction::EndBlock),
        "NOP should be handled"
    );
}

#[test]
fn translate_add_imm_continues() {
    // ADD X0, X1, #1
    let action = translate(0x91000420);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_sub_imm_continues() {
    // SUB X0, X1, #1
    let action = translate(0xD1000420);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_b_ends_block() {
    // B #0
    let action = translate(0x14000000);
    assert_eq!(action, TranslateAction::EndBlock);
}

#[test]
fn translate_bl_ends_block() {
    // BL #0
    let action = translate(0x94000000);
    assert_eq!(action, TranslateAction::EndBlock);
}

#[test]
fn translate_ret_ends_block() {
    // RET
    let action = translate(0xD65F03C0);
    assert_eq!(action, TranslateAction::EndBlock);
}

#[test]
fn translate_ldr_continues() {
    // LDR X0, [X1]
    let action = translate(0xF9400020);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_str_continues() {
    // STR X0, [X1]
    let action = translate(0xF9000020);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_svc_ends_block() {
    // SVC #0
    let action = translate(0xD4000001);
    assert_eq!(action, TranslateAction::EndBlock);
}

#[test]
fn translate_simd_is_unhandled() {
    // FADD S0, S1, S2
    let action = translate(0x1E222820);
    assert_eq!(action, TranslateAction::Unhandled);
}

#[test]
fn translate_reserved_encoding_unhandled() {
    let action = translate(0x00000000);
    assert_eq!(action, TranslateAction::Unhandled);
}

#[test]
fn emitter_produces_ops() {
    let mut ctx = TcgContext::new();
    {
        let mut emitter = A64TcgEmitter::new(&mut ctx, 0x1000);
        emitter.translate_insn(0x91000420); // ADD X0, X1, #1
    }
    let ops = ctx.finish();
    assert!(!ops.is_empty());
}

#[test]
fn translate_dp_reg_continues() {
    // ADD X0, X1, X2 (shifted register)
    let action = translate(0x8B020020);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_cbz_ends_block() {
    // CBZ X0, #0
    let action = translate(0xB4000000);
    assert_eq!(action, TranslateAction::EndBlock);
}

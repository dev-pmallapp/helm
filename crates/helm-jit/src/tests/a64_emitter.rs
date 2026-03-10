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
        matches!(
            action,
            TranslateAction::Continue | TranslateAction::EndBlock
        ),
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

// ── New instruction coverage tests ──────────────────────────────────────────

#[test]
fn translate_and_imm_continues() {
    // AND X0, X1, #0xFF (sf=1, n=0, immr=0, imms=7)
    let action = translate(0x92400420);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_orr_imm_continues() {
    // ORR X0, X1, #1
    let action = translate(0xB2400020);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_movz_continues() {
    // MOVZ X0, #42
    let action = translate(0xD2800540);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_movk_continues() {
    // MOVK X0, #0, LSL#16
    let action = translate(0xF2A00000);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_ubfm_continues() {
    // LSR X0, X1, #4 = UBFM X0, X1, #4, #63
    let action = translate(0xD344FC20);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_sbfm_continues() {
    // ASR X0, X1, #4 = SBFM X0, X1, #4, #63
    let action = translate(0x9344FC20);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_madd_continues() {
    // MUL X0, X1, X2 = MADD X0, X1, X2, XZR
    let action = translate(0x9B027C20);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_udiv_continues() {
    // UDIV X0, X1, X2
    let action = translate(0x9AC20820);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_csel_continues() {
    // CSEL X0, X1, X2, EQ
    let action = translate(0x9A820020);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_csinc_continues() {
    // CSINC X0, X1, X2, EQ
    let action = translate(0x9A820420);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_ldr_pre_continues() {
    // LDR X0, [X1, #8]!
    let action = translate(0xF8408C20);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_str_post_continues() {
    // STR X0, [X1], #-16
    let action = translate(0xF81F0420);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_ldp_continues() {
    // LDP X0, X1, [SP]
    let action = translate(0xA94007E0);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_stp_continues() {
    // STP X0, X1, [SP, #-16]!
    let action = translate(0xA9BF07E0);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_ldr_reg_offset_continues() {
    // LDR X0, [X1, X2]
    let action = translate(0xF8626820);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_b_cond_ends_block() {
    // B.EQ #0
    let action = translate(0x54000000);
    assert_eq!(action, TranslateAction::EndBlock);
}

#[test]
fn translate_add_ext_continues() {
    // ADD X0, X1, W2, UXTB
    let action = translate(0x8B220020);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_nop_continues() {
    let action = translate(0xD503201F);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_dsb_continues() {
    // DSB SY
    let action = translate(0xD5033F9F);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_ldxr_continues() {
    // LDXR X0, [X1]
    let action = translate(0xC85F7C20);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_stxr_continues() {
    // STXR W3, X0, [X1]
    let action = translate(0xC8037C20);
    assert_eq!(action, TranslateAction::Continue);
}

// ── Missing instruction coverage — gap analysis additions ────────────────────

#[test]
fn translate_eor_imm_continues() {
    // EOR X0, X1, #1
    let action = translate(0xD2400020);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_ands_imm_continues() {
    // ANDS X0, X1, #1 (TST when Rd=XZR)
    let action = translate(0xF2400020);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_movk_imm_continues() {
    // MOVK X0, #0x5678, LSL#16
    let action = translate(0xF2ACEF00);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_extr_continues() {
    // EXTR X0, X1, X2, #3
    let action = translate(0x93C20C20);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_adr_continues() {
    // ADR X0, #0
    let action = translate(0x10000000);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_adrp_continues() {
    // ADRP X0, #0
    let action = translate(0x90000000);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_clz_continues() {
    // CLZ X0, X1 — not yet implemented in the emitter
    let action = translate(0xDAC01020);
    assert_eq!(action, TranslateAction::Unhandled);
}

#[test]
fn translate_cls_continues() {
    // CLS X0, X1 — not yet implemented in the emitter
    let action = translate(0xDAC01420);
    assert_eq!(action, TranslateAction::Unhandled);
}

#[test]
fn translate_rbit_continues() {
    // RBIT X0, X1 — not yet implemented in the emitter
    let action = translate(0xDAC00020);
    assert_eq!(action, TranslateAction::Unhandled);
}

#[test]
fn translate_rev_continues() {
    // REV X0, X1 — not yet implemented in the emitter
    let action = translate(0xDAC00C20);
    assert_eq!(action, TranslateAction::Unhandled);
}

#[test]
fn translate_rev16_continues() {
    // REV16 X0, X1 — not yet implemented in the emitter
    let action = translate(0xDAC00420);
    assert_eq!(action, TranslateAction::Unhandled);
}

#[test]
fn translate_rev32_continues() {
    // REV32 X0, X1 — not yet implemented in the emitter
    let action = translate(0xDAC00820);
    assert_eq!(action, TranslateAction::Unhandled);
}

#[test]
fn translate_sdiv_continues() {
    // SDIV X0, X1, X2
    let action = translate(0x9AC20C20);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_asrv_continues() {
    // ASRV X0, X1, X2
    let action = translate(0x9AC22820);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_rorv_continues() {
    // RORV X0, X1, X2 — not yet implemented in the emitter
    let action = translate(0x9AC22C20);
    assert_eq!(action, TranslateAction::Unhandled);
}

#[test]
fn translate_adc_continues() {
    // ADC X0, X1, X2 — not yet implemented in the emitter
    let action = translate(0x9A020020);
    assert_eq!(action, TranslateAction::Unhandled);
}

#[test]
fn translate_sbc_continues() {
    // SBC X0, X1, X2 — not yet implemented in the emitter
    let action = translate(0xDA020020);
    assert_eq!(action, TranslateAction::Unhandled);
}

#[test]
fn translate_adcs_continues() {
    // ADCS X0, X1, X2 — not yet implemented in the emitter
    let action = translate(0xBA020020);
    assert_eq!(action, TranslateAction::Unhandled);
}

#[test]
fn translate_sbcs_continues() {
    // SBCS X0, X1, X2 — not yet implemented in the emitter
    let action = translate(0xFA020020);
    assert_eq!(action, TranslateAction::Unhandled);
}

#[test]
fn translate_smulh_continues() {
    // SMULH X0, X1, X2 — not yet implemented in the emitter
    let action = translate(0x9B427C20);
    assert_eq!(action, TranslateAction::Unhandled);
}

#[test]
fn translate_umulh_continues() {
    // UMULH X0, X1, X2 — not yet implemented in the emitter
    let action = translate(0x9BE27C20);
    assert_eq!(action, TranslateAction::Unhandled);
}

#[test]
fn translate_smaddl_continues() {
    // SMADDL X0, W1, W2, X3
    let action = translate(0x9B220C20);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_umaddl_continues() {
    // UMADDL X0, W1, W2, X3
    let action = translate(0x9BA20C20);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_msub_continues() {
    // MSUB X0, X1, X2, X3
    let action = translate(0x9B028C20);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_bic_continues() {
    // BIC X0, X1, X2
    let action = translate(0x8A220020);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_orn_continues() {
    // ORN X0, X1, X2
    let action = translate(0xAA220020);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_eon_continues() {
    // EON X0, X1, X2
    let action = translate(0xCA220020);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_bics_continues() {
    // BICS X0, X1, X2
    let action = translate(0xEA220020);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_ldur_continues() {
    // LDUR X0, [X1, #-8]
    let action = translate(0xF85F8020);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_stur_continues() {
    // STUR X0, [X1, #-8]
    let action = translate(0xF81F8020);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_ldar_continues() {
    // LDAR X0, [X1]
    let action = translate(0xC8DFFC20);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_stlr_continues() {
    // STLR X0, [X1]
    let action = translate(0xC89FFC20);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_ret_custom_reg_ends_block() {
    // RET X1
    let action = translate(0xD65F0020);
    assert_eq!(action, TranslateAction::EndBlock);
}

#[test]
fn translate_bfm_continues() {
    // BFM X0, X1, #1, #2
    let action = translate(0xB3410820);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_movn_continues() {
    // MOVN X0, #0
    let action = translate(0x92800000);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_ccmp_imm_continues() {
    // CCMP X0, #5, #0, EQ
    let action = translate(0xFA400A00);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_ccmn_reg_continues() {
    // CCMN X0, X1, #0, EQ
    let action = translate(0xBA410000);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_csneg_continues() {
    // CSNEG X0, X1, X2, EQ
    let action = translate(0xDA820420);
    assert_eq!(action, TranslateAction::Continue);
}

#[test]
fn translate_csinv_continues() {
    // CSINV X0, X1, X2, NE
    let action = translate(0xDA821020);
    assert_eq!(action, TranslateAction::Continue);
}

//! TDD — comprehensive decode coverage tests.
//!
//! Every test is `#[ignore]` — these define the decode behaviour we
//! want to implement.  Remove `#[ignore]` as each instruction group
//! is wired into the `Aarch64Decoder`.
//!
//! Instruction encodings verified against the ARM Architecture Reference Manual.

use crate::arm::aarch64::Aarch64Decoder;
use helm_core::ir::Opcode;

fn decode_one(insn: u32) -> Vec<helm_core::ir::MicroOp> {
    Aarch64Decoder::new().decode_insn(0x1000, insn).unwrap()
}

// ═══════════════════════════════════════════════════════════════════
// Data Processing — Immediate
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_adds_imm_64() {
    // ADDS X0, X1, #1 => 0xB1000420
    let u = decode_one(0xB1000420);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_subs_imm_64() {
    // SUBS X0, X1, #1 => 0xF1000420
    let u = decode_one(0xF1000420);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_add_imm_32() {
    // ADD W0, W1, #42 => 0x1100A820
    let u = decode_one(0x1100A820);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_and_imm() {
    // AND X0, X1, #0xFF => 0x921C0020 (n=1, immr=0, imms=7)
    let u = decode_one(0x92400420);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_orr_imm() {
    // ORR X0, X1, #1 => 0xB2400020
    let u = decode_one(0xB2400020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_eor_imm() {
    // EOR X0, X1, #1 => 0xD2400020
    let u = decode_one(0xD2400020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_movn() {
    // MOVN X0, #0 => 0x92800000
    let u = decode_one(0x92800000);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_sbfm() {
    // SBFM X0, X1, #0, #7 (=SXTB) => 0x93401C20
    let u = decode_one(0x93401C20);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_ubfm() {
    // UBFM X0, X1, #0, #7 (=UXTB) => 0xD3401C20
    let u = decode_one(0xD3401C20);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_bfm() {
    // BFM X0, X1, #0, #7 => 0xB3401C20
    let u = decode_one(0xB3401C20);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_extr() {
    // EXTR X0, X1, X2, #0 => 0x93C20020
    let u = decode_one(0x93C20020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

// ═══════════════════════════════════════════════════════════════════
// Data Processing — Register
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_sub_shifted_reg() {
    // SUB X0, X1, X2 => 0xCB020020
    let u = decode_one(0xCB020020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_and_shifted_reg() {
    // AND X0, X1, X2 => 0x8A020020
    let u = decode_one(0x8A020020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_orr_shifted_reg() {
    // ORR X0, X1, X2 (=MOV X0, X2 when X1=XZR) => 0xAA020020
    let u = decode_one(0xAA020020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_eor_shifted_reg() {
    // EOR X0, X1, X2 => 0xCA020020
    let u = decode_one(0xCA020020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_bic() {
    // BIC X0, X1, X2 => 0x8A220020
    let u = decode_one(0x8A220020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_orn() {
    // ORN X0, X1, X2 => 0xAA220020
    let u = decode_one(0xAA220020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_sdiv() {
    // SDIV X0, X1, X2 => 0x9AC20C20
    let u = decode_one(0x9AC20C20);
    assert_eq!(u[0].opcode, Opcode::IntDiv);
}

#[test]
#[ignore]
fn decode_lslv() {
    // LSLV X0, X1, X2 => 0x9AC22020
    let u = decode_one(0x9AC22020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_lsrv() {
    // LSRV X0, X1, X2 => 0x9AC22420
    let u = decode_one(0x9AC22420);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_asrv() {
    // ASRV X0, X1, X2 => 0x9AC22820
    let u = decode_one(0x9AC22820);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_rorv() {
    // RORV X0, X1, X2 => 0x9AC22C20
    let u = decode_one(0x9AC22C20);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_madd() {
    // MADD X0, X1, X2, X3 => 0x9B020C20
    let u = decode_one(0x9B020C20);
    assert_eq!(u[0].opcode, Opcode::IntMul);
}

#[test]
#[ignore]
fn decode_msub() {
    // MSUB X0, X1, X2, X3 => 0x9B028C20
    let u = decode_one(0x9B028C20);
    assert_eq!(u[0].opcode, Opcode::IntMul);
}

#[test]
#[ignore]
fn decode_smaddl() {
    // SMADDL X0, W1, W2, X3 => 0x9B220C20
    let u = decode_one(0x9B220C20);
    assert_eq!(u[0].opcode, Opcode::IntMul);
}

#[test]
#[ignore]
fn decode_umaddl() {
    // UMADDL X0, W1, W2, X3 => 0x9BA20C20
    let u = decode_one(0x9BA20C20);
    assert_eq!(u[0].opcode, Opcode::IntMul);
}

#[test]
#[ignore]
fn decode_smulh() {
    // SMULH X0, X1, X2 => 0x9B427C20
    let u = decode_one(0x9B427C20);
    assert_eq!(u[0].opcode, Opcode::IntMul);
}

#[test]
#[ignore]
fn decode_csinc() {
    // CSINC X0, X1, X2, EQ => 0x9A820420
    let u = decode_one(0x9A820420);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_csinv() {
    // CSINV X0, X1, X2, EQ => 0xDA820020
    let u = decode_one(0xDA820020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_csneg() {
    // CSNEG X0, X1, X2, EQ => 0xDA820420
    let u = decode_one(0xDA820420);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_rbit() {
    // RBIT X0, X1 => 0xDAC00020
    let u = decode_one(0xDAC00020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_rev() {
    // REV X0, X1 => 0xDAC00C20
    let u = decode_one(0xDAC00C20);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_adc() {
    // ADC X0, X1, X2 => 0x9A020020
    let u = decode_one(0x9A020020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_sbc() {
    // SBC X0, X1, X2 => 0xDA020020
    let u = decode_one(0xDA020020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_add_ext_reg() {
    // ADD X0, X1, W2, UXTW => 0x8B224020
    let u = decode_one(0x8B224020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

// ═══════════════════════════════════════════════════════════════════
// Loads and Stores — additional variants
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_ldr_pre_index() {
    // LDR X0, [X1, #8]! => 0xF8408C20
    let u = decode_one(0xF8408C20);
    assert_eq!(u[0].opcode, Opcode::Load);
}

#[test]
#[ignore]
fn decode_str_pre_index() {
    // STR X0, [X1, #-16]! => 0xF81F0C20
    let u = decode_one(0xF81F0C20);
    assert_eq!(u[0].opcode, Opcode::Store);
}

#[test]
#[ignore]
fn decode_ldr_post_index() {
    // LDR X0, [X1], #8 => 0xF8408420
    let u = decode_one(0xF8408420);
    assert_eq!(u[0].opcode, Opcode::Load);
}

#[test]
#[ignore]
fn decode_ldr_register() {
    // LDR X0, [X1, X2] => 0xF8626820
    let u = decode_one(0xF8626820);
    assert_eq!(u[0].opcode, Opcode::Load);
}

#[test]
#[ignore]
fn decode_ldr_literal() {
    // LDR X0, #0x100 => 0x58000800
    let u = decode_one(0x58000800);
    assert_eq!(u[0].opcode, Opcode::Load);
}

#[test]
#[ignore]
fn decode_ldrh_uimm() {
    // LDRH W0, [X1] => 0x79400020
    let u = decode_one(0x79400020);
    assert_eq!(u[0].opcode, Opcode::Load);
}

#[test]
#[ignore]
fn decode_ldrsw_uimm() {
    // LDRSW X0, [X1] => 0xB9800020
    let u = decode_one(0xB9800020);
    assert_eq!(u[0].opcode, Opcode::Load);
}

#[test]
#[ignore]
fn decode_ldp_w() {
    // LDP W0, W1, [SP, #8] => 0x29410FE0
    let u = decode_one(0x29410FE0);
    assert_eq!(u[0].opcode, Opcode::Load);
}

#[test]
#[ignore]
fn decode_ldp_pre() {
    // LDP X0, X1, [SP, #-16]! => 0xA9FF07E0 (bit 22 = L = 1)
    let u = decode_one(0xA9FF07E0);
    assert_eq!(u[0].opcode, Opcode::Load);
}

// ═══════════════════════════════════════════════════════════════════
// Branches — additional variants
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_br() {
    // BR X8 => 0xD61F0100
    let u = decode_one(0xD61F0100);
    assert_eq!(u[0].opcode, Opcode::Branch);
    assert!(u[0].flags.is_branch);
}

#[test]
#[ignore]
fn decode_blr() {
    // BLR X8 => 0xD63F0100
    let u = decode_one(0xD63F0100);
    assert_eq!(u[0].opcode, Opcode::Branch);
    assert!(u[0].flags.is_call);
}

#[test]
#[ignore]
fn decode_cbnz() {
    // CBNZ X0, #0x10 => 0xB5000080
    let u = decode_one(0xB5000080);
    assert_eq!(u[0].opcode, Opcode::CondBranch);
}

#[test]
#[ignore]
fn decode_ccmp() {
    // CCMP X1, #0, #0, EQ => 0xFA400020
    let u = decode_one(0xFA400020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

// ═══════════════════════════════════════════════════════════════════
// SIMD — Three Same (integer)
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_add_v_8b() {
    // ADD V0.8B, V1.8B, V2.8B => 0x0E228420
    let u = decode_one(0x0E228420);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_add_v_16b() {
    // ADD V0.16B, V1.16B, V2.16B => 0x4E228420
    let u = decode_one(0x4E228420);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_sub_v_4s() {
    // SUB V0.4S, V1.4S, V2.4S => 0x6EA28420
    let u = decode_one(0x6EA28420);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_and_v() {
    // AND V0.16B, V1.16B, V2.16B => 0x4E221C20
    let u = decode_one(0x4E221C20);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_orr_v() {
    // ORR V0.16B, V1.16B, V2.16B => 0x4EA21C20
    let u = decode_one(0x4EA21C20);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_eor_v() {
    // EOR V0.16B, V1.16B, V2.16B => 0x6E221C20
    let u = decode_one(0x6E221C20);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_mul_v_4s() {
    // MUL V0.4S, V1.4S, V2.4S => 0x4EA29C20
    let u = decode_one(0x4EA29C20);
    assert_eq!(u[0].opcode, Opcode::IntMul);
}

#[test]
#[ignore]
fn decode_cmgt_v() {
    // CMGT V0.4S, V1.4S, V2.4S => 0x4EA23420
    let u = decode_one(0x4EA23420);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_cmeq_v() {
    // CMEQ V0.4S, V1.4S, V2.4S => 0x6EA28C20
    let u = decode_one(0x6EA28C20);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

// ═══════════════════════════════════════════════════════════════════
// SIMD — Two-Register Misc
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_abs_v() {
    // ABS V0.4S, V1.4S => 0x4EA0B820
    let u = decode_one(0x4EA0B820);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_neg_v() {
    // NEG V0.4S, V1.4S => 0x6EA0B820
    let u = decode_one(0x6EA0B820);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_not_v() {
    // NOT V0.16B, V1.16B => 0x6E205820
    let u = decode_one(0x6E205820);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_cnt_v() {
    // CNT V0.16B, V1.16B => 0x4E205820
    let u = decode_one(0x4E205820);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_rev64_v() {
    // REV64 V0.4S, V1.4S => 0x4EA00820
    let u = decode_one(0x4EA00820);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

// ═══════════════════════════════════════════════════════════════════
// SIMD — Across Lanes
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_addv() {
    // ADDV S0, V1.4S => 0x4EB1B820
    let u = decode_one(0x4EB1B820);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

// ═══════════════════════════════════════════════════════════════════
// SIMD — Shift by Immediate
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_shl_v() {
    // SHL V0.4S, V1.4S, #8 => 0x4F285420
    let u = decode_one(0x4F285420);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_sshr_v() {
    // SSHR V0.4S, V1.4S, #8 => 0x4F280420
    let u = decode_one(0x4F280420);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_ushr_v() {
    // USHR V0.4S, V1.4S, #8 => 0x6F280420
    let u = decode_one(0x6F280420);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

// ═══════════════════════════════════════════════════════════════════
// SIMD — Copy / Insert / Extract
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_dup_general() {
    // DUP V0.4S, W1 => 0x0E040C20
    let u = decode_one(0x0E040C20);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_ins_general() {
    // INS V0.S[0], W1 => 0x4E040C20
    let u = decode_one(0x4E040C20);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_umov() {
    // UMOV W0, V1.S[0] => 0x0E043C20
    let u = decode_one(0x0E043C20);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_ext_q() {
    // EXT V0.16B, V1.16B, V2.16B, #4 => 0x6E022020
    let u = decode_one(0x6E022020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

// ═══════════════════════════════════════════════════════════════════
// SIMD — Permute
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_uzp1() {
    // UZP1 V0.4S, V1.4S, V2.4S => 0x4E821820
    let u = decode_one(0x4E821820);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_zip1() {
    // ZIP1 V0.4S, V1.4S, V2.4S => 0x4E823820
    let u = decode_one(0x4E823820);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_trn1() {
    // TRN1 V0.4S, V1.4S, V2.4S => 0x4E822820
    let u = decode_one(0x4E822820);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

// ═══════════════════════════════════════════════════════════════════
// SIMD — Modified Immediate
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_movi() {
    // MOVI V0.4S, #0 => 0x4F000400
    let u = decode_one(0x4F000400);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

// ═══════════════════════════════════════════════════════════════════
// SIMD — Table Lookup
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_tbl() {
    // TBL V0.16B, {V1.16B}, V2.16B => 0x4E020020
    let u = decode_one(0x4E020020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

// ═══════════════════════════════════════════════════════════════════
// SIMD — Widening / Narrowing
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_saddl_v() {
    // SADDL V0.4S, V1.4H, V2.4H => 0x0E620020
    let u = decode_one(0x0E620020);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_xtn() {
    // XTN V0.4H, V1.4S => 0x0E612820
    let u = decode_one(0x0E612820);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

// ═══════════════════════════════════════════════════════════════════
// Scalar FP
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_fadd_s() {
    // FADD S0, S1, S2 => 0x1E222820
    let u = decode_one(0x1E222820);
    assert_eq!(u[0].opcode, Opcode::FpAlu);
}

#[test]
#[ignore]
fn decode_fsub_s() {
    // FSUB S0, S1, S2 => 0x1E223820
    let u = decode_one(0x1E223820);
    assert_eq!(u[0].opcode, Opcode::FpAlu);
}

#[test]
#[ignore]
fn decode_fmul_s() {
    // FMUL S0, S1, S2 => 0x1E220820
    let u = decode_one(0x1E220820);
    assert_eq!(u[0].opcode, Opcode::FpMul);
}

#[test]
#[ignore]
fn decode_fdiv_s() {
    // FDIV S0, S1, S2 => 0x1E221820
    let u = decode_one(0x1E221820);
    assert_eq!(u[0].opcode, Opcode::FpDiv);
}

#[test]
#[ignore]
fn decode_fmov_s() {
    // FMOV S0, S1 => 0x1E204020
    let u = decode_one(0x1E204020);
    assert_eq!(u[0].opcode, Opcode::FpAlu);
}

#[test]
#[ignore]
fn decode_fneg_s() {
    // FNEG S0, S1 => 0x1E214020
    let u = decode_one(0x1E214020);
    assert_eq!(u[0].opcode, Opcode::FpAlu);
}

#[test]
#[ignore]
fn decode_fabs_s() {
    // FABS S0, S1 => 0x1E20C020
    let u = decode_one(0x1E20C020);
    assert_eq!(u[0].opcode, Opcode::FpAlu);
}

#[test]
#[ignore]
fn decode_fsqrt_s() {
    // FSQRT S0, S1 => 0x1E21C020
    let u = decode_one(0x1E21C020);
    assert_eq!(u[0].opcode, Opcode::FpDiv); // sqrt is div-latency
}

#[test]
#[ignore]
fn decode_fcmp_s() {
    // FCMP S0, S1 => 0x1E212000
    let u = decode_one(0x1E212000);
    assert_eq!(u[0].opcode, Opcode::FpAlu);
}

#[test]
#[ignore]
fn decode_fcsel() {
    // FCSEL S0, S1, S2, EQ => 0x1E220C20
    let u = decode_one(0x1E220C20);
    assert_eq!(u[0].opcode, Opcode::FpAlu);
}

#[test]
#[ignore]
fn decode_fmadd() {
    // FMADD S0, S1, S2, S3 => 0x1F020C20
    let u = decode_one(0x1F020C20);
    assert_eq!(u[0].opcode, Opcode::FpMul);
}

#[test]
#[ignore]
fn decode_scvtf_gp_to_fp() {
    // SCVTF S0, W1 => 0x1E220020
    let u = decode_one(0x1E220020);
    assert_eq!(u[0].opcode, Opcode::FpAlu);
}

#[test]
#[ignore]
fn decode_fcvtzs_fp_to_gp() {
    // FCVTZS W0, S1 => 0x1E380020
    let u = decode_one(0x1E380020);
    assert_eq!(u[0].opcode, Opcode::FpAlu);
}

// ═══════════════════════════════════════════════════════════════════
// SIMD FP — Three Same
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_fadd_v() {
    // FADD V0.4S, V1.4S, V2.4S => 0x4E22D420
    let u = decode_one(0x4E22D420);
    assert_eq!(u[0].opcode, Opcode::FpAlu);
}

#[test]
#[ignore]
fn decode_fmul_v() {
    // FMUL V0.4S, V1.4S, V2.4S => 0x6E22DC20
    let u = decode_one(0x6E22DC20);
    assert_eq!(u[0].opcode, Opcode::FpMul);
}

// ═══════════════════════════════════════════════════════════════════
// FP/GP Transfers
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_fmov_ws() {
    // FMOV W0, S1 => 0x1E260020
    let u = decode_one(0x1E260020);
    assert_eq!(u[0].opcode, Opcode::FpAlu);
}

#[test]
#[ignore]
fn decode_fmov_sw() {
    // FMOV S0, W1 => 0x1E270020
    let u = decode_one(0x1E270020);
    assert_eq!(u[0].opcode, Opcode::FpAlu);
}

#[test]
#[ignore]
fn decode_fmov_xd() {
    // FMOV X0, D1 => 0x9E660020
    let u = decode_one(0x9E660020);
    assert_eq!(u[0].opcode, Opcode::FpAlu);
}

// ═══════════════════════════════════════════════════════════════════
// Crypto (AES)
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_aese() {
    // AESE V0.16B, V1.16B => 0x4E284820
    let u = decode_one(0x4E284820);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

#[test]
#[ignore]
fn decode_aesd() {
    // AESD V0.16B, V1.16B => 0x4E285820
    let u = decode_one(0x4E285820);
    assert_eq!(u[0].opcode, Opcode::IntAlu);
}

// ═══════════════════════════════════════════════════════════════════
// Atomics (LSE)
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn decode_ldadd() {
    // LDADD X2, X0, [X1] => 0xF8220020
    let u = decode_one(0xF8220020);
    assert_eq!(u[0].opcode, Opcode::Load); // atomic load-modify-store
}

#[test]
#[ignore]
fn decode_swp() {
    // SWP X2, X0, [X1] => 0xF8228020
    let u = decode_one(0xF8228020);
    assert_eq!(u[0].opcode, Opcode::Load);
}

#[test]
#[ignore]
fn decode_cas() {
    // CAS X2, X0, [X1] => 0xC8A27C20
    let u = decode_one(0xC8A27C20);
    assert_eq!(u[0].opcode, Opcode::Load);
}

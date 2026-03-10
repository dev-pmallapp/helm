//! Tests that the build.rs-generated SIMD decoder (from aarch64-simd.decode)
//! correctly identifies instruction mnemonics.
//!
//! These test cases are the actual instruction words observed during fish
//! shell execution — if any regresses, the .decode file or codegen is wrong.

use crate::arm::aarch64::exec::decode_aarch64_simd;

#[test]
fn generated_decoder_cmeq_v() {
    assert_eq!(decode_aarch64_simd(0x2e288d20), "CMEQ_v");
}

#[test]
fn generated_decoder_umaxv() {
    assert_eq!(decode_aarch64_simd(0x2e30a800), "UMAXV");
}

#[test]
fn generated_decoder_uminv() {
    assert_eq!(decode_aarch64_simd(0x2e31a800), "UMINV");
}

#[test]
fn generated_decoder_and_v() {
    assert_eq!(decode_aarch64_simd(0x4e231c41), "AND_v");
}

#[test]
fn generated_decoder_eor_v() {
    assert_eq!(decode_aarch64_simd(0x6e211c00), "EOR_v");
}

#[test]
fn generated_decoder_add_v() {
    assert_eq!(decode_aarch64_simd(0x4ee78463), "ADD_v");
}

#[test]
fn generated_decoder_sub_v() {
    assert_eq!(decode_aarch64_simd(0x6ee08420), "SUB_v");
}

#[test]
fn generated_decoder_cmgt_v() {
    assert_eq!(decode_aarch64_simd(0x0e283484), "CMGT_v");
}

#[test]
fn generated_decoder_cmtst_v() {
    assert_eq!(decode_aarch64_simd(0x0e208c00), "CMTST_v");
}

#[test]
fn generated_decoder_cmeq0_v() {
    assert_eq!(decode_aarch64_simd(0x0e609821), "CMEQ0_v");
}

#[test]
fn generated_decoder_cmlt0_v() {
    assert_eq!(decode_aarch64_simd(0x0e20a800), "CMLT0_v");
}

#[test]
fn generated_decoder_cmge0_v() {
    assert_eq!(decode_aarch64_simd(0x2e208800), "CMGE0_v");
}

#[test]
fn generated_decoder_dup_general() {
    assert_eq!(decode_aarch64_simd(0x4e010c20), "DUP_general");
}

#[test]
fn generated_decoder_ins_general() {
    assert_eq!(decode_aarch64_simd(0x4e081d00), "INS_general");
}

#[test]
fn generated_decoder_ins_element() {
    assert_eq!(decode_aarch64_simd(0x6e0c2401), "INS_element");
}

#[test]
fn generated_decoder_umov() {
    assert_eq!(decode_aarch64_simd(0x4e083c01), "UMOV");
}

#[test]
fn generated_decoder_fmov_xd() {
    assert_eq!(decode_aarch64_simd(0x9e660008), "FMOV_xd");
}

#[test]
fn generated_decoder_fmov_ws() {
    assert_eq!(decode_aarch64_simd(0x1e260008), "FMOV_ws");
}

#[test]
fn generated_decoder_fmov_dx() {
    assert_eq!(decode_aarch64_simd(0x9e6703e0), "FMOV_dx");
}

#[test]
fn generated_decoder_addp_s() {
    assert_eq!(decode_aarch64_simd(0x5ef1b800), "ADDP_s");
}

#[test]
fn generated_decoder_shl_v() {
    assert_eq!(decode_aarch64_simd(0x0f0f5420), "SHL_v");
}

#[test]
fn generated_decoder_ushll_v() {
    assert_eq!(decode_aarch64_simd(0x2f08a421), "USHLL_v");
}

#[test]
fn generated_decoder_movi() {
    assert_eq!(decode_aarch64_simd(0x6f00e400), "MOVI");
}

#[test]
fn generated_decoder_uzp1() {
    assert_eq!(decode_aarch64_simd(0x4e411841), "UZP1");
}

#[test]
fn generated_decoder_zip1() {
    assert_eq!(decode_aarch64_simd(0x0e003802), "ZIP1");
}

#[test]
fn generated_decoder_bic_v() {
    assert_eq!(decode_aarch64_simd(0x0e611c41), "BIC_v");
}

#[test]
fn generated_decoder_unknown_returns_unknown() {
    assert_eq!(decode_aarch64_simd(0x00000000), "UNKNOWN");
}

use crate::tree::DecodeTree;
use crate::validate::{has_errors, validate, Severity};

#[test]
fn clean_tree_has_no_diagnostics() {
    let text = "
B      0 00101 imm26:26
BL     1 00101 imm26:26
NOP    11010101 00000011 00100000 00011111
";
    let tree = DecodeTree::from_decode_text(text);
    let diags = validate(&tree);
    let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
}

#[test]
fn detects_identical_encoding_same_mnemonic() {
    let text = "
ADD_imm  sf:1 0 0 10001 sh:2 imm12:12 rn:5 rd:5
ADD_imm  sf:1 0 0 10001 sh:2 imm12:12 rn:5 rd:5
";
    let tree = DecodeTree::from_decode_text(text);
    let diags = validate(&tree);
    assert!(!diags.is_empty());
    assert!(
        diags.iter().any(|d| d.message.contains("identical encoding")),
        "expected 'identical encoding' diagnostic, got: {diags:?}"
    );
}

#[test]
fn detects_identical_encoding_different_mnemonic_as_error() {
    let text = "
FOO  sf:1 0 0 10001 sh:2 imm12:12 rn:5 rd:5
BAR  sf:1 0 0 10001 sh:2 imm12:12 rn:5 rd:5
";
    let tree = DecodeTree::from_decode_text(text);
    let diags = validate(&tree);
    assert!(has_errors(&diags), "expected error for different-name duplicate");
    assert!(diags.iter().any(|d| d.severity == Severity::Error
        && d.message.contains("FOO")
        && d.message.contains("BAR")));
}

#[test]
fn detects_shadowed_pattern() {
    let text = "
# General pattern (fewer fixed bits) comes first
WIDE     0 q:1 0 0 1110 size:2 1 rm:5 10000 1 rn:5 rd:5
# Specific pattern is unreachable because WIDE matches everything NARROW does
NARROW   0 0   0 0 1110 00     1 rm:5 10000 1 rn:5 rd:5
";
    let tree = DecodeTree::from_decode_text(text);
    let diags = validate(&tree);
    assert!(
        diags.iter().any(|d| d.severity == Severity::Error
            && d.message.contains("shadowed")),
        "expected shadowed error, got: {diags:?}"
    );
}

#[test]
fn constraint_prevents_shadow_error() {
    let text = "
CMP_imm  sf:1 1 1 10001 sh:2 imm12:12 rn:5 rd:5  rd=31
SUBS_imm sf:1 1 1 10001 sh:2 imm12:12 rn:5 rd:5
";
    let tree = DecodeTree::from_decode_text(text);
    let diags = validate(&tree);
    let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(
        errors.is_empty(),
        "constraint should prevent shadow error: {errors:?}"
    );
}

#[test]
fn detects_overlap_between_patterns() {
    let text = "
FADD_v   0 q:1 0 0 1110 0 size:1 1 rm:5 11010 1 rn:5 rd:5
FSUB_v   0 q:1 0 0 1110 1 size:1 1 rm:5 11010 1 rn:5 rd:5
";
    let tree = DecodeTree::from_decode_text(text);
    let diags = validate(&tree);
    let overlaps: Vec<_> = diags.iter().filter(|d| d.message.contains("overlap")).collect();
    assert!(
        overlaps.is_empty(),
        "FADD/FSUB differ in bit 23 — should NOT overlap: {overlaps:?}"
    );
}

#[test]
fn detects_real_overlap() {
    let text = "
# GENERAL leaves bit 23 as don't-care; SPECIFIC fixes it to 0.
# Every insn matching SPECIFIC also matches GENERAL → overlap (shadow).
GENERAL  0 q:1 0 0 1110 . size:1 1 rm:5 10000 1 rn:5 rd:5
SPECIFIC 0 q:1 0 0 1110 0 size:1 1 rm:5 10000 1 rn:5 rd:5
";
    let tree = DecodeTree::from_decode_text(text);
    let diags = validate(&tree);
    assert!(
        diags.iter().any(|d| d.message.contains("overlap") || d.message.contains("shadowed")),
        "expected overlap/shadow, got: {diags:?}"
    );
}

#[test]
fn empty_mask_is_error() {
    let text = "
CATCHALL ................................
";
    let tree = DecodeTree::from_decode_text(text);
    let diags = validate(&tree);
    assert!(
        diags.iter().any(|d| d.severity == Severity::Error
            && d.message.contains("mask is 0")),
        "expected 'mask is 0' error, got: {diags:?}"
    );
}

#[test]
fn aarch64_simd_decode_has_no_errors() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/helm-isa/src/arm/decode_files/aarch64-simd.decode"
    );
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return,
    };
    let tree = DecodeTree::from_decode_text(&text);
    let diags = validate(&tree);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "aarch64-simd.decode has validation errors:\n{}",
        errors.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("\n")
    );
}

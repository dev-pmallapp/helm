use crate::codegen::{generate_decoder, generate_name_decoder, CodegenOpts};
use crate::tree::DecodeTree;

const SMALL: &str = "
B      0 00101 imm26:26
BL     1 00101 imm26:26
ADD_imm sf:1 0 0 10001 sh:2 imm12:12 rn:5 rd:5
NOP    11010101 00000011 00100000 00011111
";

#[test]
fn codegen_name_decoder_compiles() {
    let tree = DecodeTree::from_decode_text(SMALL);
    let code = generate_name_decoder(&tree, "test_decode");
    assert!(code.contains("fn test_decode"));
    assert!(code.contains("\"B\""));
    assert!(code.contains("\"BL\""));
    assert!(code.contains("\"NOP\""));
    assert!(code.contains("\"ADD_imm\""));
    assert!(code.contains("\"UNKNOWN\""));
}

#[test]
fn codegen_with_trait() {
    let tree = DecodeTree::from_decode_text(SMALL);
    let code = generate_decoder(
        &tree,
        &CodegenOpts {
            fn_name: "dispatch",
            trait_name: Some("ArmHandler"),
            return_type: "Result<(), String>",
            fallthrough: "Err(\"unimplemented\".into())",
            extract_fields: true,
            ..Default::default()
        },
    );
    assert!(code.contains("trait ArmHandler"));
    assert!(code.contains("fn handle_b("));
    assert!(code.contains("fn handle_nop("));
    assert!(code.contains("fn dispatch("));
    assert!(code.contains("let imm26 ="));
    assert!(code.contains("self.handle_b("));
}

#[test]
fn codegen_nested_match() {
    let tree = DecodeTree::from_decode_text(SMALL);
    let code = generate_decoder(
        &tree,
        &CodegenOpts {
            fn_name: "decode_nested",
            nested_match: true,
            ..Default::default()
        },
    );
    assert!(code.contains("match (insn >> 25) & 0xF"));
    assert!(code.contains("\"B\""));
}

#[test]
fn codegen_from_qemu_a64() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/helm-isa/src/arm/decode_files/qemu/a64.decode"
    );
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return,
    };
    let tree = DecodeTree::from_decode_text(&text);
    let code = generate_name_decoder(&tree, "decode_a64");

    // Should be a valid, non-trivial function
    assert!(code.contains("fn decode_a64"));
    assert!(code.len() > 10_000, "expected >10KB, got {}", code.len());

    // Spot-check a few SIMD mnemonics
    assert!(code.contains("\"CMEQ_v\""), "CMEQ_v not found in codegen");
    assert!(code.contains("\"UMAXV\""), "UMAXV not found");
    assert!(code.contains("\"ADD_v\""), "ADD_v not found");
}

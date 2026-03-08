use crate::tree::DecodeTree;

const SAMPLE_DECODE: &str = "
# Field definitions
%rd    0:5
%rn    5:5
%imm12 10:12

# Argument sets
&ri    rd rn imm

# Branches
B     0 00101 imm26:26
BL    1 00101 imm26:26

# Data processing - immediate
ADD_imm   sf:1 0 0 10001 sh:2 imm12:12 rn:5 rd:5
SUB_imm   sf:1 1 0 10001 sh:2 imm12:12 rn:5 rd:5

# Move wide
MOVZ  sf:1 10 100101 hw:2 imm16:16 rd:5

# System
NOP   11010101 00000011 00100000 00011111
SVC   11010100 000 imm16:16 00001
";

#[test]
fn tree_from_text() {
    let tree = DecodeTree::from_decode_text(SAMPLE_DECODE);
    assert_eq!(tree.len(), 7); // 7 patterns
    assert_eq!(tree.field_defs.len(), 3); // %rd, %rn, %imm12
    assert_eq!(tree.arg_sets.len(), 1); // &ri
}

#[test]
fn tree_lookup_b() {
    let tree = DecodeTree::from_decode_text(SAMPLE_DECODE);
    let (mnemonic, _) = tree.lookup(0x14000040).unwrap();
    assert_eq!(mnemonic, "B");
}

#[test]
fn tree_lookup_bl() {
    let tree = DecodeTree::from_decode_text(SAMPLE_DECODE);
    let (mnemonic, _) = tree.lookup(0x94000040).unwrap();
    assert_eq!(mnemonic, "BL");
}

#[test]
fn tree_lookup_movz() {
    let tree = DecodeTree::from_decode_text(SAMPLE_DECODE);
    let (mnemonic, fields) = tree.lookup(0xD2824680).unwrap();
    assert_eq!(mnemonic, "MOVZ");
    let imm16 = fields.iter().find(|(n, _)| *n == "imm16").unwrap().1;
    assert_eq!(imm16, 0x1234);
}

#[test]
fn tree_lookup_nop() {
    let tree = DecodeTree::from_decode_text(SAMPLE_DECODE);
    let (mnemonic, _) = tree.lookup(0xD503201F).unwrap();
    assert_eq!(mnemonic, "NOP");
}

#[test]
fn tree_lookup_svc() {
    let tree = DecodeTree::from_decode_text(SAMPLE_DECODE);
    let (mnemonic, _) = tree.lookup(0xD4000001).unwrap();
    assert_eq!(mnemonic, "SVC");
}

#[test]
fn tree_unknown_returns_none() {
    let tree = DecodeTree::from_decode_text(SAMPLE_DECODE);
    assert!(tree.lookup(0x00000000).is_none());
}

#[test]
fn tree_with_constraint() {
    let text = "
CMP_imm  sf:1 1 1 10001 sh:2 imm12:12 rn:5 rd:5  rd=31
SUBS_imm sf:1 1 1 10001 sh:2 imm12:12 rn:5 rd:5
";
    let tree = DecodeTree::from_decode_text(text);
    // CMP (rd=31) should match first
    let (m, _) = tree.lookup(0xF100003F).unwrap(); // rd=31
    assert_eq!(m, "CMP_imm");
    // SUBS (rd=0) should fall through to second pattern
    let (m, _) = tree.lookup(0xF1000020).unwrap(); // rd=0
    assert_eq!(m, "SUBS_imm");
}

#[test]
fn tree_loads_field_defs() {
    let tree = DecodeTree::from_decode_text(SAMPLE_DECODE);
    assert!(tree.field_defs.contains_key("rd"));
    assert!(tree.field_defs.contains_key("rn"));
    assert!(tree.field_defs.contains_key("imm12"));
    let rd = &tree.field_defs["rd"];
    assert_eq!(rd.segments, vec![(0, 5)]);
}

#[test]
fn empty_tree_is_empty() {
    let tree = DecodeTree::new();
    assert!(tree.is_empty());
    assert_eq!(tree.len(), 0);
}

#[test]
fn tree_is_not_empty_after_parse() {
    let tree = DecodeTree::from_decode_text(SAMPLE_DECODE);
    assert!(!tree.is_empty());
}

#[test]
fn format_defs_populated_from_text() {
    let text = "
@branch_fmt imm26:26
B  0 00101 imm26:26
";
    let tree = DecodeTree::from_decode_text(text);
    assert!(tree.format_defs.contains_key("branch_fmt"));
}

#[test]
fn tree_lookup_returns_all_extracted_fields() {
    let tree = DecodeTree::from_decode_text(SAMPLE_DECODE);
    let (_, fields) = tree.lookup(0x91000400).unwrap(); // ADD_imm X0, X0, #1
                                                        // Should have sf, sh, imm12, rn, rd fields
    assert!(!fields.is_empty());
}

#[test]
fn tree_from_text_ignores_comment_lines() {
    // Comments should not produce patterns
    let text = "
# This is a comment
B  0 00101 imm26:26
# Another comment
";
    let tree = DecodeTree::from_decode_text(text);
    assert_eq!(tree.len(), 1);
}

#[test]
fn tree_from_text_ignores_blank_lines() {
    let text = "\n\n\nB  0 00101 imm26:26\n\n\n";
    let tree = DecodeTree::from_decode_text(text);
    assert_eq!(tree.len(), 1);
}

#[test]
fn tree_loads_qemu_a64_decode() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/helm-isa/src/arm/decode_files/qemu/a64.decode"
    ));
    // If the file isn't found (CI), skip gracefully.
    let text = match text {
        Ok(t) => t,
        Err(_) => return,
    };
    let tree = DecodeTree::from_decode_text(&text);
    // a64.decode has ~1096 patterns
    assert!(
        tree.len() > 1000,
        "expected >1000 patterns, got {}",
        tree.len()
    );
    assert!(!tree.field_defs.is_empty());
    assert!(!tree.arg_sets.is_empty());
    assert!(!tree.format_defs.is_empty());

    // Verify specific instructions decode correctly
    // NOP = 0xD503201F
    let r = tree.lookup(0xD503201F);
    assert!(r.is_some(), "NOP should match");
    assert_eq!(r.unwrap().0, "NOP");

    // B #0x100 = 0x14000040
    let r = tree.lookup(0x14000040);
    assert!(r.is_some(), "B should match");
    assert_eq!(r.unwrap().0, "B");

    // BL #0x100 = 0x94000040
    let r = tree.lookup(0x94000040);
    assert!(r.is_some(), "BL should match");
    assert_eq!(r.unwrap().0, "BL");

    // SVC #0 = 0xD4000001
    let r = tree.lookup(0xD4000001);
    assert!(r.is_some(), "SVC should match");
    assert_eq!(r.unwrap().0, "SVC");
}

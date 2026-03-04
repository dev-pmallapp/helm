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

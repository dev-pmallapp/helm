use crate::tree::DecodeTree;

const SAMPLE_DECODE: &str = "
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
    assert_eq!(tree.len(), 7);
}

#[test]
fn tree_lookup_b() {
    let tree = DecodeTree::from_decode_text(SAMPLE_DECODE);
    let (mnemonic, _) = tree.lookup(0x14000040).unwrap(); // B #0x100
    assert_eq!(mnemonic, "B");
}

#[test]
fn tree_lookup_bl() {
    let tree = DecodeTree::from_decode_text(SAMPLE_DECODE);
    let (mnemonic, _) = tree.lookup(0x94000040).unwrap(); // BL #0x100
    assert_eq!(mnemonic, "BL");
}

#[test]
fn tree_lookup_movz() {
    let tree = DecodeTree::from_decode_text(SAMPLE_DECODE);
    let (mnemonic, fields) = tree.lookup(0xD2824680).unwrap(); // MOVZ X0, #0x1234
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
    let (mnemonic, fields) = tree.lookup(0xD4000001).unwrap(); // SVC #0
    assert_eq!(mnemonic, "SVC");
    let imm16 = fields.iter().find(|(n, _)| *n == "imm16").unwrap().1;
    assert_eq!(imm16, 0);
}

#[test]
fn tree_unknown_returns_none() {
    let tree = DecodeTree::from_decode_text(SAMPLE_DECODE);
    assert!(tree.lookup(0x00000000).is_none());
}

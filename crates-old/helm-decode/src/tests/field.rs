use crate::field::*;

#[test]
fn extract_rd_from_bottom_5_bits() {
    let rd = BitField::new("rd", 0, 5);
    assert_eq!(rd.extract(0b11111), 31);
    assert_eq!(rd.extract(0b00001), 1);
}

#[test]
fn extract_imm12() {
    let imm12 = BitField::new("imm12", 10, 12);
    let insn: u32 = 42 << 10;
    assert_eq!(imm12.extract(insn), 42);
}

#[test]
fn mask_covers_correct_bits() {
    let f = BitField::new("x", 5, 3);
    assert_eq!(f.mask(), 0b111 << 5);
}

#[test]
fn signed_extraction() {
    let f = BitField::new("simm", 0, 8).signed();
    assert_eq!(f.extract(0xFF), 0xFFFF_FFFF); // -1 in u32
    assert_eq!(f.extract(0x7F), 0x7F); // 127
}

#[test]
fn parse_simple_field_def() {
    let fd = parse_field_def("%rd 0:5").unwrap();
    assert_eq!(fd.name, "rd");
    assert_eq!(fd.segments, vec![(0, 5)]);
}

#[test]
fn parse_multi_segment_field() {
    // Split immediate: %imm  5:7  0:5
    let fd = parse_field_def("%imm 5:7 0:5").unwrap();
    assert_eq!(fd.name, "imm");
    assert_eq!(fd.segments.len(), 2);
}

#[test]
fn multi_segment_extraction() {
    let fd = parse_field_def("%imm 5:3 0:2").unwrap();
    // bits [7:5] = 0b101, bits [1:0] = 0b11
    let insn: u32 = (0b101 << 5) | 0b11;
    // Concatenated: 0b101_11 = 23
    assert_eq!(fd.extract(insn), 0b10111);
}

#[test]
fn non_field_line_returns_none() {
    assert!(parse_field_def("ADD_imm ...").is_none());
    assert!(parse_field_def("# comment").is_none());
}

#[test]
fn bit_field_name_stored_correctly() {
    let f = BitField::new("my_field", 3, 7);
    assert_eq!(f.name, "my_field");
    assert_eq!(f.lsb, 3);
    assert_eq!(f.width, 7);
    assert!(!f.sext);
}

#[test]
fn signed_flag_set_by_builder() {
    let f = BitField::new("s", 0, 4).signed();
    assert!(f.sext);
}

#[test]
fn bit_field_extract_all_zeros() {
    let f = BitField::new("z", 0, 8);
    assert_eq!(f.extract(0), 0);
}

#[test]
fn bit_field_extract_all_ones_in_field() {
    let f = BitField::new("ones", 0, 8);
    assert_eq!(f.extract(0xFF), 0xFF);
}

#[test]
fn bit_field_extract_does_not_bleed_upper_bits() {
    let f = BitField::new("narrow", 0, 4);
    // upper bits of input should not appear in output
    assert_eq!(f.extract(0xFFFF_FFF5), 5);
}

#[test]
fn field_def_sext_flag_not_set_by_default() {
    let fd = parse_field_def("%rd 0:5").unwrap();
    assert!(!fd.sext);
}

#[test]
fn parse_field_def_empty_string_returns_none() {
    assert!(parse_field_def("").is_none());
}

#[test]
fn field_def_name_matches_parsed_name() {
    let fd = parse_field_def("%rn 5:5").unwrap();
    assert_eq!(fd.name, "rn");
}

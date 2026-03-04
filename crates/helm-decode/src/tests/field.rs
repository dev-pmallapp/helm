use crate::field::BitField;

#[test]
fn extract_rd_from_bottom_5_bits() {
    let rd = BitField::new("rd", 0, 5);
    assert_eq!(rd.extract(0b11111), 31);
    assert_eq!(rd.extract(0b00001), 1);
    assert_eq!(rd.extract(0b10000), 16);
}

#[test]
fn extract_imm12() {
    let imm12 = BitField::new("imm12", 10, 12);
    // Instruction with imm12 = 42 at bits [21:10]
    let insn: u32 = 42 << 10;
    assert_eq!(imm12.extract(insn), 42);
}

#[test]
fn mask_covers_correct_bits() {
    let f = BitField::new("x", 5, 3);
    assert_eq!(f.mask(), 0b111 << 5);
}

#[test]
fn extract_sf_top_bit() {
    let sf = BitField::new("sf", 31, 1);
    assert_eq!(sf.extract(0x8000_0000), 1);
    assert_eq!(sf.extract(0x7FFF_FFFF), 0);
}

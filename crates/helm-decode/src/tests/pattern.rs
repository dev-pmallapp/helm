use crate::pattern::*;

#[test]
fn parse_add_imm() {
    // ADD_imm  sf:1 0 0 10001 sh:2 imm12:12 rn:5 rd:5
    let line = "ADD_imm  sf:1 0 0 10001 sh:2 imm12:12 rn:5 rd:5";
    let dl = parse_decode_line(line).unwrap();
    assert_eq!(dl.mnemonic, "ADD_imm");
    // Fixed bits: positions 30=0, 29=0, [28:24]=10001
    // sf(31), sh(22), imm12(21:10), rn(9:5), rd(4:0) are fields.
    assert_eq!(dl.pattern.fields.len(), 5);
    assert_eq!(dl.pattern.fields[0].name, "sf");
    assert_eq!(dl.pattern.fields[4].name, "rd");
}

#[test]
fn parse_b_unconditional() {
    let line = "B  0 00101 imm26:26";
    let dl = parse_decode_line(line).unwrap();
    assert_eq!(dl.mnemonic, "B");
    // bit 31 = 0 (fixed), bits [30:26] = 00101 (fixed), imm26 = field
    assert!(dl.pattern.matches(0x14000040)); // B #0x100
    assert!(!dl.pattern.matches(0x94000040)); // BL (bit 31 = 1)
}

#[test]
fn parse_nop() {
    let line = "NOP  11010101 00000011 00100000 00011111";
    let dl = parse_decode_line(line).unwrap();
    assert_eq!(dl.mnemonic, "NOP");
    assert!(dl.pattern.matches(0xD503201F));
    assert!(!dl.pattern.matches(0xD503201E));
}

#[test]
fn parse_comments_and_blanks() {
    assert!(parse_decode_line("").is_none());
    assert!(parse_decode_line("# comment").is_none());
    assert!(parse_decode_line("   ").is_none());
}

#[test]
fn parse_bl() {
    let line = "BL  1 00101 imm26:26";
    let dl = parse_decode_line(line).unwrap();
    assert!(dl.pattern.matches(0x94000040)); // BL
    assert!(!dl.pattern.matches(0x14000040)); // B (not BL)
}

#[test]
fn fields_extracted_correctly() {
    // MOVZ  sf:1 10 100101 hw:2 imm16:16 rd:5
    let line = "MOVZ  sf:1 10 100101 hw:2 imm16:16 rd:5";
    let dl = parse_decode_line(line).unwrap();
    // MOVZ X0, #0x1234  =>  0xD2824680
    let insn: u32 = 0xD2824680;
    assert!(dl.pattern.matches(insn));
    let fields = dl.pattern.extract_fields(insn);
    let sf = fields.iter().find(|(n, _)| *n == "sf").unwrap().1;
    let rd = fields.iter().find(|(n, _)| *n == "rd").unwrap().1;
    let imm16 = fields.iter().find(|(n, _)| *n == "imm16").unwrap().1;
    assert_eq!(sf, 1); // 64-bit
    assert_eq!(rd, 0); // X0
    assert_eq!(imm16, 0x1234);
}

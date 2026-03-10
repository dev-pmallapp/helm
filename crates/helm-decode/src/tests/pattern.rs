use crate::pattern::*;

#[test]
fn parse_add_imm() {
    let line = "ADD_imm  sf:1 0 0 10001 sh:2 imm12:12 rn:5 rd:5";
    let dl = parse_decode_line(line).unwrap();
    assert_eq!(dl.mnemonic, "ADD_imm");
    assert_eq!(dl.pattern.fields.len(), 5); // sf, sh, imm12, rn, rd
    assert_eq!(dl.pattern.fields[0].name, "sf");
    assert_eq!(dl.pattern.fields[4].name, "rd");
}

#[test]
fn parse_b_unconditional() {
    let line = "B  0 00101 imm26:26";
    let dl = parse_decode_line(line).unwrap();
    assert!(dl.pattern.matches(0x14000040));
    assert!(!dl.pattern.matches(0x94000040)); // BL
}

#[test]
fn parse_nop() {
    let line = "NOP  11010101 00000011 00100000 00011111";
    let dl = parse_decode_line(line).unwrap();
    assert!(dl.pattern.matches(0xD503201F));
}

#[test]
fn parse_with_dont_care_dots() {
    // QEMU uses . for don't-care bits
    let line = "TEST  .... .... .... .... .... .... .... ....";
    let dl = parse_decode_line(line).unwrap();
    assert!(dl.pattern.matches(0x0000_0000));
    assert!(dl.pattern.matches(0xFFFF_FFFF));
}

#[test]
fn parse_with_dash_must_be_zero() {
    // QEMU uses - for must-be-zero
    let line = "TEST2  1111 ---- ---- ---- ---- ---- ---- ----";
    let dl = parse_decode_line(line).unwrap();
    assert!(dl.pattern.matches(0xF000_0000));
    assert!(!dl.pattern.matches(0xF100_0000)); // bit 24 = 1, but must be 0
}

#[test]
fn parse_with_constraint() {
    // Pattern with field constraint: rd must be 31
    let line = "CMP_imm  sf:1 1 1 10001 sh:2 imm12:12 rn:5 rd:5  rd=31";
    let dl = parse_decode_line(line).unwrap();
    assert_eq!(dl.pattern.constraints.len(), 1);
    assert_eq!(dl.pattern.constraints[0], ("rd".to_string(), 31));

    // rd=31 -> matches
    let insn_rd31 = 0xF100003F_u32; // SUBS XZR, X1, #0
    assert!(dl.pattern.matches(insn_rd31));

    // rd=0 -> does not match
    let insn_rd0 = 0xF1000020_u32; // SUBS X0, X1, #0
    assert!(!dl.pattern.matches(insn_rd0));
}

#[test]
fn parse_comments_and_blanks() {
    assert!(parse_decode_line("").is_none());
    assert!(parse_decode_line("# comment").is_none());
    assert!(parse_decode_line("   ").is_none());
}

#[test]
fn parse_skips_format_refs() {
    // Pattern referencing a @format and &argset (skipped in simple parse)
    let line = "ADD_imm  sf:1 0 0 10001 sh:2 imm12:12 rn:5 rd:5  @addsub &ri";
    let dl = parse_decode_line(line).unwrap();
    assert_eq!(dl.mnemonic, "ADD_imm");
    assert_eq!(dl.pattern.fields.len(), 5);
}

#[test]
fn parse_bl() {
    let line = "BL  1 00101 imm26:26";
    let dl = parse_decode_line(line).unwrap();
    assert!(dl.pattern.matches(0x94000040));
}

#[test]
fn fields_extracted_correctly() {
    let line = "MOVZ  sf:1 10 100101 hw:2 imm16:16 rd:5";
    let dl = parse_decode_line(line).unwrap();
    let insn: u32 = 0xD2824680; // MOVZ X0, #0x1234
    assert!(dl.pattern.matches(insn));
    let fields = dl.pattern.extract_fields(insn);
    let sf = fields.iter().find(|(n, _)| *n == "sf").unwrap().1;
    let rd = fields.iter().find(|(n, _)| *n == "rd").unwrap().1;
    let imm16 = fields.iter().find(|(n, _)| *n == "imm16").unwrap().1;
    assert_eq!(sf, 1);
    assert_eq!(rd, 0);
    assert_eq!(imm16, 0x1234);
}

#[test]
fn parse_argset() {
    let aset = parse_arg_set("&ri rd rn imm").unwrap();
    assert_eq!(aset.name, "ri");
    assert_eq!(aset.fields, vec!["rd", "rn", "imm"]);
}

#[test]
fn parse_argset_non_argset_returns_none() {
    assert!(parse_arg_set("ADD ...").is_none());
    assert!(parse_arg_set("%rd 0:5").is_none());
}

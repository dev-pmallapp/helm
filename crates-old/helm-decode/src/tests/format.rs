use crate::format::*;

#[test]
fn parse_basic_format() {
    let fmt = parse_format_def("@addsub .... .... .. imm:12 rn:5 rd:5 &ri").unwrap();
    assert_eq!(fmt.name, "addsub");
    assert_eq!(fmt.arg_set, Some("ri".to_string()));
    assert!(!fmt.tokens.is_empty());
}

#[test]
fn parse_format_without_argset() {
    let fmt = parse_format_def("@branch .... .... imm26:26").unwrap();
    assert_eq!(fmt.name, "branch");
    assert!(fmt.arg_set.is_none());
}

#[test]
fn non_format_returns_none() {
    assert!(parse_format_def("ADD_imm ...").is_none());
    assert!(parse_format_def("%rd 0:5").is_none());
}

#[test]
fn format_name_stored_correctly() {
    let fmt = parse_format_def("@myfmt 1:3 2:5").unwrap();
    assert_eq!(fmt.name, "myfmt");
}

#[test]
fn format_tokens_include_bit_fields() {
    let fmt = parse_format_def("@addi imm:12 rn:5 rd:5").unwrap();
    assert_eq!(fmt.tokens.len(), 3);
    assert!(fmt.tokens.contains(&"imm:12".to_string()));
}

#[test]
fn format_without_tokens_has_empty_token_list() {
    let fmt = parse_format_def("@empty &myarg").unwrap();
    assert!(fmt.tokens.is_empty());
    assert_eq!(fmt.arg_set, Some("myarg".to_string()));
}

#[test]
fn format_argset_stripped_of_ampersand() {
    let fmt = parse_format_def("@f 0:1 &setname").unwrap();
    assert_eq!(fmt.arg_set, Some("setname".to_string()));
}

#[test]
fn parse_format_returns_none_for_pattern_line() {
    assert!(parse_format_def("ADD_imm sf:1 0 0 10001").is_none());
}

#[test]
fn parse_format_returns_none_for_empty_line() {
    assert!(parse_format_def("").is_none());
}

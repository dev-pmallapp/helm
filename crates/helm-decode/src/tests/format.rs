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

use crate::ir::*;

#[test]
fn tcg_temp_equality() {
    assert_eq!(TcgTemp(0), TcgTemp(0));
    assert_ne!(TcgTemp(0), TcgTemp(1));
}

#[test]
fn movi_stores_value() {
    let op = TcgOp::Movi {
        dst: TcgTemp(0),
        value: 0xDEAD,
    };
    if let TcgOp::Movi { value, .. } = op {
        assert_eq!(value, 0xDEAD);
    } else {
        panic!("wrong variant");
    }
}

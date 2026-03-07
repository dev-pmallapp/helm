use crate::se::backend::ExecBackend;

#[test]
fn interpretive_is_default() {
    let b = ExecBackend::default();
    assert!(matches!(b, ExecBackend::Interpretive));
}

#[test]
fn interpretive_constructor() {
    let b = ExecBackend::interpretive();
    assert!(matches!(b, ExecBackend::Interpretive));
}

#[test]
fn tcg_constructor() {
    let b = ExecBackend::tcg();
    assert!(matches!(b, ExecBackend::Tcg { .. }));
}

use crate::types::*;

#[test]
fn exec_mode_variants_are_distinct() {
    assert_ne!(ExecMode::SE, ExecMode::CAE);
}

#[test]
fn isa_kind_variants_are_distinct() {
    assert_ne!(IsaKind::X86_64, IsaKind::RiscV64);
    assert_ne!(IsaKind::RiscV64, IsaKind::Arm64);
}

#[test]
fn exec_mode_roundtrips_through_serde() {
    let mode = ExecMode::CAE;
    let json = serde_json::to_string(&mode).unwrap();
    let back: ExecMode = serde_json::from_str(&json).unwrap();
    assert_eq!(mode, back);
}

#[test]
fn isa_kind_roundtrips_through_serde() {
    for isa in [IsaKind::X86_64, IsaKind::RiscV64, IsaKind::Arm64] {
        let json = serde_json::to_string(&isa).unwrap();
        let back: IsaKind = serde_json::from_str(&json).unwrap();
        assert_eq!(isa, back);
    }
}

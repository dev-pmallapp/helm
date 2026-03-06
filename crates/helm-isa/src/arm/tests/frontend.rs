use crate::arm::*;
use crate::frontend::IsaFrontend;

#[test]
fn decode_produces_uop() {
    let fe = ArmFrontend::new();
    let bytes = [0u8; 16];
    let (uops, consumed) = fe.decode(0x4000, &bytes).unwrap();
    assert!(!uops.is_empty());
    assert_eq!(consumed, 4);
}

#[test]
fn name_is_aarch64() {
    assert_eq!(ArmFrontend::new().name(), "aarch64");
}

#[test]
fn alignment_is_word() {
    assert_eq!(ArmFrontend::new().min_insn_align(), 4);
}

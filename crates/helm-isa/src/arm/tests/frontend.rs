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

#[test]
fn decode_too_short_fails() {
    let fe = ArmFrontend::new();
    let bytes = [0u8; 3]; // need 4
    assert!(fe.decode(0x1000, &bytes).is_err());
}

#[test]
fn decode_exactly_4_bytes_succeeds() {
    let fe = ArmFrontend::new();
    let nop = 0xD503201Fu32.to_le_bytes(); // NOP
    let (uops, consumed) = fe.decode(0x2000, &nop).unwrap();
    assert_eq!(consumed, 4);
    assert_eq!(uops[0].guest_pc, 0x2000);
}

#[test]
fn decode_empty_bytes_fails() {
    let fe = ArmFrontend::new();
    assert!(fe.decode(0x0, &[]).is_err());
}

#[test]
fn arm_frontend_default_constructs() {
    let fe = ArmFrontend::default();
    assert_eq!(fe.name(), "aarch64");
}

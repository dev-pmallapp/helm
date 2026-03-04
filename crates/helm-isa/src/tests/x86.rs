use crate::frontend::IsaFrontend;
use crate::x86::*;

#[test]
fn decode_produces_uop() {
    let fe = X86Frontend::new();
    let bytes = [0x90u8; 16]; // NOP padding
    let (uops, consumed) = fe.decode(0x1000, &bytes).unwrap();
    assert!(!uops.is_empty());
    assert!(consumed > 0);
    assert_eq!(uops[0].guest_pc, 0x1000);
}

#[test]
fn name_is_x86_64() {
    assert_eq!(X86Frontend::new().name(), "x86_64");
}

#[test]
fn alignment_is_byte() {
    assert_eq!(X86Frontend::new().min_insn_align(), 1);
}

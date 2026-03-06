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

#[test]
fn x86_default_constructs() {
    let fe = X86Frontend::default();
    assert_eq!(fe.name(), "x86_64");
}

#[test]
fn decode_uop_pc_matches_input() {
    let fe = X86Frontend::new();
    let (uops, _) = fe.decode(0xDEAD_0000, &[0x90u8; 4]).unwrap();
    assert_eq!(uops[0].guest_pc, 0xDEAD_0000);
}

#[test]
fn decode_empty_bytes_still_returns_uop() {
    // Stub consumes 1 byte and emits NOP regardless
    let fe = X86Frontend::new();
    let (uops, consumed) = fe.decode(0x5000, &[0u8; 1]).unwrap();
    assert!(!uops.is_empty());
    assert!(consumed > 0);
}

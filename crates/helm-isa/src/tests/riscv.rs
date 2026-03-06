use crate::frontend::IsaFrontend;
use crate::riscv::*;

#[test]
fn decode_produces_uop() {
    let fe = RiscVFrontend::new();
    let bytes = [0u8; 16];
    let (uops, consumed) = fe.decode(0x8000_0000, &bytes).unwrap();
    assert!(!uops.is_empty());
    assert_eq!(consumed, 4);
    assert_eq!(uops[0].guest_pc, 0x8000_0000);
}

#[test]
fn name_is_riscv64() {
    assert_eq!(RiscVFrontend::new().name(), "riscv64");
}

#[test]
fn alignment_allows_compressed() {
    assert_eq!(RiscVFrontend::new().min_insn_align(), 2);
}

#[test]
fn riscv_default_constructs() {
    let fe = RiscVFrontend::default();
    assert_eq!(fe.name(), "riscv64");
}

#[test]
fn decode_uop_pc_matches_input() {
    let fe = RiscVFrontend::new();
    let (uops, _) = fe.decode(0xBEEF_0000, &[0u8; 4]).unwrap();
    assert_eq!(uops[0].guest_pc, 0xBEEF_0000);
}

#[test]
fn decode_always_consumes_4_bytes() {
    let fe = RiscVFrontend::new();
    let (_, consumed) = fe.decode(0x1000, &[0u8; 8]).unwrap();
    assert_eq!(consumed, 4);
}

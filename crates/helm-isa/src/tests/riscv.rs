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

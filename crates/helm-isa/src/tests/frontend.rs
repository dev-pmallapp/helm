use crate::arm::ArmFrontend;
use crate::frontend::*;
use crate::riscv::RiscVFrontend;
use crate::x86::X86Frontend;

// Verifies the trait is object-safe (can be used as dyn).
#[test]
fn trait_is_object_safe() {
    fn _accepts_dyn(_f: &dyn IsaFrontend) {}
}

#[test]
fn all_frontends_have_unique_names() {
    let a = ArmFrontend::new();
    let r = RiscVFrontend::new();
    let x = X86Frontend::new();
    let names = [a.name(), r.name(), x.name()];
    for i in 0..names.len() {
        for j in 0..names.len() {
            if i != j {
                assert_ne!(names[i], names[j], "frontend names must be distinct");
            }
        }
    }
}

#[test]
fn arm_frontend_as_dyn_decodes() {
    let fe: Box<dyn IsaFrontend> = Box::new(ArmFrontend::new());
    let bytes = [0xD5, 0x03, 0x20, 0x1F, 0, 0, 0, 0]; // NOP
    let (uops, sz) = fe.decode(0x1000, &bytes).unwrap();
    assert_eq!(sz, 4);
    assert!(!uops.is_empty());
}

#[test]
fn riscv_frontend_as_dyn_decodes() {
    let fe: Box<dyn IsaFrontend> = Box::new(RiscVFrontend::new());
    let (uops, sz) = fe.decode(0x8000_0000, &[0u8; 4]).unwrap();
    assert_eq!(sz, 4);
    assert!(!uops.is_empty());
}

#[test]
fn x86_frontend_as_dyn_decodes() {
    let fe: Box<dyn IsaFrontend> = Box::new(X86Frontend::new());
    let (uops, sz) = fe.decode(0x400_000, &[0x90u8; 4]).unwrap();
    assert!(sz > 0);
    assert!(!uops.is_empty());
}

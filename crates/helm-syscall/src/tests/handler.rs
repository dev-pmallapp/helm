use crate::os::linux::generic::*;
use helm_core::types::IsaKind;
use helm_memory::address_space::AddressSpace;

#[test]
fn brk_returns_current_when_zero() {
    let mut handler = SyscallHandler::new(IsaKind::X86_64);
    let mut addr_space = AddressSpace::new();
    let args = [0u64; 6];
    let result = handler.handle(12, &args, &mut addr_space).unwrap(); // brk(0)
    assert!(result > 0, "brk(0) should return current break address");
}

#[test]
fn brk_advances() {
    let mut handler = SyscallHandler::new(IsaKind::X86_64);
    let mut addr_space = AddressSpace::new();
    let current = handler.handle(12, &[0; 6], &mut addr_space).unwrap();
    let new_brk = current + 0x1000;
    let mut args = [0u64; 6];
    args[0] = new_brk;
    let result = handler.handle(12, &args, &mut addr_space).unwrap();
    assert_eq!(result, new_brk);
}

#[test]
fn exit_returns_status_code() {
    let mut handler = SyscallHandler::new(IsaKind::X86_64);
    let mut addr_space = AddressSpace::new();
    let mut args = [0u64; 6];
    args[0] = 42;
    let result = handler.handle(60, &args, &mut addr_space).unwrap(); // exit(42)
    assert_eq!(result, 42);
}

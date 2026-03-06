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

#[test]
fn write_syscall_returns_count() {
    let mut handler = SyscallHandler::new(IsaKind::X86_64);
    let mut addr_space = AddressSpace::new();
    addr_space.map(0x1000, 0x1000, (true, true, false));
    let data = b"hello";
    addr_space.write(0x1000, data).unwrap();
    let args = [1u64, 0x1000, data.len() as u64, 0, 0, 0];
    let result = handler.handle(1, &args, &mut addr_space).unwrap(); // write(1, buf, 5)
    assert_eq!(result, data.len() as u64);
}

#[test]
fn unknown_syscall_returns_max() {
    let mut handler = SyscallHandler::new(IsaKind::X86_64);
    let mut addr_space = AddressSpace::new();
    let result = handler.handle(99999, &[0; 6], &mut addr_space).unwrap();
    assert_eq!(result, u64::MAX);
}

#[test]
fn handler_tracks_brk_across_calls() {
    let mut handler = SyscallHandler::new(IsaKind::X86_64);
    let mut addr_space = AddressSpace::new();
    let initial = handler.handle(12, &[0; 6], &mut addr_space).unwrap();
    let bump1 = initial + 0x2000;
    handler.handle(12, &[bump1, 0, 0, 0, 0, 0], &mut addr_space).unwrap();
    let bump2 = bump1 + 0x3000;
    handler.handle(12, &[bump2, 0, 0, 0, 0, 0], &mut addr_space).unwrap();
    let current = handler.handle(12, &[0; 6], &mut addr_space).unwrap();
    assert_eq!(current, bump2);
}

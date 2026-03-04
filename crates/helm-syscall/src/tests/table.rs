use crate::os::linux::table::*;
use helm_core::types::IsaKind;

#[test]
fn x86_write_is_syscall_1() {
    assert_eq!(lookup(IsaKind::X86_64, 1), Syscall::Write);
}

#[test]
fn x86_exit_is_syscall_60() {
    assert_eq!(lookup(IsaKind::X86_64, 60), Syscall::Exit);
}

#[test]
fn riscv_exit_is_syscall_93() {
    assert_eq!(lookup(IsaKind::RiscV64, 93), Syscall::Exit);
}

#[test]
fn unknown_syscall_returns_unknown() {
    let sc = lookup(IsaKind::X86_64, 99999);
    assert!(matches!(sc, Syscall::Unknown(99999)));
}

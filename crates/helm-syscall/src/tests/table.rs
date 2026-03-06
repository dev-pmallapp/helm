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

#[test]
fn x86_read_is_syscall_0() {
    assert_eq!(lookup(IsaKind::X86_64, 0), Syscall::Read);
}

#[test]
fn aarch64_write_is_64() {
    assert_eq!(lookup(IsaKind::Arm64, 64), Syscall::Write);
}

#[test]
fn aarch64_read_is_63() {
    assert_eq!(lookup(IsaKind::Arm64, 63), Syscall::Read);
}

#[test]
fn aarch64_exit_is_93() {
    assert_eq!(lookup(IsaKind::Arm64, 93), Syscall::Exit);
}

#[test]
fn riscv_write_is_64() {
    assert_eq!(lookup(IsaKind::RiscV64, 64), Syscall::Write);
}

#[test]
fn unknown_high_number_all_isas() {
    for isa in [IsaKind::X86_64, IsaKind::Arm64, IsaKind::RiscV64] {
        let sc = lookup(isa, 0xFFFF_FFFF);
        assert!(matches!(sc, Syscall::Unknown(_)));
    }
}

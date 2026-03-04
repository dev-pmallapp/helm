//! Syscall number tables for supported ISAs.

use helm_core::types::IsaKind;

/// Well-known Linux syscall numbers (subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syscall {
    Read,
    Write,
    Open,
    Close,
    Mmap,
    Munmap,
    Brk,
    Exit,
    ExitGroup,
    Unknown(u64),
}

/// Map a raw syscall number to our enum, given the ISA.
pub fn lookup(isa: IsaKind, number: u64) -> Syscall {
    match isa {
        IsaKind::X86_64 => match number {
            0 => Syscall::Read,
            1 => Syscall::Write,
            2 => Syscall::Open,
            3 => Syscall::Close,
            9 => Syscall::Mmap,
            11 => Syscall::Munmap,
            12 => Syscall::Brk,
            60 => Syscall::Exit,
            231 => Syscall::ExitGroup,
            _ => Syscall::Unknown(number),
        },
        IsaKind::RiscV64 => match number {
            63 => Syscall::Read,
            64 => Syscall::Write,
            // Simplified — real table is larger.
            93 => Syscall::Exit,
            94 => Syscall::ExitGroup,
            _ => Syscall::Unknown(number),
        },
        IsaKind::Arm64 => match number {
            63 => Syscall::Read,
            64 => Syscall::Write,
            93 => Syscall::Exit,
            94 => Syscall::ExitGroup,
            _ => Syscall::Unknown(number),
        },
    }
}

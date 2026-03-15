//! Hart-level exceptions produced during instruction execution.

use thiserror::Error;

/// An exception raised by the hart during `execute()`.
///
/// Returned as `Err(HartException)` from `execute()`. The engine loop
/// dispatches to the appropriate handler (syscall, trap, GDB, exit).
#[derive(Debug, Clone, PartialEq, Error)]
pub enum HartException {
    /// Instruction encoding not recognised.
    #[error("illegal instruction at pc={pc:#x} (raw={raw:#010x})")]
    IllegalInstruction { pc: u64, raw: u32 },

    /// `ebreak` / software breakpoint.
    #[error("breakpoint at pc={pc:#x}")]
    Breakpoint { pc: u64 },

    /// `ecall` / environment call. `nr` is the syscall number (a7 on RISC-V, x8 on AArch64).
    #[error("environment call at pc={pc:#x} (nr={nr})")]
    EnvironmentCall { pc: u64, nr: u64 },

    /// Branch/jump target is not aligned.
    #[error("instruction address misaligned: {addr:#x}")]
    InstructionAddressMisaligned { addr: u64 },

    /// Load triggered a memory fault.
    #[error("load access fault at {addr:#x}")]
    LoadAccessFault { addr: u64 },

    /// Store/AMO triggered a memory fault.
    #[error("store/AMO access fault at {addr:#x}")]
    StoreAccessFault { addr: u64 },

    /// Fetch triggered a memory fault (e.g. PC out of range).
    #[error("instruction access fault at {addr:#x}")]
    InstructionAccessFault { addr: u64 },

    /// ISA operation not implemented yet.
    #[error("unsupported ISA operation")]
    Unsupported,

    /// Guest requested simulation exit (e.g. `exit_group` syscall).
    #[error("simulation exit with code {code}")]
    Exit { code: i32 },
}

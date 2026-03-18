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
    IllegalInstruction {
        /// Program counter of the offending instruction.
        pc: u64,
        /// Raw 32-bit instruction word that failed to decode or execute.
        raw: u32,
    },

    /// `ebreak` / software breakpoint.
    #[error("breakpoint at pc={pc:#x}")]
    Breakpoint {
        /// Program counter where the breakpoint was taken.
        pc: u64,
    },

    /// `ecall` / environment call. `nr` is the syscall number (a7 on RISC-V, x8 on AArch64).
    #[error("environment call at pc={pc:#x} (nr={nr})")]
    EnvironmentCall {
        /// Program counter of the trapping instruction.
        pc: u64,
        /// Decoded environment-call or syscall number.
        nr: u64,
    },

    /// Branch/jump target is not aligned.
    #[error("instruction address misaligned: {addr:#x}")]
    InstructionAddressMisaligned {
        /// Misaligned target address.
        addr: u64,
    },

    /// Load triggered a memory fault.
    #[error("load access fault at {addr:#x}")]
    LoadAccessFault {
        /// Faulting load address.
        addr: u64,
    },

    /// Store/AMO triggered a memory fault.
    #[error("store/AMO access fault at {addr:#x}")]
    StoreAccessFault {
        /// Faulting store or atomic address.
        addr: u64,
    },

    /// Fetch triggered a memory fault (e.g. PC out of range).
    #[error("instruction access fault at {addr:#x}")]
    InstructionAccessFault {
        /// Faulting instruction fetch address.
        addr: u64,
    },

    /// ISA operation not implemented yet.
    #[error("unsupported ISA operation")]
    Unsupported,

    /// Guest requested simulation exit (e.g. `exit_group` syscall).
    #[error("simulation exit with code {code}")]
    Exit {
        /// Guest-provided process exit code.
        code: i32,
    },
}

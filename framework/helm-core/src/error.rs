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

    /// `ecall` / environment call. `nr` is the syscall number (a7 on RISC-V, x8 on `AArch64`).
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

    /// WFI instruction -- hint to wait for interrupt.
    #[error("wait for interrupt")]
    WaitForInterrupt,

    /// PSCI firmware call that must be handled at machine level.
    #[error("psci call func={function:#x} via {conduit}")]
    PsciCall {
        /// `hvc` or `smc`.
        conduit: &'static str,
        /// PSCI function identifier in X0.
        function: u32,
        /// X1 argument.
        arg1: u64,
        /// X2 argument.
        arg2: u64,
        /// X3 argument.
        arg3: u64,
    },

    /// Data abort (MMU translation fault or permission fault).
    #[error("data abort at {addr:#x} (iss={iss:#x})")]
    DataAbort {
        /// Faulting data address.
        addr: u64,
        /// Instruction-specific syndrome.
        iss: u32,
        /// Override target exception level for virtualization faults.
        target_el: Option<u8>,
        /// Intermediate physical address to report via hypervisor fault state.
        ipa: Option<u64>,
    },

    /// Instruction abort (MMU translation fault on fetch).
    #[error("instruction abort at {addr:#x} (iss={iss:#x})")]
    InstructionAbort {
        /// Faulting instruction address.
        addr: u64,
        /// Instruction-specific syndrome.
        iss: u32,
        /// Override target exception level for virtualization faults.
        target_el: Option<u8>,
        /// Intermediate physical address to report via hypervisor fault state.
        ipa: Option<u64>,
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

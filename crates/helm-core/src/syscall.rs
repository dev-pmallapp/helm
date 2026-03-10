//! Syscall handler trait for SE mode.

use crate::cpu::CpuState;
use crate::mem::MemoryAccess;
use crate::types::Addr;

/// Action returned by syscall emulation.
#[derive(Debug)]
pub enum SyscallAction {
    /// Syscall fully handled; value goes into return register.
    Handled(u64),
    /// Current thread should block on FUTEX_WAIT.
    FutexWait { uaddr: Addr, val: u32 },
    /// Wake threads on FUTEX_WAKE; return wake-count.
    FutexWake { uaddr: Addr, count: u32 },
    /// Spawn a new thread via clone.
    Clone {
        flags: u64,
        child_stack: Addr,
        parent_tid_ptr: Addr,
        child_tid_ptr: Addr,
        tls: u64,
    },
    /// Current thread exits.
    ThreadExit { code: u64 },
    /// Process exits.
    Exit { code: u64 },
}

/// Syscall handler for SE mode.
pub trait SyscallHandler: Send {
    fn handle(
        &mut self,
        nr: u64,
        cpu: &mut dyn CpuState,
        mem: &mut dyn MemoryAccess,
    ) -> SyscallAction;
}

//! Callback type definitions.

use super::info::*;

/// Callback invoked when a vCPU is initialised.
pub type VcpuInitCb = Box<dyn Fn(usize) + Send + Sync>;

/// Callback invoked when a vCPU exits.
pub type VcpuExitCb = Box<dyn Fn(usize) + Send + Sync>;

/// Callback invoked once per translated block during translation.
pub type TbTransCb = Box<dyn Fn(&TbInfo, &[InsnInfo]) + Send + Sync>;

/// Callback invoked every time a TB executes.
pub type TbExecCb = Box<dyn Fn(usize, &TbInfo) + Send + Sync>;

/// Callback invoked every time an instruction executes.
pub type InsnExecCb = Box<dyn Fn(usize, &InsnInfo) + Send + Sync>;

/// Callback invoked on memory access.
pub type MemAccessCb = Box<dyn Fn(usize, &MemInfo) + Send + Sync>;

/// Callback invoked on syscall entry.
pub type SyscallCb = Box<dyn Fn(&SyscallInfo) + Send + Sync>;

/// Callback invoked on syscall return.
pub type SyscallRetCb = Box<dyn Fn(&SyscallRetInfo) + Send + Sync>;

/// Which memory accesses to observe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemFilter {
    All,
    ReadsOnly,
    WritesOnly,
}

impl MemFilter {
    pub fn matches(&self, is_store: bool) -> bool {
        match self {
            Self::All => true,
            Self::ReadsOnly => !is_store,
            Self::WritesOnly => is_store,
        }
    }
}

/// Callback invoked when the engine detects an execution fault.
pub type FaultCb = Box<dyn Fn(&FaultInfo) + Send + Sync>;

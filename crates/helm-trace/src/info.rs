//! Introspection types passed to plugin callbacks.

use helm_core::types::Addr;

/// Read-only view of an instruction during translation.
#[derive(Debug, Clone)]
pub struct InsnInfo {
    pub vaddr: Addr,
    pub bytes: Vec<u8>,
    pub size: usize,
    pub mnemonic: String,
    pub symbol: Option<String>,
}

/// Read-only view of a translated block.
#[derive(Debug, Clone)]
pub struct TbInfo {
    pub pc: Addr,
    pub insn_count: usize,
    pub size: usize,
}

/// Memory access details provided to mem callbacks.
#[derive(Debug, Clone)]
pub struct MemInfo {
    pub vaddr: Addr,
    pub size: usize,
    pub is_store: bool,
    pub paddr: Option<Addr>,
}

/// Syscall entry details.
#[derive(Debug, Clone)]
pub struct SyscallInfo {
    pub number: u64,
    pub args: [u64; 6],
    pub vcpu_idx: usize,
}

/// Syscall return details.
#[derive(Debug, Clone)]
pub struct SyscallRetInfo {
    pub number: u64,
    pub ret_value: u64,
    pub vcpu_idx: usize,
}

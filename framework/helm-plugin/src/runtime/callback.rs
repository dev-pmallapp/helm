use super::info::*;

pub type InsnExecCb = Box<dyn Fn(usize, &PluginInsnInfo) + Send + Sync>;
pub type MemAccessCb = Box<dyn Fn(usize, &MemInfo) + Send + Sync>;
pub type BranchCb = Box<dyn Fn(usize, &BranchInfo) + Send + Sync>;
pub type SyscallCb = Box<dyn Fn(&SyscallInfo) + Send + Sync>;
pub type SyscallRetCb = Box<dyn Fn(&SyscallRetInfo) + Send + Sync>;
pub type FaultCb = Box<dyn Fn(&FaultInfo) + Send + Sync>;
pub type ExceptionCb = Box<dyn Fn(&ExceptionInfo) + Send + Sync>;
pub type VcpuInitCb = Box<dyn Fn(usize) + Send + Sync>;
pub type VcpuExitCb = Box<dyn Fn(usize) + Send + Sync>;
pub type TimerCb = Box<dyn Fn(usize, u64) + Send + Sync>; // (vcpu_idx, insn_count)

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

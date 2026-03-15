use super::info::*;

pub type InsnExecCb    = Box<dyn Fn(usize, &InsnInfo) + Send + Sync>;
pub type MemAccessCb   = Box<dyn Fn(usize, &MemInfo) + Send + Sync>;
pub type BranchCb      = Box<dyn Fn(usize, &BranchInfo) + Send + Sync>;
pub type SyscallCb     = Box<dyn Fn(&SyscallInfo) + Send + Sync>;
pub type SyscallRetCb  = Box<dyn Fn(&SyscallRetInfo) + Send + Sync>;
pub type FaultCb       = Box<dyn Fn(&FaultInfo) + Send + Sync>;
pub type VcpuInitCb    = Box<dyn Fn(usize) + Send + Sync>;
pub type VcpuExitCb    = Box<dyn Fn(usize) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemFilter { All, ReadsOnly, WritesOnly }

impl MemFilter {
    pub fn matches(&self, is_store: bool) -> bool {
        match self { Self::All => true, Self::ReadsOnly => !is_store, Self::WritesOnly => is_store }
    }
}

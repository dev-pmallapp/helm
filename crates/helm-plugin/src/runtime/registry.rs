use super::callback::*;
use super::info::*;

#[derive(Default)]
pub struct PluginRegistry {
    pub insn_exec:    Vec<InsnExecCb>,
    pub mem_access:   Vec<(MemFilter, MemAccessCb)>,
    pub branch:       Vec<BranchCb>,
    pub syscall:      Vec<SyscallCb>,
    pub syscall_ret:  Vec<SyscallRetCb>,
    pub fault:        Vec<FaultCb>,
    pub vcpu_init:    Vec<VcpuInitCb>,
    pub vcpu_exit:    Vec<VcpuExitCb>,
}

impl PluginRegistry {
    pub fn new() -> Self { Self::default() }

    // Registration methods
    pub fn on_insn_exec(&mut self, cb: InsnExecCb) { self.insn_exec.push(cb); }
    pub fn on_mem_access(&mut self, filter: MemFilter, cb: MemAccessCb) { self.mem_access.push((filter, cb)); }
    pub fn on_branch(&mut self, cb: BranchCb) { self.branch.push(cb); }
    pub fn on_syscall(&mut self, cb: SyscallCb) { self.syscall.push(cb); }
    pub fn on_syscall_ret(&mut self, cb: SyscallRetCb) { self.syscall_ret.push(cb); }
    pub fn on_fault(&mut self, cb: FaultCb) { self.fault.push(cb); }
    pub fn on_vcpu_init(&mut self, cb: VcpuInitCb) { self.vcpu_init.push(cb); }
    pub fn on_vcpu_exit(&mut self, cb: VcpuExitCb) { self.vcpu_exit.push(cb); }

    // Fast-path flags
    pub fn has_insn_callbacks(&self) -> bool { !self.insn_exec.is_empty() }
    pub fn has_mem_callbacks(&self) -> bool { !self.mem_access.is_empty() }
    pub fn has_branch_callbacks(&self) -> bool { !self.branch.is_empty() }

    // Dispatch methods
    pub fn fire_insn_exec(&self, vcpu: usize, insn: &InsnInfo) {
        for cb in &self.insn_exec { cb(vcpu, insn); }
    }
    pub fn fire_mem_access(&self, vcpu: usize, info: &MemInfo) {
        for (filter, cb) in &self.mem_access { if filter.matches(info.is_store) { cb(vcpu, info); } }
    }
    pub fn fire_branch(&self, vcpu: usize, info: &BranchInfo) {
        for cb in &self.branch { cb(vcpu, info); }
    }
    pub fn fire_syscall(&self, info: &SyscallInfo) {
        for cb in &self.syscall { cb(info); }
    }
    pub fn fire_syscall_ret(&self, info: &SyscallRetInfo) {
        for cb in &self.syscall_ret { cb(info); }
    }
    pub fn fire_fault(&self, info: &FaultInfo) {
        for cb in &self.fault { cb(info); }
    }
    pub fn fire_vcpu_init(&self, vcpu: usize) {
        for cb in &self.vcpu_init { cb(vcpu); }
    }
}

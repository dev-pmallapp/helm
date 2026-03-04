//! Callback registry — stores all registered plugin callbacks.

use super::callback::*;
use super::info::*;

/// Stores all registered callbacks from all plugins.
#[derive(Default)]
pub struct PluginRegistry {
    pub vcpu_init: Vec<VcpuInitCb>,
    pub vcpu_exit: Vec<VcpuExitCb>,
    pub tb_trans: Vec<TbTransCb>,
    pub tb_exec: Vec<TbExecCb>,
    pub insn_exec: Vec<InsnExecCb>,
    pub mem_access: Vec<(MemFilter, MemAccessCb)>,
    pub syscall: Vec<SyscallCb>,
    pub syscall_ret: Vec<SyscallRetCb>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_vcpu_init(&mut self, cb: VcpuInitCb) {
        self.vcpu_init.push(cb);
    }

    pub fn on_vcpu_exit(&mut self, cb: VcpuExitCb) {
        self.vcpu_exit.push(cb);
    }

    pub fn on_tb_translate(&mut self, cb: TbTransCb) {
        self.tb_trans.push(cb);
    }

    pub fn on_tb_exec(&mut self, cb: TbExecCb) {
        self.tb_exec.push(cb);
    }

    pub fn on_insn_exec(&mut self, cb: InsnExecCb) {
        self.insn_exec.push(cb);
    }

    pub fn on_mem_access(&mut self, filter: MemFilter, cb: MemAccessCb) {
        self.mem_access.push((filter, cb));
    }

    pub fn on_syscall(&mut self, cb: SyscallCb) {
        self.syscall.push(cb);
    }

    pub fn on_syscall_ret(&mut self, cb: SyscallRetCb) {
        self.syscall_ret.push(cb);
    }

    // -- Dispatch helpers (called by the engine) --------------------------

    pub fn fire_vcpu_init(&self, vcpu_idx: usize) {
        for cb in &self.vcpu_init {
            cb(vcpu_idx);
        }
    }

    pub fn fire_tb_exec(&self, vcpu_idx: usize, tb: &TbInfo) {
        for cb in &self.tb_exec {
            cb(vcpu_idx, tb);
        }
    }

    pub fn fire_insn_exec(&self, vcpu_idx: usize, insn: &InsnInfo) {
        for cb in &self.insn_exec {
            cb(vcpu_idx, insn);
        }
    }

    pub fn fire_mem_access(&self, vcpu_idx: usize, info: &MemInfo) {
        for (filter, cb) in &self.mem_access {
            if filter.matches(info.is_store) {
                cb(vcpu_idx, info);
            }
        }
    }

    pub fn fire_syscall(&self, info: &SyscallInfo) {
        for cb in &self.syscall {
            cb(info);
        }
    }

    pub fn fire_syscall_ret(&self, info: &SyscallRetInfo) {
        for cb in &self.syscall_ret {
            cb(info);
        }
    }

    pub fn has_insn_callbacks(&self) -> bool {
        !self.insn_exec.is_empty()
    }

    pub fn has_mem_callbacks(&self) -> bool {
        !self.mem_access.is_empty()
    }
}

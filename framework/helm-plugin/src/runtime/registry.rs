use super::callback::*;
use super::info::*;

/// Bitmask bits for callback presence (avoids checking Vec emptiness per instruction).
const CB_INSN: u32 = 1 << 0;
const CB_MEM: u32 = 1 << 1;
const CB_BRANCH: u32 = 1 << 2;
const CB_TIMER: u32 = 1 << 3;
const CB_FAULT: u32 = 1 << 4;

#[derive(Default)]
/// Legacy callback registry used by compatibility plugins.
///
/// The long-term primary observability path is probes feeding `helm-spy`
/// sessions and `helm-report` delivery. Keep new instrumentation work off this
/// registry unless compatibility requires it.
pub struct HelmPluginRegistry {
    pub insn_exec: Vec<InsnExecCb>,
    pub mem_access: Vec<(MemFilter, MemAccessCb)>,
    pub branch: Vec<BranchCb>,
    pub syscall: Vec<SyscallCb>,
    pub syscall_ret: Vec<SyscallRetCb>,
    pub fault: Vec<FaultCb>,
    pub vcpu_init: Vec<VcpuInitCb>,
    pub vcpu_exit: Vec<VcpuExitCb>,
    pub timer: Vec<(u64, TimerCb)>, // (interval_insns, callback)
    /// Cached bitmask of which callback types have subscribers.
    /// Updated on registration; avoids per-instruction Vec::is_empty() checks.
    cb_mask: u32,
}

impl HelmPluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    // Registration methods
    pub fn on_insn_exec(&mut self, cb: InsnExecCb) {
        self.insn_exec.push(cb);
        self.cb_mask |= CB_INSN;
    }
    pub fn on_mem_access(&mut self, filter: MemFilter, cb: MemAccessCb) {
        self.mem_access.push((filter, cb));
        self.cb_mask |= CB_MEM;
    }
    pub fn on_branch(&mut self, cb: BranchCb) {
        self.branch.push(cb);
        self.cb_mask |= CB_BRANCH;
    }
    pub fn on_syscall(&mut self, cb: SyscallCb) {
        self.syscall.push(cb);
    }
    pub fn on_syscall_ret(&mut self, cb: SyscallRetCb) {
        self.syscall_ret.push(cb);
    }
    pub fn on_fault(&mut self, cb: FaultCb) {
        self.fault.push(cb);
        self.cb_mask |= CB_FAULT;
    }
    pub fn on_vcpu_init(&mut self, cb: VcpuInitCb) {
        self.vcpu_init.push(cb);
    }
    pub fn on_vcpu_exit(&mut self, cb: VcpuExitCb) {
        self.vcpu_exit.push(cb);
    }
    pub fn on_timer(&mut self, interval: u64, cb: TimerCb) {
        self.timer.push((interval, cb));
        self.cb_mask |= CB_TIMER;
    }

    // Fast-path flags — single u32 bitmask test instead of Vec::is_empty()
    #[inline]
    pub fn has_insn_callbacks(&self) -> bool {
        self.cb_mask & CB_INSN != 0
    }
    #[inline]
    pub fn has_mem_callbacks(&self) -> bool {
        self.cb_mask & CB_MEM != 0
    }
    #[inline]
    pub fn has_branch_callbacks(&self) -> bool {
        self.cb_mask & CB_BRANCH != 0
    }
    #[inline]
    pub fn has_fault_callbacks(&self) -> bool {
        self.cb_mask & CB_FAULT != 0
    }
    #[inline]
    pub fn has_timer_callbacks(&self) -> bool {
        self.cb_mask & CB_TIMER != 0
    }

    /// Returns `true` if any hot-path callback type has subscribers.
    /// Single u32 test — no Vec::is_empty() checks on the hot path.
    #[inline]
    pub fn has_any_callbacks(&self) -> bool {
        self.cb_mask & (CB_INSN | CB_MEM | CB_BRANCH | CB_TIMER) != 0
    }

    // Dispatch methods
    pub fn fire_insn_exec(&self, vcpu: usize, insn: &PluginInsnInfo) {
        for cb in &self.insn_exec {
            cb(vcpu, insn);
        }
    }
    pub fn fire_mem_access(&self, vcpu: usize, info: &MemInfo) {
        for (filter, cb) in &self.mem_access {
            if filter.matches(info.is_store) {
                cb(vcpu, info);
            }
        }
    }
    pub fn fire_branch(&self, vcpu: usize, info: &BranchInfo) {
        for cb in &self.branch {
            cb(vcpu, info);
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
    pub fn fire_fault(&self, info: &FaultInfo) {
        for cb in &self.fault {
            cb(info);
        }
    }
    pub fn fire_vcpu_init(&self, vcpu: usize) {
        for cb in &self.vcpu_init {
            cb(vcpu);
        }
    }
    pub fn fire_vcpu_exit(&self, vcpu: usize) {
        for cb in &self.vcpu_exit {
            cb(vcpu);
        }
    }
    pub fn fire_timer(&self, vcpu: usize, insn_count: u64) {
        for (interval, cb) in &self.timer {
            if insn_count % interval == 0 {
                cb(vcpu, insn_count);
            }
        }
    }
}

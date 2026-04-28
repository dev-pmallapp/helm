use super::callback::*;
use super::info::*;

/// Bitmask bits for callback presence (avoids checking Vec emptiness per instruction).
const CB_INSN: u32 = 1 << 0;
const CB_MEM: u32 = 1 << 1;
const CB_BRANCH: u32 = 1 << 2;
const CB_TIMER: u32 = 1 << 3;
const CB_FAULT: u32 = 1 << 4;
const CB_SYSCALL: u32 = 1 << 5;
const CB_SYSCALL_RET: u32 = 1 << 6;
const CB_VCPU_INIT: u32 = 1 << 7;
const CB_VCPU_EXIT: u32 = 1 << 8;
const CB_EXCEPTION: u32 = 1 << 9;
const CB_JIT_BLOCK: u32 = 1 << 10;
const CB_JIT_FALLBACK: u32 = 1 << 11;

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
    pub exception: Vec<ExceptionCb>,
    pub vcpu_init: Vec<VcpuInitCb>,
    pub vcpu_exit: Vec<VcpuExitCb>,
    pub timer: Vec<(u64, TimerCb)>, // (interval_insns, callback)
    pub jit_block: Vec<JitBlockCb>,
    pub jit_fallback: Vec<JitFallbackCb>,
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
        self.cb_mask |= CB_SYSCALL;
    }
    pub fn on_syscall_ret(&mut self, cb: SyscallRetCb) {
        self.syscall_ret.push(cb);
        self.cb_mask |= CB_SYSCALL_RET;
    }
    pub fn on_fault(&mut self, cb: FaultCb) {
        self.fault.push(cb);
        self.cb_mask |= CB_FAULT;
    }
    pub fn on_exception(&mut self, cb: ExceptionCb) {
        self.exception.push(cb);
        self.cb_mask |= CB_EXCEPTION;
    }
    pub fn on_vcpu_init(&mut self, cb: VcpuInitCb) {
        self.vcpu_init.push(cb);
        self.cb_mask |= CB_VCPU_INIT;
    }
    pub fn on_vcpu_exit(&mut self, cb: VcpuExitCb) {
        self.vcpu_exit.push(cb);
        self.cb_mask |= CB_VCPU_EXIT;
    }
    pub fn on_timer(&mut self, interval: u64, cb: TimerCb) {
        self.timer.push((interval, cb));
        self.cb_mask |= CB_TIMER;
    }
    pub fn on_jit_block(&mut self, cb: JitBlockCb) {
        self.jit_block.push(cb);
        self.cb_mask |= CB_JIT_BLOCK;
    }
    pub fn on_jit_fallback(&mut self, cb: JitFallbackCb) {
        self.jit_fallback.push(cb);
        self.cb_mask |= CB_JIT_FALLBACK;
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
    pub fn has_exception_callbacks(&self) -> bool {
        self.cb_mask & CB_EXCEPTION != 0
    }
    #[inline]
    pub fn has_timer_callbacks(&self) -> bool {
        self.cb_mask & CB_TIMER != 0
    }
    #[inline]
    pub fn has_jit_block_callbacks(&self) -> bool {
        self.cb_mask & CB_JIT_BLOCK != 0
    }
    #[inline]
    pub fn has_jit_fallback_callbacks(&self) -> bool {
        self.cb_mask & CB_JIT_FALLBACK != 0
    }

    /// Returns `true` if any hot-path callback type has subscribers.
    /// Single u32 test — no Vec::is_empty() checks on the hot path.
    #[inline]
    pub fn has_any_callbacks(&self) -> bool {
        self.cb_mask
            & (CB_INSN
                | CB_MEM
                | CB_BRANCH
                | CB_TIMER
                | CB_FAULT
                | CB_EXCEPTION
                | CB_SYSCALL
                | CB_SYSCALL_RET
                | CB_VCPU_INIT
                | CB_VCPU_EXIT
                | CB_JIT_BLOCK
                | CB_JIT_FALLBACK)
            != 0
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
    pub fn fire_exception(&self, info: &ExceptionInfo) {
        for cb in &self.exception {
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
    pub fn fire_jit_block(&self, info: &JitBlockInfo) {
        for cb in &self.jit_block {
            cb(info);
        }
    }
    pub fn fire_jit_fallback(&self, pc: u64, reason: Option<&'static str>) {
        for cb in &self.jit_fallback {
            cb(pc, reason);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syscall_only_plugins_count_as_callbacks() {
        let mut reg = HelmPluginRegistry::new();
        reg.on_syscall(Box::new(|_info| {}));

        assert!(reg.has_any_callbacks());
    }

    #[test]
    fn syscall_ret_only_plugins_count_as_callbacks() {
        let mut reg = HelmPluginRegistry::new();
        reg.on_syscall_ret(Box::new(|_info| {}));

        assert!(reg.has_any_callbacks());
    }

    #[test]
    fn vcpu_lifecycle_plugins_count_as_callbacks() {
        let mut reg = HelmPluginRegistry::new();
        reg.on_vcpu_init(Box::new(|_vcpu| {}));
        reg.on_vcpu_exit(Box::new(|_vcpu| {}));

        assert!(reg.has_any_callbacks());
    }

    #[test]
    fn fault_only_plugins_count_as_callbacks() {
        let mut reg = HelmPluginRegistry::new();
        reg.on_fault(Box::new(|_info| {}));

        assert!(reg.has_fault_callbacks());
        assert!(reg.has_any_callbacks());
    }
}

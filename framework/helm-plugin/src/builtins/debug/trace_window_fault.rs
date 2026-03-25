use crate::api::{HelmPlugin, HelmPluginArgs};
use crate::runtime::{
    ArchContext, BranchInfo, InsnClass, HelmPluginRegistry, SyscallInfo,
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
struct RecentInsn {
    pc: u64,
    raw: u32,
    class: InsnClass,
    opcode_name: &'static str,
    is_stub: bool,
}

#[derive(Clone, Debug, Default)]
struct RecentMem {
    vaddr: u64,
    size: u8,
    is_store: bool,
    is_atomic: bool,
}

#[derive(Clone, Debug)]
struct RecentBranch {
    pc: u64,
    target: u64,
    taken: bool,
    kind: crate::runtime::BranchKind,
}

#[derive(Clone, Debug, Default)]
struct RecentSyscall {
    vcpu_idx: usize,
    number: u64,
    args: [u64; 6],
}

#[derive(Clone, Debug)]
struct Ring<T> {
    items: Vec<T>,
    head: usize,
    filled: usize,
}

impl<T: Clone> Ring<T> {
    fn new(capacity: usize, default: T) -> Self {
        Self {
            items: vec![default; capacity.max(1)],
            head: 0,
            filled: 0,
        }
    }

    fn push(&mut self, value: T) {
        self.items[self.head] = value;
        self.head = (self.head + 1) % self.items.len();
        if self.filled < self.items.len() {
            self.filled += 1;
        }
    }

    fn entries(&self) -> Vec<T> {
        if self.filled == 0 {
            return Vec::new();
        }
        let start = if self.filled < self.items.len() { 0 } else { self.head };
        let mut out = Vec::with_capacity(self.filled);
        for i in 0..self.filled {
            out.push(self.items[(start + i) % self.items.len()].clone());
        }
        out
    }
}

struct Inner {
    insns: Ring<RecentInsn>,
    mem: Ring<RecentMem>,
    branches: Ring<RecentBranch>,
    syscalls: Ring<RecentSyscall>,
}

impl Inner {
    fn new(insns: usize, mem: usize, branches: usize, syscalls: usize) -> Self {
        Self {
            insns: Ring::new(
                insns,
                RecentInsn {
                    pc: 0,
                    raw: 0,
                    class: InsnClass::Unknown,
                    opcode_name: "",
                    is_stub: false,
                },
            ),
            mem: Ring::new(mem, RecentMem::default()),
            branches: Ring::new(
                branches,
                RecentBranch {
                    pc: 0,
                    target: 0,
                    taken: false,
                    kind: crate::runtime::BranchKind::DirectUncond,
                },
            ),
            syscalls: Ring::new(syscalls, RecentSyscall::default()),
        }
    }
}

/// Debug plugin: keep rolling trace windows and dump them on fault.
pub struct TraceWindowFault {
    inner: Arc<Mutex<Inner>>,
}

impl TraceWindowFault {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::new(32, 16, 16, 8))),
        }
    }
}

impl Default for TraceWindowFault {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for TraceWindowFault {
    fn name(&self) -> &str {
        "trace_window_fault"
    }

    fn install(&mut self, reg: &mut HelmPluginRegistry, args: &HelmPluginArgs) {
        let insn_hist = args.get_usize("insns").unwrap_or(32);
        let mem_hist = args.get_usize("mem").unwrap_or(16);
        let branch_hist = args.get_usize("branches").unwrap_or(16);
        let syscall_hist = args.get_usize("syscalls").unwrap_or(8);
        self.inner = Arc::new(Mutex::new(Inner::new(
            insn_hist,
            mem_hist,
            branch_hist,
            syscall_hist,
        )));

        let inner_insn = Arc::clone(&self.inner);
        reg.on_insn_exec(Box::new(move |_vcpu_idx, insn| {
            inner_insn.lock().unwrap().insns.push(RecentInsn {
                pc: insn.pc,
                raw: insn.raw,
                class: insn.class,
                opcode_name: insn.opcode_name,
                is_stub: insn.is_stub,
            });
        }));

        let inner_mem = Arc::clone(&self.inner);
        reg.on_mem_access(crate::runtime::MemFilter::All, Box::new(move |_vcpu_idx, info| {
            inner_mem.lock().unwrap().mem.push(RecentMem {
                vaddr: info.vaddr,
                size: info.size,
                is_store: info.is_store,
                is_atomic: info.is_atomic,
            });
        }));

        let inner_branch = Arc::clone(&self.inner);
        reg.on_branch(Box::new(move |_vcpu_idx, info: &BranchInfo| {
            inner_branch.lock().unwrap().branches.push(RecentBranch {
                pc: info.pc,
                target: info.target,
                taken: info.taken,
                kind: info.kind,
            });
        }));

        let inner_syscall = Arc::clone(&self.inner);
        reg.on_syscall(Box::new(move |info: &SyscallInfo| {
            inner_syscall.lock().unwrap().syscalls.push(RecentSyscall {
                vcpu_idx: info.vcpu_idx,
                number: info.number,
                args: info.args,
            });
        }));

        let inner_fault = Arc::clone(&self.inner);
        reg.on_fault(Box::new(move |fault| {
            let guard = inner_fault.lock().unwrap();
            eprintln!("[trace-window-fault] ====== FAULT ======");
            eprintln!(
                "[trace-window-fault] vcpu={} pc={:#018x} kind={} insn_count={}",
                fault.vcpu_idx, fault.pc, fault.kind, fault.insn_count
            );
            eprintln!("[trace-window-fault] message: {}", fault.message);
            eprintln!("[trace-window-fault] raw={:#010x}", fault.raw);
            match &fault.context {
                ArchContext::Aarch64 { x, sp, pc, nzcv } => {
                    eprintln!(
                        "[trace-window-fault] arch=AArch64 pc={:#018x} sp={:#018x} nzcv={:#010x}",
                        pc, sp, nzcv
                    );
                    for (i, r) in x.iter().enumerate() {
                        if *r != 0 {
                            eprintln!("[trace-window-fault]   x{:<2} = {:#018x}", i, r);
                        }
                    }
                }
                ArchContext::RiscV { x, pc } => {
                    eprintln!("[trace-window-fault] arch=RiscV pc={:#018x}", pc);
                    for (i, r) in x.iter().enumerate() {
                        if *r != 0 {
                            eprintln!("[trace-window-fault]   x{:<2} = {:#018x}", i, r);
                        }
                    }
                }
                ArchContext::None => {}
            }

            let insns = guard.insns.entries();
            eprintln!(
                "[trace-window-fault] recent instructions ({}):",
                insns.len()
            );
            for (i, insn) in insns.iter().enumerate() {
                eprintln!(
                    "[trace-window-fault]   insn[{i:02}] pc={:#018x} raw={:#010x} opcode={} class={:?} stub={}",
                    insn.pc, insn.raw, insn.opcode_name, insn.class, insn.is_stub
                );
            }

            let mem = guard.mem.entries();
            eprintln!("[trace-window-fault] recent mem ({}):", mem.len());
            for (i, access) in mem.iter().enumerate() {
                let kind = if access.is_store { 'W' } else { 'R' };
                let atomic = if access.is_atomic { " atomic" } else { "" };
                eprintln!(
                    "[trace-window-fault]   mem[{i:02}] [{kind}] {:#018x} size={}{}",
                    access.vaddr, access.size, atomic
                );
            }

            let branches = guard.branches.entries();
            eprintln!("[trace-window-fault] recent branches ({}):", branches.len());
            for (i, branch) in branches.iter().enumerate() {
                let taken = if branch.taken { 'T' } else { 'N' };
                eprintln!(
                    "[trace-window-fault]   br[{i:02}] {:#018x} -> {:#018x} [{taken}] {:?}",
                    branch.pc, branch.target, branch.kind
                );
            }

            let syscalls = guard.syscalls.entries();
            if !syscalls.is_empty() {
                eprintln!("[trace-window-fault] recent syscalls ({}):", syscalls.len());
                for (i, sc) in syscalls.iter().enumerate() {
                    eprintln!(
                        "[trace-window-fault]   sc[{i:02}] vcpu={} nr={} args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x}]",
                        sc.vcpu_idx,
                        sc.number,
                        sc.args[0],
                        sc.args[1],
                        sc.args[2],
                        sc.args[3],
                        sc.args[4],
                        sc.args[5],
                    );
                }
            }
            eprintln!("[trace-window-fault] ==================");
        }));
    }
}

#[cfg(test)]
#[path = "tests/trace_window_fault.rs"]
mod tests;

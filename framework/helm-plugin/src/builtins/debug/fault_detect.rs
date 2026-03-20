use crate::api::{HelmPlugin, PluginArgs};
use crate::runtime::{InsnClass, InsnInfo, PluginRegistry};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
struct RecentInsn {
    pc: u64,
    raw: u32,
    class: InsnClass,
    opcode_name: &'static str,
    is_stub: bool,
}

/// Ring-buffer state shared between callbacks.
struct Inner {
    /// Ring buffer of recent executed instructions.
    ring: Vec<RecentInsn>,
    /// Next write position.
    head: usize,
    /// How many entries have been written (caps at ring.len()).
    filled: usize,
    /// Syscall log entries.
    syscall_log: Vec<String>,
}

impl Inner {
    fn new(capacity: usize) -> Self {
        Self {
            ring: vec![
                RecentInsn {
                    pc: 0,
                    raw: 0,
                    class: InsnClass::Unknown,
                    opcode_name: "",
                    is_stub: false,
                };
                capacity.max(1)
            ],
            head: 0,
            filled: 0,
            syscall_log: Vec::new(),
        }
    }

    fn push_insn(&mut self, insn: &InsnInfo) {
        let cap = self.ring.len();
        self.ring[self.head % cap] = RecentInsn {
            pc: insn.pc,
            raw: insn.raw,
            class: insn.class,
            opcode_name: insn.opcode_name,
            is_stub: insn.is_stub,
        };
        self.head += 1;
        if self.filled < cap {
            self.filled += 1;
        }
    }

    /// Iterate entries oldest -> newest.
    fn recent_insns(&self) -> Vec<RecentInsn> {
        let cap = self.ring.len();
        let count = self.filled;
        if count == 0 {
            return vec![];
        }
        let start = if count < cap { 0 } else { self.head % cap };
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            out.push(self.ring[(start + i) % cap].clone());
        }
        out
    }
}

/// Execution fault detector with ring-buffer PC history.
pub struct FaultDetect {
    inner: Arc<Mutex<Inner>>,
}

impl FaultDetect {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::new(64))),
        }
    }
}

impl Default for FaultDetect {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for FaultDetect {
    fn name(&self) -> &str {
        "fault_detect"
    }

    fn install(&mut self, reg: &mut PluginRegistry, args: &PluginArgs) {
        let history = args.get_usize("history").unwrap_or(64).max(1);
        // Re-create inner with the configured capacity.
        self.inner = Arc::new(Mutex::new(Inner::new(history)));

        // Callback 1: ring-buffer every executed instruction.
        let inner_insn = Arc::clone(&self.inner);
        reg.on_insn_exec(Box::new(move |_vcpu_idx, insn| {
            inner_insn.lock().unwrap().push_insn(insn);
        }));

        // Callback 2: log each syscall entry.
        let inner_sc = Arc::clone(&self.inner);
        reg.on_syscall(Box::new(move |info| {
            let line = format!(
                "vcpu={} syscall={} args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x}]",
                info.vcpu_idx,
                info.number,
                info.args[0],
                info.args[1],
                info.args[2],
                info.args[3],
                info.args[4],
                info.args[5],
            );
            inner_sc.lock().unwrap().syscall_log.push(line);
        }));

        // Callback 3: dump everything on fault.
        let inner_fault = Arc::clone(&self.inner);
        reg.on_fault(Box::new(move |fault| {
            let guard = inner_fault.lock().unwrap();
            eprintln!("[fault_detect] ====== FAULT DETECTED ======");
            eprintln!(
                "[fault_detect] vcpu={}  pc={:#018x}  kind={}  insn_count={}",
                fault.vcpu_idx, fault.pc, fault.kind, fault.insn_count
            );
            eprintln!("[fault_detect] message: {}", fault.message);
            eprintln!("[fault_detect] raw={:#010x}", fault.raw);

            // Arch context
            match &fault.context {
                crate::runtime::ArchContext::RiscV { x, pc } => {
                    eprintln!("[fault_detect] arch: RiscV  pc={:#018x}", pc);
                    for (i, r) in x.iter().enumerate() {
                        if *r != 0 {
                            eprintln!("[fault_detect]   x{:<2} = {:#018x}", i, r);
                        }
                    }
                }
                crate::runtime::ArchContext::Aarch64 { x, sp, pc, nzcv } => {
                    eprintln!(
                        "[fault_detect] arch: AArch64  pc={:#018x}  sp={:#018x}  nzcv={:#010x}",
                        pc, sp, nzcv
                    );
                    for (i, r) in x.iter().enumerate() {
                        if *r != 0 {
                            eprintln!("[fault_detect]   x{:<2} = {:#018x}", i, r);
                        }
                    }
                }
                crate::runtime::ArchContext::None => {}
            }

            // Recent instructions
            let insns = guard.recent_insns();
            eprintln!(
                "[fault_detect] recent instructions ({} entries, oldest->newest):",
                insns.len()
            );
            for (i, insn) in insns.iter().enumerate() {
                eprintln!(
                    "[fault_detect]   [{:>4}] pc={:#018x} raw={:#010x} opcode={} class={:?} stub={}",
                    i,
                    insn.pc,
                    insn.raw,
                    insn.opcode_name,
                    insn.class,
                    insn.is_stub
                );
            }

            // Syscall log
            if !guard.syscall_log.is_empty() {
                eprintln!(
                    "[fault_detect] syscall log ({} entries):",
                    guard.syscall_log.len()
                );
                for line in &guard.syscall_log {
                    eprintln!("[fault_detect]   {}", line);
                }
            }
            eprintln!("[fault_detect] ============================");
        }));
    }

    fn atexit(&mut self) {
        // Nothing to print unless a fault was fired — the on_fault callback handles reporting.
    }
}

#[cfg(test)]
#[path = "tests/fault_detect.rs"]
mod tests;

use std::sync::{Arc, Mutex};
use crate::api::{HelmPlugin, PluginArgs};
use crate::runtime::PluginRegistry;

/// Ring-buffer state shared between callbacks.
struct Inner {
    /// Ring buffer of recent PCs.
    ring: Vec<u64>,
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
            ring: vec![0u64; capacity.max(1)],
            head: 0,
            filled: 0,
            syscall_log: Vec::new(),
        }
    }

    fn push_pc(&mut self, pc: u64) {
        let cap = self.ring.len();
        self.ring[self.head % cap] = pc;
        self.head += 1;
        if self.filled < cap {
            self.filled += 1;
        }
    }

    /// Iterate PCs oldest → newest.
    fn recent_pcs(&self) -> Vec<u64> {
        let cap = self.ring.len();
        let count = self.filled;
        if count == 0 {
            return vec![];
        }
        let start = if count < cap { 0 } else { self.head % cap };
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            out.push(self.ring[(start + i) % cap]);
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

        // Callback 1: ring-buffer every executed PC.
        let inner_insn = Arc::clone(&self.inner);
        reg.on_insn_exec(Box::new(move |_vcpu_idx, insn| {
            inner_insn.lock().unwrap().push_pc(insn.pc);
        }));

        // Callback 2: log each syscall entry.
        let inner_sc = Arc::clone(&self.inner);
        reg.on_syscall(Box::new(move |info| {
            let line = format!(
                "vcpu={} syscall={} args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x}]",
                info.vcpu_idx,
                info.number,
                info.args[0], info.args[1], info.args[2],
                info.args[3], info.args[4], info.args[5],
            );
            inner_sc.lock().unwrap().syscall_log.push(line);
        }));

        // Callback 3: dump everything on fault.
        let inner_fault = Arc::clone(&self.inner);
        reg.on_fault(Box::new(move |fault| {
            let guard = inner_fault.lock().unwrap();
            log::error!("[fault_detect] ====== FAULT DETECTED ======");
            log::error!("[fault_detect] vcpu={}  pc={:#018x}  kind={}  insn_count={}",
                fault.vcpu_idx, fault.pc, fault.kind, fault.insn_count);
            log::error!("[fault_detect] message: {}", fault.message);
            log::error!("[fault_detect] raw={:#010x}", fault.raw);

            // Arch context
            match &fault.context {
                crate::runtime::ArchContext::RiscV { x, pc } => {
                    log::error!("[fault_detect] arch: RiscV  pc={:#018x}", pc);
                    for (i, r) in x.iter().enumerate() {
                        if *r != 0 {
                            log::error!("[fault_detect]   x{:<2} = {:#018x}", i, r);
                        }
                    }
                }
                crate::runtime::ArchContext::Aarch64 { x, sp, pc, nzcv } => {
                    log::error!("[fault_detect] arch: AArch64  pc={:#018x}  sp={:#018x}  nzcv={:#010x}", pc, sp, nzcv);
                    for (i, r) in x.iter().enumerate() {
                        if *r != 0 {
                            log::error!("[fault_detect]   x{:<2} = {:#018x}", i, r);
                        }
                    }
                }
                crate::runtime::ArchContext::None => {}
            }

            // PC history
            let pcs = guard.recent_pcs();
            log::error!("[fault_detect] PC history ({} entries, oldest→newest):", pcs.len());
            for (i, pc) in pcs.iter().enumerate() {
                log::error!("[fault_detect]   [{:>4}] {:#018x}", i, pc);
            }

            // Syscall log
            if !guard.syscall_log.is_empty() {
                log::error!("[fault_detect] syscall log ({} entries):", guard.syscall_log.len());
                for line in &guard.syscall_log {
                    log::error!("[fault_detect]   {}", line);
                }
            }
            log::error!("[fault_detect] ============================");
        }));
    }

    fn atexit(&mut self) {
        // Nothing to print unless a fault was fired — the on_fault callback handles reporting.
    }
}

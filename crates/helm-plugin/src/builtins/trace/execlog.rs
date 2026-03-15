use std::sync::{Arc, Mutex};
use crate::api::{HelmPlugin, PluginArgs};
use crate::runtime::PluginRegistry;

/// Execution trace logger — records "vcpu PC raw" lines up to `max`.
pub struct ExecLog {
    lines: Arc<Mutex<Vec<String>>>,
}

impl ExecLog {
    pub fn new() -> Self {
        Self {
            lines: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Return all recorded trace lines.
    pub fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}

impl Default for ExecLog {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for ExecLog {
    fn name(&self) -> &str {
        "execlog"
    }

    fn install(&mut self, reg: &mut PluginRegistry, args: &PluginArgs) {
        let max = args.get_usize("max").unwrap_or(usize::MAX);
        let show_regs = args.get_bool("regs").unwrap_or(false);
        let lines = Arc::clone(&self.lines);

        reg.on_insn_exec(Box::new(move |vcpu_idx, insn| {
            let mut guard = lines.lock().unwrap();
            if guard.len() >= max {
                return;
            }
            let entry = if show_regs {
                format!("vcpu={} pc={:#018x} raw={:#010x} class={:?}", vcpu_idx, insn.pc, insn.raw, insn.class)
            } else {
                format!("vcpu={} pc={:#018x} raw={:#010x}", vcpu_idx, insn.pc, insn.raw)
            };
            guard.push(entry);
        }));
    }

    fn atexit(&mut self) {
        let guard = self.lines.lock().unwrap();
        for line in guard.iter() {
            log::info!("[execlog] {}", line);
        }
    }
}

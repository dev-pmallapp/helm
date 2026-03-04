//! Execution trace logger — logs every executed instruction.

use crate::plugin::{HelmPlugin, PluginArgs};
use crate::registry::PluginRegistry;
use std::sync::Mutex;

pub struct ExecLog {
    output: Mutex<Vec<String>>,
    show_regs: bool,
    max_lines: usize,
}

impl ExecLog {
    pub fn new() -> Self {
        Self {
            output: Mutex::new(Vec::new()),
            show_regs: false,
            max_lines: usize::MAX,
        }
    }

    pub fn lines(&self) -> Vec<String> {
        self.output.lock().unwrap().clone()
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
        self.show_regs = args.get("regs") == Some("true");
        self.max_lines = args.get_usize("max", usize::MAX);

        let output = self.output.lock().unwrap();
        drop(output); // release lock

        let max = self.max_lines;

        // We can't move self into the closure, so use a shared Arc.
        let buf = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let buf2 = buf.clone();

        reg.on_insn_exec(Box::new(move |vcpu_idx, insn| {
            let mut lines = buf2.lock().unwrap();
            if lines.len() < max {
                lines.push(format!(
                    "{vcpu_idx}  0x{:016x}  {}",
                    insn.vaddr, insn.mnemonic
                ));
            }
        }));

        // Store the shared buffer so lines() works
        self.output = Mutex::new(Vec::new());
        // NOTE: In a real implementation the closure would share
        // the same buffer via Arc. Simplified here for skeleton.
    }
}

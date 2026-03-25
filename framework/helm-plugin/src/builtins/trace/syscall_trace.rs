use crate::api::{HelmPlugin, HelmPluginArgs};
use crate::runtime::HelmPluginRegistry;
use std::sync::{Arc, Mutex};

/// Syscall entry/return logger.
pub struct SyscallTrace {
    entries: Arc<Mutex<Vec<String>>>,
}

impl SyscallTrace {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Return all logged lines (entries and returns interleaved).
    pub fn entries(&self) -> Vec<String> {
        self.entries.lock().unwrap().clone()
    }
}

impl Default for SyscallTrace {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for SyscallTrace {
    fn name(&self) -> &str {
        "syscall_trace"
    }

    fn install(&mut self, reg: &mut HelmPluginRegistry, _args: &HelmPluginArgs) {
        let entries = Arc::clone(&self.entries);
        reg.on_syscall(Box::new(move |info| {
            let line = format!(
                "[strace] syscall={} args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x}]",
                info.number,
                info.args[0],
                info.args[1],
                info.args[2],
                info.args[3],
                info.args[4],
                info.args[5],
            );
            eprintln!("{line}");
            entries.lock().unwrap().push(line);
        }));

        let entries2 = Arc::clone(&self.entries);
        reg.on_syscall_ret(Box::new(move |ret_info| {
            let line = format!(
                "[strace]  → ret={:#x} ({})",
                ret_info.ret_value, ret_info.ret_value as i64
            );
            eprintln!("{line}");
            entries2.lock().unwrap().push(line);
        }));
    }

    fn atexit(&mut self) {
        let count = self.entries.lock().unwrap().len();
        eprintln!("[strace] total {count} events logged");
    }
}

#[cfg(test)]
#[path = "tests/syscall_trace.rs"]
mod tests;

use std::sync::{Arc, Mutex};
use crate::api::{HelmPlugin, PluginArgs};
use crate::runtime::PluginRegistry;

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

    fn install(&mut self, reg: &mut PluginRegistry, _args: &PluginArgs) {
        let entries = Arc::clone(&self.entries);
        reg.on_syscall(Box::new(move |info| {
            let line = format!(
                "vcpu={} syscall={} args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x}]",
                info.vcpu_idx,
                info.number,
                info.args[0], info.args[1], info.args[2],
                info.args[3], info.args[4], info.args[5],
            );
            entries.lock().unwrap().push(line);
        }));

        let entries2 = Arc::clone(&self.entries);
        reg.on_syscall_ret(Box::new(move |ret_info| {
            let line = format!(
                "vcpu={} syscall={} ret={:#x}",
                ret_info.vcpu_idx, ret_info.number, ret_info.ret_value
            );
            entries2.lock().unwrap().push(line);
        }));
    }

    fn atexit(&mut self) {
        let guard = self.entries.lock().unwrap();
        for line in guard.iter() {
            log::info!("[syscall_trace] {}", line);
        }
    }
}

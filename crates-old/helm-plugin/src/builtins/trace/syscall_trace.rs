//! Syscall entry/return logger.

use crate::api::plugin::{HelmPlugin, PluginArgs};
use crate::runtime::registry::PluginRegistry;
use std::sync::Mutex;

pub struct SyscallTrace {
    log: Mutex<Vec<String>>,
}

impl SyscallTrace {
    pub fn new() -> Self {
        Self {
            log: Mutex::new(Vec::new()),
        }
    }

    pub fn entries(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl Default for SyscallTrace {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for SyscallTrace {
    fn name(&self) -> &str {
        "syscall-trace"
    }

    fn install(&mut self, reg: &mut PluginRegistry, _args: &PluginArgs) {
        let log: &'static Mutex<Vec<String>> = Box::leak(Box::new(Mutex::new(Vec::new())));

        reg.on_syscall(Box::new(move |info| {
            let mut entries = log.lock().unwrap();
            entries.push(format!(
                "vcpu={} syscall={} args=[{:#x},{:#x},{:#x},{:#x},{:#x},{:#x}]",
                info.vcpu_idx,
                info.number,
                info.args[0],
                info.args[1],
                info.args[2],
                info.args[3],
                info.args[4],
                info.args[5],
            ));
        }));

        reg.on_syscall_ret(Box::new(move |info| {
            log::trace!(
                "[syscall-trace] vcpu={} syscall={} ret={:#x}",
                info.vcpu_idx,
                info.number,
                info.ret_value
            );
        }));
    }
}

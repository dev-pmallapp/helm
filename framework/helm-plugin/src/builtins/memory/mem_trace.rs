use crate::api::{HelmPlugin, HelmPluginArgs};
use crate::runtime::{MemFilter, HelmPluginRegistry};
use std::sync::{Arc, Mutex};

/// Memory access trace logger — records load/store events.
pub struct MemTrace {
    entries: Arc<Mutex<Vec<String>>>,
}

impl MemTrace {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Default for MemTrace {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for MemTrace {
    fn name(&self) -> &str {
        "mem_trace"
    }

    fn install(&mut self, reg: &mut HelmPluginRegistry, args: &HelmPluginArgs) {
        let max = args.get_usize("max").unwrap_or(usize::MAX);
        let writes_only = args.get_bool("writes-only").unwrap_or(false);
        let entries = Arc::clone(&self.entries);

        let filter = if writes_only {
            MemFilter::WritesOnly
        } else {
            MemFilter::All
        };

        reg.on_mem_access(
            filter,
            Box::new(move |_vcpu_idx, info| {
                let mut guard = entries.lock().unwrap();
                if guard.len() >= max {
                    return;
                }
                let tag = if info.is_store { "W" } else { "R" };
                let atomic = if info.is_atomic { " atomic" } else { "" };
                let line = format!("[{tag}] {:#018x} {}{}", info.vaddr, info.size, atomic);
                guard.push(line);
            }),
        );
    }

    fn atexit(&mut self) {
        let guard = self.entries.lock().unwrap();
        eprintln!("[mem-trace] {} access(es) recorded:", guard.len());
        for line in guard.iter() {
            eprintln!("[mem-trace] {}", line);
        }
    }
}

#[cfg(test)]
#[path = "tests/mem_trace.rs"]
mod tests;

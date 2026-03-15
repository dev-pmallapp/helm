use std::sync::Arc;
use crate::api::{HelmPlugin, PluginArgs};
use crate::runtime::PluginRegistry;
use crate::runtime::Scoreboard;

/// Per-vCPU instruction counter.
pub struct InsnCount {
    /// Number of vCPUs — set during `install`, used to size the scoreboard.
    num_vcpus: usize,
    /// Shared scoreboard; `Arc` so the callback closure can own a reference.
    counts: Arc<Scoreboard<u64>>,
}

impl InsnCount {
    pub fn new() -> Self {
        Self {
            num_vcpus: 0,
            counts: Arc::new(Scoreboard::new(0)),
        }
    }

    /// Total instruction count across all vCPUs.
    pub fn total(&self) -> u64 {
        self.counts.iter().copied().sum()
    }

    /// Per-vCPU instruction counts.
    pub fn per_vcpu(&self) -> Vec<u64> {
        self.counts.iter().copied().collect()
    }
}

impl Default for InsnCount {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for InsnCount {
    fn name(&self) -> &str {
        "insn_count"
    }

    fn install(&mut self, reg: &mut PluginRegistry, args: &PluginArgs) {
        self.num_vcpus = args.get_usize("vcpus").unwrap_or(1).max(1);
        self.counts = Arc::new(Scoreboard::new(self.num_vcpus));

        let counts = Arc::clone(&self.counts);
        reg.on_insn_exec(Box::new(move |vcpu_idx, _insn| {
            if vcpu_idx < counts.len() {
                *counts.get_mut(vcpu_idx) += 1;
            }
        }));
    }

    fn atexit(&mut self) {
        let total = self.total();
        log::info!("[insn_count] total={}", total);
        for (i, c) in self.counts.iter().enumerate() {
            log::info!("[insn_count] vcpu[{}]={}", i, c);
        }
    }
}

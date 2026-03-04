//! Instruction counter — uses inline scoreboard for minimal overhead.

use crate::api::plugin::{HelmPlugin, PluginArgs};
use crate::runtime::registry::PluginRegistry;
use crate::runtime::scoreboard::Scoreboard;
use std::sync::Arc;

pub struct InsnCount {
    scoreboard: Option<Arc<Scoreboard<u64>>>,
}

impl InsnCount {
    pub fn new() -> Self {
        Self { scoreboard: None }
    }

    /// Total instructions across all vCPUs.
    pub fn total(&self) -> u64 {
        self.scoreboard
            .as_ref()
            .map(|sb| sb.iter().sum())
            .unwrap_or(0)
    }

    /// Per-vCPU instruction counts.
    pub fn per_vcpu(&self) -> Vec<u64> {
        self.scoreboard
            .as_ref()
            .map(|sb| sb.iter().copied().collect())
            .unwrap_or_default()
    }
}

impl Default for InsnCount {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for InsnCount {
    fn name(&self) -> &str {
        "insn-count"
    }

    fn install(&mut self, reg: &mut PluginRegistry, _args: &PluginArgs) {
        let sb = Arc::new(Scoreboard::<u64>::new(64)); // up to 64 vCPUs
        self.scoreboard = Some(sb.clone());

        reg.on_insn_exec(Box::new(move |vcpu_idx, _insn| {
            *sb.get_mut(vcpu_idx) += 1;
        }));
    }

    fn atexit(&mut self) {
        let total = self.total();
        let per_vcpu = self.per_vcpu();
        log::info!("[insn-count] Total: {total}");
        for (i, c) in per_vcpu.iter().enumerate() {
            if *c > 0 {
                log::info!("[insn-count] vCPU {i}: {c}");
            }
        }
    }
}

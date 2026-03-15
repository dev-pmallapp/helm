use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use crate::api::{HelmPlugin, PluginArgs};
use crate::runtime::PluginRegistry;

/// Hot-block profiler — ranks PCs by execution count.
pub struct HotBlocks {
    counts: Arc<Mutex<HashMap<u64, u64>>>,
}

impl HotBlocks {
    pub fn new() -> Self {
        Self {
            counts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Return the top `n` (pc, count) pairs, sorted descending by count.
    pub fn top(&self, n: usize) -> Vec<(u64, u64)> {
        let guard = self.counts.lock().unwrap();
        let mut v: Vec<(u64, u64)> = guard.iter().map(|(&pc, &c)| (pc, c)).collect();
        v.sort_unstable_by(|a, b| b.1.cmp(&a.1));
        v.truncate(n);
        v
    }
}

impl Default for HotBlocks {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for HotBlocks {
    fn name(&self) -> &str {
        "hotblocks"
    }

    fn install(&mut self, reg: &mut PluginRegistry, _args: &PluginArgs) {
        let counts = Arc::clone(&self.counts);
        reg.on_insn_exec(Box::new(move |_vcpu_idx, insn| {
            *counts.lock().unwrap().entry(insn.pc).or_insert(0) += 1;
        }));
    }

    fn atexit(&mut self) {
        let top20 = self.top(20);
        log::info!("[hotblocks] top {} PCs:", top20.len());
        for (rank, (pc, count)) in top20.iter().enumerate() {
            log::info!("[hotblocks]  #{:>2}  pc={:#018x}  count={}", rank + 1, pc, count);
        }
    }
}

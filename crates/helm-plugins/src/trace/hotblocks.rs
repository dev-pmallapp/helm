//! Hot-block profiler — ranks TBs by execution count.

use crate::plugin::{HelmPlugin, PluginArgs};
use crate::registry::PluginRegistry;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct HotBlocks {
    counts: Mutex<HashMap<u64, (u64, usize)>>, // pc -> (exec_count, insn_count)
}

impl HotBlocks {
    pub fn new() -> Self {
        Self {
            counts: Mutex::new(HashMap::new()),
        }
    }

    /// Top N blocks by execution count.
    pub fn top(&self, n: usize) -> Vec<(u64, u64, usize)> {
        let counts = self.counts.lock().unwrap();
        let mut entries: Vec<_> = counts
            .iter()
            .map(|(&pc, &(count, insns))| (pc, count, insns))
            .collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        entries.truncate(n);
        entries
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
        let counts = Mutex::new(HashMap::<u64, (u64, usize)>::new());
        // Share via leak for simplicity in skeleton (real impl uses Arc).
        let counts_ref: &'static Mutex<HashMap<u64, (u64, usize)>> = Box::leak(Box::new(counts));

        reg.on_tb_exec(Box::new(move |_vcpu_idx, tb| {
            let mut map = counts_ref.lock().unwrap();
            let entry = map.entry(tb.pc).or_insert((0, tb.insn_count));
            entry.0 += 1;
        }));
    }

    fn atexit(&mut self) {
        for (pc, count, insns) in self.top(20) {
            log::info!("[hotblocks] 0x{pc:016x}  exec={count}  insns={insns}");
        }
    }
}

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use crate::api::{HelmPlugin, PluginArgs};
use crate::runtime::PluginRegistry;

struct BranchRecord {
    taken: u64,
    not_taken: u64,
}

/// Branch direction/target logger with per-PC taken/not-taken counts.
pub struct BranchTrace {
    records: Arc<Mutex<HashMap<u64, BranchRecord>>>,
    top_n: usize,
}

impl BranchTrace {
    pub fn new() -> Self {
        Self {
            records: Arc::new(Mutex::new(HashMap::new())),
            top_n: 20,
        }
    }
}

impl Default for BranchTrace {
    fn default() -> Self { Self::new() }
}

impl HelmPlugin for BranchTrace {
    fn name(&self) -> &str { "branch_trace" }

    fn install(&mut self, reg: &mut PluginRegistry, args: &PluginArgs) {
        self.top_n = args.get_usize("top").unwrap_or(20);
        let records = Arc::clone(&self.records);

        reg.on_branch(Box::new(move |_vcpu_idx, info| {
            let mut guard = records.lock().unwrap();
            let rec = guard.entry(info.pc).or_insert(BranchRecord { taken: 0, not_taken: 0 });
            if info.taken {
                rec.taken += 1;
            } else {
                rec.not_taken += 1;
            }
        }));
    }

    fn atexit(&mut self) {
        let guard = self.records.lock().unwrap();
        let total: u64 = guard.values().map(|r| r.taken + r.not_taken).sum();
        eprintln!("[branch-trace] {} unique branch PCs, {} total branches", guard.len(), total);

        let mut entries: Vec<_> = guard.iter().collect();
        entries.sort_by(|a, b| (b.1.taken + b.1.not_taken).cmp(&(a.1.taken + a.1.not_taken)));

        for (pc, rec) in entries.iter().take(self.top_n) {
            let total = rec.taken + rec.not_taken;
            let pct = if total > 0 { (rec.taken as f64 / total as f64) * 100.0 } else { 0.0 };
            eprintln!("[branch-trace]   {:#018x}: {} taken / {} not-taken ({:.1}% taken)",
                pc, rec.taken, rec.not_taken, pct);
        }
    }
}

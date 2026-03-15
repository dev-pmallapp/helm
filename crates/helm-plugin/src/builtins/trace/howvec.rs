use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use crate::api::{HelmPlugin, PluginArgs};
use crate::runtime::PluginRegistry;
use crate::runtime::InsnClass;

/// Instruction class histogram.
pub struct HowVec {
    counts: Arc<Mutex<HashMap<InsnClass, u64>>>,
}

impl HowVec {
    pub fn new() -> Self {
        Self {
            counts: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for HowVec {
    fn default() -> Self {
        Self::new()
    }
}

// `InsnClass` uses `#[derive(PartialEq, Eq)]` — we need `Hash` as well.
// Since we can't add derives to a type we don't own, we implement Hash manually
// by delegating to the Debug representation (stable for enums with no data).
// A cleaner approach: use the discriminant index directly.
impl std::hash::Hash for InsnClass {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        core::mem::discriminant(self).hash(state);
    }
}

impl HelmPlugin for HowVec {
    fn name(&self) -> &str {
        "howvec"
    }

    fn install(&mut self, reg: &mut PluginRegistry, _args: &PluginArgs) {
        let counts = Arc::clone(&self.counts);
        reg.on_insn_exec(Box::new(move |_vcpu_idx, insn| {
            *counts.lock().unwrap().entry(insn.class).or_insert(0) += 1;
        }));
    }

    fn atexit(&mut self) {
        let guard = self.counts.lock().unwrap();
        let total: u64 = guard.values().copied().sum();
        if total == 0 {
            log::info!("[howvec] no instructions recorded");
            return;
        }
        let mut v: Vec<(InsnClass, u64)> = guard.iter().map(|(&c, &n)| (c, n)).collect();
        // Sort descending by count, then by class name for stability.
        v.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| format!("{:?}", a.0).cmp(&format!("{:?}", b.0))));
        log::info!("[howvec] instruction class histogram (total={})", total);
        for (class, count) in &v {
            let pct = (*count as f64 / total as f64) * 100.0;
            log::info!("[howvec]  {:<16} {:>12}  {:6.2}%", format!("{:?}", class), count, pct);
        }
    }
}

//! Instruction-class histogram.

use crate::plugin::{HelmPlugin, PluginArgs};
use crate::registry::PluginRegistry;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct HowVec {
    counts: Mutex<HashMap<String, u64>>,
}

impl HowVec {
    pub fn new() -> Self {
        Self {
            counts: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for HowVec {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for HowVec {
    fn name(&self) -> &str {
        "howvec"
    }

    fn install(&mut self, reg: &mut PluginRegistry, _args: &PluginArgs) {
        let counts: &'static Mutex<HashMap<String, u64>> =
            Box::leak(Box::new(Mutex::new(HashMap::new())));

        reg.on_insn_exec(Box::new(move |_vcpu_idx, insn| {
            let category = classify_mnemonic(&insn.mnemonic);
            *counts.lock().unwrap().entry(category).or_insert(0) += 1;
        }));
    }

    fn atexit(&mut self) {
        let counts = self.counts.lock().unwrap();
        let total: u64 = counts.values().sum();
        let mut sorted: Vec<_> = counts.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        for (cat, count) in sorted {
            let pct = *count as f64 / total as f64 * 100.0;
            log::info!("[howvec] {cat:<12} {count:>10}  {pct:5.1}%");
        }
    }
}

fn classify_mnemonic(mnemonic: &str) -> String {
    let m = mnemonic.to_uppercase();
    if m.starts_with("ADD")
        || m.starts_with("SUB")
        || m.starts_with("MOV")
        || m.starts_with("AND")
        || m.starts_with("ORR")
        || m.starts_with("EOR")
        || m.starts_with("CMP")
    {
        "IntAlu".to_string()
    } else if m.starts_with("MUL") || m.starts_with("MADD") || m.starts_with("DIV") {
        "IntMul".to_string()
    } else if m.starts_with('B') || m.starts_with("CB") || m.starts_with("TB") || m == "RET" {
        "Branch".to_string()
    } else if m.starts_with("LDR") || m.starts_with("LDP") {
        "Load".to_string()
    } else if m.starts_with("STR") || m.starts_with("STP") {
        "Store".to_string()
    } else if m.starts_with('F') {
        "FpAlu".to_string()
    } else if m == "SVC" {
        "Syscall".to_string()
    } else {
        "Other".to_string()
    }
}

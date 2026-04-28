//! JIT reject reason histogram plugin.
//!
//! Subscribes to JIT fallback callbacks and accumulates a per-reason count.
//! On `atexit()`, prints a histogram to stderr.
//!
//! # Usage
//!
//! ```text
//! --plugin jit_rejects
//! ```

use crate::api::{HelmPlugin, HelmPluginArgs};
use crate::runtime::HelmPluginRegistry;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub struct JitRejects {
    counts: Arc<Mutex<BTreeMap<String, u64>>>,
}

impl JitRejects {
    pub fn new() -> Self {
        Self {
            counts: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
}

impl Default for JitRejects {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for JitRejects {
    fn name(&self) -> &str {
        "jit_rejects"
    }

    fn install(&mut self, reg: &mut HelmPluginRegistry, _args: &HelmPluginArgs) {
        let counts = Arc::clone(&self.counts);

        reg.on_jit_fallback(Box::new(move |_pc, reason| {
            let key = reason.unwrap_or("unknown");
            let mut guard = counts.lock().unwrap();
            *guard.entry(key.to_string()).or_insert(0) += 1;
        }));
    }

    fn atexit(&mut self) {
        let guard = self.counts.lock().unwrap();
        if guard.is_empty() {
            return;
        }
        for (reason, count) in guard.iter() {
            eprintln!("[jit-rejects] {reason}: {count}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_fallback_reasons() {
        let mut plugin = JitRejects::new();
        let mut reg = HelmPluginRegistry::new();
        plugin.install(&mut reg, &HelmPluginArgs::parse(""));

        reg.fire_jit_fallback(0x1000, Some("w-register"));
        reg.fire_jit_fallback(0x1004, Some("w-register"));
        reg.fire_jit_fallback(0x1008, Some("complex-addressing"));
        reg.fire_jit_fallback(0x100c, None);

        let counts = plugin.counts.lock().unwrap();
        assert_eq!(counts.get("w-register"), Some(&2));
        assert_eq!(counts.get("complex-addressing"), Some(&1));
        assert_eq!(counts.get("unknown"), Some(&1));
        assert_eq!(counts.len(), 3);
    }
}

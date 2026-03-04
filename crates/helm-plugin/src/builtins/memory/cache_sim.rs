//! Cache simulation plugin — set-associative L1/L2 simulation using
//! memory-access callbacks.

use crate::runtime::callback::MemFilter;
use crate::api::plugin::{HelmPlugin, PluginArgs};
use crate::runtime::registry::PluginRegistry;
use std::sync::atomic::{AtomicU64, Ordering};

/// Simple cache statistics.
pub struct CacheStats {
    pub hits: AtomicU64,
    pub misses: AtomicU64,
}

impl CacheStats {
    pub fn new() -> Self {
        Self {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    pub fn hit_rate(&self) -> f64 {
        let h = self.hits.load(Ordering::Relaxed) as f64;
        let m = self.misses.load(Ordering::Relaxed) as f64;
        if h + m == 0.0 {
            0.0
        } else {
            h / (h + m)
        }
    }
}

impl Default for CacheStats {
    fn default() -> Self {
        Self::new()
    }
}

pub struct CacheSim {
    l1d_stats: &'static CacheStats,
}

impl CacheSim {
    pub fn new() -> Self {
        Self {
            l1d_stats: Box::leak(Box::new(CacheStats::new())),
        }
    }

    pub fn l1d_hit_rate(&self) -> f64 {
        self.l1d_stats.hit_rate()
    }
}

impl Default for CacheSim {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for CacheSim {
    fn name(&self) -> &str {
        "cache"
    }

    fn install(&mut self, reg: &mut PluginRegistry, _args: &PluginArgs) {
        let stats = self.l1d_stats;

        reg.on_mem_access(
            MemFilter::All,
            Box::new(move |_vcpu_idx, _info| {
                // Stub: count everything as a hit.
                // Real implementation would do set-associative lookup.
                stats.hits.fetch_add(1, Ordering::Relaxed);
            }),
        );
    }

    fn atexit(&mut self) {
        let h = self.l1d_stats.hits.load(Ordering::Relaxed);
        let m = self.l1d_stats.misses.load(Ordering::Relaxed);
        log::info!(
            "[cache] L1D: hits={h} misses={m} rate={:.1}%",
            self.l1d_hit_rate() * 100.0
        );
    }
}

//! `helm-stats` — lock-free performance counters, histograms, and derived formulas.
//!
//! Counters are `Arc<AtomicU64>` — cloned and handed to the component that owns them.
//! The `StatsRegistry` retains a clone for reporting.

#![allow(clippy::module_name_repetitions)]
#![allow(missing_docs)]

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// ── PerfCounter ───────────────────────────────────────────────────────────────

/// A lock-free monotonic counter. Clone to share between owner and registry.
#[derive(Clone, Default)]
pub struct PerfCounter(Arc<AtomicU64>);

impl PerfCounter {
    pub fn new() -> Self {
        Self::default()
    }
    /// Increment by 1.
    #[inline]
    pub fn inc(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    /// Increment by `n`.
    #[inline]
    pub fn add(&self, n: u64) {
        self.0.fetch_add(n, Ordering::Relaxed);
    }
    /// Read current value (relaxed, not sequentially consistent).
    #[inline]
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn reset(&self) {
        self.0.store(0, Ordering::Relaxed);
    }
}

// ── JIT performance stats ───────────────────────────────────────────────────

/// Snapshot of JIT-side runtime counters.
#[derive(Debug, Clone, Default)]
pub struct JitPerfStats {
    pub block_cache_hits: u64,
    pub block_cache_misses: u64,
    pub blocks_compiled: u64,
    pub compiled_guest_insns: u64,
    pub blocks_executed: u64,
    pub traces_compiled: u64,
    pub trace_guest_insns: u64,
    pub traces_executed: u64,
    pub trace_cache_hits: u64,
    pub trace_cache_misses: u64,
    pub trace_guard_exits: u64,
    pub trace_retired: u64,
    pub fallback_count: u64,
    pub fallback_insns: u64,
    pub unsupported_block_starts: u64,
    pub unsupported_opcodes: BTreeMap<String, u64>,
    pub cache_entries: usize,
    pub trace_cache_entries: usize,
    pub cache_promotions: u64,
    pub cache_evictions: u64,
}

// ── PerfHistogram ─────────────────────────────────────────────────────────────

/// A histogram with fixed bucket boundaries.
///
/// Boundaries define upper-exclusive edges. A value ≥ last boundary goes into
/// the overflow bucket. There are `boundaries.len() + 1` buckets total.
pub struct PerfHistogram {
    buckets: Vec<AtomicU64>,
    boundaries: Vec<u64>,
}

impl PerfHistogram {
    pub fn new(boundaries: Vec<u64>) -> Arc<Self> {
        let n = boundaries.len() + 1;
        let buckets = (0..n).map(|_| AtomicU64::new(0)).collect();
        Arc::new(Self {
            buckets,
            boundaries,
        })
    }

    /// Record a sample.
    pub fn record(&self, value: u64) {
        // partition_point gives the first index where boundary > value
        let idx = self.boundaries.partition_point(|&b| value >= b);
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    /// Return all bucket counts.
    pub fn counts(&self) -> Vec<u64> {
        self.buckets
            .iter()
            .map(|b| b.load(Ordering::Relaxed))
            .collect()
    }
}

// ── StatsRegistry ─────────────────────────────────────────────────────────────

/// Central registry — creates and tracks all counters and histograms.
#[derive(Default)]
pub struct StatsRegistry {
    counters: HashMap<String, (PerfCounter, String)>,
    histograms: HashMap<String, (Arc<PerfHistogram>, String)>,
}

impl StatsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create (or retrieve) a named counter. The caller clones the returned handle.
    pub fn counter(&mut self, path: &str, desc: &str) -> PerfCounter {
        let entry = self
            .counters
            .entry(path.to_string())
            .or_insert_with(|| (PerfCounter::new(), desc.to_string()));
        entry.0.clone()
    }

    /// Create (or retrieve) a named histogram. The caller clones the returned handle.
    pub fn histogram(&mut self, path: &str, desc: &str, boundaries: &[u64]) -> Arc<PerfHistogram> {
        let entry = self
            .histograms
            .entry(path.to_string())
            .or_insert_with(|| (PerfHistogram::new(boundaries.to_vec()), desc.to_string()));
        Arc::clone(&entry.0)
    }

    /// Dump all counters as a JSON string.
    pub fn dump_json(&self) -> String {
        let mut map = serde_json::Map::new();
        for (path, (counter, _)) in &self.counters {
            map.insert(path.clone(), serde_json::Value::from(counter.get()));
        }
        for (path, (histogram, _)) in &self.histograms {
            let counts = histogram
                .counts()
                .into_iter()
                .map(serde_json::Value::from)
                .collect();
            map.insert(path.clone(), serde_json::Value::Array(counts));
        }
        serde_json::to_string_pretty(&map).unwrap_or_default()
    }

    /// Print a human-readable table to stdout.
    pub fn print_table(&self) {
        let mut counters: Vec<_> = self.counters.iter().collect();
        counters.sort_by_key(|(k, _)| k.as_str());
        for (path, (counter, desc)) in &counters {
            println!("{:<50} {:>16}  # {}", path, counter.get(), desc);
        }
        let mut histograms: Vec<_> = self.histograms.iter().collect();
        histograms.sort_by_key(|(k, _)| k.as_str());
        for (path, (histogram, desc)) in &histograms {
            println!("{:<50} {:>16?}  # {}", path, histogram.counts(), desc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_reuses_histogram_and_records_samples() {
        let mut reg = StatsRegistry::new();
        let hist = reg.histogram("jit.block.size", "compiled block size", &[4, 8]);
        hist.record(1);
        hist.record(4);
        hist.record(9);

        let same = reg.histogram("jit.block.size", "ignored desc", &[1]);
        assert!(Arc::ptr_eq(&hist, &same));
        assert_eq!(hist.counts(), vec![1, 1, 1]);
    }

    #[test]
    fn registry_dump_json_includes_histograms() {
        let mut reg = StatsRegistry::new();
        let counter = reg.counter("jit.blocks", "compiled blocks");
        counter.add(3);
        let hist = reg.histogram("jit.block.size", "compiled block size", &[4, 8]);
        hist.record(2);
        hist.record(9);

        let value: serde_json::Value =
            serde_json::from_str(&reg.dump_json()).expect("registry JSON");
        assert_eq!(value["jit.blocks"], 3);
        assert_eq!(value["jit.block.size"], serde_json::json!([1, 0, 1]));
    }
}

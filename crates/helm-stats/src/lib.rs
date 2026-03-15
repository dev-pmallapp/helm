//! `helm-stats` — lock-free performance counters, histograms, and derived formulas.
//!
//! Counters are `Arc<AtomicU64>` — cloned and handed to the component that owns them.
//! The `StatsRegistry` retains a clone for reporting.

#![allow(clippy::module_name_repetitions)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// ── PerfCounter ───────────────────────────────────────────────────────────────

/// A lock-free monotonic counter. Clone to share between owner and registry.
#[derive(Clone, Default)]
pub struct PerfCounter(Arc<AtomicU64>);

impl PerfCounter {
    pub fn new() -> Self { Self::default() }
    /// Increment by 1.
    pub fn inc(&self) { self.0.fetch_add(1, Ordering::Relaxed); }
    /// Increment by `n`.
    pub fn add(&self, n: u64) { self.0.fetch_add(n, Ordering::Relaxed); }
    /// Read current value (relaxed, not sequentially consistent).
    pub fn get(&self) -> u64 { self.0.load(Ordering::Relaxed) }
    pub fn reset(&self) { self.0.store(0, Ordering::Relaxed); }
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
        Arc::new(Self { buckets, boundaries })
    }

    /// Record a sample.
    pub fn record(&self, value: u64) {
        // partition_point gives the first index where boundary > value
        let idx = self.boundaries.partition_point(|&b| value >= b);
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    /// Return all bucket counts.
    pub fn counts(&self) -> Vec<u64> {
        self.buckets.iter().map(|b| b.load(Ordering::Relaxed)).collect()
    }
}

// ── StatsRegistry ─────────────────────────────────────────────────────────────

/// Central registry — creates and tracks all counters and histograms.
#[derive(Default)]
pub struct StatsRegistry {
    counters: HashMap<String, (PerfCounter, String)>,
}

impl StatsRegistry {
    pub fn new() -> Self { Self::default() }

    /// Create (or retrieve) a named counter. The caller clones the returned handle.
    pub fn counter(&mut self, path: &str, desc: &str) -> PerfCounter {
        let entry = self
            .counters
            .entry(path.to_string())
            .or_insert_with(|| (PerfCounter::new(), desc.to_string()));
        entry.0.clone()
    }

    /// Dump all counters as a JSON string.
    pub fn dump_json(&self) -> String {
        let map: HashMap<&str, u64> = self
            .counters
            .iter()
            .map(|(k, (v, _))| (k.as_str(), v.get()))
            .collect();
        serde_json::to_string_pretty(&map).unwrap_or_default()
    }

    /// Print a human-readable table to stdout.
    pub fn print_table(&self) {
        let mut pairs: Vec<_> = self.counters.iter().collect();
        pairs.sort_by_key(|(k, _)| k.as_str());
        for (path, (counter, desc)) in &pairs {
            println!("{:<50} {:>16}  # {}", path, counter.get(), desc);
        }
    }
}

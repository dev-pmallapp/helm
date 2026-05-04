//! `MemStats` -- aggregate of memory-side runtime counters
//! (loads, stores, bytes read / written).
//!
//! Same shape as `JitPerfStats` / `CpuStats`: every field is a
//! `PerfCounter`, hot-path increments are interior-mutable
//! (`Clone`-cheap Arc bumps), and the struct collapses to
//! zero-sized fields when `helm-stats/stats` is off.
//!
//! Implements `StatsProducer` so the engine / a memory backend
//! can hand it to a `StatsScope` rooted at the canonical
//! `system.mem` (or per-region) path; the registry view shares
//! the underlying `Arc<AtomicU64>` storage with the hot path.

use crate::{PerfCounter, StatsProducer, StatsScope};

/// Aggregate memory-side runtime counters. Today there is one
/// instance per backend; per-region fan-out (gem5 `system.mem.dram`
/// vs `system.mem.scratchpad`) becomes a `Vec<MemStats>` once each
/// `FlatMemRegion` carries its own slot.
#[derive(Clone, Default)]
pub struct MemStats {
    /// Load operations issued through the backend.
    pub loads: PerfCounter,
    /// Store operations issued through the backend.
    pub stores: PerfCounter,
    /// Bytes read by load operations.
    pub bytes_read: PerfCounter,
    /// Bytes written by store operations.
    pub bytes_written: PerfCounter,
}

impl MemStats {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StatsProducer for MemStats {
    fn register_stats(&self, scope: &mut StatsScope<'_>) {
        scope.adopt_counter("loads", "Memory load operations", self.loads.clone());
        scope.adopt_counter("stores", "Memory store operations", self.stores.clone());
        scope.adopt_counter(
            "bytes_read",
            "Bytes read by load operations",
            self.bytes_read.clone(),
        );
        scope.adopt_counter(
            "bytes_written",
            "Bytes written by store operations",
            self.bytes_written.clone(),
        );
    }
}

#[cfg(all(test, feature = "stats"))]
mod tests {
    use super::MemStats;
    use crate::{StatsProducer, StatsRegistry, StatsRegistryRead, StatsScope};

    #[test]
    fn register_under_canonical_path() {
        let stats = MemStats::new();
        stats.loads.add(3);
        stats.stores.inc();
        stats.bytes_read.add(24);
        stats.bytes_written.add(8);

        let mut reg = StatsRegistry::new();
        {
            let mut scope = StatsScope::new(&mut reg, "system.mem");
            stats.register_stats(&mut scope);
        }
        assert_eq!(reg.counter_value("system.mem.loads"), Some(3));
        assert_eq!(reg.counter_value("system.mem.stores"), Some(1));
        assert_eq!(reg.counter_value("system.mem.bytes_read"), Some(24));
        assert_eq!(reg.counter_value("system.mem.bytes_written"), Some(8));
    }

    #[test]
    fn shared_storage_after_register() {
        let stats = MemStats::new();
        let mut reg = StatsRegistry::new();
        {
            let mut scope = StatsScope::new(&mut reg, "system.mem");
            stats.register_stats(&mut scope);
        }
        stats.loads.add(7);
        stats.bytes_read.add(56);
        assert_eq!(reg.counter_value("system.mem.loads"), Some(7));
        assert_eq!(reg.counter_value("system.mem.bytes_read"), Some(56));
    }
}

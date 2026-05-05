//! `IoStats` -- aggregate of I/O-style runtime counters
//! (bytes in/out, requests issued/completed).
//!
//! Same shape as `JitPerfStats` / `CpuStats` / `MemStats` /
//! `IntcStats`: every field is a `PerfCounter`, hot-path
//! increments are interior-mutable (`Clone`-cheap Arc bumps),
//! and the struct collapses to zero-sized fields when
//! `helm-stats/stats` is off.
//!
//! Designed for VirtIO / PCI / network / block devices where
//! the natural counters are byte counts plus request /
//! completion counts. Implements `StatsProducer` so it can be
//! registered at any canonical scope; the registry view shares
//! the underlying `Arc<AtomicU64>` storage with the device's
//! hot path.

use crate::{PerfCounter, StatsProducer, StatsScope};

/// Aggregate I/O-style runtime counters for one device.
#[derive(Clone, Default)]
pub struct IoStats {
    /// Bytes transmitted (device -> external world).
    pub tx_bytes: PerfCounter,
    /// Bytes received (external world -> device).
    pub rx_bytes: PerfCounter,
    /// Requests issued (e.g. virtqueue descriptors processed,
    /// block reads/writes initiated).
    pub requests: PerfCounter,
    /// Requests completed (used-ring entries pushed back, block
    /// I/O finished).
    pub completions: PerfCounter,
    /// Total descriptors processed across all chains. For
    /// virtqueues this is the sum of every chain length the
    /// device walked. Mirrors gem5's per-queue descriptor
    /// counters at a per-device granularity.
    pub descriptors: PerfCounter,
}

impl IoStats {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StatsProducer for IoStats {
    fn register_stats(&self, scope: &mut StatsScope<'_>) {
        scope.adopt_counter("tx_bytes", "Bytes transmitted", self.tx_bytes.clone());
        scope.adopt_counter("rx_bytes", "Bytes received", self.rx_bytes.clone());
        scope.adopt_counter("requests", "I/O requests issued", self.requests.clone());
        scope.adopt_counter(
            "completions",
            "I/O requests completed",
            self.completions.clone(),
        );
        scope.adopt_counter(
            "descriptors",
            "Descriptors processed across all chains",
            self.descriptors.clone(),
        );
    }
}

#[cfg(all(test, feature = "stats"))]
mod tests {
    use super::IoStats;
    use crate::{StatsProducer, StatsRegistry, StatsRegistryRead, StatsScope};

    #[test]
    fn register_under_canonical_path() {
        let stats = IoStats::new();
        stats.tx_bytes.add(1500);
        stats.rx_bytes.add(64);
        stats.requests.inc();
        stats.completions.inc();
        stats.descriptors.add(3);

        let mut reg = StatsRegistry::new();
        {
            let mut scope = StatsScope::new(&mut reg, "system.virtio.net0");
            stats.register_stats(&mut scope);
        }
        assert_eq!(reg.counter_value("system.virtio.net0.tx_bytes"), Some(1500));
        assert_eq!(reg.counter_value("system.virtio.net0.rx_bytes"), Some(64));
        assert_eq!(reg.counter_value("system.virtio.net0.requests"), Some(1));
        assert_eq!(reg.counter_value("system.virtio.net0.completions"), Some(1));
        assert_eq!(
            reg.counter_value("system.virtio.net0.descriptors"),
            Some(3)
        );
    }

    #[test]
    fn shared_storage_after_register() {
        let stats = IoStats::new();
        let mut reg = StatsRegistry::new();
        {
            let mut scope = StatsScope::new(&mut reg, "system.virtio.blk0");
            stats.register_stats(&mut scope);
        }
        stats.tx_bytes.add(4096);
        stats.requests.add(3);
        assert_eq!(reg.counter_value("system.virtio.blk0.tx_bytes"), Some(4096));
        assert_eq!(reg.counter_value("system.virtio.blk0.requests"), Some(3));
    }
}

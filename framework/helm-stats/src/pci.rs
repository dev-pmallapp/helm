//! `PciBusStats` -- aggregate PCI host-bridge runtime counters.
//!
//! Same shape as the other producers: every field is a
//! `PerfCounter`, hot-path increments are interior-mutable
//! (`Clone`-cheap Arc bumps), and the struct collapses to
//! zero-sized fields when `helm-stats/stats` is off.
//!
//! Designed for an ECAM-style PCI host bridge. Tracks total
//! config-space reads / writes, the subset of reads that hit an
//! unoccupied BDF (return `0xFFFF_FFFF`), and BAR-remap commands
//! queued.

use crate::{PerfCounter, StatsProducer, StatsScope};

/// Aggregate PCI host-bridge counters. One instance per bus.
#[derive(Clone, Default)]
pub struct PciBusStats {
    /// ECAM config-space reads dispatched.
    pub config_reads: PerfCounter,
    /// ECAM config-space writes dispatched.
    pub config_writes: PerfCounter,
    /// Reads against unoccupied BDF slots (returned the
    /// PCI-defined `0xFFFF_FFFF` "no device" sentinel).
    pub missing_reads: PerfCounter,
    /// BAR-remap commands queued for the address-space owner.
    pub remaps_queued: PerfCounter,
}

impl PciBusStats {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StatsProducer for PciBusStats {
    fn register_stats(&self, scope: &mut StatsScope<'_>) {
        scope.adopt_counter(
            "config_reads",
            "ECAM config-space reads dispatched",
            self.config_reads.clone(),
        );
        scope.adopt_counter(
            "config_writes",
            "ECAM config-space writes dispatched",
            self.config_writes.clone(),
        );
        scope.adopt_counter(
            "missing_reads",
            "Config reads against unoccupied BDF slots",
            self.missing_reads.clone(),
        );
        scope.adopt_counter(
            "remaps_queued",
            "BAR re-programming commands queued",
            self.remaps_queued.clone(),
        );
    }
}

#[cfg(all(test, feature = "stats"))]
mod tests {
    use super::PciBusStats;
    use crate::{StatsProducer, StatsRegistry, StatsRegistryRead, StatsScope};

    #[test]
    fn register_under_canonical_path() {
        let stats = PciBusStats::new();
        stats.config_reads.add(10);
        stats.config_writes.add(4);
        stats.missing_reads.inc();
        stats.remaps_queued.add(2);

        let mut reg = StatsRegistry::new();
        {
            let mut scope = StatsScope::new(&mut reg, "system.pci.pci0");
            stats.register_stats(&mut scope);
        }
        assert_eq!(reg.counter_value("system.pci.pci0.config_reads"), Some(10));
        assert_eq!(reg.counter_value("system.pci.pci0.config_writes"), Some(4));
        assert_eq!(reg.counter_value("system.pci.pci0.missing_reads"), Some(1));
        assert_eq!(reg.counter_value("system.pci.pci0.remaps_queued"), Some(2));
    }
}

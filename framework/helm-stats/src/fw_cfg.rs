//! `FwCfgStats` -- aggregate of fw_cfg MMIO counters.
//!
//! Same shape as the other producers: every field is a
//! `PerfCounter`, hot-path increments are interior-mutable
//! (`Clone`-cheap Arc bumps), and the struct collapses to
//! zero-sized fields when `helm-stats/stats` is off.
//!
//! Designed for the QEMU-style `fw_cfg` MMIO device. The natural
//! counters here are selector switches, data reads (which advance
//! the cursor), and DMA-window writes -- these map poorly to
//! `IoStats`'s tx/rx/request/completion shape, so we keep a
//! dedicated struct.

use crate::{PerfCounter, StatsProducer, StatsScope};

/// Aggregate fw_cfg runtime counters. One instance per
/// device.
#[derive(Clone, Default)]
pub struct FwCfgStats {
    /// MMIO reads against the data register (each one advances
    /// the cursor by one byte / size).
    pub data_reads: PerfCounter,
    /// Writes that change the active selector. Equivalent to the
    /// number of fw_cfg entries the firmware has walked.
    pub selector_writes: PerfCounter,
    /// Writes against the DMA window (currently a no-op in the
    /// model). Useful as a coverage signal for guests that
    /// exercise the DMA path.
    pub dma_writes: PerfCounter,
}

impl FwCfgStats {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StatsProducer for FwCfgStats {
    fn register_stats(&self, scope: &mut StatsScope<'_>) {
        scope.adopt_counter(
            "data_reads",
            "MMIO reads against the fw_cfg data register",
            self.data_reads.clone(),
        );
        scope.adopt_counter(
            "selector_writes",
            "Writes against the fw_cfg selector register",
            self.selector_writes.clone(),
        );
        scope.adopt_counter(
            "dma_writes",
            "Writes against the fw_cfg DMA window",
            self.dma_writes.clone(),
        );
    }
}

#[cfg(all(test, feature = "stats"))]
mod tests {
    use super::FwCfgStats;
    use crate::{StatsProducer, StatsRegistry, StatsRegistryRead, StatsScope};

    #[test]
    fn register_under_canonical_path() {
        let stats = FwCfgStats::new();
        stats.data_reads.add(7);
        stats.selector_writes.inc();
        stats.dma_writes.add(2);

        let mut reg = StatsRegistry::new();
        {
            let mut scope = StatsScope::new(&mut reg, "system.fw_cfg");
            stats.register_stats(&mut scope);
        }
        assert_eq!(reg.counter_value("system.fw_cfg.data_reads"), Some(7));
        assert_eq!(reg.counter_value("system.fw_cfg.selector_writes"), Some(1));
        assert_eq!(reg.counter_value("system.fw_cfg.dma_writes"), Some(2));
    }
}

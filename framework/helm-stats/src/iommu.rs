//! `IommuStats` -- aggregate of IOMMU-side runtime counters.
//!
//! Same shape as the other producers: every field is a
//! `PerfCounter`, hot-path increments are interior-mutable
//! (`Clone`-cheap Arc bumps), and the struct collapses to
//! zero-sized fields when `helm-stats/stats` is off.
//!
//! Implements `StatsProducer` so the engine / IOMMU model can
//! hand it to a `StatsScope` rooted at the canonical
//! `system.iommu.<id>` path.

use crate::{PerfCounter, StatsProducer, StatsScope};

/// Aggregate IOMMU runtime counters. Today there is one
/// instance per IOMMU; multi-IOMMU platforms grow a producer
/// per controller as they're added.
#[derive(Clone, Default)]
pub struct IommuStats {
    /// IOVA->PA translations issued (every `translate` call).
    pub translations: PerfCounter,
    /// Translations that hit the IOMMU TLB.
    pub tlb_hits: PerfCounter,
    /// Translations that missed the IOMMU TLB and required a walk.
    pub tlb_misses: PerfCounter,
    /// Translation faults (bad STE/CD, permission, page faults).
    pub faults: PerfCounter,
}

impl IommuStats {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StatsProducer for IommuStats {
    fn register_stats(&self, scope: &mut StatsScope<'_>) {
        scope.adopt_counter(
            "translations",
            "IOVA->PA translations issued",
            self.translations.clone(),
        );
        scope.adopt_counter(
            "tlb_hits",
            "IOMMU TLB hits",
            self.tlb_hits.clone(),
        );
        scope.adopt_counter(
            "tlb_misses",
            "IOMMU TLB misses (page-table walks)",
            self.tlb_misses.clone(),
        );
        scope.adopt_counter(
            "faults",
            "IOMMU translation faults",
            self.faults.clone(),
        );
    }
}

#[cfg(all(test, feature = "stats"))]
mod tests {
    use super::IommuStats;
    use crate::{StatsProducer, StatsRegistry, StatsRegistryRead, StatsScope};

    #[test]
    fn register_under_canonical_path() {
        let stats = IommuStats::new();
        stats.translations.add(10);
        stats.tlb_hits.add(8);
        stats.tlb_misses.add(2);
        stats.faults.inc();

        let mut reg = StatsRegistry::new();
        {
            let mut scope = StatsScope::new(&mut reg, "system.iommu.smmu");
            stats.register_stats(&mut scope);
        }
        assert_eq!(reg.counter_value("system.iommu.smmu.translations"), Some(10));
        assert_eq!(reg.counter_value("system.iommu.smmu.tlb_hits"), Some(8));
        assert_eq!(reg.counter_value("system.iommu.smmu.tlb_misses"), Some(2));
        assert_eq!(reg.counter_value("system.iommu.smmu.faults"), Some(1));
    }
}

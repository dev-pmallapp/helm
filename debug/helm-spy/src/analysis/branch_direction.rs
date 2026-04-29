#[cfg(feature = "instrumentation")]
use crate::filter::PcRangeFilter;
#[cfg(feature = "instrumentation")]
use crate::trigger::Gate;
#[cfg(feature = "instrumentation")]
use std::sync::Arc;

use crate::primitives::IndexedCounter;

pub const BRANCH_DIRECTION_LABELS: &[&str] = &["taken", "not_taken"];

/// Branch direction scoreboard built on an IndexedCounter.
pub struct BranchDirectionStats {
    counts: IndexedCounter,
}

impl BranchDirectionStats {
    pub fn new() -> Self {
        Self {
            counts: IndexedCounter::new("branch_direction", BRANCH_DIRECTION_LABELS),
        }
    }

    #[inline]
    pub fn record(&self, taken: bool) {
        self.counts.inc(if taken { 0 } else { 1 });
    }

    pub fn taken(&self) -> u64 {
        self.counts.value(0)
    }

    pub fn not_taken(&self) -> u64 {
        self.counts.value(1)
    }

    pub fn total(&self) -> u64 {
        self.counts.total()
    }

    pub fn table(&self) -> Vec<(&'static str, u64, f64)> {
        self.counts.table()
    }

    #[cfg(feature = "instrumentation")]
    pub fn subscribe_to_branches(self: &Arc<Self>, probes: &mut helm_probe::CpuProbes) {
        let stats = Arc::clone(self);
        probes
            .branch
            .subscribe(move |ev: &helm_probe::BranchEvent| stats.record(ev.taken));
    }

    #[cfg(feature = "instrumentation")]
    pub fn subscribe_to_branches_gated(
        self: &Arc<Self>,
        probes: &mut helm_probe::CpuProbes,
        gate: Gate,
    ) {
        let stats = Arc::clone(self);
        probes
            .branch
            .subscribe(move |ev: &helm_probe::BranchEvent| {
                if gate.load(std::sync::atomic::Ordering::Relaxed) {
                    stats.record(ev.taken);
                }
            });
    }

    #[cfg(feature = "instrumentation")]
    pub fn subscribe_to_branches_filtered(
        self: &Arc<Self>,
        probes: &mut helm_probe::CpuProbes,
        filter: Arc<PcRangeFilter>,
    ) {
        let stats = Arc::clone(self);
        probes
            .branch
            .subscribe(move |ev: &helm_probe::BranchEvent| {
                if filter.contains(ev.pc) {
                    stats.record(ev.taken);
                }
            });
    }

    #[cfg(feature = "instrumentation")]
    pub fn subscribe_to_branches_filtered_gated(
        self: &Arc<Self>,
        probes: &mut helm_probe::CpuProbes,
        gate: Gate,
        filter: Arc<PcRangeFilter>,
    ) {
        let stats = Arc::clone(self);
        probes
            .branch
            .subscribe(move |ev: &helm_probe::BranchEvent| {
                if gate.load(std::sync::atomic::Ordering::Relaxed) && filter.contains(ev.pc) {
                    stats.record(ev.taken);
                }
            });
    }
}

impl Default for BranchDirectionStats {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, feature = "collection"))]
mod tests {
    use super::BranchDirectionStats;

    #[test]
    fn branch_direction_records_taken_and_not_taken() {
        let stats = BranchDirectionStats::new();
        stats.record(true);
        stats.record(false);
        stats.record(true);

        assert_eq!(stats.taken(), 2);
        assert_eq!(stats.not_taken(), 1);
        assert_eq!(stats.total(), 3);
    }
}

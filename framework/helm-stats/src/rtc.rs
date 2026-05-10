//! `RtcStats` -- aggregate of real-time clock runtime counters.
//!
//! Same shape as the other producers: every field is a
//! `PerfCounter`, hot-path increments are interior-mutable
//! (`Clone`-cheap Arc bumps), and the struct collapses to
//! zero-sized fields when `helm-stats/stats` is off.
//!
//! Implements `StatsProducer` so the engine can hand it to a
//! `StatsScope` rooted at the canonical `system.rtc` path.

use crate::{PerfCounter, StatsProducer, StatsScope};

/// Aggregate RTC runtime counters. Today there is one instance
/// per RTC controller; multi-RTC platforms would grow a producer
/// per device.
#[derive(Clone, Default)]
pub struct RtcStats {
    /// Register-space MMIO reads issued to the RTC.
    pub reads: PerfCounter,
    /// Register-space MMIO writes issued to the RTC.
    pub writes: PerfCounter,
    /// Alarm-match interrupts fired (counter == match_reg).
    pub alarms_fired: PerfCounter,
    /// Total simulated seconds the counter advanced through.
    pub ticks: PerfCounter,
}

impl RtcStats {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StatsProducer for RtcStats {
    fn register_stats(&self, scope: &mut StatsScope<'_>) {
        scope.adopt_counter("reads", "MMIO reads against the RTC", self.reads.clone());
        scope.adopt_counter("writes", "MMIO writes against the RTC", self.writes.clone());
        scope.adopt_counter(
            "alarms_fired",
            "Alarm-match interrupts raised",
            self.alarms_fired.clone(),
        );
        scope.adopt_counter(
            "ticks",
            "Simulated seconds the RTC counter advanced through",
            self.ticks.clone(),
        );
    }
}

#[cfg(all(test, feature = "stats"))]
mod tests {
    use super::RtcStats;
    use crate::{StatsProducer, StatsRegistry, StatsRegistryRead, StatsScope};

    #[test]
    fn register_under_canonical_path() {
        let stats = RtcStats::new();
        stats.reads.add(5);
        stats.writes.add(3);
        stats.alarms_fired.inc();
        stats.ticks.add(60);

        let mut reg = StatsRegistry::new();
        {
            let mut scope = StatsScope::new(&mut reg, "system.rtc");
            stats.register_stats(&mut scope);
        }
        assert_eq!(reg.counter_value("system.rtc.reads"), Some(5));
        assert_eq!(reg.counter_value("system.rtc.writes"), Some(3));
        assert_eq!(reg.counter_value("system.rtc.alarms_fired"), Some(1));
        assert_eq!(reg.counter_value("system.rtc.ticks"), Some(60));
    }
}

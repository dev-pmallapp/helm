//! `IntcStats` -- aggregate of interrupt-controller-side runtime
//! counters (per-type IRQ raise / ack).
//!
//! Same shape as `JitPerfStats` / `CpuStats` / `MemStats`: every
//! field is a `PerfCounter`, hot-path increments are
//! interior-mutable (`Clone`-cheap Arc bumps), and the struct
//! collapses to zero-sized fields when `helm-stats/stats` is off.
//!
//! Implements `StatsProducer` so the engine can hand it to a
//! `StatsScope` rooted at `system.gic` (or any other interrupt
//! controller path); the registry view shares the underlying
//! `Arc<AtomicU64>` storage with the hot path.

use crate::{PerfCounter, StatsProducer, StatsScope};

/// Aggregate interrupt-controller runtime counters. Today there
/// is one instance per GIC; multi-GIC platforms grow a producer
/// per controller as they're added.
#[derive(Clone, Default)]
pub struct IntcStats {
    /// SGI (Software-Generated Interrupt, IRQs 0-15) raise events.
    pub sgi_raised: PerfCounter,
    /// PPI (Private Peripheral Interrupt, IRQs 16-31) raise events.
    pub ppi_raised: PerfCounter,
    /// SPI (Shared Peripheral Interrupt, IRQs 32+) raise events.
    pub spi_raised: PerfCounter,
    /// Total interrupts acknowledged by a CPU.
    pub irq_acked: PerfCounter,
    /// Total interrupts EOI'd by a CPU.
    pub irq_eoi: PerfCounter,
}

impl IntcStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bump the per-type raise counter for `irq`. Inline so the hot
    /// path is one `match` + one `fetch_add` (or both elided when
    /// `helm-stats/stats` is off).
    #[inline]
    pub fn note_raise(&self, irq: u32) {
        if irq < 16 {
            self.sgi_raised.inc();
        } else if irq < 32 {
            self.ppi_raised.inc();
        } else {
            self.spi_raised.inc();
        }
    }
}

impl StatsProducer for IntcStats {
    fn register_stats(&self, scope: &mut StatsScope<'_>) {
        let mut interrupts = scope.child("interrupts");
        interrupts.adopt_counter("sgi", "SGI raise events", self.sgi_raised.clone());
        interrupts.adopt_counter("ppi", "PPI raise events", self.ppi_raised.clone());
        interrupts.adopt_counter("spi", "SPI raise events", self.spi_raised.clone());
        scope.adopt_counter(
            "irq_acked",
            "Interrupts acknowledged by a CPU",
            self.irq_acked.clone(),
        );
        scope.adopt_counter(
            "irq_eoi",
            "Interrupts EOI'd by a CPU",
            self.irq_eoi.clone(),
        );
    }
}

#[cfg(all(test, feature = "stats"))]
mod tests {
    use super::IntcStats;
    use crate::{StatsProducer, StatsRegistry, StatsRegistryRead, StatsScope};

    #[test]
    fn note_raise_buckets_by_irq_id() {
        let stats = IntcStats::new();
        stats.note_raise(0); // SGI
        stats.note_raise(15); // SGI
        stats.note_raise(16); // PPI
        stats.note_raise(31); // PPI
        stats.note_raise(32); // SPI
        stats.note_raise(127); // SPI
        assert_eq!(stats.sgi_raised.get(), 2);
        assert_eq!(stats.ppi_raised.get(), 2);
        assert_eq!(stats.spi_raised.get(), 2);
    }

    #[test]
    fn register_under_canonical_path() {
        let stats = IntcStats::new();
        stats.note_raise(33);
        stats.note_raise(34);
        stats.irq_acked.inc();
        stats.irq_eoi.inc();

        let mut reg = StatsRegistry::new();
        {
            let mut scope = StatsScope::new(&mut reg, "system.gic");
            stats.register_stats(&mut scope);
        }
        assert_eq!(reg.counter_value("system.gic.interrupts.spi"), Some(2));
        assert_eq!(reg.counter_value("system.gic.interrupts.sgi"), Some(0));
        assert_eq!(reg.counter_value("system.gic.irq_acked"), Some(1));
        assert_eq!(reg.counter_value("system.gic.irq_eoi"), Some(1));
    }

    #[test]
    fn shared_storage_after_register() {
        let stats = IntcStats::new();
        let mut reg = StatsRegistry::new();
        {
            let mut scope = StatsScope::new(&mut reg, "system.gic");
            stats.register_stats(&mut scope);
        }
        stats.note_raise(40);
        stats.irq_acked.add(3);
        assert_eq!(reg.counter_value("system.gic.interrupts.spi"), Some(1));
        assert_eq!(reg.counter_value("system.gic.irq_acked"), Some(3));
    }
}

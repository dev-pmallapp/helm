use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(debug_assertions)]
use std::sync::Arc;

/// Monotonic atomic counter. Thread-safe, lock-free.
/// Hot-path cost: one `fetch_add(Relaxed)` per increment.
pub struct Counter {
    name: String,
    value: AtomicU64,
}

impl Counter {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: AtomicU64::new(0),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub fn inc(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn add(&self, n: u64) {
        self.value.fetch_add(n, Ordering::Relaxed);
    }

    pub fn value(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    pub fn reset(&self) {
        self.value.store(0, Ordering::Relaxed);
    }

    /// Subscribe to post_step probe events. Increments on every step.
    #[cfg(debug_assertions)]
    pub fn subscribe_to_steps(self: &Arc<Self>, probes: &mut helm_probe::CpuProbes) {
        let c = Arc::clone(self);
        probes.post_step.subscribe(move |_| c.inc());
    }

    /// Subscribe gated by a Gate (`Arc<AtomicBool>`). Only increments when gate is armed.
    #[cfg(debug_assertions)]
    pub fn subscribe_to_steps_gated(
        self: &Arc<Self>,
        probes: &mut helm_probe::CpuProbes,
        gate: crate::trigger::Gate,
    ) {
        let c = Arc::clone(self);
        probes.post_step.subscribe(move |_| {
            if gate.load(Ordering::Relaxed) {
                c.inc();
            }
        });
    }
}

/// One counter slot per vCPU. Uses a Vec of AtomicU64.
pub struct PerVcpuCounter {
    name: String,
    slots: Vec<AtomicU64>,
}

impl PerVcpuCounter {
    pub fn new(name: impl Into<String>, num_vcpus: usize) -> Self {
        let mut slots = Vec::with_capacity(num_vcpus);
        for _ in 0..num_vcpus {
            slots.push(AtomicU64::new(0));
        }
        Self {
            name: name.into(),
            slots,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub fn inc(&self, vcpu: usize) {
        self.slots[vcpu].fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn add(&self, vcpu: usize, n: u64) {
        self.slots[vcpu].fetch_add(n, Ordering::Relaxed);
    }

    pub fn value(&self, vcpu: usize) -> u64 {
        self.slots[vcpu].load(Ordering::Relaxed)
    }

    pub fn total(&self) -> u64 {
        self.slots.iter().map(|s| s.load(Ordering::Relaxed)).sum()
    }

    pub fn per_vcpu(&self) -> Vec<u64> {
        self.slots
            .iter()
            .map(|s| s.load(Ordering::Relaxed))
            .collect()
    }

    pub fn num_vcpus(&self) -> usize {
        self.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_basic_increment_and_read() {
        let c = Counter::new("basic");
        assert_eq!(c.value(), 0, "fresh counter must be zero");

        c.inc();
        assert_eq!(c.value(), 1);

        c.inc();
        c.inc();
        assert_eq!(c.value(), 3);
    }

    #[test]
    fn counter_add_by_n() {
        let c = Counter::new("add_n");
        c.add(500);
        assert_eq!(c.value(), 500);
        c.add(1_000_000);
        assert_eq!(c.value(), 1_000_500);
    }

    #[test]
    fn counter_reset() {
        let c = Counter::new("reset");
        c.add(42);
        assert_eq!(c.value(), 42);
        c.reset();
        assert_eq!(c.value(), 0);
    }

    #[test]
    fn counter_name() {
        let c = Counter::new("my_counter");
        assert_eq!(c.name(), "my_counter");
    }

    #[test]
    fn per_vcpu_counter_basic() {
        let c = PerVcpuCounter::new("per_vcpu", 4);
        assert_eq!(c.num_vcpus(), 4);
        assert_eq!(c.total(), 0);

        c.inc(0);
        c.inc(0);
        c.inc(2);
        c.add(3, 100);

        assert_eq!(c.value(0), 2);
        assert_eq!(c.value(1), 0);
        assert_eq!(c.value(2), 1);
        assert_eq!(c.value(3), 100);
        assert_eq!(c.total(), 103);

        let vals = c.per_vcpu();
        assert_eq!(vals, vec![2, 0, 1, 100]);
    }
}

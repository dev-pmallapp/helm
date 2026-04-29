//! `Counter` and `PerVcpuCounter` -- dual-impl, feature-gated.
//!
//! - `--features=collection` (off by default): live `AtomicU64`-backed
//!   monotonic counters. Hot-path cost: one `fetch_add(Relaxed)`.
//! - default build: ZST shells, every method is `#[inline(always)]`
//!   empty. `cargo build --release` strips the call sites entirely.
//!
//! Probe-subscription helpers live behind both `collection` *and*
//! `instrumentation`; without `instrumentation` they are absent.
//!
//! See `docs/design/helm-spy/HLD.md` § 9 and
//! `docs/design/helm-spy/LLD-primitives.md`.

#[cfg(feature = "collection")]
pub use live::{Counter, PerVcpuCounter};
#[cfg(not(feature = "collection"))]
pub use noop::{Counter, PerVcpuCounter};

#[cfg(feature = "collection")]
mod live {
    use std::sync::atomic::{AtomicU64, Ordering};
    #[cfg(feature = "instrumentation")]
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
        #[cfg(feature = "instrumentation")]
        pub fn subscribe_to_steps(self: &Arc<Self>, probes: &mut helm_probe::CpuProbes) {
            let c = Arc::clone(self);
            probes.post_step.subscribe(move |_| c.inc());
        }

        /// Subscribe gated by a Gate (`Arc<AtomicBool>`). Only increments when gate is armed.
        #[cfg(feature = "instrumentation")]
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
}

#[cfg(not(feature = "collection"))]
mod noop {
    /// ZST no-op counter. All methods are inlined empty bodies.
    #[derive(Clone, Copy, Default)]
    pub struct Counter;

    impl Counter {
        #[inline(always)]
        pub fn new(_name: impl Into<String>) -> Self {
            Self
        }
        #[inline(always)]
        pub fn name(&self) -> &str {
            ""
        }
        #[inline(always)]
        pub fn inc(&self) {}
        #[inline(always)]
        pub fn add(&self, _n: u64) {}
        #[inline(always)]
        pub fn value(&self) -> u64 {
            0
        }
        #[inline(always)]
        pub fn reset(&self) {}
    }

    /// ZST no-op per-vCPU counter.
    #[derive(Clone, Copy, Default)]
    pub struct PerVcpuCounter;

    impl PerVcpuCounter {
        #[inline(always)]
        pub fn new(_name: impl Into<String>, _num_vcpus: usize) -> Self {
            Self
        }
        #[inline(always)]
        pub fn name(&self) -> &str {
            ""
        }
        #[inline(always)]
        pub fn inc(&self, _vcpu: usize) {}
        #[inline(always)]
        pub fn add(&self, _vcpu: usize, _n: u64) {}
        #[inline(always)]
        pub fn value(&self, _vcpu: usize) -> u64 {
            0
        }
        #[inline(always)]
        pub fn total(&self) -> u64 {
            0
        }
        #[inline(always)]
        pub fn per_vcpu(&self) -> Vec<u64> {
            Vec::new()
        }
        #[inline(always)]
        pub fn num_vcpus(&self) -> usize {
            0
        }
    }
}

#[cfg(all(test, feature = "collection"))]
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

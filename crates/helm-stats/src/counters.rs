//! Atomic stat counters.

use std::sync::atomic::{AtomicU64, Ordering};

/// A named counter that can be incremented from multiple threads.
pub struct Counter {
    pub name: &'static str,
    value: AtomicU64,
}

impl Counter {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            value: AtomicU64::new(0),
        }
    }

    pub fn increment(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add(&self, n: u64) {
        self.value.fetch_add(n, Ordering::Relaxed);
    }

    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    pub fn reset(&self) {
        self.value.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_counter_starts_at_zero() {
        let c = Counter::new("test");
        assert_eq!(c.get(), 0);
    }

    #[test]
    fn increment_adds_one() {
        let c = Counter::new("test");
        c.increment();
        c.increment();
        assert_eq!(c.get(), 2);
    }

    #[test]
    fn add_increases_by_n() {
        let c = Counter::new("test");
        c.add(100);
        assert_eq!(c.get(), 100);
    }

    #[test]
    fn reset_clears_value() {
        let c = Counter::new("test");
        c.add(50);
        c.reset();
        assert_eq!(c.get(), 0);
    }
}

//! `PerfCounter` -- gem5-style scalar performance counter.
//!
//! Dual-impl, feature-gated:
//!
//! - `--features=stats` (off by default): live `Arc<AtomicU64>` backing,
//!   single `fetch_add(Relaxed)` per `inc()`/`add()`. Multi-hart safe,
//!   lock-free.
//! - default build: ZST shell, every method is `#[inline(always)]` empty.
//!   `cargo build --release` strips the call sites entirely.
//!
//! See `docs/design/helm-stats/LLD-stats.md` § 0 and § 1.

#[cfg(feature = "stats")]
pub use live::PerfCounter;
#[cfg(not(feature = "stats"))]
pub use noop::PerfCounter;

#[cfg(feature = "stats")]
mod live {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// Lock-free monotonic 64-bit counter.
    /// `Clone` is cheap (Arc bump); the inner atomic is shared.
    #[derive(Clone, Default)]
    pub struct PerfCounter(Arc<AtomicU64>);

    impl PerfCounter {
        pub fn new() -> Self {
            Self::default()
        }

        /// Increment by 1. Hot path: single `fetch_add(Relaxed)`.
        #[inline(always)]
        pub fn inc(&self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }

        /// Increment by `n`. Hot path: single `fetch_add(Relaxed)`.
        #[inline(always)]
        pub fn add(&self, n: u64) {
            self.0.fetch_add(n, Ordering::Relaxed);
        }

        /// Read current value. Cold-path; not for hot loops.
        #[inline]
        pub fn get(&self) -> u64 {
            self.0.load(Ordering::Relaxed)
        }

        /// Reset to zero. Cold-path; should only be called when the
        /// simulation is paused.
        pub fn reset(&self) {
            self.0.store(0, Ordering::Relaxed);
        }
    }
}

#[cfg(not(feature = "stats"))]
mod noop {
    /// ZST no-op counter. All methods are inlined empty bodies.
    #[derive(Clone, Copy, Default)]
    pub struct PerfCounter;

    impl PerfCounter {
        #[inline(always)]
        pub fn new() -> Self {
            Self
        }
        #[inline(always)]
        pub fn inc(&self) {}
        #[inline(always)]
        pub fn add(&self, _n: u64) {}
        #[inline(always)]
        pub fn get(&self) -> u64 {
            0
        }
        #[inline(always)]
        pub fn reset(&self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::PerfCounter;

    #[test]
    fn default_value_is_zero() {
        let c = PerfCounter::new();
        assert_eq!(c.get(), 0);
    }

    #[test]
    fn inc_and_add_accumulate() {
        let c = PerfCounter::new();
        c.inc();
        c.inc();
        c.add(10);
        // With `stats`: 12. Without: 0.
        let expected: u64 = if cfg!(feature = "stats") { 12 } else { 0 };
        assert_eq!(c.get(), expected);
    }

    #[test]
    fn reset_zeros_value() {
        let c = PerfCounter::new();
        c.add(5);
        c.reset();
        assert_eq!(c.get(), 0);
    }

    #[test]
    #[cfg(not(feature = "stats"))]
    fn type_is_zst_when_disabled() {
        assert_eq!(std::mem::size_of::<PerfCounter>(), 0);
    }
}

//! `PerfHistogram` -- gem5-style fixed-bucket histogram.
//!
//! Boundaries define upper-exclusive edges. A value >= last boundary
//! goes into the overflow bucket. There are `boundaries.len() + 1`
//! buckets total (one underflow + N inner + one overflow when
//! `boundaries` is non-empty).
//!
//! Hot-path cost (with `stats`): one `partition_point` + one
//! `fetch_add(Relaxed)`.
//! Without `stats`: ZST, `record(_)` no-op.
//!
//! See `docs/design/helm-stats/LLD-stats.md` § 2.

#[cfg(feature = "stats")]
pub use live::PerfHistogram;
#[cfg(not(feature = "stats"))]
pub use noop::PerfHistogram;

#[cfg(feature = "stats")]
mod live {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    pub struct PerfHistogram {
        buckets: Vec<AtomicU64>,
        boundaries: Vec<u64>,
    }

    impl PerfHistogram {
        /// Construct with the given upper-exclusive bucket edges.
        /// Returns `Arc<Self>` because histograms are typically shared.
        pub fn new(boundaries: Vec<u64>) -> Arc<Self> {
            let n = boundaries.len() + 1;
            let buckets = (0..n).map(|_| AtomicU64::new(0)).collect();
            Arc::new(Self {
                buckets,
                boundaries,
            })
        }

        /// Record a sample. Hot path: `partition_point` + `fetch_add`.
        #[inline]
        pub fn record(&self, value: u64) {
            let idx = self.boundaries.partition_point(|&b| value >= b);
            self.buckets[idx].fetch_add(1, Ordering::Relaxed);
        }

        /// Snapshot all bucket counts, ordered underflow -> overflow.
        pub fn counts(&self) -> Vec<u64> {
            self.buckets
                .iter()
                .map(|b| b.load(Ordering::Relaxed))
                .collect()
        }

        /// Return the configured boundary list (does not include the
        /// implicit underflow / overflow positions).
        pub fn boundaries(&self) -> &[u64] {
            &self.boundaries
        }
    }
}

#[cfg(not(feature = "stats"))]
mod noop {
    use std::sync::Arc;

    /// ZST no-op histogram. `record(_)` is a no-op; `counts()` returns
    /// an empty `Vec`.
    pub struct PerfHistogram;

    impl PerfHistogram {
        #[inline(always)]
        pub fn new(_boundaries: Vec<u64>) -> Arc<Self> {
            Arc::new(Self)
        }
        #[inline(always)]
        pub fn record(&self, _value: u64) {}
        #[inline(always)]
        pub fn counts(&self) -> Vec<u64> {
            Vec::new()
        }
        #[inline(always)]
        pub fn boundaries(&self) -> &[u64] {
            &[]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PerfHistogram;

    #[test]
    fn record_partitions_into_buckets_when_enabled() {
        let h = PerfHistogram::new(vec![4, 8]);
        h.record(1);
        h.record(4);
        h.record(9);

        if cfg!(feature = "stats") {
            // [<4, 4..8, >=8] -> [1, 1, 1]
            assert_eq!(h.counts(), vec![1, 1, 1]);
        } else {
            assert!(h.counts().is_empty());
        }
    }

    #[test]
    fn boundaries_are_preserved_when_enabled() {
        let h = PerfHistogram::new(vec![10, 100]);
        if cfg!(feature = "stats") {
            assert_eq!(h.boundaries(), &[10, 100]);
        } else {
            assert!(h.boundaries().is_empty());
        }
    }
}

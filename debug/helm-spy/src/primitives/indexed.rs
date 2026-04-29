//! `IndexedCounter` -- dual-impl, feature-gated.
//!
//! Live impl is the `AtomicU64`-per-bucket form. Without `collection`,
//! it is a unit ZST whose methods are inlined empty (`value`/`total`/
//! `fraction` return 0; `table` returns an empty `Vec`).

#[cfg(feature = "collection")]
pub use live::IndexedCounter;
#[cfg(not(feature = "collection"))]
pub use noop::IndexedCounter;

#[cfg(feature = "collection")]
mod live {
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Fixed-dimension indexed counter. Each label maps to an AtomicU64 bucket.
    /// Hot-path cost: one slice index + one `fetch_add(Relaxed)`.
    pub struct IndexedCounter {
        name: String,
        labels: Vec<&'static str>,
        buckets: Vec<AtomicU64>,
    }

    impl IndexedCounter {
        pub fn new(name: impl Into<String>, labels: &[&'static str]) -> Self {
            let mut buckets = Vec::with_capacity(labels.len());
            for _ in 0..labels.len() {
                buckets.push(AtomicU64::new(0));
            }
            Self {
                name: name.into(),
                labels: labels.to_vec(),
                buckets,
            }
        }

        pub fn name(&self) -> &str {
            &self.name
        }

        pub fn len(&self) -> usize {
            self.labels.len()
        }

        pub fn is_empty(&self) -> bool {
            self.labels.is_empty()
        }

        #[inline]
        pub fn inc(&self, idx: usize) {
            self.buckets[idx].fetch_add(1, Ordering::Relaxed);
        }

        #[inline]
        pub fn add(&self, idx: usize, n: u64) {
            self.buckets[idx].fetch_add(n, Ordering::Relaxed);
        }

        pub fn value(&self, idx: usize) -> u64 {
            self.buckets[idx].load(Ordering::Relaxed)
        }

        pub fn total(&self) -> u64 {
            self.buckets.iter().map(|b| b.load(Ordering::Relaxed)).sum()
        }

        pub fn fraction(&self, idx: usize) -> f64 {
            let t = self.total();
            if t == 0 {
                0.0
            } else {
                self.value(idx) as f64 / t as f64
            }
        }

        /// Returns a table of (label, count, fraction) for all buckets.
        pub fn table(&self) -> Vec<(&'static str, u64, f64)> {
            let t = self.total();
            self.labels
                .iter()
                .zip(self.buckets.iter())
                .map(|(&label, bucket)| {
                    let v = bucket.load(Ordering::Relaxed);
                    let frac = if t == 0 { 0.0 } else { v as f64 / t as f64 };
                    (label, v, frac)
                })
                .collect()
        }

        pub fn reset(&self) {
            for b in &self.buckets {
                b.store(0, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(not(feature = "collection"))]
mod noop {
    /// ZST no-op indexed counter.
    #[derive(Clone, Copy, Default)]
    pub struct IndexedCounter;

    impl IndexedCounter {
        #[inline(always)]
        pub fn new(_name: impl Into<String>, _labels: &[&'static str]) -> Self {
            Self
        }
        #[inline(always)]
        pub fn name(&self) -> &str {
            ""
        }
        #[inline(always)]
        pub fn len(&self) -> usize {
            0
        }
        #[inline(always)]
        pub fn is_empty(&self) -> bool {
            true
        }
        #[inline(always)]
        pub fn inc(&self, _idx: usize) {}
        #[inline(always)]
        pub fn add(&self, _idx: usize, _n: u64) {}
        #[inline(always)]
        pub fn value(&self, _idx: usize) -> u64 {
            0
        }
        #[inline(always)]
        pub fn total(&self) -> u64 {
            0
        }
        #[inline(always)]
        pub fn fraction(&self, _idx: usize) -> f64 {
            0.0
        }
        #[inline(always)]
        pub fn table(&self) -> Vec<(&'static str, u64, f64)> {
            Vec::new()
        }
        #[inline(always)]
        pub fn reset(&self) {}
    }
}

#[cfg(all(test, feature = "collection"))]
mod tests {
    use super::*;

    #[test]
    fn indexed_counter_basic() {
        let labels: &[&str] = &["Alpha", "Beta", "Gamma"];
        let ic = IndexedCounter::new("test", labels);
        assert_eq!(ic.len(), 3);
        assert_eq!(ic.total(), 0);

        ic.inc(0);
        ic.inc(0);
        ic.inc(1);
        ic.add(2, 7);

        assert_eq!(ic.value(0), 2);
        assert_eq!(ic.value(1), 1);
        assert_eq!(ic.value(2), 7);
        assert_eq!(ic.total(), 10);
    }

    #[test]
    fn indexed_counter_fraction() {
        let labels: &[&str] = &["A", "B"];
        let ic = IndexedCounter::new("frac", labels);
        ic.add(0, 75);
        ic.add(1, 25);

        let f0 = ic.fraction(0);
        let f1 = ic.fraction(1);
        assert!((f0 - 0.75).abs() < 1e-10);
        assert!((f1 - 0.25).abs() < 1e-10);
    }

    #[test]
    fn indexed_counter_fraction_zero_total() {
        let labels: &[&str] = &["A", "B"];
        let ic = IndexedCounter::new("zero", labels);
        assert_eq!(ic.fraction(0), 0.0);
        assert_eq!(ic.fraction(1), 0.0);
    }

    #[test]
    fn indexed_counter_table() {
        let labels: &[&str] = &["X", "Y", "Z"];
        let ic = IndexedCounter::new("table", labels);
        ic.add(0, 50);
        ic.add(1, 30);
        ic.add(2, 20);

        let tbl = ic.table();
        assert_eq!(tbl.len(), 3);
        assert_eq!(tbl[0].0, "X");
        assert_eq!(tbl[0].1, 50);
        assert!((tbl[0].2 - 0.5).abs() < 1e-10);
        assert_eq!(tbl[1].0, "Y");
        assert_eq!(tbl[1].1, 30);
        assert!((tbl[1].2 - 0.3).abs() < 1e-10);
        assert_eq!(tbl[2].0, "Z");
        assert_eq!(tbl[2].1, 20);
        assert!((tbl[2].2 - 0.2).abs() < 1e-10);
    }

    #[test]
    fn indexed_counter_reset() {
        let labels: &[&str] = &["A", "B"];
        let ic = IndexedCounter::new("reset", labels);
        ic.add(0, 100);
        ic.add(1, 200);
        ic.reset();
        assert_eq!(ic.total(), 0);
    }
}

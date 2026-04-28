//! `LabelCounter` -- gem5-`Vector`/`SparseHistogram` analogue for
//! label-keyed sparse counts (JIT reject reasons, unsupported opcodes,
//! syscall numbers).
//!
//! With `--features=stats`: backed by `DashMap<&'static str, AtomicU64>`.
//! Hot-path cost: one DashMap shard lock + one `fetch_add(Relaxed)`.
//! Keys are `&'static str` to avoid the per-event allocation that
//! `BTreeMap<String, u64>` performs on each new key (the JIT runtime
//! call sites already pass string literals).
//!
//! Without `stats`: ZST, `bump(_)` no-op, `snapshot()` empty.
//!
//! See `docs/design/helm-stats/LLD-stats.md` § 2b.

#[cfg(feature = "stats")]
pub use live::LabelCounter;
#[cfg(not(feature = "stats"))]
pub use noop::LabelCounter;

#[cfg(feature = "stats")]
mod live {
    use dashmap::DashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// Sparse, label-keyed counter.
    /// `Clone` is cheap (Arc bump on the inner DashMap).
    #[derive(Clone, Default)]
    pub struct LabelCounter {
        slots: Arc<DashMap<&'static str, AtomicU64>>,
    }

    impl LabelCounter {
        pub fn new() -> Self {
            Self::default()
        }

        /// Increment the slot for `key` by 1. Idempotent insert.
        ///
        /// Hot path: one shard lock + one `fetch_add(Relaxed)`. The
        /// `&'static str` requirement avoids the `String::from` that
        /// a `BTreeMap<String, u64>` would force per event.
        #[inline]
        pub fn bump(&self, key: &'static str) {
            self.slots
                .entry(key)
                .or_insert_with(|| AtomicU64::new(0))
                .fetch_add(1, Ordering::Relaxed);
        }

        /// Snapshot `(label, count)` pairs sorted by descending count.
        pub fn snapshot(&self) -> Vec<(&'static str, u64)> {
            let mut out: Vec<_> = self
                .slots
                .iter()
                .map(|kv| (*kv.key(), kv.value().load(Ordering::Relaxed)))
                .collect();
            out.sort_by(|a, b| b.1.cmp(&a.1));
            out
        }

        /// Sum across all label slots.
        pub fn total(&self) -> u64 {
            self.slots
                .iter()
                .map(|kv| kv.value().load(Ordering::Relaxed))
                .sum()
        }

        /// Number of distinct labels seen.
        pub fn cardinality(&self) -> usize {
            self.slots.len()
        }

        /// Reset all slots to zero (does not remove keys).
        pub fn reset(&self) {
            for kv in self.slots.iter() {
                kv.value().store(0, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(not(feature = "stats"))]
mod noop {
    /// ZST no-op label counter.
    #[derive(Clone, Copy, Default)]
    pub struct LabelCounter;

    impl LabelCounter {
        #[inline(always)]
        pub fn new() -> Self {
            Self
        }
        #[inline(always)]
        pub fn bump(&self, _key: &'static str) {}
        #[inline(always)]
        pub fn snapshot(&self) -> Vec<(&'static str, u64)> {
            Vec::new()
        }
        #[inline(always)]
        pub fn total(&self) -> u64 {
            0
        }
        #[inline(always)]
        pub fn cardinality(&self) -> usize {
            0
        }
        #[inline(always)]
        pub fn reset(&self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::LabelCounter;

    #[test]
    fn bump_records_per_label() {
        let c = LabelCounter::new();
        c.bump("alpha");
        c.bump("alpha");
        c.bump("beta");

        if cfg!(feature = "stats") {
            assert_eq!(c.total(), 3);
            assert_eq!(c.cardinality(), 2);
            let snap = c.snapshot();
            assert_eq!(snap, vec![("alpha", 2), ("beta", 1)]);
        } else {
            assert_eq!(c.total(), 0);
            assert!(c.snapshot().is_empty());
        }
    }

    #[test]
    fn reset_zeroes_slots() {
        let c = LabelCounter::new();
        c.bump("x");
        c.bump("y");
        c.reset();
        assert_eq!(c.total(), 0);
    }

    #[test]
    #[cfg(not(feature = "stats"))]
    fn type_is_zst_when_disabled() {
        assert_eq!(std::mem::size_of::<LabelCounter>(), 0);
    }
}

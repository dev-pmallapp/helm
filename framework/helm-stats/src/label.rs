//! `LabelCounter` -- gem5-`Vector`/`SparseHistogram` analogue for
//! label-keyed sparse counts (JIT reject reasons, unsupported opcodes,
//! syscall numbers).
//!
//! With `--features=stats`: backed by `DashMap<String, AtomicU64>`.
//! Hot-path cost on the *common case* (label already present): one
//! DashMap shard lock + one `fetch_add(Relaxed)`. First-sight inserts
//! pay one `String` clone for the key.
//!
//! Two entry points:
//!
//! - `bump_static(&'static str)` -- preferred when the label is a
//!   compile-time constant (reject reasons in `data::reject`). The
//!   key is cloned only on first sight.
//! - `bump_dynamic(impl Into<String>)` -- for runtime-formatted
//!   labels (e.g. `format!("{:?}", insn.opcode)`). Caller has
//!   already paid the allocation; we adopt the `String`.
//!
//! Without `stats`: ZST, both `bump_*` are no-ops, `snapshot()` empty.
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
        slots: Arc<DashMap<String, AtomicU64>>,
    }

    impl LabelCounter {
        pub fn new() -> Self {
            Self::default()
        }

        /// Increment the slot for a compile-time `&'static str` label.
        /// Idempotent insert; clones the key only on first sight.
        #[inline]
        pub fn bump_static(&self, key: &'static str) {
            if let Some(slot) = self.slots.get(key) {
                slot.fetch_add(1, Ordering::Relaxed);
                return;
            }
            self.slots
                .entry(key.to_string())
                .or_insert_with(|| AtomicU64::new(0))
                .fetch_add(1, Ordering::Relaxed);
        }

        /// Increment the slot for a runtime-allocated label.
        /// Caller has already paid for the `String`; this adopts it.
        #[inline]
        pub fn bump_dynamic(&self, key: impl Into<String>) {
            let key = key.into();
            if let Some(slot) = self.slots.get(&key) {
                slot.fetch_add(1, Ordering::Relaxed);
                return;
            }
            self.slots
                .entry(key)
                .or_insert_with(|| AtomicU64::new(0))
                .fetch_add(1, Ordering::Relaxed);
        }

        /// Snapshot `(label, count)` pairs sorted by descending count.
        pub fn snapshot(&self) -> Vec<(String, u64)> {
            let mut out: Vec<_> = self
                .slots
                .iter()
                .map(|kv| (kv.key().clone(), kv.value().load(Ordering::Relaxed)))
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

        /// Look up the count for `key`. Returns `None` if absent.
        pub fn value(&self, key: &str) -> Option<u64> {
            self.slots
                .get(key)
                .map(|slot| slot.load(Ordering::Relaxed))
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
        pub fn bump_static(&self, _key: &'static str) {}
        #[inline(always)]
        pub fn bump_dynamic<S: Into<String>>(&self, _key: S) {}
        #[inline(always)]
        pub fn snapshot(&self) -> Vec<(String, u64)> {
            Vec::new()
        }
        #[inline(always)]
        pub fn total(&self) -> u64 {
            0
        }
        #[inline(always)]
        pub fn value(&self, _key: &str) -> Option<u64> {
            None
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
    fn bump_static_records_per_label() {
        let c = LabelCounter::new();
        c.bump_static("alpha");
        c.bump_static("alpha");
        c.bump_static("beta");

        if cfg!(feature = "stats") {
            assert_eq!(c.total(), 3);
            assert_eq!(c.cardinality(), 2);
            let snap = c.snapshot();
            assert_eq!(
                snap,
                vec![("alpha".to_string(), 2), ("beta".to_string(), 1)]
            );
        } else {
            assert_eq!(c.total(), 0);
            assert!(c.snapshot().is_empty());
        }
    }

    #[test]
    fn bump_dynamic_records_runtime_labels() {
        let c = LabelCounter::new();
        for i in 0..3 {
            c.bump_dynamic(format!("opcode_{i}"));
        }
        c.bump_dynamic(String::from("opcode_1"));

        if cfg!(feature = "stats") {
            assert_eq!(c.total(), 4);
            assert_eq!(c.cardinality(), 3);
        } else {
            assert_eq!(c.total(), 0);
        }
    }

    #[test]
    fn reset_zeroes_slots() {
        let c = LabelCounter::new();
        c.bump_static("x");
        c.bump_dynamic("y");
        c.reset();
        assert_eq!(c.total(), 0);
    }

    #[test]
    #[cfg(not(feature = "stats"))]
    fn type_is_zst_when_disabled() {
        assert_eq!(std::mem::size_of::<LabelCounter>(), 0);
    }
}

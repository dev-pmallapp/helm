//! `StatsRegistry` -- central registry for `PerfCounter` and
//! `PerfHistogram` handles, keyed by dot-path.
//!
//! With `--features=stats`: backed by `HashMap<String, ...>`, supports
//! `dump_json()` and `print_table()` for cold-path inspection.
//!
//! Without `stats`: ZST shell. `counter()` and `histogram()` return
//! the ZST handle types immediately (no hash map insert, no allocation),
//! `dump_json()` returns `"{}"`, `print_table()` is a no-op.
//!
//! See `docs/design/helm-stats/LLD-stats.md` § 4.

#[cfg(feature = "stats")]
pub use live::StatsRegistry;
#[cfg(not(feature = "stats"))]
pub use noop::StatsRegistry;

#[cfg(feature = "stats")]
mod live {
    use crate::{PerfCounter, PerfHistogram};
    use std::collections::HashMap;
    use std::sync::Arc;

    /// Global counter / histogram registry.
    /// Keyed by dot-path (e.g. `"system.cpu0.icache.hits"`).
    #[derive(Default)]
    pub struct StatsRegistry {
        counters: HashMap<String, (PerfCounter, String)>,
        histograms: HashMap<String, (Arc<PerfHistogram>, String)>,
    }

    impl StatsRegistry {
        pub fn new() -> Self {
            Self::default()
        }

        /// Create-or-retrieve a counter at `path`. The caller clones
        /// the returned handle into its own struct field for hot-path
        /// access.
        pub fn counter(&mut self, path: &str, desc: &str) -> PerfCounter {
            let entry = self
                .counters
                .entry(path.to_string())
                .or_insert_with(|| (PerfCounter::new(), desc.to_string()));
            entry.0.clone()
        }

        /// Create-or-retrieve a histogram at `path`. The boundary list
        /// is honoured only on first insert; subsequent calls return
        /// the existing histogram regardless of `boundaries`.
        pub fn histogram(
            &mut self,
            path: &str,
            desc: &str,
            boundaries: &[u64],
        ) -> Arc<PerfHistogram> {
            let entry = self.histograms.entry(path.to_string()).or_insert_with(|| {
                (PerfHistogram::new(boundaries.to_vec()), desc.to_string())
            });
            Arc::clone(&entry.0)
        }

        /// Dump every registered counter and histogram as JSON (sorted).
        pub fn dump_json(&self) -> String {
            let mut map = serde_json::Map::new();
            for (path, (counter, _)) in &self.counters {
                map.insert(path.clone(), serde_json::Value::from(counter.get()));
            }
            for (path, (histogram, _)) in &self.histograms {
                let counts = histogram
                    .counts()
                    .into_iter()
                    .map(serde_json::Value::from)
                    .collect();
                map.insert(path.clone(), serde_json::Value::Array(counts));
            }
            serde_json::to_string_pretty(&map).unwrap_or_default()
        }

        /// Print every registered counter / histogram to stdout.
        pub fn print_table(&self) {
            let mut counters: Vec<_> = self.counters.iter().collect();
            counters.sort_by_key(|(k, _)| k.as_str());
            for (path, (counter, desc)) in &counters {
                println!("{:<50} {:>16}  # {}", path, counter.get(), desc);
            }
            let mut histograms: Vec<_> = self.histograms.iter().collect();
            histograms.sort_by_key(|(k, _)| k.as_str());
            for (path, (histogram, desc)) in &histograms {
                println!("{:<50} {:>16?}  # {}", path, histogram.counts(), desc);
            }
        }
    }
}

#[cfg(not(feature = "stats"))]
mod noop {
    use crate::{PerfCounter, PerfHistogram};
    use std::sync::Arc;

    /// ZST no-op registry. All accessors return the ZST handle types
    /// immediately; no allocation, no hashing.
    #[derive(Clone, Copy, Default)]
    pub struct StatsRegistry;

    impl StatsRegistry {
        #[inline(always)]
        pub fn new() -> Self {
            Self
        }
        #[inline(always)]
        pub fn counter(&mut self, _path: &str, _desc: &str) -> PerfCounter {
            PerfCounter::new()
        }
        #[inline(always)]
        pub fn histogram(
            &mut self,
            _path: &str,
            _desc: &str,
            _boundaries: &[u64],
        ) -> Arc<PerfHistogram> {
            PerfHistogram::new(Vec::new())
        }
        #[inline(always)]
        pub fn dump_json(&self) -> String {
            "{}".to_string()
        }
        #[inline(always)]
        pub fn print_table(&self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::StatsRegistry;
    use std::sync::Arc;

    #[test]
    fn registry_reuses_histogram_and_records_samples() {
        let mut reg = StatsRegistry::new();
        let hist = reg.histogram("jit.block.size", "compiled block size", &[4, 8]);
        hist.record(1);
        hist.record(4);
        hist.record(9);

        let same = reg.histogram("jit.block.size", "ignored desc", &[1]);
        if cfg!(feature = "stats") {
            assert!(Arc::ptr_eq(&hist, &same));
            assert_eq!(hist.counts(), vec![1, 1, 1]);
        } else {
            // Without the feature, the registry returns a fresh ZST
            // handle each call; the underlying type has no state.
            assert!(hist.counts().is_empty());
        }
    }

    #[test]
    fn registry_dump_json_includes_histograms() {
        let mut reg = StatsRegistry::new();
        let counter = reg.counter("jit.blocks", "compiled blocks");
        counter.add(3);
        let hist = reg.histogram("jit.block.size", "compiled block size", &[4, 8]);
        hist.record(2);
        hist.record(9);

        let dump = reg.dump_json();
        if cfg!(feature = "stats") {
            let value: serde_json::Value =
                serde_json::from_str(&dump).expect("registry JSON");
            assert_eq!(value["jit.blocks"], 3);
            assert_eq!(value["jit.block.size"], serde_json::json!([1, 0, 1]));
        } else {
            assert_eq!(dump, "{}");
        }
    }

    #[test]
    #[cfg(not(feature = "stats"))]
    fn type_is_zst_when_disabled() {
        assert_eq!(std::mem::size_of::<StatsRegistry>(), 0);
    }
}

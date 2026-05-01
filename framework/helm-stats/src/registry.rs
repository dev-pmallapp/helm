//! `StatsRegistry` -- central registry for `PerfCounter`,
//! `PerfHistogram`, `LabelCounter`, and (Slice S5) `PerfFormula`
//! handles, keyed by dot-path.
//!
//! With `--features=stats`: backed by `HashMap<String, ...>`, supports
//! `dump_json()`, `dump_text()`, and `print_table()` for cold-path
//! inspection. With `--features=formulas`, also stores lazy formulas
//! that are evaluated at dump time.
//!
//! Without `stats`: ZST shell. `counter()` and friends return the ZST
//! handle types immediately (no hash map insert, no allocation),
//! `dump_json()` returns `"{}"`, `dump_text()` returns an empty
//! statistics block, `print_table()` is a no-op.
//!
//! Cold-path readers use the `StatsRegistryRead` trait so consumers
//! (helm-report writers, `PerfFormula::eval`) work against `&dyn`
//! without coupling to the concrete map representation.
//!
//! See `docs/design/helm-stats/LLD-stats.md` § 4 and
//! `docs/research/gem5-stats-helm-adaptation.md` § 3.6 / § 4 (S5).

#[cfg(feature = "stats")]
pub use live::StatsRegistry;
#[cfg(not(feature = "stats"))]
pub use noop::StatsRegistry;

/// Cold-path read view onto the registry. Implemented by both the
/// live and ZST registries so writers (`helm-report::emit_*`) and
/// `PerfFormula::eval` can hold a `&dyn StatsRegistryRead`.
///
/// All `*_value` accessors return `Option<u64>` (`None` = not
/// registered). `for_each_*` iterators visit entries in lexical
/// dot-path order so dump output is deterministic across runs.
pub trait StatsRegistryRead {
    fn counter_value(&self, path: &str) -> Option<u64>;
    fn histogram_total(&self, path: &str) -> Option<u64>;
    fn histogram_buckets(&self, path: &str) -> Option<Vec<u64>>;
    fn label_total(&self, path: &str) -> Option<u64>;
    fn label_snapshot(&self, path: &str) -> Option<Vec<(String, u64)>>;

    /// Iterate `(path, value, desc)` for every counter, sorted.
    fn for_each_counter(&self, f: &mut dyn FnMut(&str, u64, &str));
    /// Iterate `(path, buckets, desc)` for every histogram, sorted.
    fn for_each_histogram(&self, f: &mut dyn FnMut(&str, &[u64], &str));
    /// Iterate `(path, snapshot, desc)` for every label counter,
    /// sorted; `snapshot` is itself sorted by descending count.
    fn for_each_label(&self, f: &mut dyn FnMut(&str, &[(String, u64)], &str));
    /// Iterate `(path, value, desc)` for every formula, sorted.
    /// Formulas are evaluated lazily inside the iterator so callers
    /// see a coherent snapshot.
    fn for_each_formula(&self, f: &mut dyn FnMut(&str, f64, &str));
}

#[cfg(feature = "stats")]
mod live {
    use super::StatsRegistryRead;
    use crate::{LabelCounter, PerfCounter, PerfHistogram};
    #[cfg(feature = "formulas")]
    use crate::PerfFormula;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// Global counter / histogram / label / formula registry.
    /// Keyed by dot-path (e.g. `"system.cpu0.icache.hits"`).
    #[derive(Default)]
    pub struct StatsRegistry {
        counters: HashMap<String, (PerfCounter, String)>,
        histograms: HashMap<String, (Arc<PerfHistogram>, String)>,
        label_counters: HashMap<String, (LabelCounter, String)>,
        #[cfg(feature = "formulas")]
        formulas: HashMap<String, (PerfFormula, String)>,
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

        /// Create-or-retrieve a label counter at `path`.
        pub fn label_counter(&mut self, path: &str, desc: &str) -> LabelCounter {
            let entry = self
                .label_counters
                .entry(path.to_string())
                .or_insert_with(|| (LabelCounter::new(), desc.to_string()));
            entry.0.clone()
        }

        /// Register a lazy formula at `path`. Replaces any existing
        /// formula at the same path -- formulas are immutable
        /// definitions, not accumulators. No-op without the
        /// `formulas` feature.
        #[cfg(feature = "formulas")]
        pub fn formula(&mut self, path: &str, desc: &str, expr: PerfFormula) {
            self.formulas
                .insert(path.to_string(), (expr, desc.to_string()));
        }
        #[cfg(not(feature = "formulas"))]
        #[inline(always)]
        pub fn formula(&mut self, _path: &str, _desc: &str, _expr: crate::PerfFormula) {}

        /// Number of registered counters (cold-path; for tests).
        pub fn counter_count(&self) -> usize {
            self.counters.len()
        }

        /// Number of registered histograms (cold-path; for tests).
        pub fn histogram_count(&self) -> usize {
            self.histograms.len()
        }

        /// Number of registered label counters (cold-path; for tests).
        pub fn label_counter_count(&self) -> usize {
            self.label_counters.len()
        }

        /// Number of registered formulas (cold-path; always 0 without
        /// the `formulas` feature).
        pub fn formula_count(&self) -> usize {
            #[cfg(feature = "formulas")]
            {
                self.formulas.len()
            }
            #[cfg(not(feature = "formulas"))]
            {
                0
            }
        }

        /// Render every registered counter, histogram, label
        /// counter, and formula in gem5 `stats.txt` line shape:
        /// `<name (col 0..40)><value (col 40..60)># <desc>`.
        ///
        /// Histograms expand to `<path>::bucket_<i>` lines plus
        /// `<path>::total`. Label counters expand to `<path>::<label>`
        /// lines plus `<path>::total`. Formulas evaluate lazily.
        ///
        /// Output is wrapped by gem5's `Begin/End Simulation
        /// Statistics` markers so the result drops into existing
        /// gem5 tooling pipelines.
        pub fn dump_text(&self) -> String {
            let mut out = String::with_capacity(2048);
            out.push_str("---------- Begin Simulation Statistics ----------\n");
            self.for_each_counter(&mut |path, val, desc| {
                line(&mut out, path, &val.to_string(), desc);
            });
            self.for_each_histogram(&mut |path, buckets, desc| {
                let total: u64 = buckets.iter().sum();
                for (i, count) in buckets.iter().enumerate() {
                    line(
                        &mut out,
                        &format!("{path}::bucket_{i}"),
                        &count.to_string(),
                        desc,
                    );
                }
                line(&mut out, &format!("{path}::total"), &total.to_string(), desc);
            });
            self.for_each_label(&mut |path, snapshot, desc| {
                let total: u64 = snapshot.iter().map(|(_, v)| *v).sum();
                for (label, count) in snapshot {
                    line(
                        &mut out,
                        &format!("{path}::{label}"),
                        &count.to_string(),
                        desc,
                    );
                }
                line(&mut out, &format!("{path}::total"), &total.to_string(), desc);
            });
            self.for_each_formula(&mut |path, val, desc| {
                line(&mut out, path, &format!("{val:.6}"), desc);
            });
            out.push_str("----------  End Simulation Statistics  ----------\n");
            out
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

    fn line(out: &mut String, name: &str, val: &str, comment: &str) {
        use std::fmt::Write as _;
        let _ = writeln!(out, "{name:<40}{val:<20}# {comment}");
    }

    impl StatsRegistryRead for StatsRegistry {
        fn counter_value(&self, path: &str) -> Option<u64> {
            self.counters.get(path).map(|(c, _)| c.get())
        }

        fn histogram_total(&self, path: &str) -> Option<u64> {
            self.histograms
                .get(path)
                .map(|(h, _)| h.counts().iter().sum())
        }

        fn histogram_buckets(&self, path: &str) -> Option<Vec<u64>> {
            self.histograms.get(path).map(|(h, _)| h.counts())
        }

        fn label_total(&self, path: &str) -> Option<u64> {
            self.label_counters.get(path).map(|(l, _)| l.total())
        }

        fn label_snapshot(&self, path: &str) -> Option<Vec<(String, u64)>> {
            self.label_counters.get(path).map(|(l, _)| l.snapshot())
        }

        fn for_each_counter(&self, f: &mut dyn FnMut(&str, u64, &str)) {
            let mut keys: Vec<&String> = self.counters.keys().collect();
            keys.sort();
            for k in keys {
                let (c, d) = &self.counters[k];
                f(k.as_str(), c.get(), d.as_str());
            }
        }

        fn for_each_histogram(&self, f: &mut dyn FnMut(&str, &[u64], &str)) {
            let mut keys: Vec<&String> = self.histograms.keys().collect();
            keys.sort();
            for k in keys {
                let (h, d) = &self.histograms[k];
                let counts = h.counts();
                f(k.as_str(), &counts, d.as_str());
            }
        }

        fn for_each_label(&self, f: &mut dyn FnMut(&str, &[(String, u64)], &str)) {
            let mut keys: Vec<&String> = self.label_counters.keys().collect();
            keys.sort();
            for k in keys {
                let (l, d) = &self.label_counters[k];
                let snap = l.snapshot();
                f(k.as_str(), &snap, d.as_str());
            }
        }

        fn for_each_formula(&self, f: &mut dyn FnMut(&str, f64, &str)) {
            #[cfg(feature = "formulas")]
            {
                let mut keys: Vec<&String> = self.formulas.keys().collect();
                keys.sort();
                for k in keys {
                    let (expr, d) = &self.formulas[k];
                    let v = expr.eval(self as &dyn StatsRegistryRead);
                    f(k.as_str(), v, d.as_str());
                }
            }
            #[cfg(not(feature = "formulas"))]
            {
                let _ = f;
            }
        }
    }
}

#[cfg(not(feature = "stats"))]
mod noop {
    use super::StatsRegistryRead;
    use crate::{LabelCounter, PerfCounter, PerfFormula, PerfHistogram};
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
        pub fn label_counter(&mut self, _path: &str, _desc: &str) -> LabelCounter {
            LabelCounter::new()
        }
        #[inline(always)]
        pub fn formula(&mut self, _path: &str, _desc: &str, _expr: PerfFormula) {}
        #[inline(always)]
        pub fn counter_count(&self) -> usize {
            0
        }
        #[inline(always)]
        pub fn histogram_count(&self) -> usize {
            0
        }
        #[inline(always)]
        pub fn label_counter_count(&self) -> usize {
            0
        }
        #[inline(always)]
        pub fn formula_count(&self) -> usize {
            0
        }
        #[inline(always)]
        pub fn dump_json(&self) -> String {
            "{}".to_string()
        }
        #[inline(always)]
        pub fn dump_text(&self) -> String {
            String::from(
                "---------- Begin Simulation Statistics ----------\n----------  End Simulation Statistics  ----------\n",
            )
        }
        #[inline(always)]
        pub fn print_table(&self) {}
    }

    impl StatsRegistryRead for StatsRegistry {
        #[inline(always)]
        fn counter_value(&self, _p: &str) -> Option<u64> {
            None
        }
        #[inline(always)]
        fn histogram_total(&self, _p: &str) -> Option<u64> {
            None
        }
        #[inline(always)]
        fn histogram_buckets(&self, _p: &str) -> Option<Vec<u64>> {
            None
        }
        #[inline(always)]
        fn label_total(&self, _p: &str) -> Option<u64> {
            None
        }
        #[inline(always)]
        fn label_snapshot(&self, _p: &str) -> Option<Vec<(String, u64)>> {
            None
        }
        #[inline(always)]
        fn for_each_counter(&self, _f: &mut dyn FnMut(&str, u64, &str)) {}
        #[inline(always)]
        fn for_each_histogram(&self, _f: &mut dyn FnMut(&str, &[u64], &str)) {}
        #[inline(always)]
        fn for_each_label(&self, _f: &mut dyn FnMut(&str, &[(String, u64)], &str)) {}
        #[inline(always)]
        fn for_each_formula(&self, _f: &mut dyn FnMut(&str, f64, &str)) {}
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

    #[test]
    #[cfg(feature = "stats")]
    fn dump_text_emits_gem5_markers_and_sorted_lines() {
        let mut reg = StatsRegistry::new();
        reg.counter("system.cpu0.cycles", "cycles").add(10);
        reg.counter("system.cpu0.insns", "instructions").add(5);
        let h = reg.histogram("system.cpu0.icache.latency", "latency", &[2, 4]);
        h.record(1);
        h.record(3);
        let l = reg.label_counter("system.cpu0.jit.rejects", "reject reasons");
        l.bump_static("disabled");
        l.bump_static("disabled");
        l.bump_static("opcode");

        let text = reg.dump_text();
        assert!(text.starts_with("---------- Begin Simulation Statistics"));
        assert!(
            text.contains("End Simulation Statistics  ----------"),
            "missing gem5-style closing marker:\n{text}"
        );
        // Counters present and sorted (cycles before insns).
        let cycles_pos = text.find("system.cpu0.cycles").expect("cycles missing");
        let insns_pos = text.find("system.cpu0.insns").expect("insns missing");
        assert!(cycles_pos < insns_pos);
        // Histogram total line emitted.
        assert!(text.contains("system.cpu0.icache.latency::total"));
        // Label counter expansion.
        assert!(text.contains("system.cpu0.jit.rejects::disabled"));
        assert!(text.contains("system.cpu0.jit.rejects::total"));
    }

    #[test]
    #[cfg(feature = "formulas")]
    fn dump_text_includes_formula_eval() {
        use crate::PerfFormula;
        let mut reg = StatsRegistry::new();
        reg.counter("c.hits", "hits").add(3);
        reg.counter("c.misses", "misses").add(1);
        reg.formula(
            "c.hit_rate",
            "L1I hit rate",
            PerfFormula::div(
                PerfFormula::counter("c.hits"),
                PerfFormula::add(
                    PerfFormula::counter("c.hits"),
                    PerfFormula::counter("c.misses"),
                ),
            ),
        );
        let text = reg.dump_text();
        assert!(text.contains("c.hit_rate"));
        assert!(text.contains("0.750000"));
    }
}

//! `JitPerfStats` -- aggregate of JIT-side runtime counters.
//!
//! This is the helm-specific counters that `framework/helm-jit` and
//! `runtime/helm-engine` populate during block / trace compile and
//! execute. It is not a gem5 analogue.
//!
//! Slice S2: scalar counter fields are now `PerfCounter` (interior-
//! mutable, `Clone`-cheap, ZST when `stats` is off). Sparse label
//! fields (`unsupported_opcodes`, `reject_reasons`) are now
//! `LabelCounter` (DashMap-backed when `stats` is on, ZST when off).
//!
//! `cache_entries` and `trace_cache_entries` are *cardinalities*
//! (snapshotted from the JIT cache itself at read time), not counters,
//! so they remain plain `usize`.
//!
//! Because every field is `Clone` (Arc-cheap) and interior-mutable,
//! consumers no longer need `&mut JitPerfStats`. The struct can be
//! held by shared reference and mutated through any clone.
//!
//! See `docs/design/helm-stats/LLD-stats.md` § 8.

use crate::{LabelCounter, PerfCounter, StatsProducer, StatsScope};

/// Aggregate JIT-side runtime counters.
#[derive(Clone, Default)]
pub struct JitPerfStats {
    pub block_cache_hits: PerfCounter,
    pub block_cache_misses: PerfCounter,
    pub blocks_compiled: PerfCounter,
    pub compiled_guest_insns: PerfCounter,
    pub blocks_executed: PerfCounter,
    pub traces_compiled: PerfCounter,
    pub trace_guest_insns: PerfCounter,
    pub traces_executed: PerfCounter,
    pub trace_cache_hits: PerfCounter,
    pub trace_cache_misses: PerfCounter,
    pub trace_guard_exits: PerfCounter,
    pub trace_retired: PerfCounter,
    pub fallback_count: PerfCounter,
    pub fallback_insns: PerfCounter,
    pub unsupported_block_starts: PerfCounter,
    pub unsupported_opcodes: LabelCounter,
    pub reject_reasons: LabelCounter,
    /// Total cache promotions (snapshotted from `JitCache::promotions()`).
    /// Counted internally by the cache, not by the JIT hot path -- so this
    /// stays plain `u64`, not `PerfCounter`.
    pub cache_promotions: u64,
    /// Total cache evictions (snapshotted from `JitCache::evictions()`).
    pub cache_evictions: u64,
    /// Cardinality of the live block cache (snapshotted at read time).
    pub cache_entries: usize,
    /// Cardinality of the live trace cache (snapshotted at read time).
    pub trace_cache_entries: usize,
}

impl StatsProducer for JitPerfStats {
    /// Register every `PerfCounter` / `LabelCounter` field on the
    /// supplied scope. Counters are interior-mutable and `Clone`-cheap,
    /// so the existing handles are simply re-registered against the
    /// scope's path prefix; the registry returns the same underlying
    /// `Arc<AtomicU64>` (or DashMap) on subsequent lookups.
    ///
    /// Cardinality fields (`cache_entries`, `trace_cache_entries`,
    /// `cache_promotions`, `cache_evictions`) are snapshots, not
    /// counters, so they are not registered here -- the engine
    /// surfaces them through `jit_perf_stats()` directly.
    fn register_stats(&self, scope: &mut StatsScope<'_>) {
        // Re-register each handle. With `--features=stats` the
        // registry's HashMap entry stores the existing handle and
        // subsequent `counter_value()` reads see live updates.
        let pairs: &[(&str, &PerfCounter, &str)] = &[
            ("block_cache_hits", &self.block_cache_hits, "JIT block cache hits"),
            ("block_cache_misses", &self.block_cache_misses, "JIT block cache misses"),
            ("blocks_compiled", &self.blocks_compiled, "JIT blocks compiled"),
            (
                "compiled_guest_insns",
                &self.compiled_guest_insns,
                "Guest instructions covered by compiled blocks",
            ),
            ("blocks_executed", &self.blocks_executed, "JIT blocks executed"),
            ("traces_compiled", &self.traces_compiled, "JIT traces compiled"),
            (
                "trace_guest_insns",
                &self.trace_guest_insns,
                "Guest instructions covered by compiled traces",
            ),
            ("traces_executed", &self.traces_executed, "JIT traces executed"),
            ("trace_cache_hits", &self.trace_cache_hits, "JIT trace cache hits"),
            (
                "trace_cache_misses",
                &self.trace_cache_misses,
                "JIT trace cache misses",
            ),
            (
                "trace_guard_exits",
                &self.trace_guard_exits,
                "JIT trace side exits via guard",
            ),
            (
                "trace_retired",
                &self.trace_retired,
                "JIT traces retired by the cache",
            ),
            ("fallback_count", &self.fallback_count, "JIT fallback events"),
            (
                "fallback_insns",
                &self.fallback_insns,
                "Guest instructions executed by the fallback path",
            ),
            (
                "unsupported_block_starts",
                &self.unsupported_block_starts,
                "Block-start sites the JIT could not compile",
            ),
        ];
        for (leaf, src, desc) in pairs {
            // Inserting `src.clone()` directly into the registry would
            // give the registry an independent handle and lose the
            // shared backing. Instead, fetch-or-insert, then drive the
            // registry's slot from the source's value at dump time --
            // but PerfCounter is a `Clone`-of-Arc, so simply storing
            // the source clone ensures both views point at the same
            // AtomicU64. To do that we replace whatever the registry
            // returned with our handle.
            //
            // The cleanest path is to bypass `scope.counter()` (which
            // would create a fresh AtomicU64) and overwrite the
            // registry slot. Expose a small helper for that on the
            // registry instead of fighting the API here.
            scope.adopt_counter(leaf, desc, (*src).clone());
        }
        scope.adopt_label_counter(
            "unsupported_opcodes",
            "Sparse counts of unsupported opcode names",
            self.unsupported_opcodes.clone(),
        );
        scope.adopt_label_counter(
            "reject_reasons",
            "Sparse counts of JIT compile reject reasons",
            self.reject_reasons.clone(),
        );
    }
}

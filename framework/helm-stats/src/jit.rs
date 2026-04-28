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

use crate::{LabelCounter, PerfCounter};

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

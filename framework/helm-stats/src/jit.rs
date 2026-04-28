//! `JitPerfStats` -- aggregate of JIT-side runtime counters.
//!
//! This is the helm-specific counters that `framework/helm-jit` and
//! `runtime/helm-engine` populate during block / trace compile and
//! execute. It is not a gem5 analogue.
//!
//! **Slice S1 leaves field types unchanged** so existing consumers
//! (helm-jit, helm-engine, helm-python) continue to build. Slice S2
//! will migrate fields to `PerfCounter` / `LabelCounter` and remove
//! the `&mut JitPerfStats` plumbing in `helm-jit/runtime.rs`.
//! See `docs/design/helm-stats/LLD-stats.md` § 8.

use std::collections::BTreeMap;

/// Snapshot of JIT-side runtime counters.
#[derive(Debug, Clone, Default)]
pub struct JitPerfStats {
    pub block_cache_hits: u64,
    pub block_cache_misses: u64,
    pub blocks_compiled: u64,
    pub compiled_guest_insns: u64,
    pub blocks_executed: u64,
    pub traces_compiled: u64,
    pub trace_guest_insns: u64,
    pub traces_executed: u64,
    pub trace_cache_hits: u64,
    pub trace_cache_misses: u64,
    pub trace_guard_exits: u64,
    pub trace_retired: u64,
    pub fallback_count: u64,
    pub fallback_insns: u64,
    pub unsupported_block_starts: u64,
    pub unsupported_opcodes: BTreeMap<String, u64>,
    pub reject_reasons: BTreeMap<String, u64>,
    pub cache_entries: usize,
    pub trace_cache_entries: usize,
    pub cache_promotions: u64,
    pub cache_evictions: u64,
}

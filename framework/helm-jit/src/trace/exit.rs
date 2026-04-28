//! Guard exit handler for compiled traces (Phase 2-D).
//!
//! When a JIT trace fires a guard exit (returns `EXIT_GUARD_BASE + id`),
//! `handle_guard_exit` is called to:
//! 1. Increment the guard's miss counter.
//! 2. Return the off-trace PC so the caller can resume in block JIT or interp.
//! 3. If the guard has missed too many times, retire the trace (remove it from
//!    the `TraceCache`) and mark it as dead.

use super::compiler::{CompiledTrace, EXIT_GUARD_BASE};
use super::GUARD_MISS_THRESHOLD;
use helm_stats::JitPerfStats;

/// Runtime events that conservatively invalidate all compiled traces.
///
/// Until the trace layer carries enough metadata to target invalidation more
/// precisely, these events flush the full trace cache to avoid running with
/// stale control-flow or guest-memory assumptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceInvalidationEvent {
    /// The block JIT cache was flushed or rebuilt.
    BlockCacheFlush,
    /// JIT code patching changed compiled control-flow edges.
    CodePatch,
    /// Guest memory layout changed (e.g. `brk`, `mmap`, `munmap`, TLB shootdown).
    AddressSpaceChange,
}

/// Result of handling a guard exit.
#[derive(Debug)]
pub struct GuardExitResult {
    /// Guard index within the trace.
    pub guard_id: u32,
    /// Guest PC at which execution should resume (the off-trace branch target).
    pub resume_pc: u64,
    /// Guest instructions retired before taking the guard exit.
    pub retired_guest_insns: u32,
    /// Cumulative miss count for the triggering guard.
    pub miss_count: u32,
    /// Whether the trace should be retired (too many misses on this guard).
    pub retire_trace: bool,
}

/// Handle a guard exit for the given `exit_code` in `trace`.
///
/// Increments the miss counter for the triggering guard and returns
/// the resume PC. If the miss count exceeds `GUARD_MISS_THRESHOLD`,
/// sets `retire_trace = true` so the caller can evict the trace.
pub fn handle_guard_exit(trace: &mut CompiledTrace, exit_code: u64) -> Option<GuardExitResult> {
    let guard_id = exit_code.checked_sub(EXIT_GUARD_BASE)? as usize;
    let guard = trace.guards.get_mut(guard_id)?;

    guard.miss_count += 1;
    let retire_trace = guard.miss_count >= GUARD_MISS_THRESHOLD;

    Some(GuardExitResult {
        guard_id: guard_id as u32,
        resume_pc: guard.exit_pc,
        retired_guest_insns: guard.retired_guest_insns,
        miss_count: guard.miss_count,
        retire_trace,
    })
}

/// Handle a guard exit and update typed JIT trace counters.
pub fn handle_guard_exit_with_stats(
    trace: &mut CompiledTrace,
    exit_code: u64,
    stats: &mut JitPerfStats,
) -> Option<GuardExitResult> {
    let result = handle_guard_exit(trace, exit_code)?;
    stats.trace_guard_exits.inc();
    Some(result)
}

/// A cache of compiled traces, keyed by `start_pc`.
///
/// Checked before the block JIT cache in `run_jit`. Capped at `MAX_LIVE_TRACES`
/// with simple LRU eviction (evict the trace with the highest total guard misses
/// when capacity is full).
pub struct TraceCache {
    traces: Vec<CompiledTrace>,
}

impl TraceCache {
    /// Create an empty `TraceCache`.
    pub fn new() -> Self {
        Self { traces: Vec::new() }
    }

    /// Look up a trace by its `start_pc`.
    pub fn lookup(&self, start_pc: u64) -> Option<&CompiledTrace> {
        self.traces.iter().find(|t| t.start_pc == start_pc)
    }

    /// Look up a trace by `start_pc`, mutably (for guard miss tracking).
    pub fn lookup_mut(&mut self, start_pc: u64) -> Option<&mut CompiledTrace> {
        self.traces.iter_mut().find(|t| t.start_pc == start_pc)
    }

    /// Insert a compiled trace. If at capacity, evict the trace with the
    /// highest total guard-miss count.
    pub fn insert(&mut self, trace: CompiledTrace) {
        use super::MAX_LIVE_TRACES;

        // Remove any existing trace for the same start_pc.
        self.traces.retain(|t| t.start_pc != trace.start_pc);

        if self.traces.len() >= MAX_LIVE_TRACES {
            // Evict the trace with the most total guard misses.
            let worst = self
                .traces
                .iter()
                .enumerate()
                .max_by_key(|(_, t)| t.guards.iter().map(|g| g.miss_count).sum::<u32>())
                .map(|(i, _)| i);
            if let Some(idx) = worst {
                self.traces.swap_remove(idx);
            }
        }

        self.traces.push(trace);
    }

    /// Remove the trace for `start_pc` (called on retirement).
    pub fn retire(&mut self, start_pc: u64) {
        self.traces.retain(|t| t.start_pc != start_pc);
    }

    /// Remove the trace for `start_pc` and update retirement stats when a trace
    /// was actually present.
    pub fn retire_with_stats(&mut self, start_pc: u64, stats: &mut JitPerfStats) {
        let before = self.traces.len();
        self.retire(start_pc);
        if self.traces.len() < before {
            stats.trace_retired.inc();
        }
    }

    /// Number of live traces.
    pub fn len(&self) -> usize {
        self.traces.len()
    }

    /// Whether there are no live traces.
    pub fn is_empty(&self) -> bool {
        self.traces.is_empty()
    }

    /// Flush all traces.
    pub fn flush(&mut self) {
        self.traces.clear();
    }

    /// Conservative invalidation hook for runtime events that may stale traces.
    ///
    /// Returns the number of traces removed.
    pub fn invalidate_for_event(&mut self, _event: TraceInvalidationEvent) -> usize {
        let retired = self.traces.len();
        self.flush();
        retired
    }

    /// Conservative invalidation hook that also updates typed retirement stats.
    pub fn invalidate_for_event_with_stats(
        &mut self,
        event: TraceInvalidationEvent,
        stats: &mut JitPerfStats,
    ) -> usize {
        let retired = self.invalidate_for_event(event);
        stats.trace_retired.add(retired as u64);
        retired
    }
}

impl Default for TraceCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::CompiledBlock;
    use crate::trace::compiler::{CompiledTrace, GuardExit};

    fn make_trace(start_pc: u64, n_guards: u32) -> CompiledTrace {
        // Minimal trace with a single NOP block.
        let _insns = [{
            // single NOP placeholder; not passed into CompiledTrace
            let mut i = helm_arch::aarch64::insn::Instruction::zeroed();
            i.opcode = helm_arch::aarch64::insn::Opcode::Nop;
            i.pc = start_pc;
            i
        }];
        let guards: Vec<GuardExit> = (0..n_guards)
            .map(|id| GuardExit {
                guard_id: id,
                exit_pc: start_pc + 0x100 + id as u64 * 4,
                retired_guest_insns: id + 1,
                miss_count: 0,
            })
            .collect();
        // We can't easily run the trace compiler in a unit test without full dynasm,
        // so build a minimal CompiledTrace directly.
        crate::trace::compiler::CompiledTrace {
            // Use a placeholder block — we won't execute it in this test.
            block: {
                use dynasm::dynasm;
                let mut ops = dynasmrt::x64::Assembler::new().unwrap();
                dynasm!(ops ; xor rax, rax ; ret);
                let buf = ops.finalize().unwrap();
                #[allow(unsafe_code)]
                unsafe {
                    CompiledBlock::new_patchable(buf, 0, start_pc, 1)
                }
            },
            start_pc,
            guards,
            insn_count: 1,
        }
    }

    #[test]
    fn guard_exit_increments_miss_count() {
        let mut trace = make_trace(0x1000, 2);
        let result = handle_guard_exit(&mut trace, EXIT_GUARD_BASE + 0).unwrap();
        assert_eq!(result.resume_pc, 0x1100);
        assert_eq!(result.retired_guest_insns, 1);
        assert!(!result.retire_trace);
        assert_eq!(trace.guards[0].miss_count, 1);
    }

    #[test]
    fn guard_retires_after_threshold() {
        let mut trace = make_trace(0x1000, 1);
        for _ in 0..GUARD_MISS_THRESHOLD - 1 {
            let r = handle_guard_exit(&mut trace, EXIT_GUARD_BASE).unwrap();
            assert!(!r.retire_trace);
        }
        let r = handle_guard_exit(&mut trace, EXIT_GUARD_BASE).unwrap();
        assert!(r.retire_trace);
    }

    #[test]
    fn guard_exit_with_stats_tracks_guard_and_retirement_counts() {
        let mut trace = make_trace(0x1000, 1);
        let mut stats = JitPerfStats::default();

        for _ in 0..GUARD_MISS_THRESHOLD - 1 {
            let r = handle_guard_exit_with_stats(&mut trace, EXIT_GUARD_BASE, &mut stats).unwrap();
            assert!(!r.retire_trace);
        }
        let r = handle_guard_exit_with_stats(&mut trace, EXIT_GUARD_BASE, &mut stats).unwrap();
        assert!(r.retire_trace);
        assert_eq!(stats.trace_guard_exits.get(), u64::from(GUARD_MISS_THRESHOLD));
        assert_eq!(stats.trace_retired.get(), 0);
    }

    #[test]
    fn trace_cache_insert_and_lookup() {
        let mut cache = TraceCache::new();
        cache.insert(make_trace(0x1000, 0));
        assert!(cache.lookup(0x1000).is_some());
        assert!(cache.lookup(0x2000).is_none());
    }

    #[test]
    fn trace_cache_retire() {
        let mut cache = TraceCache::new();
        cache.insert(make_trace(0x1000, 0));
        cache.retire(0x1000);
        assert!(cache.lookup(0x1000).is_none());
    }

    #[test]
    fn trace_cache_retire_with_stats_counts_removed_trace() {
        let mut cache = TraceCache::new();
        let mut stats = JitPerfStats::default();
        cache.insert(make_trace(0x1000, 0));
        cache.retire_with_stats(0x1000, &mut stats);
        assert!(cache.lookup(0x1000).is_none());
        assert_eq!(stats.trace_retired.get(), 1);
    }

    #[test]
    fn trace_cache_invalidate_for_event_flushes_all_traces() {
        let mut cache = TraceCache::new();
        cache.insert(make_trace(0x1000, 0));
        cache.insert(make_trace(0x2000, 1));

        let retired = cache.invalidate_for_event(TraceInvalidationEvent::BlockCacheFlush);

        assert_eq!(retired, 2);
        assert!(cache.is_empty());
    }

    #[test]
    fn trace_cache_invalidate_for_event_with_stats_counts_retired_traces() {
        let mut cache = TraceCache::new();
        let mut stats = JitPerfStats::default();
        cache.insert(make_trace(0x1000, 0));
        cache.insert(make_trace(0x2000, 1));

        let retired =
            cache.invalidate_for_event_with_stats(TraceInvalidationEvent::CodePatch, &mut stats);

        assert_eq!(retired, 2);
        assert_eq!(stats.trace_retired.get(), 2);
        assert!(cache.is_empty());
    }

    #[test]
    fn trace_cache_invalidate_for_event_is_safe_when_empty() {
        let mut cache = TraceCache::new();

        let retired = cache.invalidate_for_event(TraceInvalidationEvent::AddressSpaceChange);

        assert_eq!(retired, 0);
        assert!(cache.is_empty());
    }
}

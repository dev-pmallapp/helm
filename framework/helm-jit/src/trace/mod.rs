//! LuaJIT-style trace recorder and compiler (Phase 2-D).
//!
//! Detects hot backward branches and records the execution path through
//! multiple basic blocks. Compiles the hot path as a single x86-64 function
//! with a direct backward `jmp` for the loop back-edge and guard exits for
//! conditional branches that leave the hot path.
//!
//! # Trace lifecycle
//!
//! 1. `TraceRecorder::on_backward_branch(pc)` — increments a per-PC counter.
//!    When the count reaches `TRACE_THRESHOLD`, recording begins.
//! 2. `TraceRecorder::record(pc, insns)` — appends instructions to the active
//!    recording. Detects loop closure (back to `start_pc`).
//! 3. `compiler::compile_trace(arena, insns, start_pc)` — emits the trace into
//!    the `CodeArena`. Returns a `CompiledTrace`.
//! 4. `TraceCache` — simple vec of up to `MAX_LIVE_TRACES` compiled traces,
//!    checked before the block JIT cache in the hot loop.
//! 5. On guard exit, `exit::handle_guard_exit` is called, which either falls
//!    back to block JIT / interpreter, or retires the trace after too many misses.

pub mod compiler;
pub mod exit;
pub mod recorder;

use helm_arch::aarch64::insn::Instruction;
use helm_stats::JitPerfStats;

/// Number of backward-branch executions before recording begins.
pub const TRACE_THRESHOLD: u32 = 64;

/// Maximum guest instructions in a single trace.
pub const TRACE_MAX_INSNS: usize = 512;

/// Maximum number of basic blocks inlined into a single trace.
pub const TRACE_MAX_DEPTH: u32 = 8;

/// Maximum number of guard exits in a single trace.
pub const MAX_GUARD_EXITS: usize = 32;

/// Maximum number of live compiled traces.
pub const MAX_LIVE_TRACES: usize = 256;

/// Guard-miss count above which a trace is retired.
pub const GUARD_MISS_THRESHOLD: u32 = 16;

/// Recording state machine for a single trace session.
#[derive(Default)]
#[allow(missing_docs)]
pub enum RecordState {
    /// Not currently recording.
    #[default]
    Idle,
    /// Actively recording a trace starting at `start_pc`.
    Recording {
        start_pc: u64,
        insns: Vec<Instruction>,
        pcs: Vec<u64>,
        depth: u32,
    },
    /// Recording complete — ready to compile.
    Complete {
        start_pc: u64,
        insns: Vec<Instruction>,
    },
}

/// Result of probing the trace cache before block-JIT dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceDispatchProbe {
    /// Trace dispatch is not available at the current call site.
    NotAvailable,
    /// No compiled trace is cached for the requested guest PC.
    Miss,
    /// A compiled trace exists, but live execution is still disabled.
    ReadyButDisabled,
}

/// Probe the trace cache ahead of block-JIT execution.
///
/// This currently only updates typed counters and reports whether a trace is
/// present. A later Phase 5 slice will replace `ReadyButDisabled` with actual
/// trace execution.
pub fn probe_trace_dispatch(
    cache: Option<&exit::TraceCache>,
    start_pc: u64,
    stats: &mut JitPerfStats,
) -> TraceDispatchProbe {
    let Some(cache) = cache else {
        return TraceDispatchProbe::NotAvailable;
    };

    if cache.lookup(start_pc).is_some() {
        stats.trace_cache_hits = stats.trace_cache_hits.saturating_add(1);
        TraceDispatchProbe::ReadyButDisabled
    } else {
        stats.trace_cache_misses = stats.trace_cache_misses.saturating_add(1);
        TraceDispatchProbe::Miss
    }
}

#[cfg(all(test, feature = "backend-dynasm"))]
mod tests {
    use super::*;
    use crate::block::CompiledBlock;
    use crate::trace::compiler::{CompiledTrace, GuardExit};
    use dynasm::dynasm;

    fn make_trace(start_pc: u64) -> CompiledTrace {
        let mut ops = dynasmrt::x64::Assembler::new().unwrap();
        dynasm!(ops ; xor rax, rax ; ret);
        let buf = ops.finalize().unwrap();
        CompiledTrace {
            block: {
                #[allow(unsafe_code)]
                unsafe {
                    CompiledBlock::new_patchable(buf, 0, start_pc, 1)
                }
            },
            start_pc,
            guards: vec![GuardExit {
                guard_id: 0,
                exit_pc: start_pc + 4,
                miss_count: 0,
            }],
            insn_count: 1,
        }
    }

    #[test]
    fn probe_trace_dispatch_counts_misses_and_hits() {
        let mut stats = JitPerfStats::default();
        assert_eq!(
            probe_trace_dispatch(None, 0x1000, &mut stats),
            TraceDispatchProbe::NotAvailable
        );
        assert_eq!(stats.trace_cache_hits, 0);
        assert_eq!(stats.trace_cache_misses, 0);

        let mut cache = exit::TraceCache::new();
        assert_eq!(
            probe_trace_dispatch(Some(&cache), 0x1000, &mut stats),
            TraceDispatchProbe::Miss
        );
        assert_eq!(stats.trace_cache_hits, 0);
        assert_eq!(stats.trace_cache_misses, 1);

        cache.insert(make_trace(0x1000));
        assert_eq!(
            probe_trace_dispatch(Some(&cache), 0x1000, &mut stats),
            TraceDispatchProbe::ReadyButDisabled
        );
        assert_eq!(stats.trace_cache_hits, 1);
        assert_eq!(stats.trace_cache_misses, 1);
    }
}

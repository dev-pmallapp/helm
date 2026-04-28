//! Runtime-facing JIT boundary types.
//!
//! This module is the start of the crate boundary move from `helm-engine`
//! into `helm-jit`: shared runtime policy/config should live here, while the
//! engine implements the host side.

use helm_arch::aarch64::mmu::{MmuConfig, Tlb};
use helm_arch::Aarch64Insn;
use helm_memory::HelmAddressSpace;
use helm_stats::JitPerfStats;

use crate::backend::JitBackend;
use crate::block::CompiledBlock;
use crate::cache::{CacheLookup, JitCache, JitTier, PROMOTE_THRESHOLD};
use crate::helpers::{self, JitFsContext, JitSeTlb};
use crate::regs::{REG_JIT_MEM_READ, REG_JIT_MEM_WRITE, REG_JIT_SE_TLB, REG_PC};
use crate::trace::compiler::{note_trace_compiled, note_trace_executed, EXIT_GUARD_BASE};
use crate::trace::exit::handle_guard_exit_with_stats;
use crate::trace::exit::TraceCache;
use crate::trace::recorder::TraceRecorder;

/// JIT runtime policy knobs shared between host and executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JitRuntimeConfig {
    /// Maximum interpreter instructions to run after a JIT fallback before
    /// re-entering the JIT loop.
    pub interp_fallback_batch_insns: u64,
    /// Whether compiled traces may execute ahead of block-JIT dispatch.
    ///
    /// This remains off by default even though guard exits and retirement are
    /// wired through the runtime path; broader activation is a host/runtime
    /// policy choice rather than a missing boundary contract.
    pub trace_dispatch_enabled: bool,
}

impl Default for JitRuntimeConfig {
    fn default() -> Self {
        Self {
            interp_fallback_batch_insns: 256,
            trace_dispatch_enabled: false,
        }
    }
}

/// Default runtime policy for the current JIT loop.
pub const DEFAULT_RUNTIME_CONFIG: JitRuntimeConfig = JitRuntimeConfig {
    interp_fallback_batch_insns: 256,
    trace_dispatch_enabled: false,
};

/// Backend activation policy for AArch64 JIT execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aarch64JitBackendPolicy {
    /// Use dynasm as the sole AArch64 backend.
    DynasmOnly,
    /// Use stencil as the sole AArch64 backend.
    StencilOnly,
    /// Use stencil for baseline blocks and dynasm for hot-tier promotion.
    Tiered,
}

/// Result of ensuring AArch64 runtime JIT state exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aarch64JitBackendMode {
    /// The requested backend policy is not available in the current build.
    Unavailable,
    /// Dynasm-only execution is active.
    DynasmOnly,
    /// Stencil-only execution is active.
    StencilOnly,
    /// Tiered stencil + dynasm execution is active.
    Tiered,
}

/// Ensure the shared runtime state required for AArch64 JIT execution exists.
pub fn ensure_aarch64_jit_runtime_state(
    cache: &mut Option<JitCache>,
    backend: &mut Option<Box<dyn JitBackend>>,
    _hot_backend: &mut Option<Box<dyn JitBackend>>,
    trace_cache: &mut Option<TraceCache>,
    trace_recorder: &mut Option<TraceRecorder>,
    policy: Aarch64JitBackendPolicy,
) -> Aarch64JitBackendMode {
    if cache.is_none() {
        *cache = Some(JitCache::new());
    }
    if trace_cache.is_none() {
        *trace_cache = Some(TraceCache::new());
    }
    if trace_recorder.is_none() {
        *trace_recorder = Some(TraceRecorder::default());
    }

    match policy {
        Aarch64JitBackendPolicy::DynasmOnly => {
            #[cfg(feature = "backend-dynasm")]
            {
                if backend.is_none() {
                    *backend = Some(Box::new(crate::dynasm::DynasmBackend::new()));
                }
                Aarch64JitBackendMode::DynasmOnly
            }
            #[cfg(not(feature = "backend-dynasm"))]
            {
                Aarch64JitBackendMode::Unavailable
            }
        }
        Aarch64JitBackendPolicy::StencilOnly => {
            #[cfg(feature = "backend-stencil")]
            {
                if backend.is_none() {
                    *backend = Some(Box::new(crate::stencil::StencilBackend::new_aarch64()));
                }
                Aarch64JitBackendMode::StencilOnly
            }
            #[cfg(not(feature = "backend-stencil"))]
            {
                Aarch64JitBackendMode::Unavailable
            }
        }
        Aarch64JitBackendPolicy::Tiered => {
            #[cfg(all(feature = "backend-stencil", feature = "backend-dynasm"))]
            {
                if backend.is_none() {
                    *backend = Some(Box::new(crate::stencil::StencilBackend::new_aarch64()));
                }
                if _hot_backend.is_none() {
                    *_hot_backend = Some(Box::new(crate::dynasm::DynasmBackend::new()));
                }
                Aarch64JitBackendMode::Tiered
            }
            #[cfg(not(all(feature = "backend-stencil", feature = "backend-dynasm")))]
            {
                Aarch64JitBackendMode::Unavailable
            }
        }
    }
}

/// Ensure the shared runtime state required for RISC-V64 stencil JIT exists.
#[cfg(feature = "backend-stencil")]
pub fn ensure_rv64_jit_runtime_state(
    cache: &mut Option<JitCache>,
    backend: &mut Option<Box<crate::stencil::StencilBackendRv64>>,
) -> bool {
    if cache.is_none() {
        *cache = Some(JitCache::new());
    }
    if backend.is_none() {
        *backend = Some(Box::new(crate::stencil::StencilBackendRv64::new()));
    }
    true
}

/// Host-provided AArch64 memory/MMU surface for one JIT dispatch step.
pub enum Aarch64JitMemoryMode<'a> {
    /// SE-mode direct RAM access with the inline JIT TLB fast path.
    Se {
        /// Opaque pointer passed to SE-mode JIT memory helpers.
        mem_ptr: *mut u8,
        /// Backing storage for the SE-mode inline TLB register slot.
        se_tlb: &'a mut JitSeTlb,
    },
    /// FS-mode RAM + MMIO access through a translated virtual-address context.
    Fs {
        /// Live system memory owner for the active board.
        sys_mem: *mut HelmAddressSpace,
        /// Software TLB shared with the active vCPU FS state.
        tlb: *mut Tlb,
        /// Snapshotted MMU configuration for this dispatch step.
        mmu_cfg: MmuConfig,
    },
}

/// Prepared AArch64 dispatch context for one compiled block or trace entry.
pub enum Aarch64JitDispatchContext {
    /// SE-mode dispatch carries a direct pointer to the flat RAM owner.
    Se {
        /// Opaque direct-memory pointer consumed by SE-mode JIT helpers.
        mem_ptr: *mut u8,
    },
    /// FS-mode dispatch owns the translated memory context for the current step.
    Fs {
        /// Owned FS translation/access context kept alive across compiled calls.
        fs_ctx: JitFsContext,
    },
}

impl Aarch64JitDispatchContext {
    /// Opaque memory/context pointer to pass into compiled code.
    pub fn mem_ptr(&mut self) -> *mut u8 {
        match self {
            Self::Se { mem_ptr } => *mem_ptr,
            Self::Fs { fs_ctx } => std::ptr::from_mut::<JitFsContext>(fs_ctx).cast::<u8>(),
        }
    }
}

/// Populate the flat-register helper slots and return the opaque memory/context
/// pointer for one AArch64 JIT dispatch step.
pub fn prepare_aarch64_jit_dispatch_context(
    flat_regs: &mut [u64; crate::regs::REG_COUNT],
    memory_mode: Aarch64JitMemoryMode<'_>,
) -> Aarch64JitDispatchContext {
    match memory_mode {
        Aarch64JitMemoryMode::Se { mem_ptr, se_tlb } => {
            flat_regs[REG_JIT_MEM_READ] = helpers::jit_mem_read as *const () as u64;
            flat_regs[REG_JIT_MEM_WRITE] = helpers::jit_mem_write as *const () as u64;
            flat_regs[REG_JIT_SE_TLB] = se_tlb.entries.as_ptr() as u64;
            Aarch64JitDispatchContext::Se { mem_ptr }
        }
        Aarch64JitMemoryMode::Fs {
            sys_mem,
            tlb,
            mmu_cfg,
        } => {
            flat_regs[REG_JIT_MEM_READ] = helpers::jit_fs_mem_read as *const () as u64;
            flat_regs[REG_JIT_MEM_WRITE] = helpers::jit_fs_mem_write as *const () as u64;
            flat_regs[REG_JIT_SE_TLB] = 0;
            Aarch64JitDispatchContext::Fs {
                fs_ctx: JitFsContext {
                    sys_mem,
                    tlb,
                    mmu_cfg,
                },
            }
        }
    }
}

/// Result of probing the block cache with hotness tracking.
pub enum BlockCacheProbe {
    /// A compiled block was found in the cache.
    Hit(CacheLookup),
    /// No compiled block exists for the requested guest PC.
    Miss,
}

/// Result of compiling a block after a cache miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileOnMiss {
    /// A block was compiled, inserted into the cache, and any waiters were linked.
    Cached {
        /// Number of guest instructions compiled into the cached block.
        insn_count: u32,
    },
    /// The backend could not compile the first instruction in the block.
    UnsupportedStart {
        /// Why the stencil lookup or backend rejected the first instruction.
        reason: Option<&'static str>,
    },
}

/// Result of resolving a decoded block cache miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileMissResolution<S> {
    /// The block was compiled and inserted into the cache.
    Cached {
        /// Number of guest instructions compiled into the cached block.
        insn_count: u32,
    },
    /// The host resumed JIT execution after a bounded interpreter batch.
    Resume {
        /// Remaining JIT budget after interpreter fallback.
        budget_remaining: u64,
    },
    /// The miss path terminated JIT execution and returned a stop reason.
    Stop {
        /// Host-specific stop reason.
        stop: S,
    },
}

/// Result of attempting hot-tier promotion for a cached block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromotionResolution {
    /// No promotion was attempted or completed.
    NotPromoted,
    /// A promoted block was executed and returned an exit code.
    Executed {
        /// Exit code returned by the promoted block.
        exit_code: u64,
    },
}

/// Result of probing the trace cache before block-JIT dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceDispatch {
    /// Trace dispatch is not available at the current call site.
    NotAvailable,
    /// No compiled trace is cached for the requested guest PC.
    Miss,
    /// A compiled trace exists, but live dispatch remains disabled.
    SkippedDisabled,
    /// A compiled trace executed and returned an exit code.
    Executed {
        /// Exit code returned by the trace body.
        exit_code: u64,
        /// Guard-exit metadata when the trace side-exited through a guard.
        guard: Option<TraceGuardInfo>,
    },
}

/// Additional metadata for a trace guard exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceGuardInfo {
    /// Stable recorder-assigned identifier for the guard that fired.
    pub guard_id: u32,
    /// Guest PC where block-JIT execution should resume after the side exit.
    pub resume_pc: u64,
    /// Number of misses recorded for this guard after applying the current exit.
    pub miss_count: u32,
    /// Whether the runtime marked the parent trace for retirement.
    pub retiring: bool,
    /// Number of guest instructions retired by the trace before the guard exit.
    pub retired_guest_insns: u32,
}

/// Decision for whether the current executed block should be decoded and fed
/// into the trace recorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceRecordPlan {
    /// The current block does not participate in trace recording.
    Ignore,
    /// Decode the current block and feed it into the active trace recording.
    DecodeCurrentBlock,
}

/// Result of feeding a decoded block into the trace recorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceRecordResult {
    /// Recording is still in progress and no compiled trace was produced yet.
    Pending,
    /// The recorded path could not be compiled into a trace.
    CompileMiss,
    /// A trace was compiled from the recorded path.
    Compiled {
        /// Guest PC at the start of the trace.
        start_pc: u64,
        /// Number of guest instructions compiled into the trace.
        insn_count: u32,
    },
}

/// Result of executing a bounded interpreter fallback batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterpreterFallback<S> {
    /// The interpreter consumed a bounded batch and the JIT loop may resume.
    Resume {
        /// Number of instructions retired by the interpreter batch.
        consumed: u64,
        /// Remaining JIT budget after the interpreter batch completed.
        budget_remaining: u64,
    },
    /// The host produced a terminal or non-resumable stop reason.
    Stop {
        /// Host-specific stop reason returned by the interpreter batch.
        stop: S,
        /// Number of instructions retired by the interpreter batch.
        consumed: u64,
        /// Remaining JIT budget after the interpreter batch completed.
        budget_remaining: u64,
    },
}

/// Minimal host-side hook for JIT/interpreter cooperation.
///
/// The full `run_jit()` loop still lives in `helm-engine` today, but this
/// trait establishes the host boundary that later slices can build on.
pub trait JitRuntimeHost {
    /// Host-specific stop-reason type returned by interpreter batches.
    type StopReason;

    /// Mutable access to the host-owned JIT runtime counters.
    fn jit_stats_mut(&mut self) -> &mut JitPerfStats;

    /// Total retired instructions visible to the host.
    fn insns_retired(&self) -> u64;

    /// Run a bounded interpreter batch from the current architectural state.
    fn run_interpreter_batch(&mut self, max_insns: u64) -> Self::StopReason;

    /// Return `true` when the stop reason allows the JIT loop to resume.
    fn is_resumable_stop(stop: &Self::StopReason) -> bool;

    /// Flush current JIT-side state into the host before interpreter fallback
    /// and commit any already-retired JIT instructions.
    fn prepare_interpreter_fallback(&mut self, flat_regs: &mut [u64], retired_insns: u64);

    /// Rebuild JIT-side state from the host after a resumable interpreter batch.
    fn restore_jit_state_after_interpreter(
        &mut self,
        flat_regs: &mut [u64],
    ) -> Result<(), Self::StopReason>;

    /// Record an interpreter fallback batch for external observability.
    fn record_interpreter_fallback(&mut self, consumed: u64, reason: Option<&'static str>);
}

/// Probe the JIT block cache and update shared hit/miss counters.
pub fn probe_block_cache(
    cache: &mut JitCache,
    stats: &mut JitPerfStats,
    pc: u64,
) -> BlockCacheProbe {
    if let Some(hit) = cache.lookup_hot(pc) {
        stats.block_cache_hits.inc();
        BlockCacheProbe::Hit(hit)
    } else {
        stats.block_cache_misses.inc();
        BlockCacheProbe::Miss
    }
}

/// Probe the trace cache ahead of block-JIT execution.
///
/// This uses the future trace-dispatch call shape already needed by the hot
/// loop, but while `trace_dispatch_enabled` is false it only updates lookup
/// counters and reports that dispatch was skipped.
/// # Safety
///
/// `mem_ptr` must point to the active memory/context object expected by the
/// compiled trace entry stub for the current execution mode, and must remain
/// valid for the duration of the call.
#[allow(unsafe_code)]
pub unsafe fn dispatch_trace(
    cache: Option<&mut TraceCache>,
    stats: &mut JitPerfStats,
    pc: u64,
    flat_regs: &mut [u64],
    mem_ptr: *mut u8,
    retired: &mut u64,
    budget_remaining: &mut u64,
    config: JitRuntimeConfig,
) -> TraceDispatch {
    let Some(cache) = cache else {
        return TraceDispatch::NotAvailable;
    };

    if cache.lookup(pc).is_none() {
        stats.trace_cache_misses.inc();
        return TraceDispatch::Miss;
    }

    stats.trace_cache_hits.inc();
    if !config.trace_dispatch_enabled {
        return TraceDispatch::SkippedDisabled;
    }

    let mut retire_trace = false;
    let mut guard_info = None;
    let exit_code;

    {
        let trace = cache
            .lookup_mut(pc)
            .expect("trace lookup should stay stable");
        note_trace_executed(stats);
        exit_code = unsafe { (trace.block.entry)(flat_regs.as_mut_ptr(), mem_ptr) };

        let retired_guest_insns = if exit_code == crate::block::EXIT_END_OF_BLOCK {
            u64::from(trace.insn_count)
        } else if exit_code >= EXIT_GUARD_BASE {
            let guard = handle_guard_exit_with_stats(trace, exit_code, stats)
                .expect("guard exit should resolve for compiled trace");
            flat_regs[REG_PC] = guard.resume_pc;
            retire_trace = guard.retire_trace;
            guard_info = Some(TraceGuardInfo {
                guard_id: guard.guard_id,
                resume_pc: guard.resume_pc,
                miss_count: guard.miss_count,
                retiring: guard.retire_trace,
                retired_guest_insns: guard.retired_guest_insns,
            });
            u64::from(guard.retired_guest_insns)
        } else {
            0
        };

        *retired = retired.saturating_add(retired_guest_insns);
        *budget_remaining = budget_remaining.saturating_sub(retired_guest_insns);
    }

    if retire_trace {
        cache.retire_with_stats(pc, stats);
    }

    TraceDispatch::Executed {
        exit_code,
        guard: guard_info,
    }
}

/// Decide whether the current executed block should be decoded and recorded as
/// part of a hot-trace candidate.
pub fn plan_aarch64_trace_recording(
    recorder: &mut TraceRecorder,
    start_pc: u64,
    next_pc: u64,
) -> TraceRecordPlan {
    let was_recording = recorder.is_recording();
    let started_now = next_pc <= start_pc && recorder.on_backward_branch(next_pc);
    if was_recording || (started_now && start_pc == next_pc) {
        TraceRecordPlan::DecodeCurrentBlock
    } else {
        TraceRecordPlan::Ignore
    }
}

/// Feed a decoded AArch64 block into the trace recorder and compile/cache the
/// completed trace when the recording closes.
pub fn record_aarch64_trace_candidate(
    recorder: &mut TraceRecorder,
    cache: Option<&mut TraceCache>,
    stats: &mut JitPerfStats,
    decoded_insns: &[Aarch64Insn],
) -> TraceRecordResult {
    let Some((trace_start, trace_insns)) = recorder.record_block(decoded_insns) else {
        return TraceRecordResult::Pending;
    };

    let Some(trace) = crate::trace::compiler::compile_trace(&trace_insns, trace_start) else {
        return TraceRecordResult::CompileMiss;
    };

    let insn_count = trace.insn_count;
    note_trace_compiled(stats, &trace);
    if let Some(cache) = cache {
        cache.insert(trace);
    }

    TraceRecordResult::Compiled {
        start_pc: trace_start,
        insn_count,
    }
}

/// Compile a decoded AArch64 block after a cache miss and insert it into the cache.
pub fn compile_block_on_miss<B: JitBackend + ?Sized>(
    cache: &mut JitCache,
    stats: &mut JitPerfStats,
    backend: &mut B,
    tier: JitTier,
    pc: u64,
    insns: &[Aarch64Insn],
) -> CompileOnMiss {
    match backend.compile_block(pc, insns) {
        Some(block) => {
            let insn_count = block.insn_count;
            stats.blocks_compiled.inc();
            stats.compiled_guest_insns.add(u64::from(insn_count));
            cache.insert_with_tier(block, tier);
            cache.link_waiters(pc);
            CompileOnMiss::Cached { insn_count }
        }
        None => CompileOnMiss::UnsupportedStart {
            reason: backend.last_reject_reason(),
        },
    }
}

/// Execute a cached compiled block and update shared execution counters.
///
/// # Safety
/// The compiled block entry must follow the JIT ABI for the provided flat
/// register slice and opaque memory pointer.
#[allow(unsafe_code)]
pub unsafe fn execute_compiled_block(
    stats: &mut JitPerfStats,
    block: &CompiledBlock,
    flat_regs: &mut [u64],
    mem_ptr: *mut u8,
    retired: &mut u64,
    budget_remaining: &mut u64,
) -> u64 {
    stats.blocks_executed.inc();
    // Clear per-exit retired slot so stale values from earlier blocks
    // don't affect retirement when the current block doesn't set it.
    if let Some(slot) = flat_regs.get_mut(crate::regs::REG_JIT_RETIRED) {
        *slot = 0;
    }
    let exit_code = (block.entry)(flat_regs.as_mut_ptr(), mem_ptr);
    // Re-zero XZR: stencils like SUBS_IMM (used for CMP) write their result
    // to rd which may be the XZR slot. Without this, the next block sees a
    // non-zero XZR and ORR-based MOV (MOV Xd, Xm = ORR Xd, XZR, Xm) breaks.
    flat_regs[crate::regs::REG_XZR] = 0;
    // Use the actual retired count written by the exit path, falling back
    // to the compiled block count for blocks that don't set it (stencil,
    // trace, test stubs).
    let actual_retired = flat_regs
        .get(crate::regs::REG_JIT_RETIRED)
        .copied()
        .unwrap_or(0);
    let block_insns = if actual_retired > 0 {
        actual_retired
    } else {
        u64::from(block.insn_count)
    };
    *retired = retired.saturating_add(block_insns);
    *budget_remaining = budget_remaining.saturating_sub(block_insns);
    exit_code
}

/// Attempt hot-tier promotion for a cached block and execute the promoted block.
///
/// # Safety
///
/// `mem_ptr` must point to a valid runtime memory/context object compatible
/// with the compiled block entry stubs being executed.
#[allow(unsafe_code)]
pub unsafe fn maybe_promote_and_execute<B: JitBackend + ?Sized>(
    cache: &mut JitCache,
    stats: &mut JitPerfStats,
    hot_backend: Option<&mut B>,
    pc: u64,
    exec_count: u32,
    tier: JitTier,
    decoded_insns: &[Aarch64Insn],
    flat_regs: &mut [u64],
    mem_ptr: *mut u8,
    retired: &mut u64,
    budget_remaining: &mut u64,
) -> PromotionResolution {
    if exec_count != PROMOTE_THRESHOLD || tier != JitTier::Stencil || decoded_insns.is_empty() {
        return PromotionResolution::NotPromoted;
    }

    let Some(hot_backend) = hot_backend else {
        return PromotionResolution::NotPromoted;
    };

    let Some(promoted) = hot_backend.compile_block(pc, decoded_insns) else {
        return PromotionResolution::NotPromoted;
    };

    let _ = cache.promote(pc, promoted, JitTier::Dynasm);
    match probe_block_cache(cache, stats, pc) {
        BlockCacheProbe::Hit(hit) => PromotionResolution::Executed {
            exit_code: execute_compiled_block(
                stats,
                &hit.block,
                flat_regs,
                mem_ptr,
                retired,
                budget_remaining,
            ),
        },
        BlockCacheProbe::Miss => PromotionResolution::NotPromoted,
    }
}

/// Execute a probed cache hit, optionally attempting hot-tier promotion first.
///
/// # Safety
///
/// `mem_ptr` must point to a valid runtime memory/context object compatible
/// with the compiled block entry stub selected by `hit`.
#[allow(unsafe_code)]
pub unsafe fn execute_cache_hit<B: JitBackend + ?Sized>(
    cache: &mut JitCache,
    stats: &mut JitPerfStats,
    hit: CacheLookup,
    hot_backend: Option<&mut B>,
    pc: u64,
    promotion_insns: Option<&[Aarch64Insn]>,
    flat_regs: &mut [u64],
    mem_ptr: *mut u8,
    retired: &mut u64,
    budget_remaining: &mut u64,
) -> u64 {
    if let Some(decoded_insns) = promotion_insns {
        match maybe_promote_and_execute(
            cache,
            stats,
            hot_backend,
            pc,
            hit.exec_count,
            hit.tier,
            decoded_insns,
            flat_regs,
            mem_ptr,
            retired,
            budget_remaining,
        ) {
            PromotionResolution::Executed { exit_code } => return exit_code,
            PromotionResolution::NotPromoted => {}
        }
    }

    execute_compiled_block(
        stats,
        &hit.block,
        flat_regs,
        mem_ptr,
        retired,
        budget_remaining,
    )
}

/// Handle fallback when block compilation fails at the first instruction.
pub fn handle_unsupported_start_fallback<H: JitRuntimeHost>(
    host: &mut H,
    flat_regs: &mut [u64],
    retired: &mut u64,
    budget_remaining: u64,
    unsupported_opcode: Option<String>,
    reject_reason: Option<&'static str>,
    config: JitRuntimeConfig,
) -> InterpreterFallback<H::StopReason> {
    host.prepare_interpreter_fallback(flat_regs, *retired);
    *retired = 0;

    let fallback_reason = reject_reason.or(Some("unsupported-start"));

    {
        let stats = host.jit_stats_mut();
        stats.fallback_count.inc();
        stats.unsupported_block_starts.inc();
        if let Some(opcode) = unsupported_opcode {
            stats.unsupported_opcodes.bump_dynamic(opcode);
        }
    }

    match run_bounded_interpreter_fallback(host, budget_remaining, config) {
        InterpreterFallback::Resume {
            consumed,
            budget_remaining,
        } => {
            let stats = host.jit_stats_mut();
            stats.fallback_insns.add(consumed);
            host.record_interpreter_fallback(consumed, fallback_reason);
            match host.restore_jit_state_after_interpreter(flat_regs) {
                Ok(()) => InterpreterFallback::Resume {
                    consumed,
                    budget_remaining,
                },
                Err(stop) => InterpreterFallback::Stop {
                    stop,
                    consumed,
                    budget_remaining,
                },
            }
        }
        InterpreterFallback::Stop {
            stop,
            consumed,
            budget_remaining,
        } => {
            let stats = host.jit_stats_mut();
            stats.fallback_insns.add(consumed);
            host.record_interpreter_fallback(consumed, fallback_reason);
            InterpreterFallback::Stop {
                stop,
                consumed,
                budget_remaining,
            }
        }
    }
}

/// Exit the JIT loop and hand the remaining budget to the interpreter.
pub fn handoff_to_interpreter<H: JitRuntimeHost>(
    host: &mut H,
    flat_regs: &mut [u64],
    retired: &mut u64,
    budget_remaining: u64,
) -> H::StopReason {
    host.prepare_interpreter_fallback(flat_regs, *retired);
    *retired = 0;
    host.run_interpreter_batch(budget_remaining)
}

/// Resolve a decoded AArch64 block cache miss.
pub fn resolve_aarch64_compile_miss<H: JitRuntimeHost, B: JitBackend + ?Sized>(
    host: &mut H,
    cache: &mut JitCache,
    backend: Option<&mut B>,
    fallback_backend: Option<&mut B>,
    pc: u64,
    decoded_insns: &[Aarch64Insn],
    flat_regs: &mut [u64],
    retired: &mut u64,
    budget_remaining: u64,
    unsupported_opcode: Option<String>,
    config: JitRuntimeConfig,
) -> CompileMissResolution<H::StopReason> {
    if decoded_insns.is_empty() {
        return CompileMissResolution::Stop {
            stop: handoff_to_interpreter(host, flat_regs, retired, budget_remaining),
        };
    }

    let Some(backend) = backend else {
        return CompileMissResolution::Stop {
            stop: handoff_to_interpreter(host, flat_regs, retired, budget_remaining),
        };
    };

    let primary_tier = match backend.name() {
        "dynasm" => JitTier::Dynasm,
        _ => JitTier::Stencil,
    };

    match compile_block_on_miss(
        cache,
        host.jit_stats_mut(),
        backend,
        primary_tier,
        pc,
        decoded_insns,
    ) {
        CompileOnMiss::Cached { insn_count } => CompileMissResolution::Cached { insn_count },
        CompileOnMiss::UnsupportedStart { reason } => {
            if let Some(fallback_backend) = fallback_backend {
                let fallback_tier = match fallback_backend.name() {
                    "dynasm" => JitTier::Dynasm,
                    _ => JitTier::Stencil,
                };
                if let CompileOnMiss::Cached { insn_count } = compile_block_on_miss(
                    cache,
                    host.jit_stats_mut(),
                    fallback_backend,
                    fallback_tier,
                    pc,
                    decoded_insns,
                ) {
                    return CompileMissResolution::Cached { insn_count };
                }
            }

            match handle_unsupported_start_fallback(
                host,
                flat_regs,
                retired,
                budget_remaining,
                unsupported_opcode,
                reason,
                config,
            ) {
                InterpreterFallback::Resume {
                    consumed: _consumed,
                    budget_remaining,
                } => CompileMissResolution::Resume { budget_remaining },
                InterpreterFallback::Stop {
                    stop,
                    consumed: _consumed,
                    budget_remaining: _remaining,
                } => CompileMissResolution::Stop { stop },
            }
        }
    }
}

/// Execute the shared bounded interpreter-fallback policy.
pub fn run_bounded_interpreter_fallback<H: JitRuntimeHost>(
    host: &mut H,
    budget_remaining: u64,
    config: JitRuntimeConfig,
) -> InterpreterFallback<H::StopReason> {
    let batch = config.interp_fallback_batch_insns.min(budget_remaining);
    let before = host.insns_retired();
    let stop = host.run_interpreter_batch(batch);
    let consumed = host.insns_retired().saturating_sub(before);
    let budget_remaining = budget_remaining.saturating_sub(consumed);

    if H::is_resumable_stop(&stop) && budget_remaining > 0 {
        InterpreterFallback::Resume {
            consumed,
            budget_remaining,
        }
    } else {
        InterpreterFallback::Stop {
            stop,
            consumed,
            budget_remaining,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{CompiledBlock, JitBlockFn, EXIT_END_OF_BLOCK};
    use crate::trace::compiler::{CompiledTrace, GuardExit};
    use crate::trace::exit::TraceCache;
    use crate::trace::recorder::TraceRecorder;
    use helm_arch::aarch64::arch_state::Aarch64ArchState;
    use helm_arch::aarch64::insn::{Instruction, Opcode};
    use helm_memory::{FlatMem, HelmAddressSpace};

    #[allow(unsafe_code)]
    unsafe extern "C" fn test_block(_regs: *mut u64, _mem: *mut u8) -> u64 {
        EXIT_END_OF_BLOCK
    }

    #[allow(unsafe_code)]
    unsafe extern "C" fn test_guard_block(_regs: *mut u64, _mem: *mut u8) -> u64 {
        EXIT_GUARD_BASE
    }

    fn make_test_block(pc: u64, insn_count: u32) -> CompiledBlock {
        #[allow(unsafe_code)]
        unsafe {
            let entry: JitBlockFn = test_block;
            CompiledBlock::new((), entry, pc, insn_count)
        }
    }

    fn make_test_guard_block(pc: u64, insn_count: u32) -> CompiledBlock {
        #[allow(unsafe_code)]
        unsafe {
            let entry: JitBlockFn = test_guard_block;
            CompiledBlock::new((), entry, pc, insn_count)
        }
    }

    fn make_test_trace(pc: u64, insn_count: u32) -> CompiledTrace {
        CompiledTrace {
            block: make_test_block(pc, insn_count),
            start_pc: pc,
            guards: vec![GuardExit {
                guard_id: 0,
                exit_pc: pc + 4,
                retired_guest_insns: insn_count,
                miss_count: 0,
            }],
            insn_count,
        }
    }

    fn make_test_guard_trace(pc: u64, insn_count: u32, miss_count: u32) -> CompiledTrace {
        CompiledTrace {
            block: make_test_guard_block(pc, insn_count),
            start_pc: pc,
            guards: vec![GuardExit {
                guard_id: 0,
                exit_pc: pc + 0x20,
                retired_guest_insns: insn_count,
                miss_count,
            }],
            insn_count,
        }
    }

    #[test]
    fn prepare_aarch64_dispatch_context_sets_se_helper_slots() {
        let mut flat_regs = [0u64; crate::regs::REG_COUNT];
        let mut mem = FlatMem::new(0, 0x1000);
        let mut se_tlb = JitSeTlb::new();

        let ctx = prepare_aarch64_jit_dispatch_context(
            &mut flat_regs,
            Aarch64JitMemoryMode::Se {
                mem_ptr: (&mut mem as *mut FlatMem).cast::<u8>(),
                se_tlb: &mut se_tlb,
            },
        );

        let mut ctx = ctx;
        assert_eq!(ctx.mem_ptr(), (&mut mem as *mut FlatMem).cast::<u8>());
        assert!(matches!(ctx, Aarch64JitDispatchContext::Se { .. }));
        assert_eq!(
            flat_regs[REG_JIT_MEM_READ],
            helpers::jit_mem_read as *const () as u64
        );
        assert_eq!(
            flat_regs[REG_JIT_MEM_WRITE],
            helpers::jit_mem_write as *const () as u64
        );
        assert_eq!(flat_regs[REG_JIT_SE_TLB], se_tlb.entries.as_ptr() as u64);
    }

    #[test]
    fn prepare_aarch64_dispatch_context_builds_fs_context() {
        let mut flat_regs = [u64::MAX; crate::regs::REG_COUNT];
        let mut sys_mem = HelmAddressSpace::new(FlatMem::new(0, 0x1000));
        let mut tlb = Tlb::new();
        let mmu_cfg = MmuConfig::from_arch(&Aarch64ArchState::new());
        let sys_mem_ptr = &mut sys_mem as *mut HelmAddressSpace;
        let tlb_ptr = &mut tlb as *mut Tlb;

        let ctx = prepare_aarch64_jit_dispatch_context(
            &mut flat_regs,
            Aarch64JitMemoryMode::Fs {
                sys_mem: sys_mem_ptr,
                tlb: tlb_ptr,
                mmu_cfg,
            },
        );

        let mut ctx = ctx;
        let expected_ptr = match &ctx {
            Aarch64JitDispatchContext::Fs { fs_ctx } => {
                assert_eq!(fs_ctx.sys_mem, sys_mem_ptr);
                assert_eq!(fs_ctx.tlb, tlb_ptr);
                assert_eq!(fs_ctx.mmu_cfg.current_el, mmu_cfg.current_el);
                (fs_ctx as *const JitFsContext).cast_mut().cast()
            }
            Aarch64JitDispatchContext::Se { .. } => panic!("expected fs context"),
        };
        assert_eq!(ctx.mem_ptr(), expected_ptr);
        assert_eq!(
            flat_regs[REG_JIT_MEM_READ],
            helpers::jit_fs_mem_read as *const () as u64
        );
        assert_eq!(
            flat_regs[REG_JIT_MEM_WRITE],
            helpers::jit_fs_mem_write as *const () as u64
        );
        assert_eq!(flat_regs[REG_JIT_SE_TLB], 0);
    }

    #[cfg(feature = "backend-dynasm")]
    #[test]
    fn ensure_aarch64_runtime_state_initializes_dynasm_mode() {
        let mut cache = None;
        let mut backend: Option<Box<dyn JitBackend>> = None;
        let mut hot_backend: Option<Box<dyn JitBackend>> = None;
        let mut trace_cache = None;
        let mut trace_recorder = None;

        let mode = ensure_aarch64_jit_runtime_state(
            &mut cache,
            &mut backend,
            &mut hot_backend,
            &mut trace_cache,
            &mut trace_recorder,
            Aarch64JitBackendPolicy::DynasmOnly,
        );

        assert_eq!(mode, Aarch64JitBackendMode::DynasmOnly);
        assert!(cache.is_some());
        assert_eq!(backend.as_ref().map(|b| b.name()), Some("dynasm"));
        assert!(hot_backend.is_none());
        assert!(trace_cache.is_some());
        assert!(trace_recorder.is_some());
    }

    #[cfg(all(feature = "backend-stencil", feature = "backend-dynasm"))]
    #[test]
    fn ensure_aarch64_runtime_state_initializes_tiered_mode() {
        let mut cache = None;
        let mut backend: Option<Box<dyn JitBackend>> = None;
        let mut hot_backend: Option<Box<dyn JitBackend>> = None;
        let mut trace_cache = None;
        let mut trace_recorder = None;

        let mode = ensure_aarch64_jit_runtime_state(
            &mut cache,
            &mut backend,
            &mut hot_backend,
            &mut trace_cache,
            &mut trace_recorder,
            Aarch64JitBackendPolicy::Tiered,
        );

        assert_eq!(mode, Aarch64JitBackendMode::Tiered);
        assert_eq!(backend.as_ref().map(|b| b.name()), Some("stencil"));
        assert_eq!(hot_backend.as_ref().map(|b| b.name()), Some("dynasm"));
        assert!(cache.is_some());
        assert!(trace_cache.is_some());
        assert!(trace_recorder.is_some());
    }

    #[cfg(feature = "backend-stencil")]
    #[test]
    fn ensure_aarch64_runtime_state_initializes_stencil_mode() {
        let mut cache = None;
        let mut backend: Option<Box<dyn JitBackend>> = None;
        let mut hot_backend: Option<Box<dyn JitBackend>> = None;
        let mut trace_cache = None;
        let mut trace_recorder = None;

        let mode = ensure_aarch64_jit_runtime_state(
            &mut cache,
            &mut backend,
            &mut hot_backend,
            &mut trace_cache,
            &mut trace_recorder,
            Aarch64JitBackendPolicy::StencilOnly,
        );

        assert_eq!(mode, Aarch64JitBackendMode::StencilOnly);
        assert_eq!(backend.as_ref().map(|b| b.name()), Some("stencil"));
        assert!(hot_backend.is_none());
        assert!(cache.is_some());
    }

    #[cfg(feature = "backend-stencil")]
    #[test]
    fn ensure_rv64_runtime_state_initializes_cache_and_backend() {
        let mut cache = None;
        let mut backend = None;

        assert!(ensure_rv64_jit_runtime_state(&mut cache, &mut backend));
        assert!(cache.is_some());
        assert!(backend.is_some());
    }

    fn make_add(pc: u64) -> Instruction {
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::AddImm;
        insn.pc = pc;
        insn
    }

    fn make_bcond(pc: u64, target_pc: u64) -> Instruction {
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::BCond;
        insn.pc = pc;
        insn.imm = target_pc.wrapping_sub(pc) as i64;
        insn
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Stop {
        Quantum,
        Exit,
    }

    struct MockHost {
        stats: JitPerfStats,
        retired: u64,
        batch_result: Stop,
        batch_consumed: u64,
        last_batch_limit: Option<u64>,
        prepare_calls: u32,
        restore_calls: u32,
        restore_result: Result<(), Stop>,
    }

    impl JitRuntimeHost for MockHost {
        type StopReason = Stop;

        fn jit_stats_mut(&mut self) -> &mut JitPerfStats {
            &mut self.stats
        }

        fn insns_retired(&self) -> u64 {
            self.retired
        }

        fn run_interpreter_batch(&mut self, max_insns: u64) -> Self::StopReason {
            self.last_batch_limit = Some(max_insns);
            self.retired = self
                .retired
                .saturating_add(self.batch_consumed.min(max_insns));
            self.batch_result
        }

        fn is_resumable_stop(stop: &Self::StopReason) -> bool {
            matches!(stop, Stop::Quantum)
        }

        fn prepare_interpreter_fallback(&mut self, flat_regs: &mut [u64], retired_insns: u64) {
            self.prepare_calls = self.prepare_calls.saturating_add(1);
            self.retired = self.retired.saturating_add(retired_insns);
            flat_regs[0] = 0xCAFE;
        }

        fn restore_jit_state_after_interpreter(
            &mut self,
            flat_regs: &mut [u64],
        ) -> Result<(), Self::StopReason> {
            self.restore_calls = self.restore_calls.saturating_add(1);
            if self.restore_result.is_ok() {
                flat_regs[0] = 0xBEEF;
            }
            self.restore_result
        }

        fn record_interpreter_fallback(&mut self, _consumed: u64, _reason: Option<&'static str>) {}
    }

    #[test]
    fn bounded_fallback_resumes_when_quantum_and_budget_remains() {
        let mut host = MockHost {
            stats: JitPerfStats::default(),
            retired: 10,
            batch_result: Stop::Quantum,
            batch_consumed: 8,
            last_batch_limit: None,
            prepare_calls: 0,
            restore_calls: 0,
            restore_result: Ok(()),
        };

        let result = run_bounded_interpreter_fallback(
            &mut host,
            32,
            JitRuntimeConfig {
                interp_fallback_batch_insns: 16,
                ..Default::default()
            },
        );

        assert_eq!(
            result,
            InterpreterFallback::Resume {
                consumed: 8,
                budget_remaining: 24,
            }
        );
    }

    #[test]
    fn bounded_fallback_stops_on_non_resumable_reason() {
        let mut host = MockHost {
            stats: JitPerfStats::default(),
            retired: 4,
            batch_result: Stop::Exit,
            batch_consumed: 6,
            last_batch_limit: None,
            prepare_calls: 0,
            restore_calls: 0,
            restore_result: Ok(()),
        };

        let result = run_bounded_interpreter_fallback(
            &mut host,
            32,
            JitRuntimeConfig {
                interp_fallback_batch_insns: 16,
                ..Default::default()
            },
        );

        assert_eq!(
            result,
            InterpreterFallback::Stop {
                stop: Stop::Exit,
                consumed: 6,
                budget_remaining: 26,
            }
        );
    }

    #[test]
    fn cache_probe_tracks_hits_and_misses() {
        let mut cache = JitCache::new();
        let mut stats = JitPerfStats::default();
        cache.insert(make_test_block(0x1000, 3));

        match probe_block_cache(&mut cache, &mut stats, 0x1000) {
            BlockCacheProbe::Hit(hit) => {
                assert_eq!(hit.block.guest_pc, 0x1000);
                assert_eq!(hit.exec_count, 1);
            }
            BlockCacheProbe::Miss => panic!("expected cache hit"),
        }
        assert_eq!(stats.block_cache_hits.get(), 1);
        assert_eq!(stats.block_cache_misses.get(), 0);

        match probe_block_cache(&mut cache, &mut stats, 0x2000) {
            BlockCacheProbe::Hit(_) => panic!("expected cache miss"),
            BlockCacheProbe::Miss => {}
        }
        assert_eq!(stats.block_cache_hits.get(), 1);
        assert_eq!(stats.block_cache_misses.get(), 1);
    }

    #[test]
    fn trace_dispatch_tracks_hits_misses_and_disabled_state() {
        let mut stats = JitPerfStats::default();
        let mut flat_regs = [0u64; 8];
        let mut retired = 0;
        let mut budget_remaining = 12;

        assert_eq!(
            unsafe {
                dispatch_trace(
                    None,
                    &mut stats,
                    0x1000,
                    &mut flat_regs,
                    std::ptr::null_mut(),
                    &mut retired,
                    &mut budget_remaining,
                    DEFAULT_RUNTIME_CONFIG,
                )
            },
            TraceDispatch::NotAvailable
        );
        assert_eq!(stats.trace_cache_hits.get(), 0);
        assert_eq!(stats.trace_cache_misses.get(), 0);

        let mut cache = TraceCache::new();
        assert_eq!(
            unsafe {
                dispatch_trace(
                    Some(&mut cache),
                    &mut stats,
                    0x1000,
                    &mut flat_regs,
                    std::ptr::null_mut(),
                    &mut retired,
                    &mut budget_remaining,
                    DEFAULT_RUNTIME_CONFIG,
                )
            },
            TraceDispatch::Miss
        );
        assert_eq!(stats.trace_cache_hits.get(), 0);
        assert_eq!(stats.trace_cache_misses.get(), 1);

        cache.insert(make_test_trace(0x1000, 3));
        assert_eq!(
            unsafe {
                dispatch_trace(
                    Some(&mut cache),
                    &mut stats,
                    0x1000,
                    &mut flat_regs,
                    std::ptr::null_mut(),
                    &mut retired,
                    &mut budget_remaining,
                    DEFAULT_RUNTIME_CONFIG,
                )
            },
            TraceDispatch::SkippedDisabled
        );
        assert_eq!(stats.trace_cache_hits.get(), 1);
        assert_eq!(stats.trace_cache_misses.get(), 1);
        assert_eq!(stats.traces_executed.get(), 0);
        assert_eq!(retired, 0);
        assert_eq!(budget_remaining, 12);
    }

    #[test]
    fn trace_dispatch_executes_enabled_trace_and_updates_budget() {
        let mut stats = JitPerfStats::default();
        let mut flat_regs = [0u64; 8];
        let mut retired = 1;
        let mut budget_remaining = 10;
        let mut cache = TraceCache::new();
        cache.insert(make_test_trace(0x1000, 3));

        assert_eq!(
            unsafe {
                dispatch_trace(
                    Some(&mut cache),
                    &mut stats,
                    0x1000,
                    &mut flat_regs,
                    std::ptr::null_mut(),
                    &mut retired,
                    &mut budget_remaining,
                    JitRuntimeConfig {
                        trace_dispatch_enabled: true,
                        ..Default::default()
                    },
                )
            },
            TraceDispatch::Executed {
                exit_code: EXIT_END_OF_BLOCK,
                guard: None,
            }
        );
        assert_eq!(stats.trace_cache_hits.get(), 1);
        assert_eq!(stats.traces_executed.get(), 1);
        assert_eq!(retired, 4);
        assert_eq!(budget_remaining, 7);
    }

    #[test]
    fn trace_dispatch_retires_trace_once_on_guard_threshold() {
        let mut stats = JitPerfStats::default();
        let mut flat_regs = [0u64; 64];
        let mut retired = 0;
        let mut budget_remaining = 8;
        let mut cache = TraceCache::new();
        cache.insert(make_test_guard_trace(
            0x1000,
            2,
            crate::trace::GUARD_MISS_THRESHOLD - 1,
        ));

        assert_eq!(
            unsafe {
                dispatch_trace(
                    Some(&mut cache),
                    &mut stats,
                    0x1000,
                    &mut flat_regs,
                    std::ptr::null_mut(),
                    &mut retired,
                    &mut budget_remaining,
                    JitRuntimeConfig {
                        trace_dispatch_enabled: true,
                        ..Default::default()
                    },
                )
            },
            TraceDispatch::Executed {
                exit_code: EXIT_GUARD_BASE,
                guard: Some(TraceGuardInfo {
                    guard_id: 0,
                    resume_pc: 0x1020,
                    miss_count: crate::trace::GUARD_MISS_THRESHOLD,
                    retiring: true,
                    retired_guest_insns: 2,
                }),
            }
        );
        assert_eq!(stats.traces_executed.get(), 1);
        assert_eq!(stats.trace_guard_exits.get(), 1);
        assert_eq!(stats.trace_retired.get(), 1);
        assert!(cache.lookup(0x1000).is_none());
        assert_eq!(flat_regs[REG_PC], 0x1020);
        assert_eq!(retired, 2);
        assert_eq!(budget_remaining, 6);
    }

    #[test]
    fn plan_trace_recording_ignores_cold_or_forward_edges() {
        let mut recorder = TraceRecorder::default();

        assert_eq!(
            plan_aarch64_trace_recording(&mut recorder, 0x1000, 0x1004),
            TraceRecordPlan::Ignore
        );
        assert_eq!(
            plan_aarch64_trace_recording(&mut recorder, 0x1000, 0x1000),
            TraceRecordPlan::Ignore
        );
        assert!(!recorder.is_recording());
    }

    #[test]
    fn plan_trace_recording_requests_decode_on_threshold_hit_and_active_recording() {
        let mut recorder = TraceRecorder::default();
        for _ in 0..crate::trace::TRACE_THRESHOLD {
            let plan = plan_aarch64_trace_recording(&mut recorder, 0x1000, 0x1000);
            if recorder.is_recording() {
                assert_eq!(plan, TraceRecordPlan::DecodeCurrentBlock);
                break;
            }
            assert_eq!(plan, TraceRecordPlan::Ignore);
        }

        assert!(recorder.is_recording());
        assert_eq!(
            plan_aarch64_trace_recording(&mut recorder, 0x1008, 0x100c),
            TraceRecordPlan::DecodeCurrentBlock
        );
    }

    #[test]
    fn record_trace_candidate_compiles_and_caches_completed_trace() {
        let mut recorder = TraceRecorder::default();
        for _ in 0..crate::trace::TRACE_THRESHOLD {
            let _ = plan_aarch64_trace_recording(&mut recorder, 0x1000, 0x1000);
        }
        assert!(recorder.is_recording());

        let mut stats = JitPerfStats::default();
        let mut cache = TraceCache::new();
        let block0 = [make_add(0x1000), make_bcond(0x1004, 0x1008)];
        let block1 = [make_add(0x1008), make_bcond(0x100c, 0x1000)];

        assert_eq!(
            record_aarch64_trace_candidate(&mut recorder, Some(&mut cache), &mut stats, &block0,),
            TraceRecordResult::Pending
        );
        assert_eq!(stats.traces_compiled.get(), 0);
        assert!(cache.lookup(0x1000).is_none());

        assert_eq!(
            record_aarch64_trace_candidate(&mut recorder, Some(&mut cache), &mut stats, &block1,),
            TraceRecordResult::Compiled {
                start_pc: 0x1000,
                insn_count: 4,
            }
        );
        assert_eq!(stats.traces_compiled.get(), 1);
        assert_eq!(stats.trace_guest_insns.get(), 4);
        assert!(cache.lookup(0x1000).is_some());
        assert!(!recorder.is_recording());
    }

    struct MockBackend {
        result: Option<CompiledBlock>,
    }

    impl JitBackend for MockBackend {
        fn compile_block(&mut self, _pc: u64, _insns: &[Aarch64Insn]) -> Option<CompiledBlock> {
            self.result.take()
        }

        fn name(&self) -> &str {
            "mock"
        }
    }

    #[test]
    fn compile_on_miss_caches_compiled_block() {
        let mut cache = JitCache::new();
        let mut stats = JitPerfStats::default();
        let mut backend = MockBackend {
            result: Some(make_test_block(0x3000, 5)),
        };
        let insns = [Aarch64Insn::zeroed()];

        let result = compile_block_on_miss(
            &mut cache,
            &mut stats,
            &mut backend,
            JitTier::Stencil,
            0x3000,
            &insns,
        );

        assert_eq!(result, CompileOnMiss::Cached { insn_count: 5 });
        assert_eq!(stats.blocks_compiled.get(), 1);
        assert_eq!(stats.compiled_guest_insns.get(), 5);
        assert!(cache.lookup(0x3000).is_some());
    }

    #[test]
    fn compile_on_miss_reports_unsupported_start() {
        let mut cache = JitCache::new();
        let mut stats = JitPerfStats::default();
        let mut backend = MockBackend { result: None };
        let insns = [Aarch64Insn::zeroed()];

        let result = compile_block_on_miss(
            &mut cache,
            &mut stats,
            &mut backend,
            JitTier::Stencil,
            0x3000,
            &insns,
        );

        assert!(matches!(
            result,
            CompileOnMiss::UnsupportedStart { .. }
        ));
        assert_eq!(stats.blocks_compiled.get(), 0);
        assert_eq!(stats.compiled_guest_insns.get(), 0);
        assert!(cache.lookup(0x3000).is_none());
    }

    #[test]
    fn execute_compiled_block_updates_stats_and_budget() {
        let block = make_test_block(0x4000, 3);
        let mut stats = JitPerfStats::default();
        let mut flat_regs = [0u64; 8];
        let mut retired = 2;
        let mut budget_remaining = 10;

        #[allow(unsafe_code)]
        let exit_code = unsafe {
            execute_compiled_block(
                &mut stats,
                &block,
                &mut flat_regs,
                std::ptr::null_mut(),
                &mut retired,
                &mut budget_remaining,
            )
        };

        assert_eq!(exit_code, EXIT_END_OF_BLOCK);
        assert_eq!(stats.blocks_executed.get(), 1);
        assert_eq!(retired, 5);
        assert_eq!(budget_remaining, 7);
    }

    #[test]
    fn unsupported_start_fallback_updates_stats_and_restores_state_on_resume() {
        let mut host = MockHost {
            stats: JitPerfStats::default(),
            retired: 10,
            batch_result: Stop::Quantum,
            batch_consumed: 5,
            last_batch_limit: None,
            prepare_calls: 0,
            restore_calls: 0,
            restore_result: Ok(()),
        };
        let mut flat_regs = [0u64; 4];
        let mut retired = 7;

        let result = handle_unsupported_start_fallback(
            &mut host,
            &mut flat_regs,
            &mut retired,
            32,
            Some("Adrp".to_string()),
            None,
            JitRuntimeConfig {
                interp_fallback_batch_insns: 16,
                ..Default::default()
            },
        );

        assert_eq!(
            result,
            InterpreterFallback::Resume {
                consumed: 5,
                budget_remaining: 27,
            }
        );
        assert_eq!(retired, 0);
        assert_eq!(flat_regs[0], 0xBEEF);
        assert_eq!(host.retired, 22);
        assert_eq!(host.prepare_calls, 1);
        assert_eq!(host.restore_calls, 1);
        assert_eq!(host.stats.fallback_count.get(), 1);
        assert_eq!(host.stats.fallback_insns.get(), 5);
        assert_eq!(host.stats.unsupported_block_starts.get(), 1);
        assert_eq!(host.stats.unsupported_opcodes.value("Adrp"), Some(1));
    }

    #[test]
    fn unsupported_start_fallback_stops_when_restore_fails() {
        let mut host = MockHost {
            stats: JitPerfStats::default(),
            retired: 2,
            batch_result: Stop::Quantum,
            batch_consumed: 4,
            last_batch_limit: None,
            prepare_calls: 0,
            restore_calls: 0,
            restore_result: Err(Stop::Exit),
        };
        let mut flat_regs = [0u64; 4];
        let mut retired = 3;

        let result = handle_unsupported_start_fallback(
            &mut host,
            &mut flat_regs,
            &mut retired,
            20,
            None,
            None,
            JitRuntimeConfig {
                interp_fallback_batch_insns: 8,
                ..Default::default()
            },
        );

        assert_eq!(
            result,
            InterpreterFallback::Stop {
                stop: Stop::Exit,
                consumed: 4,
                budget_remaining: 16,
            }
        );
        assert_eq!(retired, 0);
        assert_eq!(flat_regs[0], 0xCAFE);
        assert_eq!(host.prepare_calls, 1);
        assert_eq!(host.restore_calls, 1);
        assert_eq!(host.stats.fallback_count.get(), 1);
        assert_eq!(host.stats.fallback_insns.get(), 4);
        assert_eq!(host.stats.unsupported_block_starts.get(), 1);
        assert!(host.stats.unsupported_opcodes.cardinality() == 0);
    }

    #[test]
    fn handoff_to_interpreter_commits_retired_and_runs_remaining_budget() {
        let mut host = MockHost {
            stats: JitPerfStats::default(),
            retired: 5,
            batch_result: Stop::Exit,
            batch_consumed: 11,
            last_batch_limit: None,
            prepare_calls: 0,
            restore_calls: 0,
            restore_result: Ok(()),
        };
        let mut flat_regs = [0u64; 4];
        let mut retired = 7;

        let stop = handoff_to_interpreter(&mut host, &mut flat_regs, &mut retired, 13);

        assert_eq!(stop, Stop::Exit);
        assert_eq!(retired, 0);
        assert_eq!(flat_regs[0], 0xCAFE);
        assert_eq!(host.retired, 23);
        assert_eq!(host.last_batch_limit, Some(13));
        assert_eq!(host.prepare_calls, 1);
        assert_eq!(host.restore_calls, 0);
    }

    #[test]
    fn resolve_compile_miss_hands_off_when_decode_is_empty() {
        let mut host = MockHost {
            stats: JitPerfStats::default(),
            retired: 9,
            batch_result: Stop::Exit,
            batch_consumed: 6,
            last_batch_limit: None,
            prepare_calls: 0,
            restore_calls: 0,
            restore_result: Ok(()),
        };
        let mut cache = JitCache::new();
        let mut flat_regs = [0u64; 4];
        let mut retired = 5;

        let result = resolve_aarch64_compile_miss::<_, MockBackend>(
            &mut host,
            &mut cache,
            None,
            None,
            0x1000,
            &[],
            &mut flat_regs,
            &mut retired,
            17,
            None,
            JitRuntimeConfig {
                interp_fallback_batch_insns: 8,
                ..Default::default()
            },
        );

        assert_eq!(result, CompileMissResolution::Stop { stop: Stop::Exit });
        assert_eq!(retired, 0);
        assert_eq!(flat_regs[0], 0xCAFE);
        assert_eq!(host.prepare_calls, 1);
        assert_eq!(host.last_batch_limit, Some(17));
    }

    #[test]
    fn resolve_compile_miss_compiles_supported_block() {
        let mut host = MockHost {
            stats: JitPerfStats::default(),
            retired: 0,
            batch_result: Stop::Quantum,
            batch_consumed: 0,
            last_batch_limit: None,
            prepare_calls: 0,
            restore_calls: 0,
            restore_result: Ok(()),
        };
        let mut cache = JitCache::new();
        let mut backend = MockBackend {
            result: Some(make_test_block(0x3000, 5)),
        };
        let mut flat_regs = [0u64; 4];
        let mut retired = 0;
        let insns = [Aarch64Insn::zeroed()];

        let result = resolve_aarch64_compile_miss(
            &mut host,
            &mut cache,
            Some(&mut backend),
            None,
            0x3000,
            &insns,
            &mut flat_regs,
            &mut retired,
            20,
            None,
            JitRuntimeConfig {
                interp_fallback_batch_insns: 8,
                ..Default::default()
            },
        );

        assert_eq!(result, CompileMissResolution::Cached { insn_count: 5 });
        assert_eq!(host.stats.blocks_compiled.get(), 1);
        assert!(cache.lookup(0x3000).is_some());
        assert_eq!(host.prepare_calls, 0);
    }

    #[test]
    fn resolve_compile_miss_resumes_after_unsupported_start() {
        let mut host = MockHost {
            stats: JitPerfStats::default(),
            retired: 3,
            batch_result: Stop::Quantum,
            batch_consumed: 4,
            last_batch_limit: None,
            prepare_calls: 0,
            restore_calls: 0,
            restore_result: Ok(()),
        };
        let mut cache = JitCache::new();
        let mut backend = MockBackend { result: None };
        let mut flat_regs = [0u64; 4];
        let mut retired = 2;
        let insns = [Aarch64Insn::zeroed()];

        let result = resolve_aarch64_compile_miss(
            &mut host,
            &mut cache,
            Some(&mut backend),
            None,
            0x4000,
            &insns,
            &mut flat_regs,
            &mut retired,
            19,
            Some("Adrp".to_string()),
            JitRuntimeConfig {
                interp_fallback_batch_insns: 8,
                ..Default::default()
            },
        );

        assert_eq!(
            result,
            CompileMissResolution::Resume {
                budget_remaining: 15,
            }
        );
        assert_eq!(retired, 0);
        assert_eq!(flat_regs[0], 0xBEEF);
        assert_eq!(host.prepare_calls, 1);
        assert_eq!(host.restore_calls, 1);
        assert_eq!(host.stats.fallback_count.get(), 1);
        assert_eq!(host.stats.unsupported_opcodes.value("Adrp"), Some(1));
    }

    #[test]
    fn resolve_compile_miss_uses_fallback_backend_before_interpreter() {
        let mut host = MockHost {
            stats: JitPerfStats::default(),
            retired: 0,
            batch_result: Stop::Exit,
            batch_consumed: 0,
            last_batch_limit: None,
            prepare_calls: 0,
            restore_calls: 0,
            restore_result: Ok(()),
        };
        let mut cache = JitCache::new();
        let mut primary = MockBackend { result: None };
        let mut fallback = MockBackend {
            result: Some(make_test_block(0x6000, 4)),
        };
        let mut flat_regs = [0u64; 4];
        let mut retired = 0;
        let insns = [Aarch64Insn::zeroed()];

        let result = resolve_aarch64_compile_miss(
            &mut host,
            &mut cache,
            Some(&mut primary),
            Some(&mut fallback),
            0x6000,
            &insns,
            &mut flat_regs,
            &mut retired,
            32,
            Some("Ccmp".to_string()),
            JitRuntimeConfig::default(),
        );

        assert_eq!(result, CompileMissResolution::Cached { insn_count: 4 });
        assert_eq!(host.prepare_calls, 0);
        assert!(cache.lookup(0x6000).is_some());
    }

    #[test]
    fn maybe_promote_and_execute_replaces_stencil_block_and_runs_promoted_code() {
        let mut cache = JitCache::new();
        cache.insert(make_test_block(0x5000, 2));

        let mut stats = JitPerfStats::default();
        let mut backend = MockBackend {
            result: Some(make_test_block(0x5000, 5)),
        };
        let mut flat_regs = [0u64; 8];
        let mut retired = 3;
        let mut budget_remaining = 20;
        let insns = [Aarch64Insn::zeroed()];

        #[allow(unsafe_code)]
        let result = unsafe {
            maybe_promote_and_execute(
                &mut cache,
                &mut stats,
                Some(&mut backend),
                0x5000,
                PROMOTE_THRESHOLD,
                JitTier::Stencil,
                &insns,
                &mut flat_regs,
                std::ptr::null_mut(),
                &mut retired,
                &mut budget_remaining,
            )
        };

        assert_eq!(
            result,
            PromotionResolution::Executed {
                exit_code: EXIT_END_OF_BLOCK,
            }
        );
        assert_eq!(stats.block_cache_hits.get(), 1);
        assert_eq!(stats.blocks_executed.get(), 1);
        assert_eq!(retired, 8);
        assert_eq!(budget_remaining, 15);
        assert_eq!(cache.promotions(), 1);
    }

    #[test]
    fn maybe_promote_and_execute_skips_when_not_hot_enough() {
        let mut cache = JitCache::new();
        let mut stats = JitPerfStats::default();
        let mut backend = MockBackend {
            result: Some(make_test_block(0x5000, 5)),
        };
        let mut flat_regs = [0u64; 8];
        let mut retired = 0;
        let mut budget_remaining = 10;
        let insns = [Aarch64Insn::zeroed()];

        #[allow(unsafe_code)]
        let result = unsafe {
            maybe_promote_and_execute(
                &mut cache,
                &mut stats,
                Some(&mut backend),
                0x5000,
                PROMOTE_THRESHOLD - 1,
                JitTier::Stencil,
                &insns,
                &mut flat_regs,
                std::ptr::null_mut(),
                &mut retired,
                &mut budget_remaining,
            )
        };

        assert_eq!(result, PromotionResolution::NotPromoted);
        assert_eq!(stats.block_cache_hits.get(), 0);
        assert_eq!(stats.blocks_executed.get(), 0);
        assert_eq!(retired, 0);
        assert_eq!(budget_remaining, 10);
        assert_eq!(cache.promotions(), 0);
    }

    #[test]
    fn execute_cache_hit_runs_original_block_when_no_promotion_insns() {
        let mut cache = JitCache::new();
        cache.insert(make_test_block(0x6000, 3));
        let mut stats = JitPerfStats::default();
        let hit = match probe_block_cache(&mut cache, &mut stats, 0x6000) {
            BlockCacheProbe::Hit(hit) => hit,
            BlockCacheProbe::Miss => panic!("expected cache hit"),
        };
        let mut flat_regs = [0u64; 8];
        let mut retired = 1;
        let mut budget_remaining = 12;

        #[allow(unsafe_code)]
        let exit_code = unsafe {
            execute_cache_hit::<MockBackend>(
                &mut cache,
                &mut stats,
                hit,
                None,
                0x6000,
                None,
                &mut flat_regs,
                std::ptr::null_mut(),
                &mut retired,
                &mut budget_remaining,
            )
        };

        assert_eq!(exit_code, EXIT_END_OF_BLOCK);
        assert_eq!(stats.block_cache_hits.get(), 1);
        assert_eq!(stats.blocks_executed.get(), 1);
        assert_eq!(retired, 4);
        assert_eq!(budget_remaining, 9);
        assert_eq!(cache.promotions(), 0);
    }

    #[test]
    fn execute_cache_hit_uses_promoted_block_when_available() {
        let mut cache = JitCache::new();
        cache.insert(make_test_block(0x7000, 2));
        for _ in 0..(PROMOTE_THRESHOLD - 1) {
            let _ = cache.lookup_hot(0x7000);
        }
        let mut stats = JitPerfStats::default();
        let hit = match probe_block_cache(&mut cache, &mut stats, 0x7000) {
            BlockCacheProbe::Hit(hit) => hit,
            BlockCacheProbe::Miss => panic!("expected cache hit"),
        };
        let mut backend = MockBackend {
            result: Some(make_test_block(0x7000, 5)),
        };
        let mut flat_regs = [0u64; 8];
        let mut retired = 0;
        let mut budget_remaining = 20;
        let insns = [Aarch64Insn::zeroed()];

        #[allow(unsafe_code)]
        let exit_code = unsafe {
            execute_cache_hit(
                &mut cache,
                &mut stats,
                hit,
                Some(&mut backend),
                0x7000,
                Some(&insns),
                &mut flat_regs,
                std::ptr::null_mut(),
                &mut retired,
                &mut budget_remaining,
            )
        };

        assert_eq!(exit_code, EXIT_END_OF_BLOCK);
        assert_eq!(stats.block_cache_hits.get(), 2);
        assert_eq!(stats.blocks_executed.get(), 1);
        assert_eq!(retired, 5);
        assert_eq!(budget_remaining, 15);
        assert_eq!(cache.promotions(), 1);
    }
}

//! Runtime-facing JIT boundary types.
//!
//! This module is the start of the crate boundary move from `helm-engine`
//! into `helm-jit`: shared runtime policy/config should live here, while the
//! engine implements the host side.

use helm_arch::Aarch64Insn;
use helm_stats::JitPerfStats;

use crate::backend::JitBackend;
use crate::block::CompiledBlock;
use crate::cache::{CacheLookup, JitCache};

/// JIT runtime policy knobs shared between host and executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JitRuntimeConfig {
    /// Maximum interpreter instructions to run after a JIT fallback before
    /// re-entering the JIT loop.
    pub interp_fallback_batch_insns: u64,
}

impl Default for JitRuntimeConfig {
    fn default() -> Self {
        Self {
            interp_fallback_batch_insns: 256,
        }
    }
}

/// Default runtime policy for the current JIT loop.
pub const DEFAULT_RUNTIME_CONFIG: JitRuntimeConfig = JitRuntimeConfig {
    interp_fallback_batch_insns: 256,
};

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
    UnsupportedStart,
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
}

/// Probe the JIT block cache and update shared hit/miss counters.
pub fn probe_block_cache(
    cache: &mut JitCache,
    stats: &mut JitPerfStats,
    pc: u64,
) -> BlockCacheProbe {
    match cache.lookup_hot(pc) {
        Some(hit) => {
            stats.block_cache_hits = stats.block_cache_hits.saturating_add(1);
            BlockCacheProbe::Hit(hit)
        }
        None => {
            stats.block_cache_misses = stats.block_cache_misses.saturating_add(1);
            BlockCacheProbe::Miss
        }
    }
}

/// Compile a decoded AArch64 block after a cache miss and insert it into the cache.
pub fn compile_block_on_miss<B: JitBackend + ?Sized>(
    cache: &mut JitCache,
    stats: &mut JitPerfStats,
    backend: &mut B,
    pc: u64,
    insns: &[Aarch64Insn],
) -> CompileOnMiss {
    match backend.compile_block(pc, insns) {
        Some(block) => {
            let insn_count = block.insn_count;
            stats.blocks_compiled = stats.blocks_compiled.saturating_add(1);
            cache.insert(block);
            cache.link_waiters(pc);
            CompileOnMiss::Cached { insn_count }
        }
        None => CompileOnMiss::UnsupportedStart,
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
    stats.blocks_executed = stats.blocks_executed.saturating_add(1);
    let exit_code = (block.entry)(flat_regs.as_mut_ptr(), mem_ptr);
    let block_insns = u64::from(block.insn_count);
    *retired = retired.saturating_add(block_insns);
    *budget_remaining = budget_remaining.saturating_sub(block_insns);
    exit_code
}

/// Handle fallback when block compilation fails at the first instruction.
pub fn handle_unsupported_start_fallback<H: JitRuntimeHost>(
    host: &mut H,
    flat_regs: &mut [u64],
    retired: &mut u64,
    budget_remaining: u64,
    unsupported_opcode: Option<&'static str>,
    config: JitRuntimeConfig,
) -> InterpreterFallback<H::StopReason> {
    host.prepare_interpreter_fallback(flat_regs, *retired);
    *retired = 0;

    {
        let stats = host.jit_stats_mut();
        stats.fallback_count = stats.fallback_count.saturating_add(1);
        stats.unsupported_block_starts = stats.unsupported_block_starts.saturating_add(1);
        if let Some(opcode) = unsupported_opcode {
            *stats.unsupported_opcodes.entry(opcode).or_insert(0) += 1;
        }
    }

    match run_bounded_interpreter_fallback(host, budget_remaining, config) {
        InterpreterFallback::Resume {
            consumed,
            budget_remaining,
        } => {
            let stats = host.jit_stats_mut();
            stats.fallback_insns = stats.fallback_insns.saturating_add(consumed);
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
            stats.fallback_insns = stats.fallback_insns.saturating_add(consumed);
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

    #[allow(unsafe_code)]
    unsafe extern "C" fn test_block(_regs: *mut u64, _mem: *mut u8) -> u64 {
        EXIT_END_OF_BLOCK
    }

    fn make_test_block(pc: u64, insn_count: u32) -> CompiledBlock {
        #[allow(unsafe_code)]
        unsafe {
            let entry: JitBlockFn = test_block;
            CompiledBlock::new((), entry, pc, insn_count)
        }
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
        assert_eq!(stats.block_cache_hits, 1);
        assert_eq!(stats.block_cache_misses, 0);

        match probe_block_cache(&mut cache, &mut stats, 0x2000) {
            BlockCacheProbe::Hit(_) => panic!("expected cache miss"),
            BlockCacheProbe::Miss => {}
        }
        assert_eq!(stats.block_cache_hits, 1);
        assert_eq!(stats.block_cache_misses, 1);
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

        let result = compile_block_on_miss(&mut cache, &mut stats, &mut backend, 0x3000, &insns);

        assert_eq!(result, CompileOnMiss::Cached { insn_count: 5 });
        assert_eq!(stats.blocks_compiled, 1);
        assert!(cache.lookup(0x3000).is_some());
    }

    #[test]
    fn compile_on_miss_reports_unsupported_start() {
        let mut cache = JitCache::new();
        let mut stats = JitPerfStats::default();
        let mut backend = MockBackend { result: None };
        let insns = [Aarch64Insn::zeroed()];

        let result = compile_block_on_miss(&mut cache, &mut stats, &mut backend, 0x3000, &insns);

        assert_eq!(result, CompileOnMiss::UnsupportedStart);
        assert_eq!(stats.blocks_compiled, 0);
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
        assert_eq!(stats.blocks_executed, 1);
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
            Some("adrp"),
            JitRuntimeConfig {
                interp_fallback_batch_insns: 16,
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
        assert_eq!(host.stats.fallback_count, 1);
        assert_eq!(host.stats.fallback_insns, 5);
        assert_eq!(host.stats.unsupported_block_starts, 1);
        assert_eq!(host.stats.unsupported_opcodes.get("adrp"), Some(&1));
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
            JitRuntimeConfig {
                interp_fallback_batch_insns: 8,
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
        assert_eq!(host.stats.fallback_count, 1);
        assert_eq!(host.stats.fallback_insns, 4);
        assert_eq!(host.stats.unsupported_block_starts, 1);
        assert!(host.stats.unsupported_opcodes.is_empty());
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
}

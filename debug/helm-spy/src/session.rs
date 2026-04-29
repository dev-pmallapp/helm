use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::analysis::{BranchDirectionStats, BranchPredictor, CacheModel, InsnMix};
use crate::filter::{AddrRangeFilter, PcRangeFilter};
use crate::primitives::{Counter, HeatMap, RingBuffer};
use crate::snapshot::{
    AddrRangeFilterSnapshot, BranchDirectionSnapshot, BranchPredSnapshot, CacheSnapshot,
    HelmSpySnapshot, JitActivitySnapshot, MmuActivitySnapshot, PcRangeFilterSnapshot,
};
use crate::trigger::Trigger;
#[cfg(feature = "instrumentation")]
use crate::window::Window;

/// The user-facing aggregator that owns all configured analysis primitives.
/// Python and CLI interact with this directly.
///
/// Fields are `Arc`-wrapped so probe subscription closures can capture them
/// with `'static` lifetime.
pub struct HelmSpy {
    pub insn_count: Arc<Counter>,
    pub insn_mix: Arc<InsnMix>,
    pub branch_direction: Arc<BranchDirectionStats>,
    pub hot_pcs: Arc<HeatMap>,
    pub branch_heatmap: Arc<HeatMap>,
    pub cache_l1d: Option<Arc<CacheModel>>,
    pub branch_pred: Option<Arc<Mutex<BranchPredictor>>>,
    pub jit_block_compile_events: Arc<Counter>,
    pub jit_block_compile_guest_insns: Arc<Counter>,
    pub jit_block_execute_events: Arc<Counter>,
    pub jit_block_retired_insns: Arc<Counter>,
    pub jit_trace_compile_events: Arc<Counter>,
    pub jit_trace_compile_guest_insns: Arc<Counter>,
    pub jit_trace_execute_events: Arc<Counter>,
    pub jit_trace_execute_insns: Arc<Counter>,
    pub jit_fallback_events: Arc<Counter>,
    pub jit_fallback_insns: Arc<Counter>,
    pub jit_cache_hit_events: Arc<Counter>,
    pub jit_cache_miss_events: Arc<Counter>,
    pub jit_cache_promote_events: Arc<Counter>,
    pub jit_guard_exit_events: Arc<Counter>,
    pub jit_guard_retire_events: Arc<Counter>,
    pub mmu_tlb_hits: Arc<Counter>,
    pub mmu_tlb_misses: Arc<Counter>,
    pub mmu_stage1_walks: Arc<Counter>,
    pub mmu_stage2_walks: Arc<Counter>,
    pub fault_history: Arc<RingBuffer<String>>,
    pub scoreboard_pc_filter: Option<Arc<PcRangeFilter>>,
    pub scoreboard_addr_filter: Option<Arc<AddrRangeFilter>>,
    pub triggers: Vec<Arc<Trigger>>,
}

impl HelmSpy {
    pub fn new() -> Self {
        Self {
            insn_count: Arc::new(Counter::new("insn_count")),
            insn_mix: Arc::new(InsnMix::new()),
            branch_direction: Arc::new(BranchDirectionStats::new()),
            hot_pcs: Arc::new(HeatMap::new("hot_pcs")),
            branch_heatmap: Arc::new(HeatMap::new("branch_heatmap")),
            cache_l1d: None,
            branch_pred: None,
            jit_block_compile_events: Arc::new(Counter::new("jit_block_compile_events")),
            jit_block_compile_guest_insns: Arc::new(Counter::new("jit_block_compile_guest_insns")),
            jit_block_execute_events: Arc::new(Counter::new("jit_block_execute_events")),
            jit_block_retired_insns: Arc::new(Counter::new("jit_block_retired_insns")),
            jit_trace_compile_events: Arc::new(Counter::new("jit_trace_compile_events")),
            jit_trace_compile_guest_insns: Arc::new(Counter::new("jit_trace_compile_guest_insns")),
            jit_trace_execute_events: Arc::new(Counter::new("jit_trace_execute_events")),
            jit_trace_execute_insns: Arc::new(Counter::new("jit_trace_execute_insns")),
            jit_fallback_events: Arc::new(Counter::new("jit_fallback_events")),
            jit_fallback_insns: Arc::new(Counter::new("jit_fallback_insns")),
            jit_cache_hit_events: Arc::new(Counter::new("jit_cache_hit_events")),
            jit_cache_miss_events: Arc::new(Counter::new("jit_cache_miss_events")),
            jit_cache_promote_events: Arc::new(Counter::new("jit_cache_promote_events")),
            jit_guard_exit_events: Arc::new(Counter::new("jit_guard_exit_events")),
            jit_guard_retire_events: Arc::new(Counter::new("jit_guard_retire_events")),
            mmu_tlb_hits: Arc::new(Counter::new("mmu_tlb_hits")),
            mmu_tlb_misses: Arc::new(Counter::new("mmu_tlb_misses")),
            mmu_stage1_walks: Arc::new(Counter::new("mmu_stage1_walks")),
            mmu_stage2_walks: Arc::new(Counter::new("mmu_stage2_walks")),
            fault_history: Arc::new(RingBuffer::new(128)),
            scoreboard_pc_filter: None,
            scoreboard_addr_filter: None,
            triggers: Vec::new(),
        }
    }

    /// Configure an L1D cache model for the session.
    pub fn with_cache_l1d(mut self, size_bytes: usize, ways: usize, line_size: usize) -> Self {
        self.cache_l1d = Some(Arc::new(CacheModel::new(
            "L1D", size_bytes, ways, line_size,
        )));
        self
    }

    /// Configure a branch predictor for the session.
    pub fn with_branch_predictor(mut self, pred: BranchPredictor) -> Self {
        self.branch_pred = Some(Arc::new(Mutex::new(pred)));
        self
    }

    /// Configure a shared PC-range filter for scoreboard counters.
    pub fn with_scoreboard_pc_filter(mut self, filter: PcRangeFilter) -> Self {
        self.scoreboard_pc_filter = Some(Arc::new(filter));
        self
    }

    /// Configure a shared address-range filter for memory-side scoreboards.
    pub fn with_scoreboard_addr_filter(mut self, filter: AddrRangeFilter) -> Self {
        self.scoreboard_addr_filter = Some(Arc::new(filter));
        self
    }

    /// Add a trigger to the session.
    pub fn add_trigger(&mut self, trigger: Trigger) {
        self.triggers.push(Arc::new(trigger));
    }

    /// Check all triggers against the current PC and instruction count.
    pub fn check_triggers(&self, pc: u64, insn_count: u64) {
        for t in &self.triggers {
            t.check(pc, insn_count);
        }
    }

    /// Create a snapshot of the current session state for reporting.
    pub fn snapshot(&self) -> HelmSpySnapshot {
        HelmSpySnapshot {
            insn_count: self.insn_count.value(),
            insn_mix: self
                .insn_mix
                .table()
                .into_iter()
                .map(|(name, count, _fraction)| (name.to_string(), count))
                .collect(),
            branch_direction: BranchDirectionSnapshot {
                taken: self.branch_direction.taken(),
                not_taken: self.branch_direction.not_taken(),
            },
            mmu_activity: MmuActivitySnapshot {
                tlb_hits: self.mmu_tlb_hits.value(),
                tlb_misses: self.mmu_tlb_misses.value(),
                stage1_walks: self.mmu_stage1_walks.value(),
                stage2_walks: self.mmu_stage2_walks.value(),
            },
            hot_pcs: self.hot_pcs.top(20),
            branch_heatmap: self.branch_heatmap.top(20),
            cache_l1d: self.cache_l1d.as_ref().map(|cache| CacheSnapshot {
                name: cache.name().to_string(),
                hits: cache.hits(),
                misses: cache.misses(),
                hit_rate: cache.hit_rate(),
            }),
            branch_pred: self.branch_pred.as_ref().and_then(|pred| {
                pred.lock().ok().map(|guard| BranchPredSnapshot {
                    name: guard.name().to_string(),
                    kind: guard.kind_name().to_string(),
                    predictions: guard.predictions(),
                    mispredictions: guard.mispredictions(),
                    miss_rate: guard.miss_rate(),
                })
            }),
            jit_activity: JitActivitySnapshot {
                block_compile_events: self.jit_block_compile_events.value(),
                block_compile_guest_insns: self.jit_block_compile_guest_insns.value(),
                block_execute_events: self.jit_block_execute_events.value(),
                block_retired_insns: self.jit_block_retired_insns.value(),
                trace_compile_events: self.jit_trace_compile_events.value(),
                trace_compile_guest_insns: self.jit_trace_compile_guest_insns.value(),
                trace_execute_events: self.jit_trace_execute_events.value(),
                trace_execute_insns: self.jit_trace_execute_insns.value(),
                fallback_events: self.jit_fallback_events.value(),
                fallback_insns: self.jit_fallback_insns.value(),
                cache_hit_events: self.jit_cache_hit_events.value(),
                cache_miss_events: self.jit_cache_miss_events.value(),
                cache_promote_events: self.jit_cache_promote_events.value(),
                guard_exit_events: self.jit_guard_exit_events.value(),
                guard_retire_events: self.jit_guard_retire_events.value(),
            },
            scoreboard_filter: self.scoreboard_pc_filter.as_ref().map(|filter| {
                PcRangeFilterSnapshot {
                    start: filter.start,
                    end: filter.end,
                }
            }),
            scoreboard_addr_filter: self.scoreboard_addr_filter.as_ref().map(|filter| {
                AddrRangeFilterSnapshot {
                    start: filter.start,
                    end: filter.end,
                }
            }),
            user_stage2_insn_abort: None,
            fault_history: None,
            tick_count: 0,
            snapshot_ns: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|delta| u64::try_from(delta.as_nanos()).ok())
                .unwrap_or(0),
        }
    }

    /// Wire all configured primitives to probe events (always-on in dev builds).
    #[cfg(feature = "instrumentation")]
    pub(crate) fn subscribe_impl(&self, probes: &mut helm_probe::CpuProbes) {
        self.insn_count.subscribe_to_steps(probes);
        if let Some(filter) = &self.scoreboard_pc_filter {
            self.insn_mix
                .subscribe_to_steps_filtered(probes, Arc::clone(filter));
            self.branch_direction
                .subscribe_to_branches_filtered(probes, Arc::clone(filter));
        } else {
            self.insn_mix.subscribe_to_steps(probes);
            self.branch_direction.subscribe_to_branches(probes);
        }
        self.hot_pcs.subscribe_to_steps(probes);
        self.branch_heatmap.subscribe_to_branches(probes);
        if let Some(ref cache) = self.cache_l1d {
            if let Some(filter) = &self.scoreboard_addr_filter {
                cache.subscribe_to_mem_filtered(probes, Arc::clone(filter));
            } else {
                cache.subscribe_to_mem(probes);
            }
        }
        if let Some(ref pred) = self.branch_pred {
            BranchPredictor::subscribe_shared(pred, probes);
        }
        let mmu_tlb_hits = Arc::clone(&self.mmu_tlb_hits);
        let mmu_tlb_misses = Arc::clone(&self.mmu_tlb_misses);
        let mmu_stage1_walks = Arc::clone(&self.mmu_stage1_walks);
        let mmu_stage2_walks = Arc::clone(&self.mmu_stage2_walks);
        let mmu_filter = self.scoreboard_addr_filter.clone();
        probes.mmu.subscribe(move |event| {
            if mmu_filter
                .as_ref()
                .is_none_or(|filter| filter.contains(event.va))
            {
                if event.tlb_hit {
                    mmu_tlb_hits.inc();
                }
                if event.tlb_miss {
                    mmu_tlb_misses.inc();
                }
                if event.stage1_walk {
                    mmu_stage1_walks.inc();
                }
                if event.stage2_walk {
                    mmu_stage2_walks.inc();
                }
            }
        });
        for trigger in &self.triggers {
            trigger.subscribe_to_pre_step(probes);
        }
    }

    #[cfg(feature = "instrumentation")]
    pub fn subscribe(&self, probes: &mut helm_probe::CpuProbes) {
        self.subscribe_impl(probes);
    }

    /// Wire JIT activity counters to JIT probe events.
    #[cfg(feature = "instrumentation")]
    pub fn subscribe_jit(&self, probes: &mut helm_probe::JitProbes) {
        let block_compile_events = Arc::clone(&self.jit_block_compile_events);
        let block_compile_guest_insns = Arc::clone(&self.jit_block_compile_guest_insns);
        probes.block_compile.subscribe(move |event| {
            block_compile_events.inc();
            block_compile_guest_insns.add(u64::from(event.insn_count));
        });

        let block_execute_events = Arc::clone(&self.jit_block_execute_events);
        let block_retired_insns = Arc::clone(&self.jit_block_retired_insns);
        probes.block_execute.subscribe(move |event| {
            block_execute_events.inc();
            block_retired_insns.add(u64::from(event.insns_retired));
        });

        let trace_compile_events = Arc::clone(&self.jit_trace_compile_events);
        let trace_compile_guest_insns = Arc::clone(&self.jit_trace_compile_guest_insns);
        probes.trace_compile.subscribe(move |event| {
            trace_compile_events.inc();
            trace_compile_guest_insns.add(u64::from(event.insn_count));
        });

        let trace_execute_events = Arc::clone(&self.jit_trace_execute_events);
        let trace_execute_insns = Arc::clone(&self.jit_trace_execute_insns);
        let trace_execute_filter = self.scoreboard_pc_filter.clone();
        probes.trace_execute.subscribe(move |event| {
            if trace_execute_filter
                .as_ref()
                .is_none_or(|filter| filter.contains(event.start_pc))
            {
                trace_execute_events.inc();
                trace_execute_insns.add(u64::from(event.insns_retired));
            }
        });

        let fallback_events = Arc::clone(&self.jit_fallback_events);
        let fallback_insns = Arc::clone(&self.jit_fallback_insns);
        let fallback_filter = self.scoreboard_pc_filter.clone();
        probes.fallback.subscribe(move |event| {
            if fallback_filter
                .as_ref()
                .is_none_or(|filter| filter.contains(event.pc))
            {
                fallback_events.inc();
                fallback_insns.add(event.insns);
            }
        });

        let cache_hit_events = Arc::clone(&self.jit_cache_hit_events);
        let cache_miss_events = Arc::clone(&self.jit_cache_miss_events);
        let cache_promote_events = Arc::clone(&self.jit_cache_promote_events);
        let cache_filter = self.scoreboard_pc_filter.clone();
        probes.cache.subscribe(move |event| {
            if cache_filter
                .as_ref()
                .is_none_or(|filter| filter.contains(event.pc))
            {
                match event.op {
                    helm_probe::JitCacheOp::Hit => cache_hit_events.inc(),
                    helm_probe::JitCacheOp::Miss => cache_miss_events.inc(),
                    helm_probe::JitCacheOp::Promote => cache_promote_events.inc(),
                    helm_probe::JitCacheOp::Evict => {}
                }
            }
        });

        let guard_exit_events = Arc::clone(&self.jit_guard_exit_events);
        let guard_retire_events = Arc::clone(&self.jit_guard_retire_events);
        let guard_filter = self.scoreboard_pc_filter.clone();
        probes.guard_exit.subscribe(move |event| {
            if guard_filter
                .as_ref()
                .is_none_or(|filter| filter.contains(event.trace_pc))
            {
                guard_exit_events.inc();
                if event.retiring {
                    guard_retire_events.inc();
                }
            }
        });
    }

    /// Wire primitives only within an instruction window.
    #[cfg(feature = "instrumentation")]
    pub fn subscribe_in_window(&self, probes: &mut helm_probe::CpuProbes, window: Arc<Window>) {
        // Auto-update window's active flag from pre_step
        window.subscribe_to_pre_step(probes);
        let gate = window.gate();
        self.insn_count
            .subscribe_to_steps_gated(probes, Arc::clone(&gate));
        if let Some(filter) = &self.scoreboard_pc_filter {
            self.insn_mix.subscribe_to_steps_filtered_gated(
                probes,
                Arc::clone(&gate),
                Arc::clone(filter),
            );
            self.branch_direction.subscribe_to_branches_filtered_gated(
                probes,
                Arc::clone(&gate),
                Arc::clone(filter),
            );
        } else {
            self.insn_mix
                .subscribe_to_steps_gated(probes, Arc::clone(&gate));
            self.branch_direction
                .subscribe_to_branches_gated(probes, Arc::clone(&gate));
        }
        self.hot_pcs
            .subscribe_to_steps_gated(probes, Arc::clone(&gate));
        self.branch_heatmap
            .subscribe_to_branches_gated(probes, Arc::clone(&gate));
        if let Some(ref cache) = self.cache_l1d {
            if let Some(filter) = &self.scoreboard_addr_filter {
                cache.subscribe_to_mem_filtered_gated(
                    probes,
                    Arc::clone(&gate),
                    Arc::clone(filter),
                );
            } else {
                cache.subscribe_to_mem_gated(probes, Arc::clone(&gate));
            }
        }
        if let Some(ref pred) = self.branch_pred {
            BranchPredictor::subscribe_shared_gated(pred, probes, gate);
        }
        let mmu_tlb_hits = Arc::clone(&self.mmu_tlb_hits);
        let mmu_tlb_misses = Arc::clone(&self.mmu_tlb_misses);
        let mmu_stage1_walks = Arc::clone(&self.mmu_stage1_walks);
        let mmu_stage2_walks = Arc::clone(&self.mmu_stage2_walks);
        let mmu_filter = self.scoreboard_addr_filter.clone();
        let mmu_window = Arc::clone(&window);
        probes.mmu.subscribe(move |event| {
            if mmu_window.is_active(helm_probe::probe_insn_count())
                && mmu_filter
                    .as_ref()
                    .is_none_or(|filter| filter.contains(event.va))
            {
                if event.tlb_hit {
                    mmu_tlb_hits.inc();
                }
                if event.tlb_miss {
                    mmu_tlb_misses.inc();
                }
                if event.stage1_walk {
                    mmu_stage1_walks.inc();
                }
                if event.stage2_walk {
                    mmu_stage2_walks.inc();
                }
            }
        });
    }

    /// Wire JIT activity counters only within an instruction window.
    #[cfg(feature = "instrumentation")]
    pub fn subscribe_jit_in_window(&self, probes: &mut helm_probe::JitProbes, window: Arc<Window>) {
        let block_compile_events = Arc::clone(&self.jit_block_compile_events);
        let block_compile_guest_insns = Arc::clone(&self.jit_block_compile_guest_insns);
        let block_compile_window = Arc::clone(&window);
        probes.block_compile.subscribe(move |event| {
            if block_compile_window.is_active(helm_probe::probe_insn_count()) {
                block_compile_events.inc();
                block_compile_guest_insns.add(u64::from(event.insn_count));
            }
        });

        let block_execute_events = Arc::clone(&self.jit_block_execute_events);
        let block_retired_insns = Arc::clone(&self.jit_block_retired_insns);
        let block_execute_window = Arc::clone(&window);
        probes.block_execute.subscribe(move |event| {
            if block_execute_window.is_active(helm_probe::probe_insn_count()) {
                block_execute_events.inc();
                block_retired_insns.add(u64::from(event.insns_retired));
            }
        });

        let trace_compile_events = Arc::clone(&self.jit_trace_compile_events);
        let trace_compile_guest_insns = Arc::clone(&self.jit_trace_compile_guest_insns);
        let trace_compile_window = Arc::clone(&window);
        probes.trace_compile.subscribe(move |event| {
            if trace_compile_window.is_active(helm_probe::probe_insn_count()) {
                trace_compile_events.inc();
                trace_compile_guest_insns.add(u64::from(event.insn_count));
            }
        });

        let trace_execute_events = Arc::clone(&self.jit_trace_execute_events);
        let trace_execute_insns = Arc::clone(&self.jit_trace_execute_insns);
        let trace_execute_filter = self.scoreboard_pc_filter.clone();
        let trace_execute_window = Arc::clone(&window);
        probes.trace_execute.subscribe(move |event| {
            if trace_execute_window.is_active(helm_probe::probe_insn_count())
                && trace_execute_filter
                    .as_ref()
                    .is_none_or(|filter| filter.contains(event.start_pc))
            {
                trace_execute_events.inc();
                trace_execute_insns.add(u64::from(event.insns_retired));
            }
        });

        let fallback_events = Arc::clone(&self.jit_fallback_events);
        let fallback_insns = Arc::clone(&self.jit_fallback_insns);
        let fallback_filter = self.scoreboard_pc_filter.clone();
        let fallback_window = Arc::clone(&window);
        probes.fallback.subscribe(move |event| {
            if fallback_window.is_active(helm_probe::probe_insn_count())
                && fallback_filter
                    .as_ref()
                    .is_none_or(|filter| filter.contains(event.pc))
            {
                fallback_events.inc();
                fallback_insns.add(event.insns);
            }
        });

        let cache_hit_events = Arc::clone(&self.jit_cache_hit_events);
        let cache_miss_events = Arc::clone(&self.jit_cache_miss_events);
        let cache_promote_events = Arc::clone(&self.jit_cache_promote_events);
        let cache_filter = self.scoreboard_pc_filter.clone();
        let cache_window = Arc::clone(&window);
        probes.cache.subscribe(move |event| {
            if cache_window.is_active(helm_probe::probe_insn_count())
                && cache_filter
                    .as_ref()
                    .is_none_or(|filter| filter.contains(event.pc))
            {
                match event.op {
                    helm_probe::JitCacheOp::Hit => cache_hit_events.inc(),
                    helm_probe::JitCacheOp::Miss => cache_miss_events.inc(),
                    helm_probe::JitCacheOp::Promote => cache_promote_events.inc(),
                    helm_probe::JitCacheOp::Evict => {}
                }
            }
        });

        let guard_exit_events = Arc::clone(&self.jit_guard_exit_events);
        let guard_retire_events = Arc::clone(&self.jit_guard_retire_events);
        let guard_filter = self.scoreboard_pc_filter.clone();
        probes.guard_exit.subscribe(move |event| {
            if window.is_active(helm_probe::probe_insn_count())
                && guard_filter
                    .as_ref()
                    .is_none_or(|filter| filter.contains(event.trace_pc))
            {
                guard_exit_events.inc();
                if event.retiring {
                    guard_retire_events.inc();
                }
            }
        });
    }

    /// Add a trigger and wire it to probe events immediately.
    #[cfg(feature = "instrumentation")]
    pub fn add_trigger_live(&mut self, trigger: Trigger, probes: &mut helm_probe::CpuProbes) {
        let arc = Arc::new(trigger);
        arc.subscribe_to_pre_step(probes);
        self.triggers.push(arc);
    }
}

impl Default for HelmSpy {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, feature = "collection"))]
mod tests {
    use super::*;
    use crate::analysis::PredictorKind;
    use crate::events::InsnClass;
    use crate::trigger::TriggerKind;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    #[test]
    fn session_new_defaults() {
        let session = HelmSpy::new();
        assert_eq!(session.insn_count.value(), 0);
        assert_eq!(session.insn_mix.total(), 0);
        assert!(session.hot_pcs.is_empty());
        assert!(session.cache_l1d.is_none());
        assert!(session.branch_pred.is_none());
    }

    #[test]
    fn session_with_cache() {
        let session = HelmSpy::new().with_cache_l1d(32 * 1024, 8, 64);
        assert!(session.cache_l1d.is_some());
    }

    #[test]
    fn session_with_branch_pred() {
        let pred = BranchPredictor::new(PredictorKind::BiModal { bits: 10 });
        let session = HelmSpy::new().with_branch_predictor(pred);
        assert!(session.branch_pred.is_some());
    }

    #[test]
    fn session_snapshot() {
        let session = HelmSpy::new();
        session.insn_count.add(1000);
        session.insn_mix.record(InsnClass::IntAlu);
        session.insn_mix.record(InsnClass::Load);
        session.hot_pcs.inc(0x1000);
        session.hot_pcs.inc(0x1000);
        session.hot_pcs.inc(0x2000);

        let snap = session.snapshot();
        assert_eq!(snap.insn_count, 1000);
        assert_eq!(snap.insn_mix.len(), InsnClass::COUNT);
        assert!(!snap.hot_pcs.is_empty());
        assert!(snap.cache_l1d.is_none());
        assert!(snap.branch_pred.is_none());
    }

    #[test]
    fn session_check_triggers() {
        let mut session = HelmSpy::new();
        let fire_count = Arc::new(AtomicU64::new(0));
        let fc = Arc::clone(&fire_count);
        session.add_trigger(Trigger::new(
            TriggerKind::AtInsn(100),
            true,
            move |_pc, _ic| {
                fc.fetch_add(1, Ordering::Relaxed);
            },
        ));

        session.check_triggers(0x1000, 50);
        assert_eq!(fire_count.load(Ordering::Relaxed), 0);

        session.check_triggers(0x1000, 100);
        assert_eq!(fire_count.load(Ordering::Relaxed), 1);

        // one-shot: should not fire again
        session.check_triggers(0x1000, 100);
        assert_eq!(fire_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn session_fault_history() {
        let session = HelmSpy::new();
        session
            .fault_history
            .push("fault at 0x1000: undefined".to_string());
        session
            .fault_history
            .push("fault at 0x2000: data abort".to_string());

        let faults = session.fault_history.snapshot();
        assert_eq!(faults.len(), 2);
        assert!(faults[0].contains("0x1000"));
    }

    #[test]
    fn session_integrated_workflow() {
        let session = HelmSpy::new().with_cache_l1d(1024, 2, 64);

        // Simulate some instructions
        for i in 0..100u64 {
            session.insn_count.inc();
            session.insn_mix.record(if i % 3 == 0 {
                InsnClass::IntAlu
            } else if i % 3 == 1 {
                InsnClass::Load
            } else {
                InsnClass::Store
            });
            session.hot_pcs.inc(0x1000 + (i % 10) * 4);
            if let Some(ref cache) = session.cache_l1d {
                cache.access(0x1000 + (i % 5) * 64);
            }
        }

        let snap = session.snapshot();
        assert_eq!(snap.insn_count, 100);
        assert!(snap.cache_l1d.is_some());
        assert!(snap.cache_l1d.unwrap().hit_rate > 0.0);
    }

    #[test]
    fn session_snapshot_accepts_user_stage2_stats() {
        let mut snap = HelmSpy::new().snapshot();
        snap.user_stage2_insn_abort = Some(crate::snapshot::UserStage2InsnAbortSnapshot {
            events: 3,
            repeats: 1,
        });

        let stats = snap
            .user_stage2_insn_abort
            .expect("user stage2 stats should be present");
        assert_eq!(stats.events, 3);
        assert_eq!(stats.repeats, 1);
    }

    #[cfg(feature = "instrumentation")]
    #[test]
    fn session_scoreboard_pc_filter_limits_insn_mix_and_branch_direction() {
        let session =
            HelmSpy::new().with_scoreboard_pc_filter(PcRangeFilter::new(0x1000, 0x1100).unwrap());
        let mut probes = helm_probe::CpuProbes::default();
        session.subscribe(&mut probes);

        probes.post_step.notify(&helm_probe::CpuStepEvent {
            pc: 0x1004,
            raw: 0,
            insn_class: helm_probe::InsnClass::Load,
            is_stub: false,
        });
        probes.post_step.notify(&helm_probe::CpuStepEvent {
            pc: 0x2004,
            raw: 0,
            insn_class: helm_probe::InsnClass::Store,
            is_stub: false,
        });
        probes.branch.notify(&helm_probe::BranchEvent {
            pc: 0x1008,
            target: 0x1010,
            taken: true,
            kind: helm_probe::BranchKind::DirectCond,
        });
        probes.branch.notify(&helm_probe::BranchEvent {
            pc: 0x2008,
            target: 0x2010,
            taken: false,
            kind: helm_probe::BranchKind::DirectCond,
        });

        let snap = session.snapshot();
        assert_eq!(snap.scoreboard_filter.as_ref().unwrap().start, 0x1000);
        assert_eq!(
            snap.insn_mix
                .iter()
                .find(|(name, _)| name == "Load")
                .map(|(_, count)| *count),
            Some(1)
        );
        assert_eq!(
            snap.insn_mix
                .iter()
                .find(|(name, _)| name == "Store")
                .map(|(_, count)| *count),
            Some(0)
        );
        assert_eq!(snap.branch_direction.taken, 1);
        assert_eq!(snap.branch_direction.not_taken, 0);
    }

    #[cfg(feature = "instrumentation")]
    #[test]
    fn session_scoreboard_addr_filter_limits_cache_model() {
        let session = HelmSpy::new()
            .with_cache_l1d(1024, 2, 64)
            .with_scoreboard_addr_filter(AddrRangeFilter::new(0x2000, 0x2100).unwrap());
        let mut probes = helm_probe::CpuProbes::default();
        session.subscribe(&mut probes);

        probes.mem.notify(&helm_probe::MemAccessEvent {
            addr: 0x2000,
            size: 8,
            is_store: false,
            pc: 0x1000,
        });
        probes.mem.notify(&helm_probe::MemAccessEvent {
            addr: 0x2000,
            size: 8,
            is_store: false,
            pc: 0x1004,
        });
        probes.mem.notify(&helm_probe::MemAccessEvent {
            addr: 0x3000,
            size: 8,
            is_store: false,
            pc: 0x1008,
        });

        let snap = session.snapshot();
        assert_eq!(snap.scoreboard_addr_filter.as_ref().unwrap().start, 0x2000);
        let cache = snap.cache_l1d.expect("cache snapshot");
        assert_eq!(cache.hits, 1);
        assert_eq!(cache.misses, 1);
    }

    #[cfg(feature = "instrumentation")]
    #[test]
    fn session_scoreboard_addr_filter_limits_mmu_activity() {
        let session = HelmSpy::new()
            .with_scoreboard_addr_filter(AddrRangeFilter::new(0x2000, 0x2100).unwrap());
        let mut probes = helm_probe::CpuProbes::default();
        session.subscribe(&mut probes);

        probes.mmu.notify(&helm_probe::MmuTranslateEvent {
            va: 0x2004,
            access: helm_probe::MmuAccessKind::Read,
            tlb_hit: false,
            tlb_miss: true,
            stage1_walk: true,
            stage2_walk: false,
        });
        probes.mmu.notify(&helm_probe::MmuTranslateEvent {
            va: 0x3004,
            access: helm_probe::MmuAccessKind::Execute,
            tlb_hit: true,
            tlb_miss: false,
            stage1_walk: false,
            stage2_walk: true,
        });

        let snap = session.snapshot();
        assert_eq!(snap.mmu_activity.tlb_hits, 0);
        assert_eq!(snap.mmu_activity.tlb_misses, 1);
        assert_eq!(snap.mmu_activity.stage1_walks, 1);
        assert_eq!(snap.mmu_activity.stage2_walks, 0);
    }

    #[cfg(feature = "instrumentation")]
    #[test]
    fn session_jit_subscriptions_record_probe_events() {
        let session = HelmSpy::new();
        let mut probes = helm_probe::JitProbes::default();
        session.subscribe_jit(&mut probes);

        probes
            .block_compile
            .notify(&helm_probe::JitBlockCompileEvent {
                pc: 0x1000,
                insn_count: 4,
                backend: helm_probe::JitBackendId::Other,
            });
        probes
            .block_execute
            .notify(&helm_probe::JitBlockExecuteEvent {
                pc: 0x1000,
                next_pc: 0x1010,
                insns_retired: 4,
                exit_code: 0,
            });
        probes
            .trace_compile
            .notify(&helm_probe::JitTraceCompileEvent {
                start_pc: 0x2000,
                insn_count: 9,
                guard_count: 1,
            });
        probes
            .trace_execute
            .notify(&helm_probe::JitTraceExecuteEvent {
                start_pc: 0x2000,
                exit_code: 0,
                resume_pc: 0x2010,
                insns_retired: 9,
            });
        probes.fallback.notify(&helm_probe::JitFallbackEvent {
            pc: 0x1000,
            insns: 5,
            reason: Some("unsupported-start"),
        });
        probes.cache.notify(&helm_probe::JitCacheEvent {
            pc: 0x1000,
            op: helm_probe::JitCacheOp::Hit,
            exec_count: 3,
        });
        probes.cache.notify(&helm_probe::JitCacheEvent {
            pc: 0x1000,
            op: helm_probe::JitCacheOp::Promote,
            exec_count: 4,
        });
        probes.guard_exit.notify(&helm_probe::JitGuardExitEvent {
            trace_pc: 0x2000,
            guard_id: 1,
            resume_pc: 0x2010,
            miss_count: 2,
            retiring: true,
        });

        let snap = session.snapshot();
        assert_eq!(snap.jit_activity.block_compile_events, 1);
        assert_eq!(snap.jit_activity.block_compile_guest_insns, 4);
        assert_eq!(snap.jit_activity.block_execute_events, 1);
        assert_eq!(snap.jit_activity.block_retired_insns, 4);
        assert_eq!(snap.jit_activity.trace_compile_events, 1);
        assert_eq!(snap.jit_activity.trace_compile_guest_insns, 9);
        assert_eq!(snap.jit_activity.trace_execute_events, 1);
        assert_eq!(snap.jit_activity.trace_execute_insns, 9);
        assert_eq!(snap.jit_activity.fallback_events, 1);
        assert_eq!(snap.jit_activity.fallback_insns, 5);
        assert_eq!(snap.jit_activity.cache_hit_events, 1);
        assert_eq!(snap.jit_activity.cache_promote_events, 1);
        assert_eq!(snap.jit_activity.guard_exit_events, 1);
        assert_eq!(snap.jit_activity.guard_retire_events, 1);
    }
}

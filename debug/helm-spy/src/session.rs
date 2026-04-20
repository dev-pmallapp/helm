use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::analysis::{BranchPredictor, CacheModel, InsnMix};
use crate::primitives::{Counter, HeatMap, RingBuffer};
use crate::snapshot::{BranchPredSnapshot, CacheSnapshot, HelmSpySnapshot, JitActivitySnapshot};
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
    pub fault_history: Arc<RingBuffer<String>>,
    pub triggers: Vec<Arc<Trigger>>,
}

impl HelmSpy {
    pub fn new() -> Self {
        Self {
            insn_count: Arc::new(Counter::new("insn_count")),
            insn_mix: Arc::new(InsnMix::new()),
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
            fault_history: Arc::new(RingBuffer::new(128)),
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
            },
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
        self.insn_mix.subscribe_to_steps(probes);
        self.hot_pcs.subscribe_to_steps(probes);
        self.branch_heatmap.subscribe_to_branches(probes);
        if let Some(ref cache) = self.cache_l1d {
            cache.subscribe_to_mem(probes);
        }
        if let Some(ref pred) = self.branch_pred {
            BranchPredictor::subscribe_shared(pred, probes);
        }
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
    }

    /// Wire primitives only within an instruction window.
    #[cfg(feature = "instrumentation")]
    pub fn subscribe_in_window(&self, probes: &mut helm_probe::CpuProbes, window: Arc<Window>) {
        // Auto-update window's active flag from pre_step
        window.subscribe_to_pre_step(probes);
        let gate = window.gate();
        self.insn_count
            .subscribe_to_steps_gated(probes, Arc::clone(&gate));
        self.insn_mix
            .subscribe_to_steps_gated(probes, Arc::clone(&gate));
        self.hot_pcs
            .subscribe_to_steps_gated(probes, Arc::clone(&gate));
        self.branch_heatmap
            .subscribe_to_branches_gated(probes, Arc::clone(&gate));
        if let Some(ref cache) = self.cache_l1d {
            cache.subscribe_to_mem_gated(probes, Arc::clone(&gate));
        }
        if let Some(ref pred) = self.branch_pred {
            BranchPredictor::subscribe_shared_gated(pred, probes, gate);
        }
    }

    /// Wire JIT activity counters only within an instruction window.
    #[cfg(feature = "instrumentation")]
    pub fn subscribe_jit_in_window(
        &self,
        probes: &mut helm_probe::JitProbes,
        window: Arc<Window>,
    ) {
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
        probes.trace_compile.subscribe(move |event| {
            if window.is_active(helm_probe::probe_insn_count()) {
                trace_compile_events.inc();
                trace_compile_guest_insns.add(u64::from(event.insn_count));
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

#[cfg(test)]
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
    fn session_jit_subscriptions_record_probe_events() {
        let session = HelmSpy::new();
        let mut probes = helm_probe::JitProbes::default();
        session.subscribe_jit(&mut probes);

        probes.block_compile.notify(&helm_probe::JitBlockCompileEvent {
            pc: 0x1000,
            insn_count: 4,
            backend: helm_probe::JitBackendId::Other,
        });
        probes.block_execute.notify(&helm_probe::JitBlockExecuteEvent {
            pc: 0x1000,
            next_pc: 0x1010,
            insns_retired: 4,
            exit_code: 0,
        });
        probes.trace_compile.notify(&helm_probe::JitTraceCompileEvent {
            start_pc: 0x2000,
            insn_count: 9,
            guard_count: 1,
        });

        let snap = session.snapshot();
        assert_eq!(snap.jit_activity.block_compile_events, 1);
        assert_eq!(snap.jit_activity.block_compile_guest_insns, 4);
        assert_eq!(snap.jit_activity.block_execute_events, 1);
        assert_eq!(snap.jit_activity.block_retired_insns, 4);
        assert_eq!(snap.jit_activity.trace_compile_events, 1);
        assert_eq!(snap.jit_activity.trace_compile_guest_insns, 9);
    }
}

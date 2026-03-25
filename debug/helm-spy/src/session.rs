use std::sync::{Arc, Mutex};

use crate::analysis::{BranchPredictor, CacheModel, InsnMix};
use crate::primitives::{Counter, HeatMap, RingBuffer};
use crate::trigger::Trigger;
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
            fault_history: Arc::new(RingBuffer::new(128)),
            triggers: Vec::new(),
        }
    }

    /// Configure an L1D cache model for the session.
    pub fn with_cache_l1d(mut self, size_bytes: usize, ways: usize, line_size: usize) -> Self {
        self.cache_l1d = Some(Arc::new(CacheModel::new("L1D", size_bytes, ways, line_size)));
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
    pub fn snapshot(&self) -> SpySnapshot {
        SpySnapshot {
            insn_count: self.insn_count.value(),
            insn_mix_table: self.insn_mix.table(),
            hot_pcs_top20: self.hot_pcs.top(20),
            cache_hit_rate: self.cache_l1d.as_ref().map(|c| c.hit_rate()),
            branch_miss_rate: self.branch_pred.as_ref()
                .and_then(|p| p.lock().ok().map(|g| g.miss_rate())),
        }
    }

    /// Wire all configured primitives to probe events (always-on in dev builds).
    #[cfg(debug_assertions)]
    pub fn subscribe(&self, probes: &mut helm_probe::CpuProbes) {
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

    /// Wire primitives only within an instruction window.
    #[cfg(debug_assertions)]
    pub fn subscribe_in_window(
        &self,
        probes: &mut helm_probe::CpuProbes,
        window: Arc<Window>,
    ) {
        // Auto-update window's active flag from pre_step
        window.subscribe_to_pre_step(probes);
        let gate = window.gate();
        self.insn_count.subscribe_to_steps_gated(probes, Arc::clone(&gate));
        self.insn_mix.subscribe_to_steps_gated(probes, Arc::clone(&gate));
        self.hot_pcs.subscribe_to_steps_gated(probes, Arc::clone(&gate));
        self.branch_heatmap.subscribe_to_branches_gated(probes, Arc::clone(&gate));
        if let Some(ref cache) = self.cache_l1d {
            cache.subscribe_to_mem_gated(probes, Arc::clone(&gate));
        }
        if let Some(ref pred) = self.branch_pred {
            BranchPredictor::subscribe_shared_gated(pred, probes, gate);
        }
    }

    /// Add a trigger and wire it to probe events immediately.
    #[cfg(debug_assertions)]
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

/// A point-in-time snapshot of session state for reporting/differential analysis.
pub struct SpySnapshot {
    pub insn_count: u64,
    pub insn_mix_table: Vec<(&'static str, u64, f64)>,
    pub hot_pcs_top20: Vec<(u64, u64)>,
    pub cache_hit_rate: Option<f64>,
    pub branch_miss_rate: Option<f64>,
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
        assert_eq!(snap.insn_mix_table.len(), InsnClass::COUNT);
        assert!(!snap.hot_pcs_top20.is_empty());
        assert!(snap.cache_hit_rate.is_none());
        assert!(snap.branch_miss_rate.is_none());
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
        let session = HelmSpy::new()
            .with_cache_l1d(1024, 2, 64);

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
        assert!(snap.cache_hit_rate.is_some());
        assert!(snap.cache_hit_rate.unwrap() > 0.0);
    }
}

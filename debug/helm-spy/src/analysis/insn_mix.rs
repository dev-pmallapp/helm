use std::sync::Arc;
#[cfg(debug_assertions)]
use crate::trigger::Gate;

use crate::events::InsnClass;
use crate::primitives::IndexedCounter;

pub const INSN_CLASS_LABELS: &[&str] = &[
    "IntAlu", "IntMul", "Branch", "Load", "Store",
    "FpAlu", "SimdAlu", "System", "Nop", "Atomic", "Unknown",
];

/// Instruction mix analyzer built on an IndexedCounter.
/// Records instruction class frequencies and computes mix ratios.
pub struct InsnMix {
    counts: IndexedCounter,
}

impl InsnMix {
    pub fn new() -> Self {
        Self {
            counts: IndexedCounter::new("insn_mix", INSN_CLASS_LABELS),
        }
    }

    #[inline]
    pub fn record(&self, class: InsnClass) {
        self.counts.inc(class as usize);
    }

    pub fn table(&self) -> Vec<(&'static str, u64, f64)> {
        self.counts.table()
    }

    pub fn total(&self) -> u64 {
        self.counts.total()
    }

    pub fn value(&self, class: InsnClass) -> u64 {
        self.counts.value(class as usize)
    }

    pub fn fraction(&self, class: InsnClass) -> f64 {
        self.counts.fraction(class as usize)
    }

    pub fn reset(&self) {
        self.counts.reset();
    }

    /// Subscribe to post_step probe events, recording the instruction class
    /// carried by `CpuStepEvent::insn_class`.
    #[cfg(debug_assertions)]
    pub fn subscribe_to_steps(self: &Arc<Self>, probes: &mut helm_probe::CpuProbes) {
        let m = Arc::clone(self);
        probes.post_step.subscribe(move |ev: &helm_probe::CpuStepEvent| {
            m.record(ev.insn_class);
        });
    }

    /// Subscribe gated — only records when gate is armed.
    #[cfg(debug_assertions)]
    pub fn subscribe_to_steps_gated(
        self: &Arc<Self>,
        probes: &mut helm_probe::CpuProbes,
        gate: Gate,
    ) {
        let m = Arc::clone(self);
        probes.post_step.subscribe(move |ev: &helm_probe::CpuStepEvent| {
            if gate.load(std::sync::atomic::Ordering::Relaxed) {
                m.record(ev.insn_class);
            }
        });
    }

    /// Subscribe with a classification callback that maps raw instruction
    /// words to `InsnClass`. Full classification requires `classify_aarch64_opcode()`
    /// which lives in helm-engine.
    #[cfg(debug_assertions)]
    pub fn subscribe_to_steps_with_classifier(
        self: &Arc<Self>,
        probes: &mut helm_probe::CpuProbes,
        classifier: Arc<dyn Fn(u32) -> InsnClass + Send + Sync>,
    ) {
        let m = Arc::clone(self);
        probes.post_step.subscribe(move |ev: &helm_probe::CpuStepEvent| {
            m.record(classifier(ev.raw));
        });
    }
}

impl Default for InsnMix {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insn_mix_record_and_table() {
        let mix = InsnMix::new();

        // Record some instructions
        for _ in 0..50 {
            mix.record(InsnClass::IntAlu);
        }
        for _ in 0..20 {
            mix.record(InsnClass::Load);
        }
        for _ in 0..15 {
            mix.record(InsnClass::Store);
        }
        for _ in 0..10 {
            mix.record(InsnClass::Branch);
        }
        for _ in 0..5 {
            mix.record(InsnClass::FpAlu);
        }

        assert_eq!(mix.total(), 100);
        assert_eq!(mix.value(InsnClass::IntAlu), 50);
        assert!((mix.fraction(InsnClass::IntAlu) - 0.5).abs() < 1e-10);
    }

    #[test]
    fn insn_mix_fractions_sum_to_one() {
        let mix = InsnMix::new();

        mix.record(InsnClass::IntAlu);
        mix.record(InsnClass::Load);
        mix.record(InsnClass::Branch);
        mix.record(InsnClass::FpAlu);
        mix.record(InsnClass::Nop);

        let table = mix.table();
        let frac_sum: f64 = table.iter().map(|(_, _, f)| f).sum();
        assert!(
            (frac_sum - 1.0).abs() < 1e-10,
            "fractions must sum to 1.0, got {}",
            frac_sum,
        );
    }

    #[test]
    fn insn_mix_empty() {
        let mix = InsnMix::new();
        assert_eq!(mix.total(), 0);
        // All fractions should be 0 when empty
        for (_, _, frac) in mix.table() {
            assert_eq!(frac, 0.0);
        }
    }

    #[test]
    fn insn_mix_all_classes() {
        let mix = InsnMix::new();
        mix.record(InsnClass::IntAlu);
        mix.record(InsnClass::IntMul);
        mix.record(InsnClass::Branch);
        mix.record(InsnClass::Load);
        mix.record(InsnClass::Store);
        mix.record(InsnClass::FpAlu);
        mix.record(InsnClass::SimdAlu);
        mix.record(InsnClass::System);
        mix.record(InsnClass::Nop);
        mix.record(InsnClass::Atomic);
        mix.record(InsnClass::Unknown);

        assert_eq!(mix.total(), 11);
        let table = mix.table();
        assert_eq!(table.len(), InsnClass::COUNT);
        // Each class should have exactly 1
        for (_, count, _) in &table {
            assert_eq!(*count, 1);
        }
    }

    #[test]
    fn insn_mix_reset() {
        let mix = InsnMix::new();
        mix.record(InsnClass::IntAlu);
        mix.record(InsnClass::Load);
        assert_eq!(mix.total(), 2);
        mix.reset();
        assert_eq!(mix.total(), 0);
    }
}

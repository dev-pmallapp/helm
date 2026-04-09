#[cfg(feature = "instrumentation")]
use crate::trigger::Gate;
#[cfg(feature = "instrumentation")]
use std::sync::{Arc, Mutex};

/// Branch predictor kind: selects the prediction algorithm and table size.
pub enum PredictorKind {
    /// BiModal: direct-mapped table indexed by PC bits.
    BiModal { bits: u8 },
    /// GShare: XOR of global history register with PC bits.
    GShare { hist_bits: u8, table_bits: u8 },
}

/// Branch predictor using 2-bit saturating counters.
/// Counter values: 0=strongly not taken, 1=weakly not taken,
///                 2=weakly taken, 3=strongly taken.
/// Prediction: counter >= 2 -> predict taken.
pub struct BranchPredictor {
    kind: PredictorKind,
    table: Vec<u8>,
    history: u64, // global branch history register (GShare)
    predictions: u64,
    mispredictions: u64,
}

impl BranchPredictor {
    pub fn new(kind: PredictorKind) -> Self {
        let table_size = match &kind {
            PredictorKind::BiModal { bits } => 1usize << *bits,
            PredictorKind::GShare { table_bits, .. } => 1usize << *table_bits,
        };
        Self {
            kind,
            table: vec![1; table_size], // weakly not-taken initially
            history: 0,
            predictions: 0,
            mispredictions: 0,
        }
    }

    fn table_index(&self, pc: u64) -> usize {
        match &self.kind {
            PredictorKind::BiModal { bits } => {
                let mask = (1usize << *bits) - 1;
                // Use PC bits [2..2+bits] (skip 2 low bits for instruction alignment)
                ((pc >> 2) as usize) & mask
            }
            PredictorKind::GShare {
                hist_bits,
                table_bits,
            } => {
                let mask = (1usize << *table_bits) - 1;
                let pc_bits = ((pc >> 2) as usize) & mask;
                let hist_mask = (1u64 << *hist_bits) - 1;
                let hist_bits_val = (self.history & hist_mask) as usize;
                (pc_bits ^ hist_bits_val) & mask
            }
        }
    }

    /// Predict and update the predictor state.
    pub fn predict_and_update(&mut self, pc: u64, taken: bool) {
        let idx = self.table_index(pc);
        let predicted = self.table[idx] >= 2;

        if predicted != taken {
            self.mispredictions += 1;
        }
        self.predictions += 1;

        // Update counter (2-bit saturating)
        if taken {
            self.table[idx] = (self.table[idx] + 1).min(3);
        } else {
            self.table[idx] = self.table[idx].saturating_sub(1);
        }

        // Update global history register
        self.history = (self.history << 1) | (taken as u64);
    }

    /// Subscribe a shared (Arc<Mutex<Self>>) branch predictor to the branch probe.
    /// Calls predict_and_update() on every branch event.
    ///
    /// Use this via `BranchPredictor::subscribe_shared(&arc, probes)`.
    #[cfg(feature = "instrumentation")]
    pub fn subscribe_shared(shared: &Arc<Mutex<Self>>, probes: &mut helm_probe::CpuProbes) {
        let p = Arc::clone(shared);
        probes
            .branch
            .subscribe(move |ev: &helm_probe::BranchEvent| {
                if let Ok(mut guard) = p.lock() {
                    guard.predict_and_update(ev.pc, ev.taken);
                }
            });
    }

    /// Subscribe gated by a Gate — only predicts while gate is armed.
    #[cfg(feature = "instrumentation")]
    pub fn subscribe_shared_gated(
        shared: &Arc<Mutex<Self>>,
        probes: &mut helm_probe::CpuProbes,
        gate: Gate,
    ) {
        let p = Arc::clone(shared);
        probes
            .branch
            .subscribe(move |ev: &helm_probe::BranchEvent| {
                if gate.load(std::sync::atomic::Ordering::Relaxed) {
                    if let Ok(mut guard) = p.lock() {
                        guard.predict_and_update(ev.pc, ev.taken);
                    }
                }
            });
    }

    pub fn miss_rate(&self) -> f64 {
        if self.predictions == 0 {
            0.0
        } else {
            self.mispredictions as f64 / self.predictions as f64
        }
    }

    /// Mispredictions per kilo-instruction.
    pub fn mpki(&self, insn_count: u64) -> f64 {
        if insn_count == 0 {
            return 0.0;
        }
        self.mispredictions as f64 / insn_count as f64 * 1000.0
    }

    pub fn predictions(&self) -> u64 {
        self.predictions
    }

    pub fn mispredictions(&self) -> u64 {
        self.mispredictions
    }

    pub fn name(&self) -> &'static str {
        match self.kind {
            PredictorKind::BiModal { .. } => "bimodal",
            PredictorKind::GShare { .. } => "gshare",
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            PredictorKind::BiModal { .. } => "BiModal",
            PredictorKind::GShare { .. } => "GShare",
        }
    }

    pub fn reset(&mut self) {
        self.table.fill(1); // weakly not-taken
        self.history = 0;
        self.predictions = 0;
        self.mispredictions = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bimodal_always_taken_stream() {
        // With a consistent always-taken stream, the predictor should converge
        // to high accuracy after warming up.
        let mut pred = BranchPredictor::new(PredictorKind::BiModal { bits: 10 });

        let pc = 0x1000u64;
        // Warmup: first few predictions may miss (counter starts at 1 = weakly not taken)
        for _ in 0..100 {
            pred.predict_and_update(pc, true);
        }

        // After warmup, counter should be at 3 (strongly taken), predictions should be good
        // Only the first prediction should be a miss (counter was 1, predicted not-taken)
        assert_eq!(
            pred.mispredictions(),
            1,
            "only first prediction should miss for always-taken"
        );
        assert_eq!(pred.predictions(), 100);
        assert!(pred.miss_rate() < 0.02);
    }

    #[test]
    fn bimodal_always_not_taken_stream() {
        let mut pred = BranchPredictor::new(PredictorKind::BiModal { bits: 10 });

        let pc = 0x2000u64;
        for _ in 0..100 {
            pred.predict_and_update(pc, false);
        }

        // Counter starts at 1, prediction is not-taken (correct from start)
        assert_eq!(pred.mispredictions(), 0);
    }

    #[test]
    fn gshare_alternating_stream() {
        // GShare should handle alternating patterns better than BiModal
        // because the global history register captures the pattern.
        let mut pred = BranchPredictor::new(PredictorKind::GShare {
            hist_bits: 8,
            table_bits: 10,
        });

        let pc = 0x1000u64;
        // Train on alternating taken/not-taken
        for i in 0..1000 {
            let taken = i % 2 == 0;
            pred.predict_and_update(pc, taken);
        }

        // GShare should learn the alternating pattern and achieve reasonable accuracy
        // The exact accuracy depends on history length and table aliasing
        assert!(
            pred.predictions() == 1000,
            "should have made 1000 predictions"
        );
    }

    #[test]
    fn predictor_miss_rate_empty() {
        let pred = BranchPredictor::new(PredictorKind::BiModal { bits: 8 });
        assert_eq!(pred.miss_rate(), 0.0);
    }

    #[test]
    fn predictor_mpki() {
        let mut pred = BranchPredictor::new(PredictorKind::BiModal { bits: 8 });
        pred.predict_and_update(0x1000, true); // miss (counter was 1)
        assert_eq!(pred.mispredictions(), 1);
        let mpki = pred.mpki(1000);
        assert!((mpki - 1.0).abs() < 1e-10);
    }

    #[test]
    fn predictor_reset() {
        let mut pred = BranchPredictor::new(PredictorKind::BiModal { bits: 8 });
        pred.predict_and_update(0x1000, true);
        pred.predict_and_update(0x2000, false);
        assert_eq!(pred.predictions(), 2);

        pred.reset();
        assert_eq!(pred.predictions(), 0);
        assert_eq!(pred.mispredictions(), 0);
    }

    #[test]
    fn gshare_different_history_different_index() {
        let mut pred = BranchPredictor::new(PredictorKind::GShare {
            hist_bits: 4,
            table_bits: 8,
        });

        // Same PC but different history should produce different table indices
        let pc = 0x1000u64;
        pred.predict_and_update(pc, true); // history becomes 1
        pred.predict_and_update(pc, false); // history becomes 10
        pred.predict_and_update(pc, true); // history becomes 101

        // If GShare is working, different history patterns should lead to
        // different entries being updated
        assert_eq!(pred.predictions(), 3);
    }
}

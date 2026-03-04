//! Branch predictor models.

use helm_core::config::BranchPredictorConfig;
use helm_core::types::Addr;

pub enum BranchPredictor {
    Static(StaticPredictor),
    Bimodal(BimodalPredictor),
    GShare(GSharePredictor),
    Tage(TagePredictor),
    Tournament(TournamentPredictor),
}

impl BranchPredictor {
    pub fn from_config(cfg: &BranchPredictorConfig) -> Self {
        match cfg {
            BranchPredictorConfig::Static => Self::Static(StaticPredictor),
            BranchPredictorConfig::Bimodal { table_size } => {
                Self::Bimodal(BimodalPredictor::new(*table_size as usize))
            }
            BranchPredictorConfig::GShare { history_bits } => {
                Self::GShare(GSharePredictor::new(*history_bits))
            }
            BranchPredictorConfig::TAGE { history_length } => {
                Self::Tage(TagePredictor::new(*history_length))
            }
            BranchPredictorConfig::Tournament => Self::Tournament(TournamentPredictor::new()),
        }
    }

    pub fn predict(&self, pc: Addr) -> bool {
        match self {
            Self::Static(p) => p.predict(pc),
            Self::Bimodal(p) => p.predict(pc),
            Self::GShare(p) => p.predict(pc),
            Self::Tage(p) => p.predict(pc),
            Self::Tournament(p) => p.predict(pc),
        }
    }
}

// --- Individual predictor stubs ---

pub struct StaticPredictor;
impl StaticPredictor {
    pub fn predict(&self, _pc: Addr) -> bool {
        false
    }
}

pub struct BimodalPredictor {
    table: Vec<u8>,
}
impl BimodalPredictor {
    pub fn new(size: usize) -> Self {
        Self {
            table: vec![1; size],
        }
    }
    pub fn predict(&self, pc: Addr) -> bool {
        let idx = (pc as usize) % self.table.len();
        self.table[idx] >= 2
    }
}

pub struct GSharePredictor {
    _history_bits: u32,
}
impl GSharePredictor {
    pub fn new(history_bits: u32) -> Self {
        Self {
            _history_bits: history_bits,
        }
    }
    pub fn predict(&self, _pc: Addr) -> bool {
        false
    }
}

pub struct TagePredictor {
    _history_length: u32,
}
impl TagePredictor {
    pub fn new(history_length: u32) -> Self {
        Self {
            _history_length: history_length,
        }
    }
    pub fn predict(&self, _pc: Addr) -> bool {
        false
    }
}

#[derive(Default)]
pub struct TournamentPredictor;
impl TournamentPredictor {
    pub fn new() -> Self {
        Self
    }
    pub fn predict(&self, _pc: Addr) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_core::config::BranchPredictorConfig;

    #[test]
    fn static_predictor_always_not_taken() {
        let bp = BranchPredictor::from_config(&BranchPredictorConfig::Static);
        assert!(!bp.predict(0x1000));
        assert!(!bp.predict(0x2000));
    }

    #[test]
    fn bimodal_initial_state_not_taken() {
        let bp = BranchPredictor::from_config(&BranchPredictorConfig::Bimodal { table_size: 64 });
        // Initial counters are 1 (weakly not-taken).
        assert!(!bp.predict(0x1000));
    }

    #[test]
    fn all_variants_construct_without_panic() {
        let configs = vec![
            BranchPredictorConfig::Static,
            BranchPredictorConfig::Bimodal { table_size: 256 },
            BranchPredictorConfig::GShare { history_bits: 12 },
            BranchPredictorConfig::TAGE { history_length: 32 },
            BranchPredictorConfig::Tournament,
        ];
        for cfg in configs {
            let bp = BranchPredictor::from_config(&cfg);
            let _ = bp.predict(0x100); // should not panic
        }
    }
}

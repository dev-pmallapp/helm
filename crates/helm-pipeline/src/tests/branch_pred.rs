use crate::branch_pred::*;
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
fn static_predictor_predict_any_pc_is_false() {
    let bp = BranchPredictor::from_config(&BranchPredictorConfig::Static);
    assert!(!bp.predict(0x0));
    assert!(!bp.predict(0xFFFF_FFFF_FFFF_FFFF));
}

#[test]
fn bimodal_predict_for_different_pcs() {
    let bp = BranchPredictor::from_config(&BranchPredictorConfig::Bimodal { table_size: 16 });
    // Initial state is weakly not-taken — all should be false
    for i in 0..16u64 {
        assert!(!bp.predict(i * 4));
    }
}

#[test]
fn gshare_predict_does_not_panic() {
    let bp = BranchPredictor::from_config(&BranchPredictorConfig::GShare { history_bits: 8 });
    let _ = bp.predict(0x8000);
}

#[test]
fn tage_predict_does_not_panic() {
    let bp = BranchPredictor::from_config(&BranchPredictorConfig::TAGE { history_length: 16 });
    let _ = bp.predict(0x4000);
}

#[test]
fn tournament_predict_does_not_panic() {
    let bp = BranchPredictor::from_config(&BranchPredictorConfig::Tournament);
    let _ = bp.predict(0x2000);
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

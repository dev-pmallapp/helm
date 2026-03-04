use crate::core_sim::*;
use helm_core::config::{BranchPredictorConfig, CoreConfig};

fn test_core_config() -> CoreConfig {
    CoreConfig {
        name: "test-core".into(),
        width: 2,
        rob_size: 16,
        iq_size: 8,
        lq_size: 4,
        sq_size: 4,
        branch_predictor: BranchPredictorConfig::Static,
    }
}

#[test]
fn new_core_starts_at_cycle_zero() {
    let core = CoreSim::new(0, test_core_config());
    assert_eq!(core.cycle, 0);
    assert!(!core.halted);
}

#[test]
fn tick_advances_cycle() {
    let mut core = CoreSim::new(0, test_core_config());
    core.tick();
    assert_eq!(core.cycle, 1);
    core.tick();
    assert_eq!(core.cycle, 2);
}

#[test]
fn halted_core_does_not_advance() {
    let mut core = CoreSim::new(0, test_core_config());
    core.halted = true;
    let events = core.tick();
    assert!(events.is_empty());
    assert_eq!(core.cycle, 0);
}

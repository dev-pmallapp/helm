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

// --- new tests ---

#[test]
fn core_id_is_stored() {
    let core = CoreSim::new(7, test_core_config());
    assert_eq!(core.id, 7);
}

#[test]
fn core_starts_not_halted() {
    let core = CoreSim::new(0, test_core_config());
    assert!(!core.halted);
}

#[test]
fn tick_on_empty_rob_returns_no_events() {
    // Without any instructions in flight the ROB has nothing to commit,
    // so tick() should return an empty event list.
    let mut core = CoreSim::new(0, test_core_config());
    let events = core.tick();
    assert!(events.is_empty());
}

#[test]
fn multiple_sequential_ticks_monotone_cycle() {
    let mut core = CoreSim::new(0, test_core_config());
    let n = 50u64;
    for _ in 0..n {
        core.tick();
    }
    assert_eq!(core.cycle, n);
}

#[test]
fn halted_core_returns_empty_events_repeatedly() {
    let mut core = CoreSim::new(0, test_core_config());
    core.halted = true;
    for _ in 0..10 {
        let events = core.tick();
        assert!(events.is_empty());
    }
    assert_eq!(core.cycle, 0, "cycle must never advance while halted");
}

#[test]
fn core_can_be_halted_after_ticks() {
    let mut core = CoreSim::new(0, test_core_config());
    core.tick();
    core.tick();
    assert_eq!(core.cycle, 2);
    core.halted = true;
    core.tick();
    // cycle frozen after halt
    assert_eq!(core.cycle, 2);
}

#[test]
fn wide_core_config_constructs() {
    let cfg = CoreConfig {
        name: "wide".into(),
        width: 8,
        rob_size: 256,
        iq_size: 64,
        lq_size: 32,
        sq_size: 32,
        branch_predictor: BranchPredictorConfig::GShare { history_bits: 12 },
    };
    let core = CoreSim::new(3, cfg);
    assert_eq!(core.id, 3);
    assert_eq!(core.cycle, 0);
    assert!(!core.halted);
}

#[test]
fn narrow_core_config_constructs() {
    let cfg = CoreConfig {
        name: "narrow".into(),
        width: 1,
        rob_size: 4,
        iq_size: 4,
        lq_size: 2,
        sq_size: 2,
        branch_predictor: BranchPredictorConfig::Static,
    };
    let core = CoreSim::new(0, cfg);
    assert_eq!(core.cycle, 0);
}

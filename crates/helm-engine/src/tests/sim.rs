use crate::sim::*;
use helm_core::config::*;
use helm_core::types::{ExecMode, IsaKind};

fn test_platform() -> PlatformConfig {
    PlatformConfig {
        name: "test".into(),
        isa: IsaKind::RiscV64,
        exec_mode: ExecMode::CAE,
        cores: vec![CoreConfig {
            name: "c0".into(),
            width: 2,
            rob_size: 16,
            iq_size: 8,
            lq_size: 4,
            sq_size: 4,
            branch_predictor: BranchPredictorConfig::Static,
        }],
        memory: MemoryConfig {
            l1i: None,
            l1d: None,
            l2: None,
            l3: None,
            dram_latency_cycles: 100,
        },
    }
}

#[test]
fn simulation_constructs() {
    let sim = Simulation::new(test_platform(), "/dev/null".into());
    assert_eq!(sim.config.name, "test");
}

#[test]
fn microarch_run_completes() {
    let mut sim = Simulation::new(test_platform(), "/dev/null".into());
    let results = sim.run(100).unwrap();
    assert!(results.cycles <= 100);
}

#[test]
fn se_mode_run_returns_results() {
    let mut platform = test_platform();
    platform.exec_mode = ExecMode::SE;
    let mut sim = Simulation::new(platform, "/dev/null".into());
    let results = sim.run(10).unwrap();
    assert_eq!(results.cycles, 0); // stub returns empty
}

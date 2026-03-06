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

fn make_core(name: &str) -> CoreConfig {
    CoreConfig {
        name: name.into(),
        width: 2,
        rob_size: 16,
        iq_size: 8,
        lq_size: 4,
        sq_size: 4,
        branch_predictor: BranchPredictorConfig::Static,
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

// --- new tests ---

#[test]
fn binary_path_is_stored() {
    let sim = Simulation::new(test_platform(), "/some/binary".into());
    assert_eq!(sim.binary_path, "/some/binary");
}

#[test]
fn single_core_platform_has_one_core_in_config() {
    let sim = Simulation::new(test_platform(), "/dev/null".into());
    assert_eq!(sim.config.cores.len(), 1);
}

#[test]
fn multi_core_platform_constructs() {
    let mut platform = test_platform();
    platform.name = "multi".into();
    platform.cores = vec![make_core("c0"), make_core("c1"), make_core("c2"), make_core("c3")];
    let sim = Simulation::new(platform, "/dev/null".into());
    assert_eq!(sim.config.cores.len(), 4);
    assert_eq!(sim.config.name, "multi");
}

#[test]
fn multi_core_cae_run_completes() {
    let mut platform = test_platform();
    platform.cores = vec![make_core("c0"), make_core("c1")];
    let mut sim = Simulation::new(platform, "/dev/null".into());
    let results = sim.run(50).unwrap();
    // No instructions in flight — cycles reported may be 0 (all cores halt immediately)
    // or up to 50 — either is valid as long as run() succeeds.
    assert!(results.cycles <= 50);
}

#[test]
fn se_mode_with_arm64_isa_constructs() {
    let platform = PlatformConfig {
        name: "arm-se".into(),
        isa: IsaKind::Arm64,
        exec_mode: ExecMode::SE,
        cores: vec![make_core("c0")],
        memory: MemoryConfig {
            l1i: None,
            l1d: None,
            l2: None,
            l3: None,
            dram_latency_cycles: 50,
        },
    };
    let sim = Simulation::new(platform, "/dev/null".into());
    assert_eq!(sim.config.isa, IsaKind::Arm64);
    assert_eq!(sim.config.exec_mode, ExecMode::SE);
}

#[test]
fn zero_cycle_limit_returns_without_error() {
    let mut sim = Simulation::new(test_platform(), "/dev/null".into());
    // CAE mode with max_cycles = 0 should immediately return empty results.
    let results = sim.run(0).unwrap();
    assert_eq!(results.cycles, 0);
    assert_eq!(results.instructions_committed, 0);
}

#[test]
fn platform_config_name_is_preserved() {
    let mut platform = test_platform();
    platform.name = "custom-platform-name".into();
    let sim = Simulation::new(platform, "/dev/null".into());
    assert_eq!(sim.config.name, "custom-platform-name");
}

#[test]
fn platform_exec_mode_defaults_to_cae_in_test_helper() {
    let platform = test_platform();
    assert_eq!(platform.exec_mode, ExecMode::CAE);
}

#[test]
fn se_mode_run_with_zero_cycles_returns_empty() {
    let mut platform = test_platform();
    platform.exec_mode = ExecMode::SE;
    let mut sim = Simulation::new(platform, "/dev/null".into());
    let results = sim.run(0).unwrap();
    // SE stub always returns the default (empty) results regardless of limit.
    assert_eq!(results.instructions_committed, 0);
}

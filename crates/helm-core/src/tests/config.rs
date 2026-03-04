use crate::config::*;
use crate::types::{ExecMode, IsaKind};

fn sample_platform() -> PlatformConfig {
    PlatformConfig {
        name: "test-platform".into(),
        isa: IsaKind::RiscV64,
        exec_mode: ExecMode::Microarchitectural,
        cores: vec![CoreConfig {
            name: "core0".into(),
            width: 4,
            rob_size: 128,
            iq_size: 64,
            lq_size: 32,
            sq_size: 32,
            branch_predictor: BranchPredictorConfig::TAGE { history_length: 64 },
        }],
        memory: MemoryConfig {
            l1i: Some(CacheConfig {
                size: "32KB".into(),
                associativity: 8,
                latency_cycles: 1,
                line_size: 64,
            }),
            l1d: None,
            l2: None,
            l3: None,
            dram_latency_cycles: 100,
        },
    }
}

#[test]
fn platform_config_roundtrips_through_json() {
    let config = sample_platform();
    let json = serde_json::to_string(&config).unwrap();
    let back: PlatformConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.name, "test-platform");
    assert_eq!(back.cores.len(), 1);
    assert_eq!(back.cores[0].rob_size, 128);
}

#[test]
fn branch_predictor_variants_roundtrip() {
    let variants = vec![
        BranchPredictorConfig::Static,
        BranchPredictorConfig::Bimodal { table_size: 4096 },
        BranchPredictorConfig::GShare { history_bits: 16 },
        BranchPredictorConfig::TAGE { history_length: 64 },
        BranchPredictorConfig::Tournament,
    ];
    for bp in variants {
        let json = serde_json::to_string(&bp).unwrap();
        let back: BranchPredictorConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(
            serde_json::to_string(&bp).unwrap(),
            serde_json::to_string(&back).unwrap()
        );
    }
}

use crate::config::*;
use crate::types::{ExecMode, IsaKind};

fn sample_platform() -> PlatformConfig {
    PlatformConfig {
        name: "test-platform".into(),
        isa: IsaKind::RiscV64,
        exec_mode: ExecMode::FS,
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

#[test]
fn memory_config_with_all_levels_roundtrips() {
    let config = MemoryConfig {
        l1i: Some(CacheConfig {
            size: "32KB".into(),
            associativity: 8,
            latency_cycles: 1,
            line_size: 64,
        }),
        l1d: Some(CacheConfig {
            size: "32KB".into(),
            associativity: 8,
            latency_cycles: 1,
            line_size: 64,
        }),
        l2: Some(CacheConfig {
            size: "256KB".into(),
            associativity: 8,
            latency_cycles: 5,
            line_size: 64,
        }),
        l3: Some(CacheConfig {
            size: "8MB".into(),
            associativity: 16,
            latency_cycles: 20,
            line_size: 64,
        }),
        dram_latency_cycles: 200,
    };
    let json = serde_json::to_string(&config).unwrap();
    let back: MemoryConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.dram_latency_cycles, 200);
    assert!(back.l1i.is_some());
    assert!(back.l1d.is_some());
    assert!(back.l2.is_some());
    assert!(back.l3.is_some());
}

#[test]
fn core_config_roundtrips_through_json() {
    let cfg = CoreConfig {
        name: "core0".into(),
        width: 8,
        rob_size: 256,
        iq_size: 128,
        lq_size: 64,
        sq_size: 64,
        branch_predictor: BranchPredictorConfig::GShare { history_bits: 16 },
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let back: CoreConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.width, 8);
    assert_eq!(back.rob_size, 256);
    assert_eq!(back.name, "core0");
}

#[test]
fn platform_config_with_multiple_cores() {
    let make_core = |name: &str| CoreConfig {
        name: name.into(),
        width: 4,
        rob_size: 128,
        iq_size: 64,
        lq_size: 32,
        sq_size: 32,
        branch_predictor: BranchPredictorConfig::Static,
    };
    let config = PlatformConfig {
        name: "dual-core".into(),
        isa: IsaKind::Arm64,
        exec_mode: ExecMode::SE,
        cores: vec![make_core("core0"), make_core("core1")],
        memory: MemoryConfig {
            l1i: None,
            l1d: None,
            l2: None,
            l3: None,
            dram_latency_cycles: 50,
        },
    };
    let json = serde_json::to_string(&config).unwrap();
    let back: PlatformConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.cores.len(), 2);
    assert_eq!(back.cores[1].name, "core1");
}

#[test]
fn cache_config_line_size_preserved() {
    let cfg = CacheConfig {
        size: "64KB".into(),
        associativity: 4,
        latency_cycles: 3,
        line_size: 128,
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let back: CacheConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.line_size, 128);
    assert_eq!(back.associativity, 4);
}

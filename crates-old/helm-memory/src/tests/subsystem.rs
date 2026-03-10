use crate::MemorySubsystem;
use helm_core::config::{CacheConfig, MemoryConfig};

fn cache_cfg(size: &str) -> CacheConfig {
    CacheConfig {
        size: size.into(),
        associativity: 4,
        latency_cycles: 2,
        line_size: 64,
    }
}

#[test]
fn from_config_no_caches_creates_none_levels() {
    let config = MemoryConfig {
        l1i: None,
        l1d: None,
        l2: None,
        l3: None,
        dram_latency_cycles: 100,
    };
    let ms = MemorySubsystem::from_config(config);
    assert!(ms.l1i.is_none());
    assert!(ms.l1d.is_none());
    assert!(ms.l2.is_none());
    assert!(ms.l3.is_none());
}

#[test]
fn from_config_l1i_only() {
    let config = MemoryConfig {
        l1i: Some(cache_cfg("32KB")),
        l1d: None,
        l2: None,
        l3: None,
        dram_latency_cycles: 50,
    };
    let ms = MemorySubsystem::from_config(config);
    assert!(ms.l1i.is_some());
    assert!(ms.l1d.is_none());
}

#[test]
fn from_config_full_hierarchy() {
    let config = MemoryConfig {
        l1i: Some(cache_cfg("32KB")),
        l1d: Some(cache_cfg("32KB")),
        l2: Some(cache_cfg("256KB")),
        l3: Some(cache_cfg("8MB")),
        dram_latency_cycles: 200,
    };
    let ms = MemorySubsystem::from_config(config);
    assert!(ms.l1i.is_some());
    assert!(ms.l1d.is_some());
    assert!(ms.l2.is_some());
    assert!(ms.l3.is_some());
}

#[test]
fn dram_latency_preserved() {
    let config = MemoryConfig {
        l1i: None,
        l1d: None,
        l2: None,
        l3: None,
        dram_latency_cycles: 333,
    };
    let ms = MemorySubsystem::from_config(config);
    assert_eq!(ms.dram_latency, 333);
}

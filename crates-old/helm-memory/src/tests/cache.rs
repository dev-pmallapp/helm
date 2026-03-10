use crate::cache::parse_size;
use crate::cache::*;
use helm_core::config::CacheConfig;

fn small_cache() -> Cache {
    Cache::from_config(&CacheConfig {
        size: "1KB".into(),
        associativity: 2,
        latency_cycles: 1,
        line_size: 64,
    })
}

#[test]
fn first_access_is_miss() {
    let mut cache = small_cache();
    assert_eq!(cache.access(0x1000, false), CacheAccessResult::Miss);
}

#[test]
fn second_access_same_addr_is_hit() {
    let mut cache = small_cache();
    cache.access(0x1000, false);
    assert_eq!(cache.access(0x1000, false), CacheAccessResult::Hit);
}

#[test]
fn different_addresses_can_miss() {
    let mut cache = small_cache();
    cache.access(0x1000, false);
    // Access an address that maps to a different set.
    assert_eq!(cache.access(0x2000, false), CacheAccessResult::Miss);
}

#[test]
fn parse_size_works() {
    assert_eq!(parse_size("32KB"), 32 * 1024);
    assert_eq!(parse_size("8MB"), 8 * 1024 * 1024);
    assert_eq!(parse_size("1GB"), 1024 * 1024 * 1024);
    assert_eq!(parse_size("4096"), 4096);
}

#[test]
fn parse_size_lowercase_kb() {
    assert_eq!(parse_size("64kb"), 64 * 1024);
}

#[test]
fn parse_size_invalid_returns_zero() {
    assert_eq!(parse_size("notanumber"), 0);
}

#[test]
fn cache_access_result_variants_are_distinct() {
    assert_ne!(CacheAccessResult::Hit, CacheAccessResult::Miss);
}

#[test]
fn write_access_first_is_miss() {
    let mut cache = small_cache();
    assert_eq!(cache.access(0x1000, true), CacheAccessResult::Miss);
}

#[test]
fn write_then_read_same_addr_is_hit() {
    let mut cache = small_cache();
    cache.access(0x1000, true); // write miss — allocates
    assert_eq!(cache.access(0x1000, false), CacheAccessResult::Hit);
}

#[test]
fn direct_mapped_cache_hit() {
    let mut cache = Cache::from_config(&CacheConfig {
        size: "1KB".into(),
        associativity: 1, // direct mapped
        latency_cycles: 1,
        line_size: 64,
    });
    cache.access(0x1000, false);
    assert_eq!(cache.access(0x1000, false), CacheAccessResult::Hit);
}

#[test]
fn large_cache_many_sets() {
    let mut cache = Cache::from_config(&CacheConfig {
        size: "32KB".into(),
        associativity: 4,
        latency_cycles: 2,
        line_size: 64,
    });
    // Multiple non-conflicting addresses should all miss then hit
    let addrs: Vec<u64> = (0..8).map(|i| 0x1000 + i * 0x1000).collect();
    for &a in &addrs {
        assert_eq!(cache.access(a, false), CacheAccessResult::Miss);
    }
    for &a in &addrs {
        assert_eq!(cache.access(a, false), CacheAccessResult::Hit);
    }
}

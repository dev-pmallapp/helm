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

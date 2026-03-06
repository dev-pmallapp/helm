//! Tests for functional unit modeling

use crate::functional_units::{
    FunctionalUnit, FunctionalUnitConfig, FunctionalUnitPool, FunctionalUnitPoolBuilder,
    FunctionalUnitType,
};

#[test]
fn test_functional_unit_pipelined() {
    let mut fu = FunctionalUnit::new(FunctionalUnitType::IntAdder, 3, true);

    // Pipelined units are always available
    assert!(fu.is_available());
    fu.issue();
    assert!(fu.is_available());
    fu.issue();
    assert!(fu.is_available());
}

#[test]
fn test_functional_unit_non_pipelined() {
    let mut fu = FunctionalUnit::new(FunctionalUnitType::IntMultiplier, 3, false);

    // Non-pipelined units block after issue
    assert!(fu.is_available());
    fu.issue();
    assert!(!fu.is_available());

    // After 3 cycles, should be available again
    fu.tick();
    assert!(!fu.is_available());
    fu.tick();
    assert!(!fu.is_available());
    fu.tick();
    assert!(fu.is_available());
}

#[test]
fn test_unlimited_resources() {
    let mut pool = FunctionalUnitPool::new();

    // Default is unlimited resources
    for _ in 0..1000 {
        assert!(pool.try_allocate(FunctionalUnitType::IntAdder));
    }

    assert_eq!(pool.available_count(FunctionalUnitType::IntAdder), -1);
}

#[test]
fn test_limited_resources() {
    let mut pool = FunctionalUnitPool::new();
    pool.configure(
        FunctionalUnitType::IntAdder,
        FunctionalUnitConfig {
            count: 2,
            latency: 1,
            pipelined: false,
        },
    );

    // Should be able to allocate 2 units
    assert!(pool.try_allocate(FunctionalUnitType::IntAdder));
    assert!(pool.try_allocate(FunctionalUnitType::IntAdder));

    // Third allocation should fail
    assert!(!pool.try_allocate(FunctionalUnitType::IntAdder));

    // After a tick, units become available again
    pool.tick();
    assert!(pool.try_allocate(FunctionalUnitType::IntAdder));
}

#[test]
fn test_builder_pattern() {
    let pool = FunctionalUnitPoolBuilder::new()
        .with_int_adders(4, 1, true)
        .with_int_multipliers(2, 3, false)
        .with_fp_sp_multipliers(8, 5, true)
        .build();

    assert_eq!(pool.available_count(FunctionalUnitType::IntAdder), 4);
    assert_eq!(pool.available_count(FunctionalUnitType::IntMultiplier), 2);
    assert_eq!(
        pool.available_count(FunctionalUnitType::FPSPMultiplier),
        8
    );
}

#[test]
fn test_salam_style_unlimited() {
    let pool = FunctionalUnitPoolBuilder::new()
        .with_int_adders(-1, 1, true) // -1 = unlimited (gem5-SALAM convention)
        .build();

    assert_eq!(pool.available_count(FunctionalUnitType::IntAdder), -1);
}

#[test]
fn test_functional_unit_config_default_fields() {
    let cfg = FunctionalUnitConfig::default();
    assert_eq!(cfg.count, -1);
    assert_eq!(cfg.latency, 1);
    assert!(cfg.pipelined);
}

#[test]
fn test_functional_unit_pipeline_depth_pipelined() {
    let fu = FunctionalUnit::new(FunctionalUnitType::FPSPMultiplier, 5, true);
    assert_eq!(fu.pipeline_depth, 5);
    assert_eq!(fu.busy_cycles, 0);
}

#[test]
fn test_functional_unit_pipeline_depth_non_pipelined() {
    let fu = FunctionalUnit::new(FunctionalUnitType::IntDivider, 10, false);
    assert_eq!(fu.pipeline_depth, 0);
}

#[test]
fn test_functional_unit_tick_decrements_busy_cycles() {
    let mut fu = FunctionalUnit::new(FunctionalUnitType::IntDivider, 3, false);
    fu.issue();
    assert_eq!(fu.busy_cycles, 3);
    fu.tick();
    assert_eq!(fu.busy_cycles, 2);
    fu.tick();
    assert_eq!(fu.busy_cycles, 1);
    fu.tick();
    assert_eq!(fu.busy_cycles, 0);
    assert!(fu.is_available());
}

#[test]
fn test_pool_tick_frees_non_pipelined_unit() {
    let mut pool = FunctionalUnitPool::new();
    pool.configure(
        FunctionalUnitType::IntDivider,
        FunctionalUnitConfig {
            count: 1,
            latency: 2,
            pipelined: false,
        },
    );

    assert!(pool.try_allocate(FunctionalUnitType::IntDivider));
    assert!(!pool.try_allocate(FunctionalUnitType::IntDivider));

    pool.tick();
    assert!(!pool.try_allocate(FunctionalUnitType::IntDivider)); // still busy (latency=2)

    pool.tick();
    assert!(pool.try_allocate(FunctionalUnitType::IntDivider)); // now free
}

#[test]
fn test_available_count_pipelined_always_all_available() {
    let mut pool = FunctionalUnitPool::new();
    pool.configure(
        FunctionalUnitType::FPSPAdder,
        FunctionalUnitConfig {
            count: 4,
            latency: 5,
            pipelined: true,
        },
    );

    // Pipelined units always show all as available
    assert_eq!(pool.available_count(FunctionalUnitType::FPSPAdder), 4);
    pool.try_allocate(FunctionalUnitType::FPSPAdder);
    pool.try_allocate(FunctionalUnitType::FPSPAdder);
    assert_eq!(pool.available_count(FunctionalUnitType::FPSPAdder), 4);
}

#[test]
fn test_available_count_non_pipelined_decrements() {
    let mut pool = FunctionalUnitPool::new();
    pool.configure(
        FunctionalUnitType::IntMultiplier,
        FunctionalUnitConfig {
            count: 3,
            latency: 3,
            pipelined: false,
        },
    );

    assert_eq!(pool.available_count(FunctionalUnitType::IntMultiplier), 3);
    pool.try_allocate(FunctionalUnitType::IntMultiplier);
    assert_eq!(pool.available_count(FunctionalUnitType::IntMultiplier), 2);
    pool.try_allocate(FunctionalUnitType::IntMultiplier);
    assert_eq!(pool.available_count(FunctionalUnitType::IntMultiplier), 1);
    pool.try_allocate(FunctionalUnitType::IntMultiplier);
    assert_eq!(pool.available_count(FunctionalUnitType::IntMultiplier), 0);
}

#[test]
fn test_builder_fp_dp_multipliers() {
    let pool = FunctionalUnitPoolBuilder::new()
        .with_fp_dp_multipliers(4, 6, true)
        .build();

    assert_eq!(pool.available_count(FunctionalUnitType::FPDPMultiplier), 4);
}

#[test]
fn test_builder_load_store_units() {
    let pool = FunctionalUnitPoolBuilder::new()
        .with_load_store_units(2, 3)
        .build();

    assert_eq!(pool.available_count(FunctionalUnitType::LoadStore), 2);
}

#[test]
fn test_builder_unlimited_passthrough() {
    // unlimited() is a no-op that returns self — just ensure it compiles and chain works
    let pool = FunctionalUnitPoolBuilder::new()
        .unlimited()
        .with_int_adders(2, 1, true)
        .build();

    assert_eq!(pool.available_count(FunctionalUnitType::IntAdder), 2);
}

#[test]
fn test_pool_default_is_same_as_new() {
    let pool_default = FunctionalUnitPool::default();
    // Unregistered type should report unlimited
    assert_eq!(pool_default.available_count(FunctionalUnitType::Compare), -1);
}

#[test]
fn test_unconfigured_type_is_unlimited() {
    let mut pool = FunctionalUnitPool::new();
    pool.configure(
        FunctionalUnitType::IntAdder,
        FunctionalUnitConfig {
            count: 1,
            latency: 1,
            pipelined: false,
        },
    );

    // A type that was never configured should behave as unlimited
    assert_eq!(pool.available_count(FunctionalUnitType::Branch), -1);
    assert!(pool.try_allocate(FunctionalUnitType::Branch));
}

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

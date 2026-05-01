//! Verify that with the `stats` feature disabled (the default for
//! release builds), every public type collapses to a ZST and the hot-
//! path methods compile to nothing.
//!
//! Run with: `cargo test -p helm-stats --no-default-features
//!           --test feature_gate_off`
//!
//! The whole file is gated `#[cfg(not(feature = "stats"))]` so it is
//! a no-op when run with `--features=stats` (i.e. the workspace's
//! default test pass with default features won't fail it).

#![cfg(not(feature = "stats"))]

use helm_stats::{
    LabelCounter, PerfCounter, PerfHistogram, StatsProducer, StatsRegistry, StatsScope,
};

#[test]
fn counter_is_zst() {
    assert_eq!(std::mem::size_of::<PerfCounter>(), 0);
}

#[test]
fn histogram_is_zst() {
    assert_eq!(std::mem::size_of::<PerfHistogram>(), 0);
}

#[test]
fn label_counter_is_zst() {
    assert_eq!(std::mem::size_of::<LabelCounter>(), 0);
}

#[test]
fn registry_is_zst() {
    assert_eq!(std::mem::size_of::<StatsRegistry>(), 0);
}

#[test]
fn counter_inc_compiles_to_nothing() {
    let c = PerfCounter::new();
    for _ in 0..1_000_000 {
        c.inc();
        c.add(7);
    }
    assert_eq!(c.get(), 0, "no-op counter must always report 0");
}

#[test]
fn histogram_record_compiles_to_nothing() {
    let h = PerfHistogram::new(vec![10, 100, 1000]);
    for v in 0..1_000 {
        h.record(v);
    }
    assert!(h.counts().is_empty());
    assert!(h.boundaries().is_empty());
}

#[test]
fn label_counter_bump_compiles_to_nothing() {
    let lc = LabelCounter::new();
    for _ in 0..1_000 {
        lc.bump_static("alpha");
        lc.bump_dynamic(String::from("beta"));
    }
    assert_eq!(lc.total(), 0);
    assert_eq!(lc.cardinality(), 0);
    assert!(lc.snapshot().is_empty());
}

#[test]
fn registry_dump_json_is_empty_object() {
    let mut r = StatsRegistry::new();
    let _c = r.counter("system.cpu0.cycles", "cycles");
    let _h = r.histogram("system.cpu0.icache.latency", "latency", &[1, 4, 16]);
    assert_eq!(r.dump_json(), "{}");
}

#[test]
fn stats_scope_is_zst() {
    assert_eq!(std::mem::size_of::<StatsScope<'_>>(), 0);
}

/// A trivial newtype implementor; the orphan rule blocks `impl … for ()`.
/// With `stats` off this method must compile to nothing and perform
/// zero allocations on the heap.
struct NullProducer;
impl StatsProducer for NullProducer {
    fn register_stats(&self, _scope: &mut StatsScope<'_>) {}
}

#[test]
fn trivial_stats_producer_is_callable() {
    let mut reg = StatsRegistry::new();
    let mut scope = StatsScope::new(&mut reg, "system.cpu0");
    NullProducer.register_stats(&mut scope);
    // Registry stays a ZST, so dump must remain the empty object.
    assert_eq!(reg.dump_json(), "{}");
    // And the producer itself is a ZST.
    assert_eq!(std::mem::size_of::<NullProducer>(), 0);
}

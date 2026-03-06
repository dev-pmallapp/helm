use crate::temporal::*;

#[test]
fn global_time_is_min() {
    let td = TemporalDecoupler::new(2, 1000);
    td.advance_core(0, 500);
    td.advance_core(1, 200);
    assert_eq!(td.global_time(), 200);
}

#[test]
fn needs_sync_when_ahead_by_quantum() {
    let td = TemporalDecoupler::new(2, 100);
    td.advance_core(0, 150);
    // core 1 is at 0, so core 0 is 150 ahead — exceeds quantum of 100.
    assert!(td.needs_sync(0));
    assert!(!td.needs_sync(1));
}

#[test]
fn needs_sync_false_when_within_quantum() {
    let td = TemporalDecoupler::new(2, 1000);
    td.advance_core(0, 50); // only 50 ahead of core 1 (at 0) — within 1000 quantum
    assert!(!td.needs_sync(0));
}

#[test]
fn global_time_both_cores_equal() {
    let td = TemporalDecoupler::new(2, 100);
    td.advance_core(0, 300);
    td.advance_core(1, 300);
    assert_eq!(td.global_time(), 300);
}

#[test]
fn decoupler_with_one_core_global_time() {
    let td = TemporalDecoupler::new(1, 1000);
    td.advance_core(0, 500);
    assert_eq!(td.global_time(), 500);
}

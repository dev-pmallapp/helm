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

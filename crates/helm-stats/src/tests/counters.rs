use crate::counters::*;

#[test]
fn new_counter_starts_at_zero() {
    let c = Counter::new("test");
    assert_eq!(c.get(), 0);
}

#[test]
fn increment_adds_one() {
    let c = Counter::new("test");
    c.increment();
    c.increment();
    assert_eq!(c.get(), 2);
}

#[test]
fn add_increases_by_n() {
    let c = Counter::new("test");
    c.add(100);
    assert_eq!(c.get(), 100);
}

#[test]
fn reset_clears_value() {
    let c = Counter::new("test");
    c.add(50);
    c.reset();
    assert_eq!(c.get(), 0);
}

#[test]
fn counter_name_stored() {
    let c = Counter::new("my_counter");
    assert_eq!(c.name, "my_counter");
}

#[test]
fn counter_add_zero_no_change() {
    let c = Counter::new("x");
    c.add(0);
    assert_eq!(c.get(), 0);
}

#[test]
fn counter_multiple_increments_and_reset() {
    let c = Counter::new("x");
    for _ in 0..100 {
        c.increment();
    }
    assert_eq!(c.get(), 100);
    c.reset();
    assert_eq!(c.get(), 0);
}

use crate::irq::*;

#[test]
fn new_lines_start_low() {
    let ctrl = IrqController::new(8);
    assert!(!ctrl.has_pending());
    assert!(ctrl.pending().is_empty());
}

#[test]
fn assert_and_deassert() {
    let mut ctrl = IrqController::new(8);
    ctrl.assert(3);
    assert!(ctrl.has_pending());
    assert_eq!(ctrl.pending(), vec![3]);

    ctrl.deassert(3);
    assert!(!ctrl.has_pending());
}

#[test]
fn multiple_pending() {
    let mut ctrl = IrqController::new(8);
    ctrl.assert(1);
    ctrl.assert(5);
    let pending = ctrl.pending();
    assert!(pending.contains(&1));
    assert!(pending.contains(&5));
    assert_eq!(pending.len(), 2);
}

#[test]
fn irq_line_new_starts_low() {
    let line = IrqLine::new(0, "irq0");
    assert!(!line.is_asserted());
    assert_eq!(line.state, IrqState::Low);
}

#[test]
fn irq_line_assert_changes_to_high() {
    let mut line = IrqLine::new(1, "irq1");
    line.assert();
    assert!(line.is_asserted());
    assert_eq!(line.state, IrqState::High);
}

#[test]
fn irq_line_deassert_returns_to_low() {
    let mut line = IrqLine::new(2, "irq2");
    line.assert();
    line.deassert();
    assert!(!line.is_asserted());
    assert_eq!(line.state, IrqState::Low);
}

#[test]
fn irq_state_variants_are_distinct() {
    assert_ne!(IrqState::Low, IrqState::High);
}

#[test]
fn irq_controller_default_has_pending_false() {
    let ctrl = IrqController::default();
    assert!(!ctrl.has_pending());
}

#[test]
fn assert_out_of_range_does_not_panic() {
    let mut ctrl = IrqController::new(4);
    ctrl.assert(100); // out of range — should silently do nothing
    assert!(!ctrl.has_pending());
}

#[test]
fn deassert_out_of_range_does_not_panic() {
    let mut ctrl = IrqController::new(4);
    ctrl.deassert(100); // out of range
    assert!(!ctrl.has_pending());
}

#[test]
fn assert_same_line_twice_still_one_pending() {
    let mut ctrl = IrqController::new(8);
    ctrl.assert(2);
    ctrl.assert(2);
    assert_eq!(ctrl.pending().len(), 1);
}

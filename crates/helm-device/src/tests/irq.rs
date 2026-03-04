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

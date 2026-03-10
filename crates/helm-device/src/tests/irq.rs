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

// ── IrqRouter remove_route tests ────────────────────────────────────────────

#[test]
fn router_add_route_returns_index() {
    let mut router = IrqRouter::new();
    let idx = router.add_route(IrqRoute {
        source_device: 1,
        source_line: 0,
        dest_controller: 0,
        dest_irq: 33,
    });
    assert_eq!(idx, 0);
    assert_eq!(router.routes().len(), 1);
}

#[test]
fn router_remove_route() {
    let mut router = IrqRouter::new();
    let idx = router.add_route(IrqRoute {
        source_device: 1,
        source_line: 0,
        dest_controller: 0,
        dest_irq: 33,
    });
    let removed = router.remove_route(idx);
    assert!(removed.is_some());
    assert_eq!(removed.unwrap().dest_irq, 33);
    assert!(router.routes().is_empty());
}

#[test]
fn router_remove_route_out_of_range() {
    let mut router = IrqRouter::new();
    assert!(router.remove_route(99).is_none());
}

#[test]
fn router_remove_routes_for_device() {
    let mut router = IrqRouter::new();
    router.add_route(IrqRoute {
        source_device: 1,
        source_line: 0,
        dest_controller: 0,
        dest_irq: 33,
    });
    router.add_route(IrqRoute {
        source_device: 1,
        source_line: 1,
        dest_controller: 0,
        dest_irq: 34,
    });
    router.add_route(IrqRoute {
        source_device: 2,
        source_line: 0,
        dest_controller: 0,
        dest_irq: 35,
    });

    router.remove_routes_for_device(1);
    assert_eq!(router.routes().len(), 1);
    assert_eq!(router.routes()[0].source_device, 2);
}

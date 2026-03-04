use crate::sampling::*;

#[test]
fn phases_progress_in_order() {
    let mut sc = SamplingController::new(100, 50, 200, 10);
    assert_eq!(sc.phase(), SamplingPhase::FastForward);
    sc.advance(100);
    assert_eq!(sc.phase(), SamplingPhase::Warmup);
    sc.advance(50);
    assert_eq!(sc.phase(), SamplingPhase::Detailed);
    sc.advance(200);
    assert_eq!(sc.phase(), SamplingPhase::Cooldown);
    sc.advance(10);
    assert_eq!(sc.phase(), SamplingPhase::Done);
}

#[test]
fn partial_advance_stays_in_phase() {
    let mut sc = SamplingController::new(1000, 100, 500, 50);
    sc.advance(500);
    assert_eq!(sc.phase(), SamplingPhase::FastForward);
}

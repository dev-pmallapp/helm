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

#[test]
fn advance_exactly_to_phase_boundary_transitions() {
    let mut sc = SamplingController::new(100, 50, 200, 10);
    sc.advance(100); // exactly FF duration
    assert_eq!(sc.phase(), SamplingPhase::Warmup);
}

#[test]
fn done_phase_stays_done_on_further_advance() {
    let mut sc = SamplingController::new(1, 1, 1, 1);
    sc.advance(1); // FF done
    sc.advance(1); // Warmup done
    sc.advance(1); // Detailed done
    sc.advance(1); // Cooldown done
    assert_eq!(sc.phase(), SamplingPhase::Done);
    sc.advance(9999); // should stay Done
    assert_eq!(sc.phase(), SamplingPhase::Done);
}

#[test]
fn sampling_phase_variants_are_distinct() {
    let phases = [
        SamplingPhase::FastForward,
        SamplingPhase::Warmup,
        SamplingPhase::Detailed,
        SamplingPhase::Cooldown,
        SamplingPhase::Done,
    ];
    for i in 0..phases.len() {
        for j in 0..phases.len() {
            if i == j {
                assert_eq!(phases[i], phases[j]);
            } else {
                assert_ne!(phases[i], phases[j]);
            }
        }
    }
}

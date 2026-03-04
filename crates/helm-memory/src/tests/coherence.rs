use crate::coherence::*;

#[test]
fn controller_constructs() {
    let _cc = CoherenceController::new();
}

#[test]
fn states_are_distinct() {
    assert_ne!(CoherenceState::Modified, CoherenceState::Invalid);
    assert_ne!(CoherenceState::Shared, CoherenceState::Exclusive);
}

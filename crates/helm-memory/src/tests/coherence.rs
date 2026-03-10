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

#[test]
fn all_coherence_states_are_distinct() {
    let states = [
        CoherenceState::Modified,
        CoherenceState::Owned,
        CoherenceState::Exclusive,
        CoherenceState::Shared,
        CoherenceState::Invalid,
    ];
    for i in 0..states.len() {
        for j in 0..states.len() {
            if i == j {
                assert_eq!(states[i], states[j]);
            } else {
                assert_ne!(states[i], states[j]);
            }
        }
    }
}

#[test]
fn coherence_controller_default_constructs() {
    let _cc = CoherenceController::default();
}

#[test]
fn directory_entry_construction() {
    let entry = DirectoryEntry {
        addr: 0xCAFE_0000,
        state: CoherenceState::Shared,
        sharers: vec![0, 1, 2],
    };
    assert_eq!(entry.addr, 0xCAFE_0000);
    assert_eq!(entry.state, CoherenceState::Shared);
    assert_eq!(entry.sharers.len(), 3);
}

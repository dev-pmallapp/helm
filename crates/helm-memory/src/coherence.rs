//! Simple MOESI-style cache coherence protocol (stub).

use helm_core::types::Addr;

/// Line state in the coherence protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoherenceState {
    Modified,
    Owned,
    Exclusive,
    Shared,
    Invalid,
}

/// Coherence directory entry.
#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    pub addr: Addr,
    pub state: CoherenceState,
    pub sharers: Vec<usize>, // core IDs
}

/// Stub coherence controller.
pub struct CoherenceController {
    _entries: Vec<DirectoryEntry>,
}

impl Default for CoherenceController {
    fn default() -> Self {
        Self::new()
    }
}

impl CoherenceController {
    pub fn new() -> Self {
        Self {
            _entries: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_constructs() {
        let _cc = CoherenceController::new();
    }

    #[test]
    fn states_are_distinct() {
        assert_ne!(CoherenceState::Modified, CoherenceState::Invalid);
        assert_ne!(CoherenceState::Shared, CoherenceState::Exclusive);
    }
}

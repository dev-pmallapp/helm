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

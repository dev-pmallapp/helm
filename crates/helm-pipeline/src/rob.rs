//! Reorder Buffer — tracks in-flight instructions and enforces in-order commit.

use helm_core::ir::MicroOp;

/// State of a single ROB entry.
#[derive(Debug, Clone)]
pub enum RobEntryState {
    Dispatched,
    Executing,
    Complete,
    Faulted(String),
}

#[derive(Debug, Clone)]
pub struct RobEntry {
    pub uop: MicroOp,
    pub state: RobEntryState,
}

pub struct ReorderBuffer {
    capacity: usize,
    entries: Vec<Option<RobEntry>>,
    head: usize,
    tail: usize,
}

impl ReorderBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: (0..capacity).map(|_| None).collect(),
            head: 0,
            tail: 0,
        }
    }

    pub fn is_full(&self) -> bool {
        (self.tail + 1) % self.capacity == self.head
    }

    pub fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    /// Allocate a new ROB entry. Returns the ROB index.
    pub fn allocate(&mut self, uop: MicroOp) -> Option<usize> {
        if self.is_full() {
            return None;
        }
        let idx = self.tail;
        self.entries[idx] = Some(RobEntry {
            uop,
            state: RobEntryState::Dispatched,
        });
        self.tail = (self.tail + 1) % self.capacity;
        Some(idx)
    }

    /// Mark an entry as completed.
    pub fn complete(&mut self, idx: usize) {
        if let Some(entry) = &mut self.entries[idx] {
            entry.state = RobEntryState::Complete;
        }
    }

    /// Try to commit entries from the head of the ROB.
    pub fn try_commit(&mut self) -> Vec<MicroOp> {
        let mut committed = Vec::new();
        while !self.is_empty() {
            if let Some(entry) = &self.entries[self.head] {
                if matches!(entry.state, RobEntryState::Complete) {
                    let entry = self.entries[self.head].take().unwrap();
                    committed.push(entry.uop);
                    self.head = (self.head + 1) % self.capacity;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        committed
    }
}

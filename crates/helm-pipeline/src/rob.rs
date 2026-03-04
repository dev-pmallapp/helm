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

#[cfg(test)]
mod tests {
    use super::*;
    use helm_core::ir::{MicroOp, MicroOpFlags, Opcode};

    fn make_uop(pc: u64) -> MicroOp {
        MicroOp {
            guest_pc: pc,
            opcode: Opcode::IntAlu,
            sources: vec![],
            dest: None,
            immediate: None,
            flags: MicroOpFlags::default(),
        }
    }

    #[test]
    fn new_rob_is_empty() {
        let rob = ReorderBuffer::new(8);
        assert!(rob.is_empty());
        assert!(!rob.is_full());
    }

    #[test]
    fn allocate_returns_index() {
        let mut rob = ReorderBuffer::new(4);
        let idx = rob.allocate(make_uop(0x100));
        assert_eq!(idx, Some(0));
        assert!(!rob.is_empty());
    }

    #[test]
    fn full_rob_rejects_allocation() {
        let mut rob = ReorderBuffer::new(3); // capacity 3 means 2 usable slots
        rob.allocate(make_uop(0x100));
        rob.allocate(make_uop(0x104));
        assert!(rob.is_full());
        assert_eq!(rob.allocate(make_uop(0x108)), None);
    }

    #[test]
    fn commit_in_order() {
        let mut rob = ReorderBuffer::new(8);
        let i0 = rob.allocate(make_uop(0x100)).unwrap();
        let i1 = rob.allocate(make_uop(0x104)).unwrap();

        // Complete second entry first — should not commit yet.
        rob.complete(i1);
        let committed = rob.try_commit();
        assert!(
            committed.is_empty(),
            "out-of-order complete should not commit"
        );

        // Complete first entry — both should commit now.
        rob.complete(i0);
        let committed = rob.try_commit();
        assert_eq!(committed.len(), 2);
        assert_eq!(committed[0].guest_pc, 0x100);
        assert_eq!(committed[1].guest_pc, 0x104);
    }
}

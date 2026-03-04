//! Instruction scheduler / issue queue.

use helm_core::ir::MicroOp;

pub struct SchedulerEntry {
    pub uop: MicroOp,
    pub rob_idx: usize,
    pub ready: bool,
}

pub struct Scheduler {
    capacity: usize,
    entries: Vec<SchedulerEntry>,
}

impl Scheduler {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: Vec::with_capacity(capacity),
        }
    }

    pub fn is_full(&self) -> bool {
        self.entries.len() >= self.capacity
    }

    /// Insert a micro-op into the issue queue.
    pub fn insert(&mut self, uop: MicroOp, rob_idx: usize) -> bool {
        if self.is_full() {
            return false;
        }
        self.entries.push(SchedulerEntry {
            uop,
            rob_idx,
            ready: false,
        });
        true
    }

    /// Wake entries whose sources are now available.
    pub fn wakeup(&mut self, _completed_phys_regs: &[u32]) {
        // Stub: mark all as ready for now.
        for entry in &mut self.entries {
            entry.ready = true;
        }
    }

    /// Select and remove ready entries for issue (up to `width`).
    pub fn select(&mut self, width: usize) -> Vec<SchedulerEntry> {
        let mut issued = Vec::new();
        let mut remaining = Vec::new();
        for entry in self.entries.drain(..) {
            if entry.ready && issued.len() < width {
                issued.push(entry);
            } else {
                remaining.push(entry);
            }
        }
        self.entries = remaining;
        issued
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_core::ir::{MicroOp, MicroOpFlags, Opcode};

    fn make_uop() -> MicroOp {
        MicroOp {
            guest_pc: 0,
            opcode: Opcode::IntAlu,
            sources: vec![],
            dest: None,
            immediate: None,
            flags: MicroOpFlags::default(),
        }
    }

    #[test]
    fn empty_scheduler_selects_nothing() {
        let mut sched = Scheduler::new(4);
        let issued = sched.select(4);
        assert!(issued.is_empty());
    }

    #[test]
    fn insert_respects_capacity() {
        let mut sched = Scheduler::new(2);
        assert!(sched.insert(make_uop(), 0));
        assert!(sched.insert(make_uop(), 1));
        assert!(!sched.insert(make_uop(), 2), "should reject when full");
    }

    #[test]
    fn wakeup_and_select_issues_ready() {
        let mut sched = Scheduler::new(4);
        sched.insert(make_uop(), 0);
        sched.insert(make_uop(), 1);
        sched.wakeup(&[]);
        let issued = sched.select(1);
        assert_eq!(issued.len(), 1, "should respect width limit");
    }
}

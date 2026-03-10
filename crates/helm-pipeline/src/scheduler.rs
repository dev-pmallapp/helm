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

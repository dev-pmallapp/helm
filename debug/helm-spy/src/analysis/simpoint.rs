//! SimPoint Basic Block Vector (BBV) computation.
//!
//! Tracks basic block execution by observing branch events. At each interval
//! boundary, emits a BasicBlockVector for clustering with the SimPoint tool.

use std::collections::HashMap;

/// Basic Block Vector — frequency counts of basic blocks in one interval.
#[derive(Debug, Clone)]
pub struct BasicBlockVector {
    pub interval: u64,
    pub counts: HashMap<u64, u64>,
    pub total_insns: u64,
}

impl BasicBlockVector {
    fn new(interval: u64) -> Self {
        Self {
            interval,
            counts: HashMap::new(),
            total_insns: 0,
        }
    }

    pub fn normalized(&self) -> HashMap<u64, f64> {
        if self.total_insns == 0 {
            return HashMap::new();
        }
        self.counts
            .iter()
            .map(|(&pc, &count)| (pc, count as f64 / self.total_insns as f64))
            .collect()
    }

    pub fn num_blocks(&self) -> usize {
        self.counts.len()
    }
}

/// SimPoint BBV collector.
pub struct SimPointCollector {
    interval_size: u64,
    current_interval: u64,
    insns_in_interval: u64,
    current_bb_start: u64,
    current_bb_insns: u64,
    current_bbv: HashMap<u64, u64>,
    completed: Vec<BasicBlockVector>,
}

impl SimPointCollector {
    pub fn new(interval_size: u64) -> Self {
        Self {
            interval_size,
            current_interval: 0,
            insns_in_interval: 0,
            current_bb_start: 0,
            current_bb_insns: 0,
            current_bbv: HashMap::new(),
            completed: Vec::new(),
        }
    }

    pub fn on_branch(&mut self, _pc: u64, target: u64, taken: bool) {
        if self.current_bb_insns > 0 {
            *self.current_bbv.entry(self.current_bb_start).or_insert(0) += self.current_bb_insns;
            self.insns_in_interval += self.current_bb_insns;
        }
        self.current_bb_start = if taken { target } else { _pc + 4 };
        self.current_bb_insns = 0;
        while self.insns_in_interval >= self.interval_size {
            self.flush_interval();
        }
    }

    pub fn on_insn(&mut self, pc: u64) {
        if self.current_bb_insns == 0 {
            self.current_bb_start = pc;
        }
        self.current_bb_insns += 1;
    }

    fn flush_interval(&mut self) {
        let mut bbv = BasicBlockVector::new(self.current_interval);
        bbv.counts = std::mem::take(&mut self.current_bbv);
        bbv.total_insns = self.insns_in_interval.min(self.interval_size);
        self.completed.push(bbv);
        self.insns_in_interval -= self.interval_size;
        self.current_interval += 1;
    }

    pub fn finish(&mut self) {
        if self.current_bb_insns > 0 {
            *self.current_bbv.entry(self.current_bb_start).or_insert(0) += self.current_bb_insns;
            self.insns_in_interval += self.current_bb_insns;
            self.current_bb_insns = 0;
        }
        if self.insns_in_interval > 0 {
            let mut bbv = BasicBlockVector::new(self.current_interval);
            bbv.counts = std::mem::take(&mut self.current_bbv);
            bbv.total_insns = self.insns_in_interval;
            self.completed.push(bbv);
            self.insns_in_interval = 0;
            self.current_interval += 1;
        }
    }

    pub fn completed_intervals(&self) -> &[BasicBlockVector] {
        &self.completed
    }
    pub fn num_intervals(&self) -> u64 {
        self.completed.len() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_collection() {
        let mut c = SimPointCollector::new(10);
        for i in 0..5 {
            c.on_insn(0x1000 + i * 4);
        }
        c.on_branch(0x1010, 0x2000, true);
        for i in 0..10 {
            c.on_insn(0x2000 + i * 4);
        }
        c.on_branch(0x2024, 0x3000, true);
        c.finish();
        assert!(c.num_intervals() >= 1);
    }

    #[test]
    fn normalized_sums_to_one() {
        let mut bbv = BasicBlockVector::new(0);
        bbv.counts.insert(0x1000, 30);
        bbv.counts.insert(0x2000, 70);
        bbv.total_insns = 100;
        let sum: f64 = bbv.normalized().values().sum();
        assert!((sum - 1.0).abs() < 1e-10);
    }
}

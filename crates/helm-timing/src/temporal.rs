//! Temporal decoupling for multi-core simulation.

use std::sync::atomic::{AtomicU64, Ordering};

/// Per-core timing state for temporal decoupling.
pub struct CoreTiming {
    pub core_id: usize,
    virtual_time: AtomicU64,
}

impl CoreTiming {
    pub fn new(core_id: usize) -> Self {
        Self {
            core_id,
            virtual_time: AtomicU64::new(0),
        }
    }

    pub fn advance(&self, cycles: u64) {
        self.virtual_time.fetch_add(cycles, Ordering::Relaxed);
    }

    pub fn time(&self) -> u64 {
        self.virtual_time.load(Ordering::Relaxed)
    }
}

/// Manages quantum-based synchronisation across cores.
pub struct TemporalDecoupler {
    /// Maximum virtual-time skew allowed between cores.
    pub quantum_size: u64,
    cores: Vec<CoreTiming>,
}

impl TemporalDecoupler {
    pub fn new(num_cores: usize, quantum_size: u64) -> Self {
        let cores = (0..num_cores).map(CoreTiming::new).collect();
        Self {
            quantum_size,
            cores,
        }
    }

    /// Global virtual time = min across all cores.
    pub fn global_time(&self) -> u64 {
        self.cores.iter().map(|c| c.time()).min().unwrap_or(0)
    }

    /// Has a core exceeded its quantum?
    pub fn needs_sync(&self, core_id: usize) -> bool {
        let core_time = self.cores[core_id].time();
        let global = self.global_time();
        core_time >= global + self.quantum_size
    }

    /// Advance a core's virtual time.
    pub fn advance_core(&self, core_id: usize, cycles: u64) {
        self.cores[core_id].advance(cycles);
    }

    /// Number of cores tracked.
    pub fn num_cores(&self) -> usize {
        self.cores.len()
    }
}

//! Central statistics collector that observes simulation events.

use helm_core::event::{EventObserver, SimEvent};
use serde::Serialize;
use std::collections::HashMap;

/// Aggregated simulation results.
#[derive(Debug, Clone, Serialize, Default)]
pub struct SimResults {
    pub cycles: u64,
    pub instructions_committed: u64,
    pub branches: u64,
    pub branch_mispredictions: u64,
    pub cache_accesses: HashMap<u8, (u64, u64)>, // level -> (hits, misses)
}

impl SimResults {
    pub fn ipc(&self) -> f64 {
        if self.cycles == 0 {
            0.0
        } else {
            self.instructions_committed as f64 / self.cycles as f64
        }
    }

    pub fn branch_mpki(&self) -> f64 {
        if self.instructions_committed == 0 {
            0.0
        } else {
            self.branch_mispredictions as f64 / (self.instructions_committed as f64 / 1000.0)
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

/// Collects events and produces aggregated `SimResults`.
pub struct StatsCollector {
    pub results: SimResults,
}

impl Default for StatsCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl StatsCollector {
    pub fn new() -> Self {
        Self {
            results: SimResults::default(),
        }
    }
}

impl EventObserver for StatsCollector {
    fn on_event(&mut self, event: &SimEvent) {
        match event {
            SimEvent::InsnCommit { cycle, .. } => {
                self.results.instructions_committed += 1;
                self.results.cycles = *cycle;
            }
            SimEvent::BranchResolved {
                predicted, taken, ..
            } => {
                self.results.branches += 1;
                if predicted != taken {
                    self.results.branch_mispredictions += 1;
                }
            }
            SimEvent::CacheAccess { level, hit, .. } => {
                let entry = self.results.cache_accesses.entry(*level).or_insert((0, 0));
                if *hit {
                    entry.0 += 1;
                } else {
                    entry.1 += 1;
                }
            }
            _ => {}
        }
    }
}

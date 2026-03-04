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

#[cfg(test)]
mod tests {
    use super::*;
    use helm_core::event::SimEvent;

    #[test]
    fn empty_results_have_zero_ipc() {
        let r = SimResults::default();
        assert_eq!(r.ipc(), 0.0);
        assert_eq!(r.branch_mpki(), 0.0);
    }

    #[test]
    fn ipc_calculated_correctly() {
        let r = SimResults {
            cycles: 100,
            instructions_committed: 200,
            ..Default::default()
        };
        assert!((r.ipc() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn collector_counts_commits() {
        let mut collector = StatsCollector::new();
        collector.on_event(&SimEvent::InsnCommit {
            pc: 0x100,
            cycle: 1,
        });
        collector.on_event(&SimEvent::InsnCommit {
            pc: 0x104,
            cycle: 2,
        });
        assert_eq!(collector.results.instructions_committed, 2);
        assert_eq!(collector.results.cycles, 2);
    }

    #[test]
    fn collector_counts_mispredictions() {
        let mut collector = StatsCollector::new();
        collector.on_event(&SimEvent::BranchResolved {
            pc: 0x100,
            predicted: true,
            taken: false,
            cycle: 1,
        });
        assert_eq!(collector.results.branches, 1);
        assert_eq!(collector.results.branch_mispredictions, 1);
    }

    #[test]
    fn collector_tracks_cache_hits_misses() {
        let mut collector = StatsCollector::new();
        collector.on_event(&SimEvent::CacheAccess {
            level: 1,
            hit: true,
            addr: 0,
            cycle: 1,
        });
        collector.on_event(&SimEvent::CacheAccess {
            level: 1,
            hit: false,
            addr: 0,
            cycle: 2,
        });
        let (hits, misses) = collector.results.cache_accesses[&1];
        assert_eq!(hits, 1);
        assert_eq!(misses, 1);
    }

    #[test]
    fn results_serialize_to_json() {
        let r = SimResults {
            cycles: 10,
            instructions_committed: 20,
            ..Default::default()
        };
        let json = r.to_json();
        assert!(json.contains("\"cycles\": 10"));
    }
}

//! Top-level simulation driver.

use super::core_sim::CoreSim;
use anyhow::Result;
use helm_core::config::PlatformConfig;
use helm_core::event::EventObserver;
use helm_core::types::ExecMode;
use helm_stats::collector::{SimResults, StatsCollector};

/// The main simulation handle.
pub struct Simulation {
    pub config: PlatformConfig,
    pub binary_path: String,
    cores: Vec<CoreSim>,
    stats: StatsCollector,
}

impl Simulation {
    pub fn new(config: PlatformConfig, binary_path: String) -> Self {
        let cores = config
            .cores
            .iter()
            .enumerate()
            .map(|(i, c)| CoreSim::new(i, c.clone()))
            .collect();
        Self {
            config,
            binary_path,
            cores,
            stats: StatsCollector::new(),
        }
    }

    /// Run the simulation to completion. Returns aggregated results.
    pub fn run(&mut self, max_cycles: u64) -> Result<SimResults> {
        log::info!(
            "Starting HELM simulation: platform={}, mode={:?}, cores={}, binary={}",
            self.config.name,
            self.config.exec_mode,
            self.cores.len(),
            self.binary_path,
        );

        match self.config.exec_mode {
            ExecMode::SE => self.run_se(max_cycles),
            ExecMode::CAE => self.run_microarch(max_cycles),
        }
    }

    fn run_se(&mut self, _max_cycles: u64) -> Result<SimResults> {
        // In SE mode we use the translation engine for fast execution.
        log::info!("SE mode: fast functional emulation (stub)");
        Ok(self.stats.results.clone())
    }

    fn run_microarch(&mut self, max_cycles: u64) -> Result<SimResults> {
        for cycle in 0..max_cycles {
            let mut all_halted = true;
            for core in &mut self.cores {
                let events = core.tick();
                for event in &events {
                    self.stats.on_event(event);
                }
                if !core.halted {
                    all_halted = false;
                }
            }
            if all_halted {
                log::info!("All cores halted at cycle {}", cycle);
                break;
            }
        }
        Ok(self.stats.results.clone())
    }
}

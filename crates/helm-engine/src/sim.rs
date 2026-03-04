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
            ExecMode::SyscallEmulation => self.run_se(max_cycles),
            ExecMode::Microarchitectural => self.run_microarch(max_cycles),
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

#[cfg(test)]
mod tests {
    use super::*;
    use helm_core::config::*;
    use helm_core::types::{ExecMode, IsaKind};

    fn test_platform() -> PlatformConfig {
        PlatformConfig {
            name: "test".into(),
            isa: IsaKind::RiscV64,
            exec_mode: ExecMode::Microarchitectural,
            cores: vec![CoreConfig {
                name: "c0".into(),
                width: 2,
                rob_size: 16,
                iq_size: 8,
                lq_size: 4,
                sq_size: 4,
                branch_predictor: BranchPredictorConfig::Static,
            }],
            memory: MemoryConfig {
                l1i: None,
                l1d: None,
                l2: None,
                l3: None,
                dram_latency_cycles: 100,
            },
        }
    }

    #[test]
    fn simulation_constructs() {
        let sim = Simulation::new(test_platform(), "/dev/null".into());
        assert_eq!(sim.config.name, "test");
    }

    #[test]
    fn microarch_run_completes() {
        let mut sim = Simulation::new(test_platform(), "/dev/null".into());
        let results = sim.run(100).unwrap();
        assert!(results.cycles <= 100);
    }

    #[test]
    fn se_mode_run_returns_results() {
        let mut platform = test_platform();
        platform.exec_mode = ExecMode::SyscallEmulation;
        let mut sim = Simulation::new(platform, "/dev/null".into());
        let results = sim.run(10).unwrap();
        assert_eq!(results.cycles, 0); // stub returns empty
    }
}

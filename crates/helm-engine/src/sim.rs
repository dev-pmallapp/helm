//! Top-level simulation driver.

use super::core_sim::CoreSim;
use super::se::linux;
use anyhow::Result;
use helm_core::config::PlatformConfig;
use helm_core::event::EventObserver;
use helm_core::types::ExecMode;
use helm_stats::collector::{SimResults, StatsCollector};
use helm_timing::model::FeModel;
use helm_timing::TimingModel;

/// The main simulation handle.
///
/// The timing model is the primary knob for simulation accuracy.
/// Pass a [`FeModel`] for maximum speed (IPC=1), or an
/// [`IteModelDetailed`](helm_timing::IteModelDetailed) for approximate
/// timing with per-opcode latencies and cache modelling.
pub struct Simulation {
    pub config: PlatformConfig,
    pub binary_path: String,
    cores: Vec<CoreSim>,
    stats: StatsCollector,
    timing: Box<dyn TimingModel>,
}

impl Simulation {
    /// Create a simulation with a specific timing model.
    ///
    /// The timing model determines the accuracy level.  Use
    /// [`FeModel`] for functional emulation or
    /// [`IteModelDetailed`](helm_timing::IteModelDetailed) for
    /// approximate timing.
    pub fn new(config: PlatformConfig, binary_path: String, timing: Box<dyn TimingModel>) -> Self {
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
            timing,
        }
    }

    /// Convenience constructor that defaults to FE (functional) timing.
    pub fn new_fe(config: PlatformConfig, binary_path: String) -> Self {
        Self::new(config, binary_path, Box::new(FeModel))
    }

    /// Run the simulation to completion. Returns aggregated results.
    pub fn run(&mut self, max_cycles: u64) -> Result<SimResults> {
        log::info!(
            "Starting HELM simulation: platform={}, mode={:?}, timing={:?}, cores={}, binary={}",
            self.config.name,
            self.config.exec_mode,
            self.timing.accuracy(),
            self.cores.len(),
            self.binary_path,
        );

        match self.config.exec_mode {
            ExecMode::SE => self.run_se(max_cycles),
            ExecMode::FS => self.run_microarch(max_cycles),
            ExecMode::HAE => todo!("HAE (KVM) execution not yet integrated into Simulation::run"),
        }
    }

    fn run_se(&mut self, max_cycles: u64) -> Result<SimResults> {
        let binary = self.binary_path.clone();
        let argv = [binary.as_str()];
        let envp: [&str; 0] = [];

        let mut backend = crate::se::ExecBackend::interpretive();
        let result = linux::run_aarch64_se_timed(
            &binary,
            &argv,
            &envp,
            max_cycles,
            self.timing.as_mut(),
            &mut backend,
            None,
            None,
            None,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;

        self.stats.results.instructions_committed = result.instructions_executed;
        self.stats.results.cycles = result.virtual_cycles;
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

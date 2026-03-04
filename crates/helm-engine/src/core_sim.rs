//! Per-core simulation logic for one cycle tick.

use helm_core::config::CoreConfig;
use helm_core::event::SimEvent;
use helm_core::types::Cycle;
use helm_pipeline::Pipeline;

/// State of a single simulated core.
pub struct CoreSim {
    pub id: usize,
    pub pipeline: Pipeline,
    pub cycle: Cycle,
    pub halted: bool,
}

impl CoreSim {
    pub fn new(id: usize, config: CoreConfig) -> Self {
        let pipeline = Pipeline::new(config);
        Self {
            id,
            pipeline,
            cycle: 0,
            halted: false,
        }
    }

    /// Advance the core by one cycle. Returns events produced.
    pub fn tick(&mut self) -> Vec<SimEvent> {
        if self.halted {
            return vec![];
        }
        self.cycle += 1;

        // Commit completed instructions from the ROB.
        let committed = self.pipeline.rob.try_commit();
        let events: Vec<SimEvent> = committed
            .iter()
            .map(|uop| SimEvent::InsnCommit {
                pc: uop.guest_pc,
                cycle: self.cycle,
            })
            .collect();

        // Wakeup and issue (stub).
        self.pipeline.scheduler.wakeup(&[]);
        let _issued = self
            .pipeline
            .scheduler
            .select(self.pipeline.config.width as usize);

        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_core::config::{BranchPredictorConfig, CoreConfig};

    fn test_core_config() -> CoreConfig {
        CoreConfig {
            name: "test-core".into(),
            width: 2,
            rob_size: 16,
            iq_size: 8,
            lq_size: 4,
            sq_size: 4,
            branch_predictor: BranchPredictorConfig::Static,
        }
    }

    #[test]
    fn new_core_starts_at_cycle_zero() {
        let core = CoreSim::new(0, test_core_config());
        assert_eq!(core.cycle, 0);
        assert!(!core.halted);
    }

    #[test]
    fn tick_advances_cycle() {
        let mut core = CoreSim::new(0, test_core_config());
        core.tick();
        assert_eq!(core.cycle, 1);
        core.tick();
        assert_eq!(core.cycle, 2);
    }

    #[test]
    fn halted_core_does_not_advance() {
        let mut core = CoreSim::new(0, test_core_config());
        core.halted = true;
        let events = core.tick();
        assert!(events.is_empty());
        assert_eq!(core.cycle, 0);
    }
}

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

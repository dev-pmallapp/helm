//! # helm-pipeline
//!
//! Models a configurable out-of-order processor pipeline. Pipeline stages
//! are composed from independent modules that communicate via internal
//! queues, making it easy to swap or extend individual stages.

pub mod branch_pred;
pub mod rename;
pub mod rob;
pub mod scheduler;
pub mod stage;

use helm_core::config::CoreConfig;

/// A fully-assembled pipeline instance for a single core.
pub struct Pipeline {
    pub config: CoreConfig,
    pub rob: rob::ReorderBuffer,
    pub rename: rename::RenameUnit,
    pub scheduler: scheduler::Scheduler,
    pub branch_predictor: branch_pred::BranchPredictor,
}

impl Pipeline {
    pub fn new(config: CoreConfig) -> Self {
        let rob = rob::ReorderBuffer::new(config.rob_size as usize);
        let rename = rename::RenameUnit::new();
        let scheduler = scheduler::Scheduler::new(config.iq_size as usize);
        let branch_predictor = branch_pred::BranchPredictor::from_config(&config.branch_predictor);
        Self {
            config,
            rob,
            rename,
            scheduler,
            branch_predictor,
        }
    }
}

#[cfg(test)]
mod tests;

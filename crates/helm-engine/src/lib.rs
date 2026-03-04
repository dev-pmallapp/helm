//! # helm-engine
//!
//! Top-level simulation orchestrator. Receives a `PlatformConfig` (usually
//! built from Python), instantiates the pipeline, memory subsystem, ISA
//! frontend, and translation engine, then drives the simulation loop.

pub mod core_sim;
pub mod loader;
pub mod sim;

pub use sim::Simulation;

#[cfg(test)]
mod tests;

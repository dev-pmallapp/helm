//! # helm-timing
//!
//! Provides the [`TimingModel`] trait (pluggable accuracy levels),
//! an event-driven [`EventQueue`], temporal decoupling for multi-core,
//! and a sampling controller for fast-forward + detailed phases.
//!
//! # Accuracy Levels
//!
//! | Level | Speed | What is modelled |
//! |-------|-------|------------------|
//! | FE | 100-1000 MIPS | IPC=1, flat memory |
//! | ITE | 1-100 MIPS | Cache latencies, device delays, optional pipeline |
//! | CAE | 0.1-1 MIPS | Full pipeline stages, bypass network |

pub mod event_queue;
pub mod model;
pub mod sampling;
pub mod temporal;

pub use event_queue::EventQueue;
pub use model::{AccuracyLevel, IteModelDetailed, InsnClass, TimingModel};
pub use sampling::{SamplingController, SamplingPhase};
pub use temporal::TemporalDecoupler;

#[cfg(test)]
mod tests;

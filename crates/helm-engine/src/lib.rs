//! # helm-engine
//!
//! Simulation orchestrator. Provides SE-mode runners, binary loaders,
//! and the cycle-level simulation driver.
//!
//! ```text
//! se/         Syscall-Emulation runners (Linux, FreeBSD)
//! loader/     Binary loaders (ELF64, ELF32)
//! core_sim.rs Per-core cycle simulation
//! sim.rs      Top-level Simulation driver
//! ```

pub mod core_sim;
pub mod loader;
pub mod se;
pub mod sim;

pub use se::classify::classify_a64;
pub use se::run_aarch64_se_with_plugins;
pub use se::{run_aarch64_se, run_aarch64_se_timed, ExecBackend, SeResult, SeTimedResult};
pub use sim::Simulation;
pub use se::{SeSession, StopReason};

#[cfg(test)]
mod tests;

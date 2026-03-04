//! # helm-trace
//!
//! Instrumentation framework modelled after QEMU's TCG plugin API.
//!
//! Plugins implement [`HelmPlugin`] and register callbacks during
//! [`install()`](HelmPlugin::install).  The engine invokes callbacks
//! at translation time, execution time, memory accesses, and syscalls.
//!
//! ## Built-in Plugins
//!
//! | Plugin | What it does |
//! |--------|-------------|
//! | [`InsnCount`](plugins::InsnCount) | Count instructions per vCPU (inline, <1% overhead) |
//! | [`ExecLog`](plugins::ExecLog) | Log every executed instruction |
//! | [`HotBlocks`](plugins::HotBlocks) | Rank basic blocks by execution count |
//! | [`HowVec`](plugins::HowVec) | Instruction-class histogram |
//! | [`SyscallTrace`](plugins::SyscallTrace) | Log syscall entry/return |
//! | [`CacheSim`](plugins::CacheSim) | Multi-level cache simulation |

pub mod callback;
pub mod info;
pub mod plugin;
pub mod registry;
pub mod scoreboard;

pub mod plugins;

pub use plugin::{HelmPlugin, PluginArgs};
pub use registry::PluginRegistry;
pub use scoreboard::Scoreboard;

#[cfg(test)]
mod tests;

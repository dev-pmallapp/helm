//! # helm-plugins
//!
//! Plugin framework and built-in plugins for HELM, modelled after
//! QEMU's TCG plugin API.
//!
//! ## Module Layout
//!
//! ```text
//! helm-plugins/
//!   plugin.rs       HelmPlugin trait, PluginArgs
//!   registry.rs     PluginRegistry (callback storage + dispatch)
//!   scoreboard.rs   Scoreboard<T> (per-vCPU lock-free data)
//!   callback.rs     Callback type aliases, MemFilter
//!   info.rs         Introspection structs (InsnInfo, MemInfo, etc.)
//!
//!   trace/          Execution tracing and profiling plugins
//!     insn_count    Instruction counter (inline scoreboard)
//!     execlog       Execution trace logger
//!     hotblocks     Basic-block profiler
//!     howvec        Instruction-class histogram
//!     syscall_trace Syscall entry/return logger
//!
//!   memory/         Memory analysis plugins
//!     cache_sim     Set-associative cache simulation
//!
//!   debug/          Debug and validation plugins (future)
//! ```

pub mod callback;
pub mod info;
pub mod plugin;
pub mod registry;
pub mod scoreboard;

pub mod bridge;
pub mod debug;
pub mod memory;
pub mod trace;

pub use bridge::{register_builtins, PluginComponentAdapter};
pub use plugin::{HelmPlugin, PluginArgs};
pub use registry::PluginRegistry;
pub use scoreboard::Scoreboard;

#[cfg(test)]
mod tests;

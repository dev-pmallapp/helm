pub mod registry;
pub mod scoreboard;
pub mod callback;
pub mod info;
pub mod bridge;

// Re-export key types at module root for convenience
pub use registry::PluginRegistry;
pub use scoreboard::Scoreboard;
pub use callback::*;
pub use info::*;
pub use bridge::PluginComponentAdapter;

#[cfg(feature = "builtins")]
pub use bridge::register_builtins;

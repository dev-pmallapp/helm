pub mod bridge;
pub mod callback;
pub mod info;
pub mod registry;
pub mod scoreboard;

// Re-export key types at module root for convenience
pub use bridge::PluginComponentAdapter;
pub use callback::*;
pub use info::*;
pub use registry::PluginRegistry;
pub use scoreboard::Scoreboard;

#[cfg(feature = "builtins")]
pub use bridge::register_builtins;

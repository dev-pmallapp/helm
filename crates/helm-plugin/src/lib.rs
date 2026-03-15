//! `helm-plugin` — instrumentation and analysis framework for helm-ng.
//!
//! # Architecture
//! - `api` — stable plugin traits (`HelmPlugin`, `PluginArgs`)
//! - `runtime` — callback registry and info types
//! - `builtins` — built-in plugins (feature-gated)

pub mod api;
pub mod runtime;

#[cfg(feature = "builtins")]
pub mod builtins;

pub use api::{HelmPlugin, PluginArgs};
pub use runtime::PluginRegistry;

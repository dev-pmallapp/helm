//! # helm-plugin
//!
//! Unified plugin system for HELM simulator.
//!
//! This crate consolidates the plugin API, runtime infrastructure, and built-in
//! plugins into a single, well-organized package. It provides:
//!
//! - **Stable API** (`api` module) for external plugin authors
//! - **Runtime infrastructure** (`runtime` module) for callback management
//! - **Built-in Plugins** (`builtins` module, feature-gated) for common use cases
//!
//! ## For Plugin Authors
//!
//! Use the `api` module to implement custom plugins:
//!
//! ```rust
//! use helm_plugin::api::*;
//!
//! pub struct MyPlugin;
//!
//! impl HelmComponent for MyPlugin {
//!     fn component_type(&self) -> &'static str { "custom.my-plugin" }
//!     fn interfaces(&self) -> &[&str] { &["custom"] }
//!     // ...
//! }
//! ```
//!
//! ## For Simulator Integrators
//!
//! Use the `runtime` module to manage plugins at runtime:
//!
//! ```rust
//! use helm_plugin::api::ComponentRegistry;
//! use helm_plugin::runtime::PluginRegistry;
//!
//! let mut comp_registry = ComponentRegistry::new();
//! # #[cfg(feature = "builtins")]
//! helm_plugin::register_builtins(&mut comp_registry);
//!
//! let mut plugin_registry = PluginRegistry::new();
//! // Install plugins and fire callbacks...
//! ```

/// Stable plugin API for external plugin authors.
///
/// This module contains the core traits and types that plugin authors depend on.
/// Breaking changes to this API require a major version bump.
pub mod api;

/// Plugin runtime and callback infrastructure.
///
/// This module provides the infrastructure for managing plugins at runtime,
/// including callback registration and dispatch.
pub mod runtime;

/// Built-in plugins shipped with HELM.
///
/// This module is only available when the `builtins` feature is enabled (default).
#[cfg(feature = "builtins")]
pub mod builtins;

// ===== CONVENIENCE RE-EXPORTS =====
// For backwards compatibility and convenience

pub use api::{ComponentInfo, HelmComponent, HelmPlugin, PluginArgs};
pub use runtime::PluginRegistry;

#[cfg(feature = "builtins")]
pub use runtime::register_builtins;

#[cfg(test)]
mod tests;

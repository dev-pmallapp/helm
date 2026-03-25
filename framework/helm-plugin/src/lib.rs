//! `helm-plugin` — instrumentation and analysis framework for helm-ng.
//!
//! # Architecture
//! - `api` — stable plugin traits (`HelmPlugin`, `HelmPluginArgs`)
//! - `runtime` — callback registry and info types
//! - `builtins` — built-in plugins (feature-gated)

#![allow(missing_docs)]

pub mod api;
pub mod runtime;

#[cfg(feature = "builtins")]
pub mod builtins;

pub use api::{HelmPlugin, HelmPluginArgs};
pub use runtime::HelmPluginRegistry;

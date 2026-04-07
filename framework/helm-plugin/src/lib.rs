//! `helm-plugin` — legacy callback-plugin instrumentation for helm-ng.
//!
//! This crate remains as a compatibility layer for existing callback-based
//! observers and built-in plugins. New observation and analysis work should
//! prefer the probe/session/report stack:
//!
//! - `helm-probe` for typed event sources
//! - `helm-spy` for collection/session state
//! - `helm-report` for formatting and delivery
//!
//! # Architecture
//! - `api` — stable plugin traits (`HelmPlugin`, `HelmPluginArgs`)
//! - `runtime` — callback registry and info types
//! - `builtins` — built-in legacy plugins (feature-gated)

#![allow(missing_docs)]

pub mod api;
pub mod runtime;

#[cfg(feature = "builtins")]
pub mod builtins;

pub use api::{HelmPlugin, HelmPluginArgs};
pub use runtime::HelmPluginRegistry;

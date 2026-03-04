//! # helm-core
//!
//! Foundation crate for HELM. Defines the intermediate representation (IR),
//! shared traits, error types, and common data structures used across all
//! other crates in the workspace.

pub mod config;
pub mod error;
pub mod event;
pub mod ir;
pub mod types;

// Re-exports for convenience.
pub use error::{HelmError, HelmResult};
pub use types::{Addr, RegId, Word};

#[cfg(test)]
mod tests;

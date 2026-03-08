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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Shared IRQ pending signal between an interrupt controller and the CPU.
///
/// The GIC raises this when any IRQ is pending for the CPU; the CPU
/// checks it at the top of `step()` to decide whether to take an
/// IRQ exception.
#[derive(Clone)]
pub struct IrqSignal(Arc<AtomicBool>);

impl IrqSignal {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    /// Signal that an IRQ is pending.
    pub fn raise(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Signal that no IRQ is pending.
    pub fn lower(&self) {
        self.0.store(false, Ordering::Relaxed);
    }

    /// Check whether an IRQ is pending.
    pub fn is_raised(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

impl Default for IrqSignal {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

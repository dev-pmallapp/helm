//! # helm-core
//!
//! Foundation crate for HELM. Defines unified instruction types, execution
//! traits, error types, and common data structures used across all crates.

pub mod error;
pub mod insn;
pub mod types;

// Re-exports for convenience.
pub use error::{HelmError, HelmResult};
pub use types::{Addr, RegId, Word};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Shared IRQ pending signal between an interrupt controller and the CPU.
#[derive(Clone)]
pub struct IrqSignal(Arc<AtomicBool>);

impl IrqSignal {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn raise(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn lower(&self) {
        self.0.store(false, Ordering::Relaxed);
    }

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

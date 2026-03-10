//! AArch64 (ARMv8 A64) TCG target.
//!
//! - [`regs`] — register map (X0–X30, SP, PC, NZCV, DAIF, system regs)
//! - Emitter — see `crate::a64_emitter` (being migrated here)

pub mod regs;

// Re-export register constants for convenience.
pub use regs::*;

//! # helm-tcg
//!
//! QEMU-style Tiny Code Generator intermediate representation.
//!
//! In SE/FE mode, the ISA decoder emits [`TcgOp`] sequences instead of
//! [`MicroOp`](helm_core::ir::MicroOp)s.  These are simpler, higher-level
//! operations that map directly to host instructions, avoiding the overhead
//! of modelling pipeline structures.
//!
//! ## Dual Backend
//!
//! The same `.decode` file drives **two** code-generation paths:
//!
//! ```text
//! .decode file
//!     │
//!     ├─► TCG backend  ─► TcgOp chain  ─► interpreted or JIT'd  (SE/FE)
//!     │
//!     └─► Static backend ─► MicroOp vec ─► pipeline model        (APE/CAE)
//! ```
//!
//! This crate defines the TCG IR.  The static backend lives in `helm-core::ir`.

pub mod block;
pub mod context;
pub mod ir;

pub use context::TcgContext;
pub use ir::{TcgOp, TcgTemp};

#[cfg(test)]
mod tests;

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
//!
//! ## Multi-ISA targets
//!
//! Per-ISA frontends live in [`target`]:
//!
//! - [`target::aarch64`] — ARMv8 AArch64 (A64)
//! - [`target::riscv64`] — RISC-V 64-bit (stub)
//! - [`target::x86_64`] — x86-64 / AMD64 (stub)

pub mod a64_emitter;
pub mod block;
pub mod context;
pub mod interp;
pub mod ir;
pub mod target;
pub mod threaded;

pub use block::TcgBlock;
pub use context::TcgContext;
pub use interp::{InterpExit, InterpResult, TcgInterp};
pub use ir::{TcgOp, TcgTemp};
pub use target::TranslateAction;

#[cfg(test)]
mod tests;

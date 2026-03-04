//! # helm-isa
//!
//! Modular ISA frontend layer. Each supported architecture implements
//! [`IsaFrontend`] to decode raw bytes into the shared [`MicroOp`] IR.

pub mod arm;
pub mod frontend;
pub mod riscv;
pub mod x86;

pub use frontend::IsaFrontend;

//! # helm-translate
//!
//! Dynamic binary translation engine used in Syscall-Emulation mode.
//! Guest code is translated into blocks of `MicroOp`s and cached for
//! re-execution, similar to QEMU's TCG.

pub mod block;
pub mod cache;
pub mod translator;

pub use translator::Translator;

#[cfg(test)]
mod tests;

//! Binary loaders for SE mode.
//!
//! ```text
//! loader/
//!   elf64.rs    — ELF64 loader (AArch64, RISC-V 64, x86-64)
//!   elf32.rs    — ELF32 loader (ARMv7) (future)
//! ```

pub mod elf64;

// Re-export for convenience
pub use elf64::{load_elf, LoadedBinary};

//! ELF loaders for SE mode.

pub mod elf64;

pub use elf64::{load_elf, LoadedBinary};

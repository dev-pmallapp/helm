//! Binary loaders.
//!
//! ```text
//! loader/
//!   elf64.rs        — ELF64 loader (AArch64 SE mode)
//!   arm64_image.rs  — ARM64 Linux Image loader (FS mode kernel boot)
//! ```

pub mod arm64_image;
pub mod elf64;

pub use arm64_image::{load_arm64_image, LoadedKernel};
pub use elf64::{load_elf, LoadedBinary, TlsInfo};

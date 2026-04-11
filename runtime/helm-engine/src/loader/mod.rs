//! Binary loaders (ELF for SE mode, ARM64 Image for FS mode).

pub mod arm64_image;
pub mod elf64;

pub use arm64_image::{
    load_arm64_kernel, load_arm64_kernel_with_dtb_bytes, Arm64KernelLoadError, LoadedKernel,
};
pub use elf64::{load_elf, setup_riscv_tp, ElfLoadError, ElfSymbol, LoadedBinary};

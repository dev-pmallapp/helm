//! AArch64 (ARMv8/v9) instruction decoder and executor.

pub mod decode;
pub mod exec;

pub use decode::Aarch64Decoder;
pub use exec::{Aarch64Cpu, MemAccess, StepTrace};

#[cfg(test)]
mod tests;

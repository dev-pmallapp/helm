//! AArch64 (ARMv8/v9) instruction decoder and executor.

pub mod cpu_state;
pub mod decode;
pub mod exec;
pub mod hcr;
pub mod sysreg;

pub use cpu_state::Aarch64CpuState;
pub use decode::Aarch64Decoder;
pub use exec::{Aarch64Cpu, MemAccess, StepTrace};

#[cfg(test)]
mod tests;

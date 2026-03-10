//! AArch64 (ARMv8/v9) instruction decoder and executor.

pub mod cpu_state;
pub mod decode;
pub mod exec;
pub mod executor;
pub mod hcr;
pub mod mem_bridge;
pub mod sysreg;
pub mod trait_decoder;

pub use cpu_state::Aarch64CpuState;
pub use decode::Aarch64Decoder;
pub use exec::{Aarch64Cpu, MemAccess, StepTrace};
pub use executor::Aarch64TraitExecutor;
pub use trait_decoder::Aarch64TraitDecoder;

#[cfg(test)]
mod tests;

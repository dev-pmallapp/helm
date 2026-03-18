//! AArch64 ISA — decode, execute, and architectural state.
//!
//! # Module layout
//! - [`arch_state`] — `Aarch64ArchState` (GPRs, NZCV, SIMD, system registers)
//! - [`decode`]     — 32-bit fixed-width instruction decoder
//! - [`execute`]    — instruction execution (by encoding group)
//! - [`step`]       — compatibility wrapper around `decode()` + `execute()`
//! - [`insn`]       — `Instruction` enum + condition code helpers

pub mod arch_state;
pub mod core_model;
pub mod decode;
pub mod exception;
pub mod execute;
pub mod insn;
pub mod mmu;
pub mod step;

pub use arch_state::Aarch64ArchState;
pub use core_model::ArmCoreModel;
pub use decode::decode;
pub use execute::execute;
pub use insn::Instruction;
pub use step::step;

#[cfg(test)]
mod tests;

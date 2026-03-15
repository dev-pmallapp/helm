//! AArch64 ISA — decode, execute, and architectural state.
//!
//! # Module layout
//! - [`arch_state`] — `Aarch64ArchState` (GPRs, NZCV, SIMD, system registers)
//! - [`decode`]     — 32-bit fixed-width instruction decoder
//! - [`execute`]    — instruction execution (by encoding group)
//! - [`insn`]       — `Instruction` enum + condition code helpers

pub mod arch_state;
pub mod decode;
pub mod execute;
pub mod insn;

pub use arch_state::Aarch64ArchState;
pub use decode::decode;
pub use execute::execute;
pub use insn::Instruction;

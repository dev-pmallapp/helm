//! RISC-V RV64GC decode + execute.
//!
//! # Modules (fill in during Phase 0)
//! - [`insn`]     — `Instruction` enum (all RV64IMACFD_Zicsr variants)
//! - [`decode`]   — 32-bit decoder + C-extension expander
//! - [`execute`]  — execute match statement
//! - [`csr`]      — CSR address constants + side-effect dispatch
//! - [`arch_state`] — `RiscvArchState` struct + `ArchState` impl

pub mod arch_state;
pub mod csr;
pub mod decode;
pub mod execute;
pub mod insn;

pub use arch_state::RiscvArchState;
pub use decode::decode;
pub use execute::execute;
pub use insn::Instruction;

//! `helm-arch` — ISA decode + execute for RISC-V, AArch64, and AArch32.
//!
//! # Structure
//! - [`riscv`]   — RV64GC decode/execute (Phase 0)
//! - [`aarch64`] — AArch64 decode/execute (Phase 2)
//! - [`aarch32`] — AArch32/Thumb decode/execute (Phase 3)
//!
//! # Hot-path contract
//! `execute()` in each ISA module takes `&mut impl ExecContext`.
//! All dispatch is static — no virtual calls inside the execute loop.

pub mod aarch32;
pub mod aarch64;
pub mod riscv;

pub use riscv::{decode as riscv_decode, execute as riscv_execute, Instruction as RiscvInsn};

/// Error from instruction decoding.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DecodeError {
    #[error("unknown encoding {raw:#010x} at pc={pc:#x}")]
    Unknown { raw: u32, pc: u64 },

    #[error("ISA not yet implemented")]
    Unimplemented,
}

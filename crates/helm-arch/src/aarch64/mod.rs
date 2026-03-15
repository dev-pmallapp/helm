//! AArch64 ISA — Phase 2 placeholder.
//!
//! Full implementation scheduled for Phase 2. Until then, every decode call
//! returns `DecodeError::Unimplemented`.

use crate::DecodeError;

/// Decoded AArch64 instruction (Phase 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instruction {
    // TODO(phase-2): populate from AArch64 ISA reference
}

pub fn decode(_raw: u32, _pc: u64) -> Result<Instruction, DecodeError> {
    Err(DecodeError::Unimplemented)
}

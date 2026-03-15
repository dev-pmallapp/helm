//! AArch32/Thumb ISA — Phase 3 placeholder.

use crate::DecodeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instruction {}

pub fn decode(_raw: u32, _pc: u64) -> Result<Instruction, DecodeError> {
    Err(DecodeError::Unimplemented)
}

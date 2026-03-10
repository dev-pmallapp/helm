//! Decoder trait — decodes raw instruction bytes into `DecodedInsn`.

use crate::error::HelmError;
use crate::insn::DecodedInsn;
use crate::types::Addr;

/// Decodes raw instruction bytes into DecodedInsn.
pub trait Decoder: Send + Sync {
    fn decode(&self, pc: Addr, bytes: &[u8]) -> Result<DecodedInsn, HelmError>;

    /// Minimum instruction size in bytes (2 for Thumb/RVC, 4 for A64, 1 for x86).
    fn min_insn_size(&self) -> usize;
}

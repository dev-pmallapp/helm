//! Translated code blocks.

use helm_core::ir::MicroOp;
use helm_core::types::Addr;

/// A translated block of guest instructions.
#[derive(Debug, Clone)]
pub struct TranslatedBlock {
    /// Start address of the guest basic block.
    pub start_pc: Addr,
    /// Number of guest bytes covered.
    pub guest_size: usize,
    /// The micro-ops produced by translating this block.
    pub uops: Vec<MicroOp>,
}

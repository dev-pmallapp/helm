//! Translated block — a sequence of TcgOps for one guest basic block.

use super::ir::TcgOp;
use helm_core::types::Addr;

/// A translated block produced by the TCG frontend.
#[derive(Debug, Clone)]
pub struct TcgBlock {
    /// Guest start address.
    pub guest_pc: Addr,
    /// Number of guest instruction bytes covered.
    pub guest_size: usize,
    /// Number of guest instructions translated.
    pub insn_count: usize,
    /// The TCG op sequence.
    pub ops: Vec<TcgOp>,
}

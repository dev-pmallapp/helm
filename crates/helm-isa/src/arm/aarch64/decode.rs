//! AArch64 A64 instruction decoding.

use helm_core::ir::{MicroOp, MicroOpFlags, Opcode};
use helm_core::types::Addr;
use helm_core::HelmResult;

/// Decodes a single A64 instruction (32-bit fixed width).
pub struct Aarch64Decoder;

impl Aarch64Decoder {
    pub fn new() -> Self {
        Self
    }

    /// Decode the 32-bit instruction word at `pc`.
    pub fn decode_insn(&self, pc: Addr, insn: u32) -> HelmResult<Vec<MicroOp>> {
        let _op0 = (insn >> 25) & 0xF; // bits [28:25] — encoding group

        // TODO: dispatch by encoding group.
        // For now emit a single NOP for every instruction.
        Ok(vec![MicroOp {
            guest_pc: pc,
            opcode: Opcode::Nop,
            sources: vec![],
            dest: None,
            immediate: None,
            flags: MicroOpFlags::default(),
        }])
    }
}

impl Default for Aarch64Decoder {
    fn default() -> Self {
        Self::new()
    }
}

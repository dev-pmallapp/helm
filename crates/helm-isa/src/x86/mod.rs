//! x86-64 ISA frontend (stub).

use crate::frontend::IsaFrontend;
use helm_core::ir::{MicroOp, MicroOpFlags, Opcode};
use helm_core::types::Addr;
use helm_core::HelmResult;

#[derive(Default)]
pub struct X86Frontend;

impl X86Frontend {
    pub fn new() -> Self {
        Self
    }
}

impl IsaFrontend for X86Frontend {
    fn name(&self) -> &str {
        "x86_64"
    }

    fn decode(&self, pc: Addr, _bytes: &[u8]) -> HelmResult<(Vec<MicroOp>, usize)> {
        // Placeholder: emit a single NOP uop and consume 1 byte.
        let uop = MicroOp {
            guest_pc: pc,
            opcode: Opcode::Nop,
            sources: vec![],
            dest: None,
            immediate: None,
            flags: MicroOpFlags::default(),
        };
        Ok((vec![uop], 1))
    }

    fn min_insn_align(&self) -> usize {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_produces_uop() {
        let fe = X86Frontend::new();
        let bytes = [0x90u8; 16]; // NOP padding
        let (uops, consumed) = fe.decode(0x1000, &bytes).unwrap();
        assert!(!uops.is_empty());
        assert!(consumed > 0);
        assert_eq!(uops[0].guest_pc, 0x1000);
    }

    #[test]
    fn name_is_x86_64() {
        assert_eq!(X86Frontend::new().name(), "x86_64");
    }

    #[test]
    fn alignment_is_byte() {
        assert_eq!(X86Frontend::new().min_insn_align(), 1);
    }
}

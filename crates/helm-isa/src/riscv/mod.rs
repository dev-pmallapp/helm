//! RISC-V 64-bit ISA frontend (stub).

use crate::frontend::IsaFrontend;
use helm_core::ir::{MicroOp, MicroOpFlags, Opcode};
use helm_core::types::Addr;
use helm_core::HelmResult;

#[derive(Default)]
pub struct RiscVFrontend;

impl RiscVFrontend {
    pub fn new() -> Self {
        Self
    }
}

impl IsaFrontend for RiscVFrontend {
    fn name(&self) -> &str {
        "riscv64"
    }

    fn decode(&self, pc: Addr, _bytes: &[u8]) -> HelmResult<(Vec<MicroOp>, usize)> {
        // Placeholder: emit a single NOP and consume 4 bytes (standard width).
        let uop = MicroOp {
            guest_pc: pc,
            opcode: Opcode::Nop,
            sources: vec![],
            dest: None,
            immediate: None,
            flags: MicroOpFlags::default(),
        };
        Ok((vec![uop], 4))
    }

    fn min_insn_align(&self) -> usize {
        2 // RISC-V C extension allows 2-byte alignment
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_produces_uop() {
        let fe = RiscVFrontend::new();
        let bytes = [0u8; 16];
        let (uops, consumed) = fe.decode(0x8000_0000, &bytes).unwrap();
        assert!(!uops.is_empty());
        assert_eq!(consumed, 4);
        assert_eq!(uops[0].guest_pc, 0x8000_0000);
    }

    #[test]
    fn name_is_riscv64() {
        assert_eq!(RiscVFrontend::new().name(), "riscv64");
    }

    #[test]
    fn alignment_allows_compressed() {
        assert_eq!(RiscVFrontend::new().min_insn_align(), 2);
    }
}

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

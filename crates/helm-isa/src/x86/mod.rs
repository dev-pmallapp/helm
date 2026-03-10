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

//! AArch64 ISA frontend (stub).

use crate::frontend::IsaFrontend;
use helm_core::ir::{MicroOp, MicroOpFlags, Opcode};
use helm_core::types::Addr;
use helm_core::HelmResult;

#[derive(Default)]
pub struct ArmFrontend;

impl ArmFrontend {
    pub fn new() -> Self {
        Self
    }
}

impl IsaFrontend for ArmFrontend {
    fn name(&self) -> &str {
        "aarch64"
    }

    fn decode(&self, pc: Addr, _bytes: &[u8]) -> HelmResult<(Vec<MicroOp>, usize)> {
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
        4
    }
}

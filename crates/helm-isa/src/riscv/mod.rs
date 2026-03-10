//! RISC-V 64-bit ISA frontend.

pub mod cpu_state;
pub mod decoder;
pub mod executor;

use crate::frontend::IsaFrontend;
use helm_core::ir::{MicroOp, MicroOpFlags, Opcode};
use helm_core::types::Addr;
use helm_core::HelmResult;

pub use cpu_state::Rv64CpuState;
pub use decoder::Rv64Decoder;
pub use executor::Rv64Executor;

/// Legacy ISA frontend (MicroOp-based).
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
        2
    }
}

//! Stencil JIT backend — implements `JitBackend` for AArch64.

#![allow(missing_docs)]

use helm_arch::aarch64::insn::Instruction;

use crate::backend::JitBackend;
use crate::block::CompiledBlock;
use super::data;
use super::fields;
use super::compiler;

/// Which guest ISA this stencil backend is configured for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StencilIsa {
    Aarch64,
    Riscv64,
}

/// Stencil (copy-and-patch) JIT backend.
///
/// Compiles guest instructions by copying pre-compiled x86-64 code templates
/// and patching relocation holes. Much faster than dynasm at the cost of less
/// per-instruction optimization.
pub struct StencilBackend {
    isa: StencilIsa,
}

impl StencilBackend {
    /// Create a new stencil backend for the given ISA.
    pub fn new(isa: StencilIsa) -> Self {
        Self { isa }
    }

    /// Create a new stencil backend for AArch64.
    pub fn new_aarch64() -> Self {
        Self::new(StencilIsa::Aarch64)
    }

    /// Create a new stencil backend for RISC-V64.
    pub fn new_riscv64() -> Self {
        Self::new(StencilIsa::Riscv64)
    }
}

impl JitBackend for StencilBackend {
    fn compile_block(&mut self, pc: u64, insns: &[Instruction]) -> Option<CompiledBlock> {
        if self.isa != StencilIsa::Aarch64 {
            // RISC-V64 uses a separate trait (different Instruction type).
            // This JitBackend impl is AArch64-only.
            return None;
        }

        // Look up stencils and extract fields for each instruction.
        let mut entries = Vec::new();
        for (i, insn) in insns.iter().enumerate() {
            let stencil = data::lookup_stencil_a64(insn)?;

            // If first instruction has no stencil, return None.
            // If a later instruction has no stencil, compile what we have.
            if i == 0 && stencil.is_none() {
                return None;
            }
            let stencil = match stencil {
                Some(s) => s,
                None => break,
            };

            let fields = fields::extract_fields_a64(insn, insn.pc);
            entries.push((stencil, fields));

            if stencil.is_terminator {
                break;
            }
        }

        if entries.is_empty() {
            return None;
        }

        compiler::compile_block(pc, &entries)
    }

    fn name(&self) -> &str {
        "stencil"
    }
}

//! Per-ISA stencil lookup tables (build-time generated).

#![allow(missing_docs)]

pub mod aarch64;
pub mod riscv64;

use helm_arch::aarch64::insn::Instruction;

use super::types::Stencil;

/// Look up a stencil for an AArch64 instruction.
///
/// Returns `Some(Some(stencil))` if found, `Some(None)` if the opcode is
/// recognized but unsupported, and `None` for unknown opcodes.
pub fn lookup_stencil_a64(insn: &Instruction) -> Option<Option<&'static Stencil>> {
    aarch64::lookup(insn)
}

/// Look up a stencil for a RISC-V64 instruction by variant name.
pub fn lookup_stencil_rv64(name: &str) -> Option<&'static Stencil> {
    riscv64::lookup(name)
}

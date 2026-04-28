//! Per-ISA stencil lookup tables (build-time generated).

#![allow(missing_docs)]

pub mod aarch64;
pub mod riscv64;

use helm_arch::aarch64::insn::Instruction;

use super::types::Stencil;

/// Generic (ISA-independent) reject reason constants.
///
/// Arch-specific stencil lookups may return these or more specific strings.
/// The `jit_rejects` plugin accumulates counts by string value regardless
/// of whether the reason is generic or arch-specific.
pub mod reject {
    pub const COMPLEX_ADDRESSING: &str = "complex-addressing";
    pub const IMM_OUT_OF_RANGE: &str = "imm-out-of-range";
    pub const UNSUPPORTED_SIZE: &str = "unsupported-size";
    pub const OPCODE_UNIMPLEMENTED: &str = "opcode-unimplemented";
    pub const W_REGISTER: &str = "w-register";
    pub const SHIFTED_REGISTER: &str = "shifted-register";
}

/// Result of looking up a stencil for an instruction.
///
/// Carries either the matched stencil or a static reason string explaining
/// why the opcode was rejected. Reason strings are `&'static str` for
/// zero-cost observability — arch-specific modules define their own
/// constants for detailed codes (e.g. `aarch64::reject::LDRSB_W_FORM`).
#[derive(Clone, Copy)]
pub enum StencilLookup {
    /// A matching stencil was found.
    Found(&'static Stencil),
    /// The opcode is recognized but rejected for a specific reason.
    Rejected(&'static str),
}

impl std::fmt::Debug for StencilLookup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Found(_) => write!(f, "Found(...)"),
            Self::Rejected(reason) => write!(f, "Rejected({reason:?})"),
        }
    }
}

/// Look up a stencil for an AArch64 instruction.
///
/// Returns `Some(StencilLookup::Found(stencil))` if found,
/// `Some(StencilLookup::Rejected(reason))` if the opcode is recognized but
/// unsupported, and `None` for unknown opcodes.
pub fn lookup_stencil_a64(insn: &Instruction) -> Option<StencilLookup> {
    aarch64::lookup(insn)
}

/// Look up a stencil for a RISC-V64 instruction by variant name.
pub fn lookup_stencil_rv64(name: &str) -> Option<&'static Stencil> {
    riscv64::lookup(name)
}

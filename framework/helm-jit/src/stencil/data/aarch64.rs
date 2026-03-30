//! AArch64 stencil lookup — dispatches opcodes to generated stencil data.

#![allow(missing_docs)]
#![allow(unused_imports)]

use helm_arch::aarch64::insn::{Instruction, Opcode};
use crate::stencil::types::{HoleKind, HelperFn, RegField, Stencil, StencilReloc};

// Include the build-time generated stencil data (byte arrays + reloc tables).
// The generated file does NOT contain `use` statements — it relies on the
// imports above.
include!(concat!(env!("OUT_DIR"), "/generated_a64.rs"));

/// Look up a stencil for an AArch64 instruction.
///
/// Returns `Some(Some(&Stencil))` for supported opcodes,
/// `Some(None)` for recognized-but-unsupported opcodes,
/// and `None` for completely unknown opcodes (first-insn failure).
pub fn lookup(insn: &Instruction) -> Option<Option<&'static Stencil>> {
    let s = match insn.opcode {
        // Data processing — immediate
        Opcode::AddImm => &STENCIL_ADD_IMM,
        Opcode::SubImm => &STENCIL_SUB_IMM,
        Opcode::AddsImm => &STENCIL_ADDS_IMM,
        Opcode::SubsImm => &STENCIL_SUBS_IMM,
        Opcode::AndImm => &STENCIL_AND_IMM,
        Opcode::OrrImm => &STENCIL_ORR_IMM,
        Opcode::EorImm => &STENCIL_EOR_IMM,
        Opcode::AndsImm => &STENCIL_ANDS_IMM,

        // MOV variants
        Opcode::Movz => &STENCIL_MOVZ,
        Opcode::Movn => &STENCIL_MOVN,
        Opcode::Movk => &STENCIL_MOVK,

        // Data processing — register
        Opcode::AddReg => &STENCIL_ADD_REG,
        Opcode::SubReg => &STENCIL_SUB_REG,
        Opcode::AddsReg => &STENCIL_ADDS_REG,
        Opcode::SubsReg => &STENCIL_SUBS_REG,

        // Loads
        Opcode::Ldr => match insn.size {
            3 => &STENCIL_LDR64,
            2 => &STENCIL_LDR32,
            1 => &STENCIL_LDR16,
            0 => &STENCIL_LDR8,
            _ => return Some(None),
        },
        Opcode::Ldrb => &STENCIL_LDR8,
        Opcode::Ldrh => &STENCIL_LDR16,
        Opcode::Ldrsw => &STENCIL_LDRSW,
        Opcode::Ldrsh => &STENCIL_LDRSH,
        Opcode::Ldrsb => &STENCIL_LDRSB,

        // Stores
        Opcode::Str => match insn.size {
            3 => &STENCIL_STR64,
            2 => &STENCIL_STR32,
            1 => &STENCIL_STR16,
            0 => &STENCIL_STR8,
            _ => return Some(None),
        },
        Opcode::Strb => &STENCIL_STR8,
        Opcode::Strh => &STENCIL_STR16,

        // Branches
        Opcode::B => &STENCIL_B,
        Opcode::Bl => &STENCIL_BL,
        Opcode::Br => &STENCIL_BR,
        Opcode::Blr => &STENCIL_BLR,
        Opcode::Ret => &STENCIL_RET,
        Opcode::Cbz => &STENCIL_CBZ,
        Opcode::Cbnz => &STENCIL_CBNZ,
        Opcode::BCond => &STENCIL_BCOND,

        // System
        Opcode::Nop => &STENCIL_NOP,

        // Not supported yet
        _ => return Some(None),
    };
    Some(Some(s))
}

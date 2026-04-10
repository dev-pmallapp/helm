//! AArch64 stencil lookup — dispatches opcodes to generated stencil data.

#![allow(missing_docs)]
#![allow(unused_imports)]

use crate::stencil::types::{HelperFn, HoleKind, RegField, RelocKind, Stencil, StencilReloc};
use helm_arch::aarch64::insn::{Instruction, Opcode};

// Include the build-time generated stencil data (byte arrays + reloc tables).
// The generated file does NOT contain `use` statements — it relies on the
// imports above.
include!(concat!(env!("OUT_DIR"), "/generated_a64.rs"));

/// Returns true if the load/store uses an addressing mode our stencils
/// don't support (register-offset, pre-index, or post-index).
fn is_complex_addressing(insn: &Instruction) -> bool {
    insn.extend_type != 0 || insn.rm != 0 || insn.pre_index || insn.post_index
}

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
        Opcode::AndReg => &STENCIL_AND_REG,
        Opcode::OrrReg => &STENCIL_ORR_REG,
        Opcode::EorReg => &STENCIL_EOR_REG,
        Opcode::OrnReg => &STENCIL_ORN_REG,
        Opcode::BicReg => &STENCIL_BIC_REG,

        // Bitfield
        Opcode::Sbfm => &STENCIL_SBFM,
        Opcode::Ubfm => &STENCIL_UBFM,

        // PC-relative (pre-computed in field extraction, fits 32-bit for SE mode)
        Opcode::Adr => &STENCIL_ADR,
        Opcode::Adrp => &STENCIL_ADRP,

        // Conditional select
        Opcode::Csel => &STENCIL_CSEL,
        Opcode::Csinc => &STENCIL_CSINC,
        Opcode::Csinv => &STENCIL_CSINV,
        Opcode::Csneg => &STENCIL_CSNEG,

        // Multiply/divide
        Opcode::Madd | Opcode::Mul => &STENCIL_MADD,
        Opcode::Msub => &STENCIL_MSUB,
        Opcode::Sdiv => &STENCIL_SDIV,
        Opcode::Udiv => &STENCIL_UDIV,

        // Bitfield extract
        Opcode::Extr => &STENCIL_EXTR,

        // Miscellaneous
        Opcode::Clz => &STENCIL_CLZ,
        Opcode::Rev => &STENCIL_REV,

        // Loads — immediate offset only (register-offset → interpreter fallback)
        Opcode::Ldr if !is_complex_addressing(insn) => match insn.size {
            3 => &STENCIL_LDR64,
            2 => &STENCIL_LDR32,
            1 => &STENCIL_LDR16,
            0 => &STENCIL_LDR8,
            _ => return Some(None),
        },
        Opcode::Ldrb if !is_complex_addressing(insn) => &STENCIL_LDR8,
        Opcode::Ldrh if !is_complex_addressing(insn) => &STENCIL_LDR16,
        Opcode::Ldrsw if !is_complex_addressing(insn) => &STENCIL_LDRSW,
        Opcode::Ldrsh if !is_complex_addressing(insn) => &STENCIL_LDRSH,
        Opcode::Ldrsb if !is_complex_addressing(insn) => &STENCIL_LDRSB,

        // Stores — immediate offset only
        Opcode::Str if !is_complex_addressing(insn) => match insn.size {
            3 => &STENCIL_STR64,
            2 => &STENCIL_STR32,
            1 => &STENCIL_STR16,
            0 => &STENCIL_STR8,
            _ => return Some(None),
        },
        Opcode::Strb if !is_complex_addressing(insn) => &STENCIL_STR8,
        Opcode::Strh if !is_complex_addressing(insn) => &STENCIL_STR16,

        // Load/store pair — immediate offset only
        Opcode::Ldp if !is_complex_addressing(insn) => match insn.size {
            3 => &STENCIL_LDP64,
            2 => &STENCIL_LDP32,
            _ => return Some(None),
        },
        Opcode::Stp if !is_complex_addressing(insn) => match insn.size {
            3 => &STENCIL_STP64,
            2 => &STENCIL_STP32,
            _ => return Some(None),
        },

        // Unscaled loads/stores
        Opcode::Ldur if !is_complex_addressing(insn) => match insn.size {
            3 => &STENCIL_LDUR64,
            2 => &STENCIL_LDUR32,
            _ => return Some(None),
        },
        Opcode::Stur if !is_complex_addressing(insn) => match insn.size {
            3 => &STENCIL_STUR64,
            2 => &STENCIL_STUR32,
            _ => return Some(None),
        },

        // Branches
        Opcode::Tbz => &STENCIL_TBZ,
        Opcode::Tbnz => &STENCIL_TBNZ,
        Opcode::B => &STENCIL_B,
        Opcode::Bl => &STENCIL_BL,
        Opcode::Br => &STENCIL_BR,
        Opcode::Blr => &STENCIL_BLR,
        Opcode::Ret => &STENCIL_RET,
        Opcode::Cbz => &STENCIL_CBZ,
        Opcode::Cbnz => &STENCIL_CBNZ,
        Opcode::BCond => &STENCIL_BCOND,

        // System
        Opcode::Svc => &STENCIL_SVC,
        Opcode::Nop => &STENCIL_NOP,

        // Not supported yet
        _ => return Some(None),
    };
    Some(Some(s))
}

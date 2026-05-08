//! AArch64 stencil lookup — dispatches opcodes to generated stencil data.

#![allow(missing_docs)]
#![allow(unused_imports)]

use crate::stencil::data::StencilLookup;
use crate::stencil::data::StencilLookup::{Found, Rejected};
use crate::stencil::data::reject as generic;
use crate::stencil::types::{HelperFn, HoleKind, RegField, RelocKind, Stencil, StencilReloc};
use helm_arch::aarch64::insn::{Instruction, Opcode};

/// AArch64-specific reject reason constants.
///
/// These extend the generic reasons in [`super::reject`] with
/// architecture-specific detail. The `jit_rejects` plugin reports
/// them as-is, so users see the exact reason in the histogram.
pub mod reject {
    pub const LDRSB_W_FORM: &str = "ldrsb-w-form";
    pub const LDRSH_W_FORM: &str = "ldrsh-w-form";
    pub const LDP_STP_DISABLED: &str = "ldp-stp-disabled";
}

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
/// Returns `Some(Found(&Stencil))` for supported opcodes,
/// `Some(Rejected(reason))` for recognized-but-unsupported opcodes,
/// and `None` for completely unknown opcodes (first-insn failure).
pub fn lookup(insn: &Instruction) -> Option<StencilLookup> {
    // Stencil templates operate on 64-bit X registers. For sf=0 (W-register)
    // instructions, the compiler emits a DWORD-zero epilogue to clear the upper
    // 32 bits of Rd after the stencil runs (AArch64 W-write zero-extension).
    //
    // Opcodes that are still unsafe for sf=0 and must be rejected:
    //   - Flag-setting: AddsImm/SubsImm/AndsImm/AddsReg/SubsReg — 32-bit flag
    //     semantics differ from 64-bit (N/V bits depend on operand width).
    //   - Bit-width-dependent: Clz/Rev/Extr — result depends on operand size.
    let needs_sf_reject = matches!(
        insn.opcode,
        Opcode::AddsImm
            | Opcode::SubsImm
            | Opcode::AndsImm
            | Opcode::AddsReg
            | Opcode::SubsReg
            | Opcode::Clz
            | Opcode::Rev
            | Opcode::Extr
    );
    if needs_sf_reject && !insn.sf {
        return Some(Rejected(generic::W_REGISTER));
    }

    // Stencil register-operation templates don't apply shifts (LSL/LSR/ASR).
    // Reject shifted-register instructions so the dynasm backend handles them.
    let is_shifted_reg = matches!(
        insn.opcode,
        Opcode::AddReg
            | Opcode::SubReg
            | Opcode::AddsReg
            | Opcode::SubsReg
            | Opcode::AndReg
            | Opcode::OrrReg
            | Opcode::EorReg
            | Opcode::OrnReg
            | Opcode::BicReg
    ) && (insn.shift_amt != 0 || insn.shift_type != 0);
    if is_shifted_reg {
        return Some(Rejected(generic::SHIFTED_REGISTER));
    }

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

        // Bitfield — split into extraction (imms >= immr) and insertion (imms < immr)
        Opcode::Sbfm => {
            if insn.imm2 >= insn.imm as u64 {
                &STENCIL_SBFM_EXT
            } else {
                &STENCIL_SBFM_INS
            }
        }
        Opcode::Ubfm => {
            if insn.imm2 >= insn.imm as u64 {
                &STENCIL_UBFM_EXT
            } else {
                &STENCIL_UBFM_INS
            }
        }

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
        Opcode::Ldr if is_complex_addressing(insn) => return Some(Rejected(generic::COMPLEX_ADDRESSING)),
        Opcode::Ldr => match insn.size {
            3 => &STENCIL_LDR64,
            2 => &STENCIL_LDR32,
            1 => &STENCIL_LDR16,
            0 => &STENCIL_LDR8,
            _ => return Some(Rejected(generic::UNSUPPORTED_SIZE)),
        },
        Opcode::Ldrb if is_complex_addressing(insn) => return Some(Rejected(generic::COMPLEX_ADDRESSING)),
        Opcode::Ldrb => &STENCIL_LDR8,
        Opcode::Ldrh if is_complex_addressing(insn) => return Some(Rejected(generic::COMPLEX_ADDRESSING)),
        Opcode::Ldrh => &STENCIL_LDR16,
        Opcode::Ldrsw if is_complex_addressing(insn) => return Some(Rejected(generic::COMPLEX_ADDRESSING)),
        Opcode::Ldrsw => &STENCIL_LDRSW,
        Opcode::Ldrsh if is_complex_addressing(insn) => return Some(Rejected(generic::COMPLEX_ADDRESSING)),
        Opcode::Ldrsh if !insn.sf => return Some(Rejected(reject::LDRSH_W_FORM)),
        Opcode::Ldrsh => &STENCIL_LDRSH,
        Opcode::Ldrsb if is_complex_addressing(insn) => return Some(Rejected(generic::COMPLEX_ADDRESSING)),
        Opcode::Ldrsb if !insn.sf => return Some(Rejected(reject::LDRSB_W_FORM)),
        Opcode::Ldrsb => &STENCIL_LDRSB,

        // Stores — immediate offset only
        Opcode::Str if is_complex_addressing(insn) => return Some(Rejected(generic::COMPLEX_ADDRESSING)),
        Opcode::Str => match insn.size {
            3 => &STENCIL_STR64,
            2 => &STENCIL_STR32,
            1 => &STENCIL_STR16,
            0 => &STENCIL_STR8,
            _ => return Some(Rejected(generic::UNSUPPORTED_SIZE)),
        },
        Opcode::Strb if is_complex_addressing(insn) => return Some(Rejected(generic::COMPLEX_ADDRESSING)),
        Opcode::Strb => &STENCIL_STR8,
        Opcode::Strh if is_complex_addressing(insn) => return Some(Rejected(generic::COMPLEX_ADDRESSING)),
        Opcode::Strh => &STENCIL_STR16,

        // Load/store pair — immediate offset only.
        // Note: insn.size for LDP/STP is the raw opc field, not element size.
        // Use insn.sf (set by decode_ldst_pair) to distinguish 32/64-bit.
        Opcode::Ldp if is_complex_addressing(insn) => return Some(Rejected(generic::COMPLEX_ADDRESSING)),
        Opcode::Ldp => if insn.sf { &STENCIL_LDP64 } else { &STENCIL_LDP32 },
        Opcode::Stp if is_complex_addressing(insn) => return Some(Rejected(generic::COMPLEX_ADDRESSING)),
        Opcode::Stp => if insn.sf { &STENCIL_STP64 } else { &STENCIL_STP32 },

        // Unscaled loads/stores
        Opcode::Ldur if is_complex_addressing(insn) => return Some(Rejected(generic::COMPLEX_ADDRESSING)),
        Opcode::Ldur => match insn.size {
            3 => &STENCIL_LDUR64,
            2 => &STENCIL_LDUR32,
            _ => return Some(Rejected(generic::UNSUPPORTED_SIZE)),
        },
        Opcode::Stur if is_complex_addressing(insn) => return Some(Rejected(generic::COMPLEX_ADDRESSING)),
        Opcode::Stur => match insn.size {
            3 => &STENCIL_STUR64,
            2 => &STENCIL_STUR32,
            _ => return Some(Rejected(generic::UNSUPPORTED_SIZE)),
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
        _ => return Some(Rejected(generic::OPCODE_UNIMPLEMENTED)),
    };
    Some(Found(s))
}

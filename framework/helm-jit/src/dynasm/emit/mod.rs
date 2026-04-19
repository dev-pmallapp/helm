//! Emitter dispatch — routes decoded AArch64 instructions to per-category
//! x86-64 code generators.

#![allow(missing_docs)]

use dynasmrt::x64::Assembler;
use helm_arch::aarch64::insn::{Instruction, Opcode};

pub mod branch;
pub mod dp;
pub mod fused;
pub mod ldst;
pub mod simd;
pub mod system;

/// Emit x86-64 code for one AArch64 instruction.
///
/// `insn_idx` is the 0-based index of this instruction within the block,
/// used by conditional branches to write the correct retired count on
/// their taken-path exit.
///
/// # Returns
/// - `Some(false)` — instruction emitted, block continues
/// - `Some(true)`  — instruction emitted, block terminates (branch)
/// - `None`        — opcode unsupported, block compilation stops here
///                    (caller falls back to interpreter)
pub fn emit_insn(
    ops: &mut Assembler,
    insn: &Instruction,
    patch_sites: &mut Vec<crate::block::PatchSite>,
    insn_idx: u32,
) -> Option<bool> {
    match insn.opcode {
        // ── Data processing — immediate ─────────────────────────────────────
        Opcode::Adr => {
            dp::emit_adr(ops, insn);
            Some(false)
        }
        Opcode::Adrp => {
            dp::emit_adrp(ops, insn);
            Some(false)
        }
        Opcode::AddImm | Opcode::SubImm => {
            dp::emit_add_sub_imm(ops, insn);
            Some(false)
        }
        Opcode::AddsImm | Opcode::SubsImm => {
            dp::emit_adds_subs_imm(ops, insn);
            Some(false)
        }
        Opcode::AndImm => {
            dp::emit_logical_imm(ops, insn);
            Some(false)
        }
        Opcode::OrrImm => {
            dp::emit_logical_imm(ops, insn);
            Some(false)
        }
        Opcode::EorImm => {
            dp::emit_logical_imm(ops, insn);
            Some(false)
        }
        Opcode::AndsImm => {
            dp::emit_ands_imm(ops, insn);
            Some(false)
        }
        Opcode::AndsReg => {
            dp::emit_ands_reg(ops, insn);
            Some(false)
        }
        Opcode::Movz => {
            dp::emit_movz(ops, insn);
            Some(false)
        }
        Opcode::Movk => {
            dp::emit_movk(ops, insn);
            Some(false)
        }
        Opcode::Movn => {
            dp::emit_movn(ops, insn);
            Some(false)
        }
        Opcode::Ubfm => {
            dp::emit_ubfm(ops, insn);
            Some(false)
        }

        // ── Data processing — register ──────────────────────────────────────
        Opcode::AddReg | Opcode::SubReg => {
            dp::emit_add_sub_reg(ops, insn);
            Some(false)
        }
        Opcode::AddExt | Opcode::SubExt => {
            dp::emit_add_sub_ext(ops, insn);
            Some(false)
        }
        Opcode::AddsReg | Opcode::SubsReg => {
            dp::emit_adds_subs_reg(ops, insn);
            Some(false)
        }
        Opcode::Ccmp | Opcode::Ccmn => {
            dp::emit_cond_cmp(ops, insn);
            Some(false)
        }
        Opcode::Csel | Opcode::Csinc | Opcode::Csinv | Opcode::Csneg => {
            dp::emit_cond_select(ops, insn);
            Some(false)
        }
        Opcode::Sbfm => {
            dp::emit_sbfm(ops, insn);
            Some(false)
        }
        Opcode::Lsl | Opcode::Lsr | Opcode::Asr | Opcode::Ror => {
            dp::emit_shift_reg(ops, insn);
            Some(false)
        }
        Opcode::Clz => {
            dp::emit_clz(ops, insn);
            Some(false)
        }
        Opcode::Rev => {
            dp::emit_rev(ops, insn);
            Some(false)
        }
        Opcode::Rev16 => {
            dp::emit_rev16(ops, insn);
            Some(false)
        }
        Opcode::Rev32 => {
            dp::emit_rev32(ops, insn);
            Some(false)
        }
        Opcode::Rbit => {
            dp::emit_rbit(ops, insn);
            Some(false)
        }
        Opcode::Extr => {
            dp::emit_extr(ops, insn);
            Some(false)
        }
        Opcode::Bfm => {
            dp::emit_bfm(ops, insn);
            Some(false)
        }
        Opcode::OrnReg | Opcode::EonReg | Opcode::BicReg | Opcode::BicsReg => {
            dp::emit_logical_neg_reg(ops, insn);
            Some(false)
        }
        Opcode::Sdiv => {
            dp::emit_sdiv(ops, insn);
            Some(false)
        }
        Opcode::Udiv => {
            dp::emit_udiv(ops, insn);
            Some(false)
        }
        Opcode::AddsExt | Opcode::SubsExt => {
            dp::emit_adds_subs_ext(ops, insn);
            Some(false)
        }
        Opcode::Adc | Opcode::Adcs => {
            dp::emit_adc(ops, insn);
            Some(false)
        }
        Opcode::Sbc | Opcode::Sbcs => {
            dp::emit_sbc(ops, insn);
            Some(false)
        }
        Opcode::Madd | Opcode::Mul => {
            dp::emit_madd(ops, insn);
            Some(false)
        }
        Opcode::Msub | Opcode::Mneg => {
            dp::emit_msub(ops, insn);
            Some(false)
        }
        Opcode::Smulh => {
            dp::emit_smulh(ops, insn);
            Some(false)
        }
        Opcode::Umulh => {
            dp::emit_umulh(ops, insn);
            Some(false)
        }
        Opcode::AndReg | Opcode::OrrReg | Opcode::EorReg => {
            dp::emit_logical_reg(ops, insn);
            Some(false)
        }

        // ── Load/Store ──────────────────────────────────────────────────────
        Opcode::Ldr
        | Opcode::Ldrb
        | Opcode::Ldrh
        | Opcode::Ldrsb
        | Opcode::Ldrsh
        | Opcode::Ldrsw => {
            ldst::emit_ldr_imm(ops, insn);
            Some(false)
        }
        Opcode::Str | Opcode::Strb | Opcode::Strh => {
            ldst::emit_str_imm(ops, insn);
            Some(false)
        }
        Opcode::Ldp => {
            ldst::emit_ldp(ops, insn);
            Some(false)
        }
        Opcode::Stp => {
            ldst::emit_stp(ops, insn);
            Some(false)
        }
        Opcode::Ldur
        | Opcode::Ldurb
        | Opcode::Ldurh
        | Opcode::Ldursb
        | Opcode::Ldursh
        | Opcode::Ldursw => {
            ldst::emit_ldr_imm(ops, insn);
            Some(false)
        }
        Opcode::Stur | Opcode::Sturb | Opcode::Sturh => {
            ldst::emit_str_imm(ops, insn);
            Some(false)
        }
        Opcode::Ldar => {
            ldst::emit_ldr_imm(ops, insn);
            Some(false)
        }
        Opcode::Stlr => {
            ldst::emit_str_imm(ops, insn);
            Some(false)
        }
        Opcode::Ldapr | Opcode::Ldaprh | Opcode::Ldaprb => {
            ldst::emit_ldr_imm(ops, insn);
            Some(false)
        }
        Opcode::Prfm => Some(false), // prefetch hint = NOP

        // ── Branches ────────────────────────────────────────────────────────
        Opcode::B => {
            branch::emit_b(ops, insn, insn_idx);
            Some(true)
        }
        Opcode::Bl => {
            branch::emit_bl(ops, insn, insn_idx);
            Some(true)
        }
        Opcode::Br => {
            branch::emit_br(ops, insn, insn_idx);
            Some(true)
        }
        Opcode::Blr => {
            branch::emit_blr(ops, insn, insn_idx);
            Some(true)
        }
        Opcode::Ret => {
            branch::emit_ret(ops, insn, insn_idx);
            Some(true)
        }
        Opcode::Cbz => {
            branch::emit_cbz(ops, insn, patch_sites, insn_idx);
            Some(false)
        }
        Opcode::Cbnz => {
            branch::emit_cbnz(ops, insn, patch_sites, insn_idx);
            Some(false)
        }
        Opcode::BCond => {
            branch::emit_bcond(ops, insn, patch_sites, insn_idx);
            Some(false)
        }
        Opcode::Tbz => {
            branch::emit_tbz(ops, insn, patch_sites, insn_idx);
            Some(false)
        }
        Opcode::Tbnz => {
            branch::emit_tbnz(ops, insn, patch_sites, insn_idx);
            Some(false)
        }

        // ── SIMD ────────────────────────────────────────────────────────────
        Opcode::SimdDup => simd::emit_simd_dup(ops, insn),
        Opcode::StrSimd => simd::emit_str_simd(ops, insn),
        Opcode::StpSimd => simd::emit_stp_simd(ops, insn),

        // ── System / unsupported ────────────────────────────────────────────
        Opcode::Svc
        | Opcode::Eret
        | Opcode::Mrs
        | Opcode::Msr
        | Opcode::MsrImm
        | Opcode::Sys
        | Opcode::Wfi
        | Opcode::Wfe
        | Opcode::DcZva
        | Opcode::Hvc
        | Opcode::Smc
        | Opcode::Nop
        | Opcode::Brk => system::emit_system(ops, insn, insn_idx),

        // Everything else: unsupported — stop block compilation
        _ => None,
    }
}

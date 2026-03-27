//! Emitter dispatch — routes decoded AArch64 instructions to per-category
//! x86-64 code generators.

#![allow(missing_docs)]

use dynasmrt::x64::Assembler;
use helm_arch::aarch64::insn::{Instruction, Opcode};

pub mod dp;
pub mod ldst;
pub mod branch;
pub mod system;

/// Emit x86-64 code for one AArch64 instruction.
///
/// # Returns
/// - `Some(false)` — instruction emitted, block continues
/// - `Some(true)`  — instruction emitted, block terminates (branch)
/// - `None`        — opcode unsupported, block compilation stops here
///                    (caller falls back to interpreter)
pub fn emit_insn(ops: &mut Assembler, insn: &Instruction) -> Option<bool> {
    match insn.opcode {
        // ── Data processing — immediate ─────────────────────────────────────
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

        // ── Data processing — register ──────────────────────────────────────
        Opcode::AddReg | Opcode::SubReg => {
            dp::emit_add_sub_reg(ops, insn);
            Some(false)
        }
        Opcode::AddsReg | Opcode::SubsReg => {
            dp::emit_adds_subs_reg(ops, insn);
            Some(false)
        }

        // ── Load/Store ──────────────────────────────────────────────────────
        Opcode::Ldr | Opcode::Ldrb | Opcode::Ldrh | Opcode::Ldrsb | Opcode::Ldrsh | Opcode::Ldrsw => {
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

        // ── Branches (all terminate the block) ──────────────────────────────
        Opcode::B => {
            branch::emit_b(ops, insn);
            Some(true)
        }
        Opcode::Bl => {
            branch::emit_bl(ops, insn);
            Some(true)
        }
        Opcode::Br => {
            branch::emit_br(ops, insn);
            Some(true)
        }
        Opcode::Blr => {
            branch::emit_blr(ops, insn);
            Some(true)
        }
        Opcode::Ret => {
            branch::emit_ret(ops, insn);
            Some(true)
        }
        Opcode::BCond => {
            branch::emit_bcond(ops, insn);
            Some(true)
        }
        Opcode::Cbz => {
            branch::emit_cbz(ops, insn);
            Some(true)
        }
        Opcode::Cbnz => {
            branch::emit_cbnz(ops, insn);
            Some(true)
        }
        Opcode::Tbz => {
            branch::emit_tbz(ops, insn);
            Some(true)
        }
        Opcode::Tbnz => {
            branch::emit_tbnz(ops, insn);
            Some(true)
        }

        // ── System / unsupported ────────────────────────────────────────────
        Opcode::Svc | Opcode::Eret | Opcode::Mrs | Opcode::Msr | Opcode::MsrImm
        | Opcode::Sys | Opcode::Wfi | Opcode::Wfe | Opcode::DcZva
        | Opcode::Hvc | Opcode::Smc | Opcode::Nop | Opcode::Brk => {
            system::emit_system(ops, insn)
        }

        // Everything else: unsupported — stop block compilation
        _ => None,
    }
}

//! Data-processing instruction emitters (AArch64 → x86-64).
//!
//! All functions take `(ops: &mut Assembler, insn: &Instruction)` and emit
//! x86-64 code that operates on the flat register array pointed to by `rdi`.
//!
//! ## Register usage convention (post register-pinning)
//! - `rdi` — pointer to flat register array (preserved across the block)
//! - `rsi` — pointer to FlatMem (preserved across the block)
//! - `r8`  — pinned to guest X0  (DO NOT use as scratch!)
//! - `r9`  — pinned to guest X1  (DO NOT use as scratch!)
//! - `r10` — pinned to guest X2  (DO NOT use as scratch!)
//! - `r11` — pinned to guest X3  (DO NOT use as scratch!)
//! - `rbx` — pinned to guest X4  (DO NOT use as scratch!)
//! - `r12` — pinned to guest X19 (DO NOT use as scratch!)
//! - `r13` — pinned to guest X20 (DO NOT use as scratch!)
//! - `r14` — pinned to guest X30/LR (DO NOT use as scratch!)
//! - `r15` — pinned to guest SP  (DO NOT use as scratch!)
//! - `rbp` — pinned to guest NZCV (DO NOT use as scratch!)
//! - `rax`, `rcx`, `rdx` — safe scratch registers (caller-saved, not pinned)

#![allow(missing_docs)]
#![allow(clippy::similar_names)]

use crate::dynasm::lazy_nzcv::{emit_defer_nzcv_imm, emit_defer_nzcv_reg};
use crate::dynasm::pinned::{
    load_guest_to_eax, load_guest_to_rax, load_guest_to_rcx, store_eax_to_guest_32,
    store_rax_to_guest,
};
use crate::regs::{FlagOp, REG_SP, REG_XZR};
use dynasm::dynasm;
use dynasmrt::{x64::Assembler, DynasmApi};
use helm_arch::aarch64::insn::{Instruction, Opcode};

/// Guest slot for data-processing source: reg 31 = XZR (zero).
#[inline]
fn src_slot(reg: u32) -> usize {
    if reg == 31 {
        REG_XZR
    } else {
        reg as usize
    }
}

/// Guest slot for data-processing destination: reg 31 = XZR.
#[inline]
fn dst_slot(reg: u32) -> usize {
    if reg == 31 {
        REG_XZR
    } else {
        reg as usize
    }
}

/// Guest slot in SP context: reg 31 = SP.
#[inline]
fn sp_slot(reg: u32) -> usize {
    if reg == 31 {
        REG_SP
    } else {
        reg as usize
    }
}

// ── ADD / SUB immediate (non-flag-setting) ──────────────────────────────────

/// Emit `ADD/SUB Xd|SP, Xn|SP, #imm{, shift}`.
///
/// Non-flag-setting: rd and rn use SP encoding (reg 31 = SP).
pub fn emit_add_sub_imm(ops: &mut Assembler, insn: &Instruction) {
    let is_sub = insn.opcode == Opcode::SubImm;
    let rn = sp_slot(insn.rn);
    let rd = sp_slot(insn.rd);
    let imm = insn.imm;

    if insn.sf {
        load_guest_to_rax(ops, rn);
        if is_sub {
            dynasm!(ops ; mov rcx, QWORD imm as i64 ; sub rax, rcx);
        } else {
            dynasm!(ops ; mov rcx, QWORD imm as i64 ; add rax, rcx);
        }
        store_rax_to_guest(ops, rd);
    } else {
        load_guest_to_eax(ops, rn);
        if is_sub {
            dynasm!(ops ; sub eax, imm as i32);
        } else {
            dynasm!(ops ; add eax, imm as i32);
        }
        store_eax_to_guest_32(ops, rd);
    }
}

// ── ADDS / SUBS immediate (flag-setting) ────────────────────────────────────

/// Emit `ADDS/SUBS Xd, Xn, #imm{, shift}`.
///
/// Flag-setting: rd and rn use XZR encoding (reg 31 = XZR).
/// Uses lazy NZCV: stores FlagOp+operands; flags materialized at next B.cond.
pub fn emit_adds_subs_imm(ops: &mut Assembler, insn: &Instruction) {
    let is_sub = insn.opcode == Opcode::SubsImm;
    let rn = src_slot(insn.rn);
    let rd = dst_slot(insn.rd);
    let imm = insn.imm;

    if insn.sf {
        // Load Rn into rcx (lhs), compute result in rax.
        load_guest_to_rcx(ops, rn);
        dynasm!(ops ; mov rax, rcx);
        if is_sub {
            dynasm!(ops ; mov rdx, QWORD imm as i64 ; sub rax, rdx);
        } else {
            dynasm!(ops ; mov rdx, QWORD imm as i64 ; add rax, rdx);
        }
        let flag_op = if is_sub { FlagOp::Sub64 } else { FlagOp::Add64 };
        emit_defer_nzcv_imm(ops, flag_op, true, imm);
        store_rax_to_guest(ops, rd);
    } else {
        // 32-bit: load Rn into rcx (lhs), compute result in eax.
        load_guest_to_rcx(ops, rn);
        dynasm!(ops ; mov eax, ecx);
        if is_sub {
            dynasm!(ops ; sub eax, imm as i32);
        } else {
            dynasm!(ops ; add eax, imm as i32);
        }
        let flag_op = if is_sub { FlagOp::Sub32 } else { FlagOp::Add32 };
        emit_defer_nzcv_imm(ops, flag_op, true, imm);
        // Zero-extend 32-bit result and store.
        dynasm!(ops ; mov eax, eax); // zero-extends into rax
        store_rax_to_guest(ops, rd);
    }
}

// ── ADD / SUB register (non-flag-setting) ───────────────────────────────────

/// Emit `ADD/SUB Xd, Xn, Xm{, shift #amt}`.
pub fn emit_add_sub_reg(ops: &mut Assembler, insn: &Instruction) {
    let is_sub = insn.opcode == Opcode::SubReg;
    let rn = sp_slot(insn.rn);
    let rm = src_slot(insn.rm);
    let rd = sp_slot(insn.rd);

    if insn.sf {
        load_guest_to_rax(ops, rn);
        load_guest_to_rcx(ops, rm);
        emit_apply_shift_64(ops, insn.shift_type, insn.shift_amt);
        if is_sub {
            dynasm!(ops ; sub rax, rcx);
        } else {
            dynasm!(ops ; add rax, rcx);
        }
        store_rax_to_guest(ops, rd);
    } else {
        load_guest_to_eax(ops, rn);
        load_guest_to_rcx(ops, rm);
        emit_apply_shift_32(ops, insn.shift_type, insn.shift_amt);
        if is_sub {
            dynasm!(ops ; sub eax, ecx);
        } else {
            dynasm!(ops ; add eax, ecx);
        }
        store_eax_to_guest_32(ops, rd);
    }
}

// ── ADDS / SUBS register (flag-setting) ─────────────────────────────────────

/// Emit `ADDS/SUBS Xd, Xn, Xm{, shift #amt}`.
/// Uses lazy NZCV: stores FlagOp+operands; flags materialized at next B.cond.
/// Convention: lhs (Rn) → rdx, rhs (Rm post-shift) → rcx, result → rax.
pub fn emit_adds_subs_reg(ops: &mut Assembler, insn: &Instruction) {
    let is_sub = insn.opcode == Opcode::SubsReg;
    let rn = src_slot(insn.rn);
    let rm = src_slot(insn.rm);
    let rd = dst_slot(insn.rd);

    if insn.sf {
        load_guest_to_rax(ops, rn);
        load_guest_to_rcx(ops, rm);
        emit_apply_shift_64(ops, insn.shift_type, insn.shift_amt);
        // Save lhs to rdx for emit_defer_nzcv_reg before arithmetic clobbers rax.
        dynasm!(ops ; mov rdx, rax);
        if is_sub {
            dynasm!(ops ; sub rax, rcx);
        } else {
            dynasm!(ops ; add rax, rcx);
        }
        let flag_op = if is_sub { FlagOp::Sub64 } else { FlagOp::Add64 };
        emit_defer_nzcv_reg(ops, flag_op);
        store_rax_to_guest(ops, rd);
    } else {
        load_guest_to_eax(ops, rn);
        load_guest_to_rcx(ops, rm);
        emit_apply_shift_32(ops, insn.shift_type, insn.shift_amt);
        // Save lhs to rdx (zero-extended from eax) before arithmetic.
        dynasm!(ops ; mov rdx, rax);
        if is_sub {
            dynasm!(ops ; sub eax, ecx);
        } else {
            dynasm!(ops ; add eax, ecx);
        }
        let flag_op = if is_sub { FlagOp::Sub32 } else { FlagOp::Add32 };
        emit_defer_nzcv_reg(ops, flag_op);
        dynasm!(ops ; mov eax, eax); // zero-extend 32-bit result
        store_rax_to_guest(ops, rd);
    }
}

// ── AND / ORR / EOR immediate (non-flag-setting) ────────────────────────────

/// Emit `AND/ORR/EOR Xd|SP, Xn, #imm`.
///
/// Non-flag-setting logical: rd uses SP encoding (reg 31 = SP).
pub fn emit_logical_imm(ops: &mut Assembler, insn: &Instruction) {
    let rn = src_slot(insn.rn);
    let rd = sp_slot(insn.rd);
    let imm = insn.imm;

    if insn.sf {
        load_guest_to_rax(ops, rn);
        dynasm!(ops ; mov rcx, QWORD imm as i64);
        match insn.opcode {
            Opcode::AndImm => dynasm!(ops ; and rax, rcx),
            Opcode::OrrImm => dynasm!(ops ; or rax, rcx),
            Opcode::EorImm => dynasm!(ops ; xor rax, rcx),
            _ => unreachable!(),
        }
        store_rax_to_guest(ops, rd);
    } else {
        load_guest_to_eax(ops, rn);
        match insn.opcode {
            Opcode::AndImm => dynasm!(ops ; and eax, imm as i32),
            Opcode::OrrImm => dynasm!(ops ; or eax, imm as i32),
            Opcode::EorImm => dynasm!(ops ; xor eax, imm as i32),
            _ => unreachable!(),
        }
        store_eax_to_guest_32(ops, rd);
    }
}

// ── ANDS immediate (flag-setting) ───────────────────────────────────────────

/// Emit `ANDS Xd, Xn, #imm` — flag-setting AND.
/// Uses lazy NZCV: stores FlagOp::And64/And32 + operands.
pub fn emit_ands_imm(ops: &mut Assembler, insn: &Instruction) {
    let rn = src_slot(insn.rn);
    let rd = dst_slot(insn.rd);
    let imm = insn.imm;

    if insn.sf {
        load_guest_to_rcx(ops, rn);
        dynasm!(ops ; mov rax, rcx);
        dynasm!(ops ; mov rdx, QWORD imm as i64 ; and rax, rdx);
        emit_defer_nzcv_imm(ops, FlagOp::And64, true, imm);
        store_rax_to_guest(ops, rd);
    } else {
        load_guest_to_rcx(ops, rn);
        dynasm!(ops ; mov eax, ecx ; and eax, imm as i32);
        emit_defer_nzcv_imm(ops, FlagOp::And32, true, imm);
        dynasm!(ops ; mov eax, eax);
        store_rax_to_guest(ops, rd);
    }
}

// ── MOV immediate variants ──────────────────────────────────────────────────
//
// The decoder pre-computes the final value:
//   Movz: insn.imm = imm16 << (hw * 16)          — already shifted
//   Movn: insn.imm = !(imm16 << (hw * 16))       — already inverted+shifted
//   Movk: insn.imm = raw imm16, insn.imm2 = hw   — executor applies shift
//
// Movz and Movn emitters just write insn.imm (with 32-bit masking if !sf).

/// Emit `MOVZ Xd, #imm{, LSL #shift}`.
pub fn emit_movz(ops: &mut Assembler, insn: &Instruction) {
    let rd = dst_slot(insn.rd);
    let val = if insn.sf {
        insn.imm as u64
    } else {
        insn.imm as u64 & 0xFFFF_FFFF
    };
    dynasm!(ops ; mov rax, QWORD val as i64);
    store_rax_to_guest(ops, rd);
}

/// Emit `MOVK Xd, #imm{, LSL #shift}` — keep other bits.
pub fn emit_movk(ops: &mut Assembler, insn: &Instruction) {
    let rd = dst_slot(insn.rd);
    let shift = insn.imm2 * 16;
    let mask = !(0xFFFF_u64 << shift);
    let bits = (insn.imm as u64 & 0xFFFF) << shift;

    load_guest_to_rax(ops, rd);
    dynasm!(ops
        ; mov rcx, QWORD mask as i64
        ; and rax, rcx
        ; mov rcx, QWORD bits as i64
        ; or rax, rcx
    );
    store_rax_to_guest(ops, rd);
}

/// Emit `MOVN Xd, #imm{, LSL #shift}` — value already inverted by decoder.
pub fn emit_movn(ops: &mut Assembler, insn: &Instruction) {
    let rd = dst_slot(insn.rd);
    let val = if insn.sf {
        insn.imm as u64
    } else {
        insn.imm as u64 & 0xFFFF_FFFF
    };
    dynasm!(ops ; mov rax, QWORD val as i64);
    store_rax_to_guest(ops, rd);
}

// ── Shift helpers ───────────────────────────────────────────────────────────

/// Apply shift to `rcx` (64-bit): shift_type 0=LSL, 1=LSR, 2=ASR, 3=ROR.
fn emit_apply_shift_64(ops: &mut Assembler, shift_type: u32, shift_amt: u32) {
    if shift_amt == 0 {
        return;
    }
    let amt = shift_amt as i8;
    match shift_type {
        0 => dynasm!(ops ; shl rcx, amt),
        1 => dynasm!(ops ; shr rcx, amt),
        2 => dynasm!(ops ; sar rcx, amt),
        3 => dynasm!(ops ; ror rcx, amt),
        _ => unreachable!(),
    }
}

/// Apply shift to `ecx` (32-bit).
fn emit_apply_shift_32(ops: &mut Assembler, shift_type: u32, shift_amt: u32) {
    if shift_amt == 0 {
        return;
    }
    let amt = shift_amt as i8;
    match shift_type {
        0 => dynasm!(ops ; shl ecx, amt),
        1 => dynasm!(ops ; shr ecx, amt),
        2 => dynasm!(ops ; sar ecx, amt),
        3 => dynasm!(ops ; ror ecx, amt),
        _ => unreachable!(),
    }
}

// NZCV is now captured lazily via emit_defer_nzcv_imm / emit_defer_nzcv_reg
// (defined in dynasm/lazy_nzcv.rs). Eager capture helpers have been removed.

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

use crate::dynasm::emit::branch::emit_cond_check;
use crate::dynasm::lazy_nzcv::{
    emit_capture_from_rflags, emit_defer_nzcv_imm, emit_defer_nzcv_reg, emit_materialize_nzcv,
};
use crate::dynasm::pinned::{
    load_guest_to_eax, load_guest_to_rax, load_guest_to_rcx, store_eax_to_guest_32,
    store_rax_to_guest,
};
use crate::regs::{reg_offset, FlagOp, REG_FLAG_OP, REG_SP, REG_XZR};
use dynasm::dynasm;
use dynasmrt::{x64::Assembler, DynasmApi, DynasmLabelApi};
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

/// Emit `ADR Xd, #imm`.
pub fn emit_adr(ops: &mut Assembler, insn: &Instruction) {
    let rd = dst_slot(insn.rd);
    let target = insn.pc.wrapping_add(insn.imm as u64);
    dynasm!(ops ; mov rax, QWORD target as i64);
    store_rax_to_guest(ops, rd);
}

/// Emit `ADRP Xd, #imm`.
pub fn emit_adrp(ops: &mut Assembler, insn: &Instruction) {
    let rd = dst_slot(insn.rd);
    let page_base = insn.pc & !0xFFF;
    let target = page_base.wrapping_add((insn.imm as u64) << 12);
    dynasm!(ops ; mov rax, QWORD target as i64);
    store_rax_to_guest(ops, rd);
}

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

/// Emit `ADD/SUB Xd|SP, Xn|SP, Rm, <extend> {#amt}`.
pub fn emit_add_sub_ext(ops: &mut Assembler, insn: &Instruction) {
    let is_sub = insn.opcode == Opcode::SubExt;
    let rn = sp_slot(insn.rn);
    let rm = src_slot(insn.rm);
    let rd = sp_slot(insn.rd);

    if insn.sf {
        load_guest_to_rax(ops, rn);
        load_guest_to_rcx(ops, rm);
        emit_apply_extend(ops, insn.extend_type, insn.extend_amt);
        if is_sub {
            dynasm!(ops ; sub rax, rcx);
        } else {
            dynasm!(ops ; add rax, rcx);
        }
        store_rax_to_guest(ops, rd);
    } else {
        load_guest_to_eax(ops, rn);
        load_guest_to_rcx(ops, rm);
        emit_apply_extend(ops, insn.extend_type, insn.extend_amt);
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

/// Emit `CCMP/CCMN` (conditional compare).
///
/// This eagerly materializes and updates NZCV because the instruction is pure
/// flag traffic and does not produce a general-purpose register result.
pub fn emit_cond_cmp(ops: &mut Assembler, insn: &Instruction) {
    let rn = src_slot(insn.rn);
    let flag_op_off = reg_offset(REG_FLAG_OP);
    let cond_false_nzcv = (insn.nzcv_imm << 28) as i32;
    let use_imm = ((insn.raw >> 11) & 1) != 0;
    let is_ccmp = insn.opcode == Opcode::Ccmp;

    emit_materialize_nzcv(ops);
    emit_cond_check(ops, insn.cond);

    if insn.sf {
        load_guest_to_rax(ops, rn);
        if use_imm {
            dynasm!(ops ; mov rcx, QWORD insn.imm);
        } else {
            load_guest_to_rcx(ops, src_slot(insn.rm));
        }
        if is_ccmp {
            dynasm!(ops ; cmp rax, rcx);
            emit_capture_from_rflags(ops, true);
        } else {
            dynasm!(ops ; add rax, rcx);
            emit_capture_from_rflags(ops, false);
        }
    } else {
        load_guest_to_eax(ops, rn);
        if use_imm {
            dynasm!(ops ; mov ecx, insn.imm as i32);
        } else {
            load_guest_to_rcx(ops, src_slot(insn.rm));
        }
        if is_ccmp {
            dynasm!(ops ; cmp eax, ecx);
            emit_capture_from_rflags(ops, true);
        } else {
            dynasm!(ops ; add eax, ecx);
            emit_capture_from_rflags(ops, false);
        }
    }

    dynasm!(ops
        ; jmp >done
        ; not_taken:
        ; mov ebp, cond_false_nzcv
        ; mov DWORD [rdi + flag_op_off], 0
        ; done:
    );
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

// ── AND / ORR / EOR register (non-flag-setting) ────────────────────────────

/// Emit `AND/ORR/EOR Xd, Xn, Xm{, shift #amt}`.
pub fn emit_logical_reg(ops: &mut Assembler, insn: &Instruction) {
    let rn = src_slot(insn.rn);
    let rm = src_slot(insn.rm);
    let rd = dst_slot(insn.rd);

    if insn.sf {
        load_guest_to_rax(ops, rn);
        load_guest_to_rcx(ops, rm);
        emit_apply_shift_64(ops, insn.shift_type, insn.shift_amt);
        match insn.opcode {
            Opcode::AndReg => dynasm!(ops ; and rax, rcx),
            Opcode::OrrReg => dynasm!(ops ; or rax, rcx),
            Opcode::EorReg => dynasm!(ops ; xor rax, rcx),
            _ => unreachable!(),
        }
        store_rax_to_guest(ops, rd);
    } else {
        load_guest_to_eax(ops, rn);
        load_guest_to_rcx(ops, rm);
        emit_apply_shift_32(ops, insn.shift_type, insn.shift_amt);
        match insn.opcode {
            Opcode::AndReg => dynasm!(ops ; and eax, ecx),
            Opcode::OrrReg => dynasm!(ops ; or eax, ecx),
            Opcode::EorReg => dynasm!(ops ; xor eax, ecx),
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

// ── UBFM (unsigned bitfield move) ───────────────────────────────────────────

#[inline]
fn low_mask(width: u32) -> u64 {
    if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    }
}

/// Emit `UBFM Xd, Xn, #immr, #imms`.
pub fn emit_ubfm(ops: &mut Assembler, insn: &Instruction) {
    let rn = src_slot(insn.rn);
    let rd = dst_slot(insn.rd);
    let immr = insn.imm as u32;
    let imms = insn.imm2 as u32;
    let esize = if insn.sf { 64u32 } else { 32u32 };

    if insn.sf {
        load_guest_to_rax(ops, rn);

        if imms >= immr {
            let width = imms - immr + 1;
            if immr != 0 {
                dynasm!(ops ; shr rax, immr as i8);
            }
            let mask = low_mask(width);
            if mask != u64::MAX {
                dynasm!(ops
                    ; mov rcx, QWORD mask as i64
                    ; and rax, rcx
                );
            }
        } else {
            let width = imms + 1;
            let mask = low_mask(width);
            if mask != u64::MAX {
                dynasm!(ops
                    ; mov rcx, QWORD mask as i64
                    ; and rax, rcx
                );
            }
            let shift = esize - immr;
            dynasm!(ops ; shl rax, shift as i8);
        }

        store_rax_to_guest(ops, rd);
    } else {
        load_guest_to_eax(ops, rn);

        if imms >= immr {
            let width = imms - immr + 1;
            if immr != 0 {
                dynasm!(ops ; shr eax, immr as i8);
            }
            let mask = low_mask(width) as u32;
            if mask != u32::MAX {
                dynasm!(ops ; and eax, mask as i32);
            }
        } else {
            let width = imms + 1;
            let mask = low_mask(width) as u32;
            if mask != u32::MAX {
                dynasm!(ops ; and eax, mask as i32);
            }
            let shift = esize - immr;
            dynasm!(ops ; shl eax, shift as i8);
        }

        store_eax_to_guest_32(ops, rd);
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

/// Apply extend to `rcx`, then left-shift by `extend_amt`.
fn emit_apply_extend(ops: &mut Assembler, extend_type: u32, extend_amt: u32) {
    match extend_type {
        0 => dynasm!(ops ; movzx ecx, cl),   // UXTB
        1 => dynasm!(ops ; movzx ecx, cx),   // UXTH
        2 => dynasm!(ops ; mov ecx, ecx),    // UXTW
        3 => {}                              // UXTX
        4 => dynasm!(ops ; movsx rcx, cl),   // SXTB
        5 => dynasm!(ops ; movsx rcx, cx),   // SXTH
        6 => dynasm!(ops ; movsxd rcx, ecx), // SXTW
        7 => {}                              // SXTX
        _ => unreachable!(),
    }

    if extend_amt != 0 {
        let amt = extend_amt as i8;
        dynasm!(ops ; shl rcx, amt);
    }
}

// NZCV is now captured lazily via emit_defer_nzcv_imm / emit_defer_nzcv_reg
// (defined in dynasm/lazy_nzcv.rs). Eager capture helpers have been removed.

// ── Conditional Select (CSEL / CSINC / CSINV / CSNEG) ──────────────────────

/// Emit CSEL/CSINC/CSINV/CSNEG.
///
/// Materializes deferred NZCV, evaluates the condition on ebp, then selects
/// between Xn (condition true) and Xm/transformed-Xm (condition false).
pub fn emit_cond_select(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    let rm_slot = src_slot(insn.rm);

    // Materialize deferred NZCV.
    emit_materialize_nzcv(ops);

    // Load Xn (true case) into rax.
    load_guest_to_rax(ops, rn_slot);

    // Load Xm (false case) into rcx, apply variant transform.
    load_guest_to_rcx(ops, rm_slot);
    match insn.opcode {
        Opcode::Csel => {} // no transform
        Opcode::Csinc => dynasm!(ops ; add rcx, 1),
        Opcode::Csinv => dynasm!(ops ; not rcx),
        Opcode::Csneg => dynasm!(ops ; neg rcx),
        _ => unreachable!(),
    }

    // Evaluate condition: if true, keep rax (Xn); if false, use rcx (Xm').
    // emit_cond_check uses >not_taken label which we repurpose: condition
    // *false* -> not_taken -> use rcx.
    super::branch::emit_cond_check(ops, insn.cond);

    // Condition was true (did not jump to not_taken): result = rax. Skip.
    dynasm!(ops ; jmp >merge);

    // Condition was false: result = rcx.
    dynasm!(ops ; not_taken:);
    dynasm!(ops ; mov rax, rcx);

    dynasm!(ops ; merge:);

    // 32-bit mode: zero-extend
    if !insn.sf {
        dynasm!(ops ; mov eax, eax);
    }
    store_rax_to_guest(ops, rd_slot);
}

// ── SBFM (signed bitfield move) ────────────────────────────────────────────

/// Emit SBFM Xd, Xn, #immr, #imms.
///
/// Covers ASR, SXTB, SXTH, SXTW, and general signed bitfield extract.
/// Uses the same algorithm as exec_sbfm in the interpreter.
pub fn emit_sbfm(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    let immr = insn.imm as u32;
    let imms = insn.imm2 as u32;

    load_guest_to_rax(ops, rn_slot);

    if insn.sf {
        // 64-bit SBFM
        if imms == 63 {
            // ASR X, X, #immr
            if immr != 0 {
                dynasm!(ops ; sar rax, immr as i8);
            }
        } else if immr == 0 {
            // SXTB/SXTH/SXTW: sign-extend from (imms+1) bits
            match imms {
                7 => dynasm!(ops ; movsx rax, al),     // SXTB
                15 => dynasm!(ops ; movsx rax, ax),    // SXTH
                31 => dynasm!(ops ; movsxd rax, eax),  // SXTW
                _ => {
                    // General: sign-extend from imms+1 bits
                    let shift = (63 - imms) as i8;
                    dynasm!(ops ; shl rax, shift ; sar rax, shift);
                }
            }
        } else if imms >= immr {
            // SBFX: extract bitfield and sign-extend
            let width = imms - immr + 1;
            dynasm!(ops ; shr rax, immr as i8);
            let shift = (64 - width) as i8;
            dynasm!(ops ; shl rax, shift ; sar rax, shift);
        } else {
            // General SBFM with wrap: immr > imms
            let src_bits = imms + 1;
            let dst_pos = 64 - immr;
            let shift_up = (64 - src_bits) as i8;
            // Sign-extend the low (imms+1) bits, then shift left to dst_pos
            dynasm!(ops
                ; shl rax, shift_up
                ; sar rax, shift_up
                ; shl rax, dst_pos as i8
            );
        }
    } else {
        // 32-bit SBFM
        if imms == 31 {
            // ASR W, W, #immr
            if immr != 0 {
                dynasm!(ops ; sar eax, immr as i8);
            }
        } else if immr == 0 {
            match imms {
                7 => dynasm!(ops ; movsx eax, al),   // SXTB -> W
                15 => dynasm!(ops ; movsx eax, ax),  // SXTH -> W
                _ => {
                    let shift = (31 - imms) as i8;
                    dynasm!(ops ; shl eax, shift ; sar eax, shift);
                }
            }
        } else if imms >= immr {
            let width = imms - immr + 1;
            dynasm!(ops ; shr eax, immr as i8);
            let shift = (32 - width) as i8;
            dynasm!(ops ; shl eax, shift ; sar eax, shift);
        } else {
            let src_bits = imms + 1;
            let dst_pos = 32 - immr;
            let shift_up = (32 - src_bits) as i8;
            dynasm!(ops
                ; shl eax, shift_up
                ; sar eax, shift_up
                ; shl eax, dst_pos as i8
            );
        }
        // Zero-extend 32-bit result to 64
        dynasm!(ops ; mov eax, eax);
    }

    store_rax_to_guest(ops, rd_slot);
}

// ── ANDS register (flag-setting AND / TST) ──────────────────────────────────

/// Emit ANDS Xd, Xn, Xm{, shift #amt} -- flag-setting AND.
///
/// Used for TST (ANDS XZR, Xn, Xm). Uses lazy NZCV deferral.
pub fn emit_ands_reg(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    let rm_slot = src_slot(insn.rm);

    load_guest_to_rax(ops, rn_slot);
    load_guest_to_rcx(ops, rm_slot);

    // Apply optional shift to rm.
    let shift_type = insn.extend_type;
    let shift_amt = insn.extend_amt;
    if shift_amt != 0 {
        if insn.sf {
            emit_apply_shift_64(ops, shift_type, shift_amt);
        } else {
            emit_apply_shift_32(ops, shift_type, shift_amt);
        }
    }

    if insn.sf {
        dynasm!(ops ; and rax, rcx);
    } else {
        dynasm!(ops ; and eax, ecx);
    }

    // Defer NZCV: AND sets N,Z from result, C=0, V=0.
    // For AND, we can use the reg deferral with FlagOp::And{32,64}.
    let flag_op = if insn.sf { FlagOp::And64 } else { FlagOp::And32 };
    // Store result as LHS, 0 as RHS (unused for AND flags -- N/Z from result).
    let op_off = reg_offset(REG_FLAG_OP);
    let lhs_off = reg_offset(crate::regs::REG_FLAG_LHS);
    let rhs_off = reg_offset(crate::regs::REG_FLAG_RHS);
    dynasm!(ops
        ; mov QWORD [rdi + op_off], flag_op as u8 as i32
        ; mov QWORD [rdi + lhs_off], rax
        ; mov QWORD [rdi + rhs_off], 0
    );

    if !insn.sf {
        dynasm!(ops ; mov eax, eax);
    }
    store_rax_to_guest(ops, rd_slot);
}

// ── MADD / MUL / MSUB / MNEG ───────────────────────────────────────────────

/// Emit MADD/MUL Xd, Xn, Xm, Xa.
///
/// MUL is MADD with Ra=XZR (addend=0).
pub fn emit_madd(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    let rm_slot = src_slot(insn.rm);
    let ra_slot = if insn.opcode == Opcode::Madd {
        src_slot(insn.ra)
    } else {
        // MUL: addend is 0 (XZR)
        REG_XZR
    };

    load_guest_to_rax(ops, rn_slot);
    load_guest_to_rcx(ops, rm_slot);

    if insn.sf {
        dynasm!(ops ; imul rax, rcx);
    } else {
        dynasm!(ops ; imul eax, ecx);
    }

    // Add Ra.
    if ra_slot != REG_XZR {
        let ra_off = reg_offset(ra_slot);
        if insn.sf {
            dynasm!(ops ; add rax, QWORD [rdi + ra_off]);
        } else {
            dynasm!(ops ; add eax, DWORD [rdi + ra_off]);
        }
    }

    if !insn.sf {
        dynasm!(ops ; mov eax, eax);
    }
    store_rax_to_guest(ops, rd_slot);
}

/// Emit MSUB/MNEG Xd, Xn, Xm, Xa.
///
/// Rd = Ra - Rn*Rm. MNEG is MSUB with Ra=XZR.
pub fn emit_msub(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    let rm_slot = src_slot(insn.rm);
    let ra_slot = if insn.opcode == Opcode::Msub {
        src_slot(insn.ra)
    } else {
        REG_XZR
    };

    // Compute Rn * Rm into rcx.
    load_guest_to_rax(ops, rn_slot);
    load_guest_to_rcx(ops, rm_slot);
    if insn.sf {
        dynasm!(ops ; imul rax, rcx);
    } else {
        dynasm!(ops ; imul eax, ecx);
    }

    // Ra - product
    load_guest_to_rcx(ops, ra_slot);  // rcx = Ra
    if insn.sf {
        dynasm!(ops ; sub rcx, rax ; mov rax, rcx);
    } else {
        dynasm!(ops ; sub ecx, eax ; mov eax, ecx);
    }

    if !insn.sf {
        dynasm!(ops ; mov eax, eax);
    }
    store_rax_to_guest(ops, rd_slot);
}

// ── Variable shifts (LSL / LSR / ASR / ROR register) ────────────────────────

/// Emit LSL/LSR/ASR/ROR Xd, Xn, Xm (variable shift by register).
pub fn emit_shift_reg(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    let rm_slot = src_slot(insn.rm);

    load_guest_to_rax(ops, rn_slot);
    load_guest_to_rcx(ops, rm_slot);

    if insn.sf {
        // Shift amount = Xm mod 64
        dynasm!(ops ; and ecx, 63);
        match insn.opcode {
            Opcode::Lsl => dynasm!(ops ; shl rax, cl),
            Opcode::Lsr => dynasm!(ops ; shr rax, cl),
            Opcode::Asr => dynasm!(ops ; sar rax, cl),
            Opcode::Ror => dynasm!(ops ; ror rax, cl),
            _ => unreachable!(),
        }
    } else {
        // Shift amount = Wm mod 32
        dynasm!(ops ; and ecx, 31);
        match insn.opcode {
            Opcode::Lsl => dynasm!(ops ; shl eax, cl),
            Opcode::Lsr => dynasm!(ops ; shr eax, cl),
            Opcode::Asr => dynasm!(ops ; sar eax, cl),
            Opcode::Ror => dynasm!(ops ; ror eax, cl),
            _ => unreachable!(),
        }
        dynasm!(ops ; mov eax, eax); // zero-extend
    }
    store_rax_to_guest(ops, rd_slot);
}

// ── CLZ / CLS / REV / REV16 / REV32 / RBIT ─────────────────────────────────

/// Emit CLZ Xd, Xn (count leading zeros).
pub fn emit_clz(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    load_guest_to_rax(ops, rn_slot);

    if insn.sf {
        // x86-64 LZCNT (BMI1) counts leading zeros directly.
        dynasm!(ops ; lzcnt rax, rax);
    } else {
        dynasm!(ops ; lzcnt eax, eax);
    }
    store_rax_to_guest(ops, rd_slot);
}

/// Emit REV Xd, Xn (byte reverse).
pub fn emit_rev(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    load_guest_to_rax(ops, rn_slot);

    if insn.sf {
        dynasm!(ops ; bswap rax);
    } else {
        dynasm!(ops ; bswap eax);
    }
    store_rax_to_guest(ops, rd_slot);
}

/// Emit REV16 Xd, Xn (byte reverse within each 16-bit halfword).
pub fn emit_rev16(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    load_guest_to_rax(ops, rn_slot);

    // Swap bytes within each 16-bit pair:
    // result = ((x & 0xFF00...) >> 8) | ((x & 0x00FF...) << 8)
    dynasm!(ops
        ; mov rcx, rax
        ; mov rdx, QWORD 0xFF00_FF00_FF00_FF00u64 as i64
        ; and rax, rdx
        ; shr rax, 8
        ; not rdx           // rdx = 0x00FF_00FF_00FF_00FF
        ; and rcx, rdx
        ; shl rcx, 8
        ; or rax, rcx
    );
    if !insn.sf {
        dynasm!(ops ; mov eax, eax);
    }
    store_rax_to_guest(ops, rd_slot);
}

/// Emit REV32 Xd, Xn (byte reverse within each 32-bit word).
pub fn emit_rev32(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    load_guest_to_rax(ops, rn_slot);

    // Swap bytes within each 32-bit half independently.
    dynasm!(ops
        ; mov ecx, eax      // lo32
        ; shr rax, 32       // hi32
        ; bswap ecx
        ; bswap eax
        ; shl rax, 32
        ; or rax, rcx
    );
    store_rax_to_guest(ops, rd_slot);
}

/// Emit RBIT Xd, Xn (reverse bits).
pub fn emit_rbit(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    load_guest_to_rax(ops, rn_slot);

    // x86 doesn't have a single RBIT instruction, so we use the classic
    // swap-parallel algorithm for 64-bit reverse.
    if insn.sf {
        // Byte-reverse first, then reverse bits within each byte.
        dynasm!(ops ; bswap rax);
        // Reverse bits within each byte: 3 swap steps (1<->4, 2<->3 within each nibble).
        emit_bit_reverse_bytes(ops, true);
    } else {
        dynasm!(ops ; bswap eax);
        emit_bit_reverse_bytes(ops, false);
        dynasm!(ops ; mov eax, eax);
    }
    store_rax_to_guest(ops, rd_slot);
}

/// Reverse bits within each byte of rax (after bswap).
/// Uses parallel swap: swap bit pairs, then nibbles.
fn emit_bit_reverse_bytes(ops: &mut Assembler, is64: bool) {
    if is64 {
        // Swap adjacent bits: ((x >> 1) & 0x5555...) | ((x & 0x5555...) << 1)
        dynasm!(ops
            ; mov rcx, rax
            ; shr rax, 1
            ; mov rdx, QWORD 0x5555_5555_5555_5555u64 as i64
            ; and rax, rdx
            ; and rcx, rdx
            ; shl rcx, 1
            ; or rax, rcx
        );
        // Swap adjacent pairs: ((x >> 2) & 0x3333...) | ((x & 0x3333...) << 2)
        dynasm!(ops
            ; mov rcx, rax
            ; shr rax, 2
            ; mov rdx, QWORD 0x3333_3333_3333_3333u64 as i64
            ; and rax, rdx
            ; and rcx, rdx
            ; shl rcx, 2
            ; or rax, rcx
        );
        // Swap adjacent nibbles: ((x >> 4) & 0x0F0F...) | ((x & 0x0F0F...) << 4)
        dynasm!(ops
            ; mov rcx, rax
            ; shr rax, 4
            ; mov rdx, QWORD 0x0F0F_0F0F_0F0F_0F0Fu64 as i64
            ; and rax, rdx
            ; and rcx, rdx
            ; shl rcx, 4
            ; or rax, rcx
        );
    } else {
        dynasm!(ops
            ; mov ecx, eax
            ; shr eax, 1
            ; and eax, 0x55555555u32 as i32
            ; and ecx, 0x55555555u32 as i32
            ; shl ecx, 1
            ; or eax, ecx
        );
        dynasm!(ops
            ; mov ecx, eax
            ; shr eax, 2
            ; and eax, 0x33333333u32 as i32
            ; and ecx, 0x33333333u32 as i32
            ; shl ecx, 2
            ; or eax, ecx
        );
        dynasm!(ops
            ; mov ecx, eax
            ; shr eax, 4
            ; and eax, 0x0F0F0F0Fu32 as i32
            ; and ecx, 0x0F0F0F0Fu32 as i32
            ; shl ecx, 4
            ; or eax, ecx
        );
    }
}

// ── EXTR (extract / rotate immediate from pair) ─────────────────────────────

/// Emit EXTR Xd, Xn, Xm, #lsb.
///
/// Concatenates Xn:Xm and extracts a register-width value starting at #lsb.
/// When Rn==Rm this is ROR Xd, Xn, #lsb.
pub fn emit_extr(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    let rm_slot = src_slot(insn.rm);
    let lsb = insn.imm as u32;

    if insn.sf {
        if lsb == 0 {
            // EXTR with lsb=0 => result = Xn (Xm bits are all shifted out)
            load_guest_to_rax(ops, rn_slot);
        } else if insn.rn == insn.rm {
            // ROR Xd, Xn, #lsb
            load_guest_to_rax(ops, rn_slot);
            dynasm!(ops ; ror rax, lsb as i8);
        } else {
            // General: (Xn << (64-lsb)) | (Xm >> lsb)
            load_guest_to_rax(ops, rn_slot);
            dynasm!(ops ; shl rax, (64 - lsb) as i8);
            load_guest_to_rcx(ops, rm_slot);
            dynasm!(ops ; shr rcx, lsb as i8 ; or rax, rcx);
        }
    } else {
        if lsb == 0 {
            load_guest_to_rax(ops, rn_slot);
        } else if insn.rn == insn.rm {
            load_guest_to_rax(ops, rn_slot);
            dynasm!(ops ; ror eax, lsb as i8);
        } else {
            load_guest_to_rax(ops, rn_slot);
            dynasm!(ops ; shl eax, (32 - lsb) as i8);
            load_guest_to_rcx(ops, rm_slot);
            dynasm!(ops ; shr ecx, lsb as i8 ; or eax, ecx);
        }
        dynasm!(ops ; mov eax, eax);
    }
    store_rax_to_guest(ops, rd_slot);
}

// ── Logical complement (ORN / EON / BIC / BICS) ────────────────────────────

/// Emit ORN/EON/BIC/BICS Xd, Xn, Xm{, shift #amt}.
pub fn emit_logical_neg_reg(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    let rm_slot = src_slot(insn.rm);

    load_guest_to_rax(ops, rn_slot);
    load_guest_to_rcx(ops, rm_slot);

    let shift_type = insn.extend_type;
    let shift_amt = insn.extend_amt;
    if shift_amt != 0 {
        if insn.sf {
            emit_apply_shift_64(ops, shift_type, shift_amt);
        } else {
            emit_apply_shift_32(ops, shift_type, shift_amt);
        }
    }

    // Invert Rm.
    if insn.sf {
        dynasm!(ops ; not rcx);
    } else {
        dynasm!(ops ; not ecx);
    }

    // Apply the base operation.
    match insn.opcode {
        Opcode::OrnReg => {
            if insn.sf {
                dynasm!(ops ; or rax, rcx);
            } else {
                dynasm!(ops ; or eax, ecx);
            }
        }
        Opcode::EonReg => {
            if insn.sf {
                dynasm!(ops ; xor rax, rcx);
            } else {
                dynasm!(ops ; xor eax, ecx);
            }
        }
        Opcode::BicReg => {
            // BIC Xd, Xn, Xm = Xn AND NOT(Xm) -- NOT already applied.
            if insn.sf {
                dynasm!(ops ; and rax, rcx);
            } else {
                dynasm!(ops ; and eax, ecx);
            }
        }
        Opcode::BicsReg => {
            if insn.sf {
                dynasm!(ops ; and rax, rcx);
            } else {
                dynasm!(ops ; and eax, ecx);
            }
            // Flag-setting: defer NZCV.
            let flag_op = if insn.sf { FlagOp::And64 } else { FlagOp::And32 };
            let op_off = reg_offset(REG_FLAG_OP);
            let lhs_off = reg_offset(crate::regs::REG_FLAG_LHS);
            let rhs_off = reg_offset(crate::regs::REG_FLAG_RHS);
            dynasm!(ops
                ; mov QWORD [rdi + op_off], flag_op as u8 as i32
                ; mov QWORD [rdi + lhs_off], rax
                ; mov QWORD [rdi + rhs_off], 0
            );
        }
        _ => unreachable!(),
    }

    if !insn.sf {
        dynasm!(ops ; mov eax, eax);
    }
    store_rax_to_guest(ops, rd_slot);
}

// ── SDIV / UDIV ─────────────────────────────────────────────────────────────

/// Emit SDIV Xd, Xn, Xm (signed divide).
pub fn emit_sdiv(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    let rm_slot = src_slot(insn.rm);

    load_guest_to_rax(ops, rn_slot);
    load_guest_to_rcx(ops, rm_slot);

    // AArch64 division by zero produces 0 (no exception).
    // AArch64 INT_MIN / -1 produces INT_MIN (no exception).
    if insn.sf {
        dynasm!(ops
            ; test rcx, rcx
            ; jz >zero_div
            ; cqo           // sign-extend rax -> rdx:rax
            ; idiv rcx
            ; jmp >done
            ; zero_div:
            ; xor eax, eax
            ; done:
        );
    } else {
        dynasm!(ops
            ; test ecx, ecx
            ; jz >zero_div
            ; cdq           // sign-extend eax -> edx:eax
            ; idiv ecx
            ; jmp >done
            ; zero_div:
            ; xor eax, eax
            ; done:
        );
        dynasm!(ops ; mov eax, eax);
    }
    store_rax_to_guest(ops, rd_slot);
}

/// Emit UDIV Xd, Xn, Xm (unsigned divide).
pub fn emit_udiv(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    let rm_slot = src_slot(insn.rm);

    load_guest_to_rax(ops, rn_slot);
    load_guest_to_rcx(ops, rm_slot);

    if insn.sf {
        dynasm!(ops
            ; test rcx, rcx
            ; jz >zero_div
            ; xor edx, edx  // zero-extend rax -> rdx:rax
            ; div rcx
            ; jmp >done
            ; zero_div:
            ; xor eax, eax
            ; done:
        );
    } else {
        dynasm!(ops
            ; test ecx, ecx
            ; jz >zero_div
            ; xor edx, edx
            ; div ecx
            ; jmp >done
            ; zero_div:
            ; xor eax, eax
            ; done:
        );
        dynasm!(ops ; mov eax, eax);
    }
    store_rax_to_guest(ops, rd_slot);
}

// ── BFM (bitfield move) ────────────────────────────────────────────────────

/// Emit BFM Xd, Xn, #immr, #imms.
///
/// Copies a bitfield from Xn into Xd without affecting other bits of Xd.
pub fn emit_bfm(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    let immr = insn.imm as u32;
    let imms = insn.imm2 as u32;

    // BFM is complex; for now handle the common BFI/BFXIL cases.
    // General formula: dst = (dst & ~mask) | ((src ROR immr) & mask)
    // where mask is derived from imms.
    // Use the interpreter's algorithm via load-compute-store.

    let reg_size = if insn.sf { 64u32 } else { 32 };

    load_guest_to_rax(ops, rn_slot);

    // Rotate source right by immr.
    if immr != 0 {
        if insn.sf {
            dynasm!(ops ; ror rax, immr as i8);
        } else {
            dynasm!(ops ; ror eax, immr as i8);
        }
    }

    // Compute mask: bits [0..imms] are set.
    let mask = if imms >= reg_size - 1 {
        if insn.sf { u64::MAX } else { 0xFFFF_FFFFu64 }
    } else {
        (1u64 << (imms + 1)) - 1
    };

    // rotated_src & mask
    dynasm!(ops ; mov rcx, rax);
    if insn.sf {
        dynasm!(ops ; mov rdx, QWORD mask as i64 ; and rcx, rdx);
    } else {
        dynasm!(ops ; and ecx, mask as i32);
    }

    // Load current Xd, clear mask bits, OR in the new bits.
    load_guest_to_rax(ops, rd_slot);
    if insn.sf {
        dynasm!(ops ; mov rdx, QWORD !(mask) as i64 ; and rax, rdx ; or rax, rcx);
    } else {
        dynasm!(ops ; and eax, !(mask as u32) as i32 ; or eax, ecx);
    }

    if !insn.sf {
        dynasm!(ops ; mov eax, eax);
    }
    store_rax_to_guest(ops, rd_slot);
}

// ── ADDS/SUBS extended register (AddsExt / SubsExt) ─────────────────────────

/// Emit ADDS/SUBS Xd, Xn, Wm, extend #amt (flag-setting extended register).
pub fn emit_adds_subs_ext(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = sp_slot(insn.rn);
    let rm_slot = src_slot(insn.rm);
    let is_sub = insn.opcode == Opcode::SubsExt;

    load_guest_to_rax(ops, rn_slot);
    load_guest_to_rcx(ops, rm_slot);
    emit_apply_extend(ops, insn.extend_type, insn.extend_amt);

    // Defer NZCV.
    let flag_op = if is_sub {
        if insn.sf { FlagOp::Sub64 } else { FlagOp::Sub32 }
    } else {
        if insn.sf { FlagOp::Add64 } else { FlagOp::Add32 }
    };
    emit_defer_nzcv_reg(ops, flag_op);

    if insn.sf {
        if is_sub {
            dynasm!(ops ; sub rax, rcx);
        } else {
            dynasm!(ops ; add rax, rcx);
        }
    } else {
        if is_sub {
            dynasm!(ops ; sub eax, ecx);
        } else {
            dynasm!(ops ; add eax, ecx);
        }
        dynasm!(ops ; mov eax, eax);
    }
    store_rax_to_guest(ops, rd_slot);
}

// ── ADC / ADCS / SBC / SBCS ────────────────────────────────────────────────

/// Emit ADC/ADCS Xd, Xn, Xm (add with carry).
pub fn emit_adc(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    let rm_slot = src_slot(insn.rm);
    let is_flag = insn.opcode == Opcode::Adcs;

    // Materialize NZCV to get current carry flag from rbp (bit 29).
    emit_materialize_nzcv(ops);

    load_guest_to_rax(ops, rn_slot);
    load_guest_to_rcx(ops, rm_slot);

    if insn.sf {
        // Extract carry bit from rbp (NZCV bit 29) into CF.
        dynasm!(ops
            ; bt ebp, 29     // set x86 CF = NZCV.C
            ; adc rax, rcx
        );
    } else {
        dynasm!(ops
            ; bt ebp, 29
            ; adc eax, ecx
        );
    }

    if is_flag {
        // Capture flags from x86 rflags after ADC.
        emit_capture_from_rflags(ops, insn.sf);
    }

    if !insn.sf {
        dynasm!(ops ; mov eax, eax);
    }
    store_rax_to_guest(ops, rd_slot);
}

/// Emit SBC/SBCS Xd, Xn, Xm (subtract with borrow).
pub fn emit_sbc(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    let rm_slot = src_slot(insn.rm);
    let is_flag = insn.opcode == Opcode::Sbcs;

    emit_materialize_nzcv(ops);

    load_guest_to_rax(ops, rn_slot);
    load_guest_to_rcx(ops, rm_slot);

    // SBC: Rd = Rn - Rm - (1 - C) = Rn + NOT(Rm) + C
    // In x86 terms: NOT rcx, then ADC rax, rcx with AArch64 carry.
    if insn.sf {
        dynasm!(ops
            ; not rcx
            ; bt ebp, 29
            ; adc rax, rcx
        );
    } else {
        dynasm!(ops
            ; not ecx
            ; bt ebp, 29
            ; adc eax, ecx
        );
    }

    if is_flag {
        emit_capture_from_rflags(ops, insn.sf);
    }

    if !insn.sf {
        dynasm!(ops ; mov eax, eax);
    }
    store_rax_to_guest(ops, rd_slot);
}

// ── SMULH / UMULH ───────────────────────────────────────────────────────────

/// Emit SMULH Xd, Xn, Xm (signed multiply high, 64-bit only).
pub fn emit_smulh(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    let rm_slot = src_slot(insn.rm);

    load_guest_to_rax(ops, rn_slot);
    load_guest_to_rcx(ops, rm_slot);

    // imul rcx produces rdx:rax = rax * rcx (signed).
    // SMULH wants the high 64 bits = rdx.
    dynasm!(ops ; imul rcx ; mov rax, rdx);

    store_rax_to_guest(ops, rd_slot);
}

/// Emit UMULH Xd, Xn, Xm (unsigned multiply high, 64-bit only).
pub fn emit_umulh(ops: &mut Assembler, insn: &Instruction) {
    let rd_slot = dst_slot(insn.rd);
    let rn_slot = src_slot(insn.rn);
    let rm_slot = src_slot(insn.rm);

    load_guest_to_rax(ops, rn_slot);
    load_guest_to_rcx(ops, rm_slot);

    // mul rcx produces rdx:rax = rax * rcx (unsigned).
    dynasm!(ops ; mul rcx ; mov rax, rdx);

    store_rax_to_guest(ops, rd_slot);
}

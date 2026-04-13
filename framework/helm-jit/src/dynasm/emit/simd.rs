//! SIMD instruction emitters (AArch64 -> x86-64).
//!
//! Minimal subset for musl memset fast paths:
//! - **SimdDup**: DUP Vd.xB, Wn -- replicate GPR across vector lanes
//! - **StrSimd**: STR Qn, [Xm, #imm] -- store 128-bit vector to memory
//! - **StpSimd**: STP Qn, Qm, [Xk, #imm] -- store pair of 128-bit vectors
//!
//! Vector registers live in the flat array at slots REG_V_BASE + vn*2 (lo)
//! and REG_V_BASE + vn*2 + 1 (hi), each holding 64 bits of the 128-bit value.

#![allow(missing_docs)]

use crate::block::EXIT_EXCEPTION;
use crate::dynasm::pinned::{
    emit_pinned_epilogue, load_guest_to_rax, store_rax_to_guest,
};
use crate::regs::{
    reg_offset, vreg_offset_hi, vreg_offset_lo, REG_JIT_MEM_WRITE, REG_PC, REG_SP, REG_XZR,
};
use dynasm::dynasm;
use dynasmrt::x64::Assembler;
use dynasmrt::{DynasmApi, DynasmLabelApi};
use helm_arch::aarch64::insn::Instruction;

// ── SimdDup (DUP Vd.xB, Wn) ────────────────────────────────────────────────

/// Emit DUP Vd.{16B,8B}, Wn -- replicate the low byte/half/word/dword of Xn
/// across all lanes of Vd.
///
/// imm5 encoding (from insn.imm):
///   bit 0 set -> byte lanes
///   bit 1 set -> halfword lanes
///   bit 2 set -> word lanes
///   bit 3 set -> doubleword lanes
/// insn.sf: true = 128-bit (Q=1), false = 64-bit (Q=0, upper 64 bits zeroed)
pub fn emit_simd_dup(ops: &mut Assembler, insn: &Instruction) -> Option<bool> {
    let rn_slot = if insn.rn == 31 { REG_XZR } else { insn.rn as usize };
    let vd = insn.rd as usize;
    let vd_lo = vreg_offset_lo(vd);
    let vd_hi = vreg_offset_hi(vd);
    let imm5 = insn.imm as u32;

    // Load source GPR into rax.
    load_guest_to_rax(ops, rn_slot);

    if imm5 & 1 != 0 {
        // Byte: replicate low byte across all 8 bytes of each 64-bit half.
        // al -> 0x0101010101010101 * al
        dynasm!(ops
            ; movzx eax, al
            ; mov rcx, QWORD 0x0101_0101_0101_0101u64 as i64
            ; imul rax, rcx
        );
    } else if imm5 & 2 != 0 {
        // Halfword: replicate low 16 bits across 4 lanes per 64-bit half.
        dynasm!(ops
            ; movzx eax, ax
            ; mov rcx, rax
            ; shl rcx, 16
            ; or rax, rcx
            ; mov rcx, rax
            ; shl rcx, 32
            ; or rax, rcx
        );
    } else if imm5 & 4 != 0 {
        // Word: replicate low 32 bits into both halves of each 64-bit slot.
        dynasm!(ops
            ; mov eax, eax   // zero-extend to 64 bits
            ; mov rcx, rax
            ; shl rcx, 32
            ; or rax, rcx
        );
    } else {
        // Doubleword: rax already holds the 64-bit value, no replication needed
        // within a 64-bit half (each half is one lane).
    }

    // Store lo half.
    dynasm!(ops ; mov QWORD [rdi + vd_lo], rax);

    if insn.sf {
        // Q=1: 128-bit -- hi half is same replicated value.
        dynasm!(ops ; mov QWORD [rdi + vd_hi], rax);
    } else {
        // Q=0: 64-bit -- zero hi half.
        dynasm!(ops ; mov QWORD [rdi + vd_hi], 0);
    }

    Some(false)
}

// ── StrSimd (STR Qn/Dn/Sn/Hn/Bn, [Xm, #imm]) ─────────────────────────────

/// Emit STR of a SIMD/FP register to memory.
///
/// Only Q-size (128-bit) stores are handled inline; smaller sizes (D/S/H/B)
/// return None to fall back to the interpreter. This covers the musl memset
/// hot path without over-engineering the emitter.
pub fn emit_str_simd(ops: &mut Assembler, insn: &Instruction) -> Option<bool> {
    let size_bytes = match insn.ftype {
        0 => 1,
        1 => 2,
        2 => 4,
        3 => 8,
        _ => 16,
    };

    // Only handle Q-register (128-bit) and D-register (64-bit) stores.
    if size_bytes < 8 {
        return None;
    }

    let base_slot = if insn.rn == 31 { REG_SP } else { insn.rn as usize };
    let vt = insn.rd as usize;
    let write_off = reg_offset(REG_JIT_MEM_WRITE);
    let stash_off = reg_offset(38);

    // Compute effective address.
    load_guest_to_rax(ops, base_slot);
    if insn.pre_index || !insn.post_index {
        // Pre-index or unsigned offset: EA = base + imm
        if insn.imm != 0 {
            emit_add_rax_imm64(ops, insn.imm);
        }
    }
    // For post-index: EA = base (no offset added before store).

    // Stash EA for multi-write use.
    dynasm!(ops ; mov QWORD [rdi + stash_off], rax);

    // Store low 64 bits.
    let vt_lo = vreg_offset_lo(vt);
    dynasm!(ops ; mov rcx, QWORD [rdi + vt_lo]);
    emit_mem_write_from_rcx(ops, write_off, stash_off, 8, 0);

    if size_bytes == 16 {
        // Store high 64 bits at EA+8.
        let vt_hi = vreg_offset_hi(vt);
        dynasm!(ops ; mov rcx, QWORD [rdi + vt_hi]);
        emit_mem_write_from_rcx(ops, write_off, stash_off, 8, 8);
    }

    // Skip past fault exit on success.
    dynasm!(ops ; jmp >done);
    emit_fault_exit_at(ops, insn);
    dynasm!(ops ; done:);

    // Writeback.
    if insn.pre_index {
        load_guest_to_rax(ops, base_slot);
        emit_add_rax_imm64(ops, insn.imm);
        store_rax_to_guest(ops, base_slot);
    }
    if insn.post_index {
        load_guest_to_rax(ops, base_slot);
        emit_add_rax_imm64(ops, insn.imm);
        store_rax_to_guest(ops, base_slot);
    }

    Some(false)
}

// ── StpSimd (STP Qn, Qm, [Xk, #imm]) ──────────────────────────────────────

/// Emit STP of two SIMD/FP registers to memory.
///
/// Only Q-size (128-bit) pair stores are handled inline.
pub fn emit_stp_simd(ops: &mut Assembler, insn: &Instruction) -> Option<bool> {
    let sz = match insn.ftype {
        0 => 4usize,
        1 => 8,
        _ => 16,
    };

    // Only handle Q-register pairs (32 bytes total) and D-register pairs (16 bytes).
    if sz < 8 {
        return None;
    }

    let base_slot = if insn.rn == 31 { REG_SP } else { insn.rn as usize };
    let vt1 = insn.rd as usize;
    let vt2 = insn.pair_second as usize;
    let write_off = reg_offset(REG_JIT_MEM_WRITE);
    let stash_off = reg_offset(38);

    // Compute effective address.
    load_guest_to_rax(ops, base_slot);
    let ea_offset = if insn.post_index { 0 } else { insn.imm };
    if ea_offset != 0 {
        emit_add_rax_imm64(ops, ea_offset);
    }
    dynasm!(ops ; mov QWORD [rdi + stash_off], rax);

    if sz == 8 {
        // D-register pair: 2 x 8-byte stores.
        let v1_lo = vreg_offset_lo(vt1);
        dynasm!(ops ; mov rcx, QWORD [rdi + v1_lo]);
        emit_mem_write_from_rcx(ops, write_off, stash_off, 8, 0);

        let v2_lo = vreg_offset_lo(vt2);
        dynasm!(ops ; mov rcx, QWORD [rdi + v2_lo]);
        emit_mem_write_from_rcx(ops, write_off, stash_off, 8, 8);
    } else {
        // Q-register pair: 4 x 8-byte stores (32 bytes total).
        let v1_lo = vreg_offset_lo(vt1);
        let v1_hi = vreg_offset_hi(vt1);
        dynasm!(ops ; mov rcx, QWORD [rdi + v1_lo]);
        emit_mem_write_from_rcx(ops, write_off, stash_off, 8, 0);
        dynasm!(ops ; mov rcx, QWORD [rdi + v1_hi]);
        emit_mem_write_from_rcx(ops, write_off, stash_off, 8, 8);

        let v2_lo = vreg_offset_lo(vt2);
        let v2_hi = vreg_offset_hi(vt2);
        dynasm!(ops ; mov rcx, QWORD [rdi + v2_lo]);
        emit_mem_write_from_rcx(ops, write_off, stash_off, 8, 16);
        dynasm!(ops ; mov rcx, QWORD [rdi + v2_hi]);
        emit_mem_write_from_rcx(ops, write_off, stash_off, 8, 24);
    }

    dynasm!(ops ; jmp >done);
    emit_fault_exit_at(ops, insn);
    dynasm!(ops ; done:);

    // Writeback (pre-index or post-index).
    if insn.pre_index || insn.post_index {
        load_guest_to_rax(ops, base_slot);
        emit_add_rax_imm64(ops, insn.imm);
        store_rax_to_guest(ops, base_slot);
    }

    Some(false)
}

// ── Shared helpers ──────────────────────────────────────────────────────────

/// Add a 64-bit immediate to rax, using the compact form when possible.
fn emit_add_rax_imm64(ops: &mut Assembler, imm: i64) {
    if i32::try_from(imm).is_ok() {
        dynasm!(ops ; add rax, imm as i32);
    } else {
        dynasm!(ops
            ; mov rcx, QWORD imm
            ; add rax, rcx
        );
    }
}

/// Emit a mem_write call: writes `rcx` (value) to address stash_off + byte_add.
///
/// On entry: rcx = value, [rdi + stash_off] = base EA.
/// Clobbers: rax, rcx, rdx, rsi, rdi, r8-r11.
/// On fault: jumps to the `fault` label.
fn emit_mem_write_from_rcx(
    ops: &mut Assembler,
    write_fn_off: i32,
    stash_off: i32,
    size: i32,
    byte_add: i32,
) {
    dynasm!(ops
        ; push rdi
        ; push rsi
        ; push r8
        ; push r9
        ; push r10
        ; push r11
        ; sub rsp, 8
    );
    dynasm!(ops
        ; mov rax, QWORD [rdi + stash_off]
    );
    if byte_add != 0 {
        dynasm!(ops ; add rax, byte_add);
    }
    dynasm!(ops
        ; mov rdx, rcx           // arg3: value
        ; mov rsi, rax           // arg2: address
        ; mov rdi, [rsp + 40]   // arg1: mem ptr (original rsi)
        ; mov ecx, size          // arg4: size

        ; mov rax, [rsp + 48]   // original rdi (flat regs)
        ; mov rax, QWORD [rax + write_fn_off]
        ; call rax

        ; add rsp, 8
        ; pop r11
        ; pop r10
        ; pop r9
        ; pop r8
        ; pop rsi
        ; pop rdi

        ; test rax, rax
        ; jnz >fault
    );
}

/// Emit a fault-exit block: sets PC, runs epilogue, returns EXIT_EXCEPTION.
fn emit_fault_exit_at(ops: &mut Assembler, insn: &Instruction) {
    dynasm!(ops ; fault:);
    let pc_off = reg_offset(REG_PC);
    let pc_val = insn.pc as i64;
    if i32::try_from(pc_val).is_ok() {
        dynasm!(ops ; mov QWORD [rdi + pc_off], pc_val as i32);
    } else {
        dynasm!(ops ; mov rax, QWORD pc_val ; mov QWORD [rdi + pc_off], rax);
    }
    emit_pinned_epilogue(ops);
    dynasm!(ops
        ; mov eax, EXIT_EXCEPTION as i32
        ; ret
    );
}

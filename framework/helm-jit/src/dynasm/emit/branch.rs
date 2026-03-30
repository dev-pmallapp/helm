//! Branch instruction emitters (AArch64 → x86-64).
//!
//! All branch emitters terminate the block: they update the guest PC in the
//! flat register array and emit a `ret` (returning `EXIT_END_OF_BLOCK` in
//! `rax`).

#![allow(missing_docs)]
#![allow(clippy::similar_names)]

use crate::block::EXIT_END_OF_BLOCK;
use crate::regs::{reg_offset, REG_NZCV, REG_PC, REG_XZR};
use dynasm::dynasm;
use dynasmrt::{x64::Assembler, DynasmApi, DynasmLabelApi};
use helm_arch::aarch64::insn::Instruction;

/// Data register offset — reg 31 = XZR.
#[inline]
fn src_offset(reg: u32) -> i32 {
    if reg == 31 {
        reg_offset(REG_XZR)
    } else {
        reg_offset(reg as usize)
    }
}

/// X30 (link register) offset.
const X30_OFF: i32 = 30 * 8;

/// Emit the block exit sequence: store exit code and return.
fn emit_exit(ops: &mut Assembler) {
    dynasm!(ops
        ; mov rax, QWORD EXIT_END_OF_BLOCK as i64
        ; ret
    );
}

// ── B (unconditional branch) ────────────────────────────────────────────────

/// Emit `B label` — unconditional PC-relative branch.
pub fn emit_b(ops: &mut Assembler, insn: &Instruction) {
    let pc_off = reg_offset(REG_PC);
    let target = insn.pc.wrapping_add(insn.imm as u64);

    dynasm!(ops
        ; mov rax, QWORD target as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops);
}

// ── BL (branch with link) ──────────────────────────────────────────────────

/// Emit `BL label` — branch with link (saves return address in X30).
pub fn emit_bl(ops: &mut Assembler, insn: &Instruction) {
    let pc_off = reg_offset(REG_PC);
    let target = insn.pc.wrapping_add(insn.imm as u64);
    let ret_addr = insn.pc.wrapping_add(4);

    dynasm!(ops
        // Save return address to X30
        ; mov rax, QWORD ret_addr as i64
        ; mov QWORD [rdi + X30_OFF], rax
        // Set PC to target
        ; mov rax, QWORD target as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops);
}

// ── BR (branch to register) ────────────────────────────────────────────────

/// Emit `BR Xn` — branch to address in register.
pub fn emit_br(ops: &mut Assembler, insn: &Instruction) {
    let pc_off = reg_offset(REG_PC);
    let rn_off = src_offset(insn.rn);

    dynasm!(ops
        ; mov rax, QWORD [rdi + rn_off]
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops);
}

// ── BLR (branch with link to register) ─────────────────────────────────────

/// Emit `BLR Xn` — branch with link to register.
pub fn emit_blr(ops: &mut Assembler, insn: &Instruction) {
    let pc_off = reg_offset(REG_PC);
    let rn_off = src_offset(insn.rn);
    let ret_addr = insn.pc.wrapping_add(4);

    dynasm!(ops
        // Save return address to X30
        ; mov rax, QWORD ret_addr as i64
        ; mov QWORD [rdi + X30_OFF], rax
        // Set PC to target from Xn
        ; mov rax, QWORD [rdi + rn_off]
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops);
}

// ── RET (return) ────────────────────────────────────────────────────────────

/// Emit `RET {Xn}` — return to address in register (default X30).
pub fn emit_ret(ops: &mut Assembler, insn: &Instruction) {
    let pc_off = reg_offset(REG_PC);
    let rn_off = src_offset(insn.rn);

    dynasm!(ops
        ; mov rax, QWORD [rdi + rn_off]
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops);
}

// ── B.cond (conditional branch) ─────────────────────────────────────────────

/// Emit `B.cond label` — conditional branch based on NZCV flags.
///
/// Evaluates the 4-bit condition code against the current NZCV value.
pub fn emit_bcond(ops: &mut Assembler, insn: &Instruction) {
    let pc_off = reg_offset(REG_PC);
    let nzcv_off = reg_offset(REG_NZCV);
    let target = insn.pc.wrapping_add(insn.imm as u64);
    let fallthrough = insn.pc.wrapping_add(4);

    // Load NZCV into r8d
    dynasm!(ops
        ; mov r8d, DWORD [rdi + nzcv_off]
    );

    // Evaluate condition and branch
    emit_cond_check(ops, insn.cond);

    // Taken path
    dynasm!(ops
        ; mov rax, QWORD target as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops);

    // Not-taken path
    dynasm!(ops
        ; not_taken:
        ; mov rax, QWORD fallthrough as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops);
}

// ── CBZ / CBNZ ──────────────────────────────────────────────────────────────

/// Emit `CBZ Xt, label` — compare and branch on zero.
pub fn emit_cbz(ops: &mut Assembler, insn: &Instruction) {
    let pc_off = reg_offset(REG_PC);
    let rt_off = src_offset(insn.rd);
    let target = insn.pc.wrapping_add(insn.imm as u64);
    let fallthrough = insn.pc.wrapping_add(4);

    if insn.sf {
        dynasm!(ops
            ; mov rax, QWORD [rdi + rt_off]
            ; test rax, rax
            ; jnz >not_taken
        );
    } else {
        dynasm!(ops
            ; mov eax, DWORD [rdi + rt_off]
            ; test eax, eax
            ; jnz >not_taken
        );
    }

    // Taken (zero)
    dynasm!(ops
        ; mov rax, QWORD target as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops);

    // Not taken
    dynasm!(ops
        ; not_taken:
        ; mov rax, QWORD fallthrough as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops);
}

/// Emit `CBNZ Xt, label` — compare and branch on non-zero.
pub fn emit_cbnz(ops: &mut Assembler, insn: &Instruction) {
    let pc_off = reg_offset(REG_PC);
    let rt_off = src_offset(insn.rd);
    let target = insn.pc.wrapping_add(insn.imm as u64);
    let fallthrough = insn.pc.wrapping_add(4);

    if insn.sf {
        dynasm!(ops
            ; mov rax, QWORD [rdi + rt_off]
            ; test rax, rax
            ; jz >not_taken
        );
    } else {
        dynasm!(ops
            ; mov eax, DWORD [rdi + rt_off]
            ; test eax, eax
            ; jz >not_taken
        );
    }

    // Taken (non-zero)
    dynasm!(ops
        ; mov rax, QWORD target as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops);

    // Not taken
    dynasm!(ops
        ; not_taken:
        ; mov rax, QWORD fallthrough as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops);
}

// ── TBZ / TBNZ ──────────────────────────────────────────────────────────────

/// Emit `TBZ Xt, #bit, label` — test bit and branch on zero.
pub fn emit_tbz(ops: &mut Assembler, insn: &Instruction) {
    let pc_off = reg_offset(REG_PC);
    let rt_off = src_offset(insn.rn); // decoder stores Rt in rn for TBZ/TBNZ
    let target = insn.pc.wrapping_add(insn.imm as u64);
    let fallthrough = insn.pc.wrapping_add(4);
    let bit_pos = insn.imm2 as i8;

    dynasm!(ops
        ; mov rax, QWORD [rdi + rt_off]
        ; bt rax, bit_pos as i8
        ; jc >not_taken  // bit is set → not taken for TBZ
    );

    // Taken (bit is zero)
    dynasm!(ops
        ; mov rax, QWORD target as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops);

    // Not taken (bit is set)
    dynasm!(ops
        ; not_taken:
        ; mov rax, QWORD fallthrough as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops);
}

/// Emit `TBNZ Xt, #bit, label` — test bit and branch on non-zero.
pub fn emit_tbnz(ops: &mut Assembler, insn: &Instruction) {
    let pc_off = reg_offset(REG_PC);
    let rt_off = src_offset(insn.rn); // decoder stores Rt in rn for TBZ/TBNZ
    let target = insn.pc.wrapping_add(insn.imm as u64);
    let fallthrough = insn.pc.wrapping_add(4);
    let bit_pos = insn.imm2 as i8;

    dynasm!(ops
        ; mov rax, QWORD [rdi + rt_off]
        ; bt rax, bit_pos as i8
        ; jnc >not_taken  // bit is clear → not taken for TBNZ
    );

    // Taken (bit is set)
    dynasm!(ops
        ; mov rax, QWORD target as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops);

    // Not taken (bit is clear)
    dynasm!(ops
        ; not_taken:
        ; mov rax, QWORD fallthrough as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops);
}

// ── Condition code evaluator ────────────────────────────────────────────────
//
// ARM condition codes (4 bits):
//   0 EQ: Z==1               1 NE: Z==0
//   2 CS: C==1               3 CC: C==0
//   4 MI: N==1               5 PL: N==0
//   6 VS: V==1               7 VC: V==0
//   8 HI: C==1 && Z==0       9 LS: C==0 || Z==1
//  10 GE: N==V              11 LT: N!=V
//  12 GT: Z==0 && N==V      13 LE: Z==1 || N!=V
//  14 AL: always            15 NV: always (same as AL)
//
// NZCV bits: N=31, Z=30, C=29, V=28.

/// Emit a condition code check. NZCV is in `r8d`.
/// If the condition is TRUE, fall through. If FALSE, jump to `>not_taken`.
fn emit_cond_check(ops: &mut Assembler, cond: u32) {
    match cond {
        0 => {
            // EQ: Z==1
            dynasm!(ops ; bt r8d, 30 ; jnc >not_taken);
        }
        1 => {
            // NE: Z==0
            dynasm!(ops ; bt r8d, 30 ; jc >not_taken);
        }
        2 => {
            // CS/HS: C==1
            dynasm!(ops ; bt r8d, 29 ; jnc >not_taken);
        }
        3 => {
            // CC/LO: C==0
            dynasm!(ops ; bt r8d, 29 ; jc >not_taken);
        }
        4 => {
            // MI: N==1
            dynasm!(ops ; bt r8d, 31 ; jnc >not_taken);
        }
        5 => {
            // PL: N==0
            dynasm!(ops ; bt r8d, 31 ; jc >not_taken);
        }
        6 => {
            // VS: V==1
            dynasm!(ops ; bt r8d, 28 ; jnc >not_taken);
        }
        7 => {
            // VC: V==0
            dynasm!(ops ; bt r8d, 28 ; jc >not_taken);
        }
        8 => {
            // HI: C==1 && Z==0
            dynasm!(ops
                ; bt r8d, 29 ; jnc >not_taken  // C must be 1
                ; bt r8d, 30 ; jc >not_taken   // Z must be 0
            );
        }
        9 => {
            // LS: C==0 || Z==1
            dynasm!(ops
                ; bt r8d, 29 ; jnc >taken      // C==0 → taken
                ; bt r8d, 30 ; jnc >not_taken  // Z==0 (and C==1) → not taken
                ; taken:
            );
        }
        10 => {
            // GE: N==V
            dynasm!(ops
                ; mov ecx, r8d
                ; shr ecx, 31   // N in bit 0
                ; mov edx, r8d
                ; shr edx, 28   // V in bit 0
                ; xor ecx, edx
                ; test ecx, 1
                ; jnz >not_taken  // N!=V → not taken
            );
        }
        11 => {
            // LT: N!=V
            dynasm!(ops
                ; mov ecx, r8d
                ; shr ecx, 31
                ; mov edx, r8d
                ; shr edx, 28
                ; xor ecx, edx
                ; test ecx, 1
                ; jz >not_taken  // N==V → not taken
            );
        }
        12 => {
            // GT: Z==0 && N==V
            dynasm!(ops
                ; bt r8d, 30 ; jc >not_taken  // Z==1 → not taken
                ; mov ecx, r8d
                ; shr ecx, 31
                ; mov edx, r8d
                ; shr edx, 28
                ; xor ecx, edx
                ; test ecx, 1
                ; jnz >not_taken  // N!=V → not taken
            );
        }
        13 => {
            // LE: Z==1 || N!=V
            dynasm!(ops
                ; bt r8d, 30 ; jc >taken      // Z==1 → taken
                ; mov ecx, r8d
                ; shr ecx, 31
                ; mov edx, r8d
                ; shr edx, 28
                ; xor ecx, edx
                ; test ecx, 1
                ; jnz >taken
                ; jmp >not_taken
                ; taken:
            );
        }
        14 | 15 => {
            // AL/NV: always taken — no check needed, fall through
        }
        _ => unreachable!(),
    }
}

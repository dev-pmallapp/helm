//! Branch instruction emitters (RV64 -> x86-64).
//!
//! All branch emitters terminate the block: they update the guest PC in the
//! flat register array and emit a `ret` (returning `EXIT_END_OF_BLOCK` in
//! `rax`).

#![allow(missing_docs)]

use crate::block::{EXIT_END_OF_BLOCK, EXIT_EXCEPTION, EXIT_SYSCALL};
use crate::regs::REG_PC_RV64;
use dynasm::dynasm;
use dynasmrt::{x64::Assembler, DynasmApi, DynasmLabelApi};

/// Branch condition for conditional branches.
#[derive(Clone, Copy)]
pub enum BranchCond {
    /// BEQ: branch if rs1 == rs2
    Eq,
    /// BNE: branch if rs1 != rs2
    Ne,
    /// BLT: branch if rs1 < rs2 (signed)
    Lt,
    /// BGE: branch if rs1 >= rs2 (signed)
    Ge,
    /// BLTU: branch if rs1 < rs2 (unsigned)
    Ltu,
    /// BGEU: branch if rs1 >= rs2 (unsigned)
    Geu,
}

/// Byte offset of register `r` in the flat register array.
#[inline]
fn reg_off(r: u8) -> i32 {
    i32::from(r) * 8
}

/// Byte offset of the PC slot in the flat register array.
#[inline]
fn pc_off() -> i32 {
    (REG_PC_RV64 * 8) as i32
}

/// Emit the block exit sequence: store exit code and return.
fn emit_exit(ops: &mut Assembler) {
    dynasm!(ops
        ; mov rax, QWORD EXIT_END_OF_BLOCK as i64
        ; ret
    );
}

// ── Conditional branches ────────────────────────────────────────────────────

/// Emit a conditional branch (BEQ/BNE/BLT/BGE/BLTU/BGEU).
///
/// Compares rs1 and rs2, then either jumps to `pc + imm` (taken) or falls
/// through to `pc + 4` (not taken). Both paths terminate the block.
pub fn emit_branch(ops: &mut Assembler, rs1: u8, rs2: u8, imm: i64, pc: u64, cond: BranchCond) {
    let rs1_off = reg_off(rs1);
    let rs2_off = reg_off(rs2);
    let target = (pc as i64).wrapping_add(imm) as u64;
    let fallthrough = pc + 4;

    dynasm!(ops
        ; mov rax, QWORD [rdi + rs1_off]
        ; cmp rax, QWORD [rdi + rs2_off]
    );

    match cond {
        BranchCond::Eq => dynasm!(ops ; je >taken),
        BranchCond::Ne => dynasm!(ops ; jne >taken),
        BranchCond::Lt => dynasm!(ops ; jl >taken),
        BranchCond::Ge => dynasm!(ops ; jge >taken),
        BranchCond::Ltu => dynasm!(ops ; jb >taken),
        BranchCond::Geu => dynasm!(ops ; jae >taken),
    }

    // Not taken: PC = pc + 4
    dynasm!(ops
        ; mov rax, QWORD fallthrough as i64
        ; mov QWORD [rdi + pc_off()], rax
    );
    emit_exit(ops);

    // Taken: PC = pc + imm
    dynasm!(ops
        ; taken:
        ; mov rax, QWORD target as i64
        ; mov QWORD [rdi + pc_off()], rax
    );
    emit_exit(ops);
}

// ── JAL (jump and link) ─────────────────────────────────────────────────────

/// Emit JAL: `rd = pc + 4; PC = pc + imm`.
pub fn emit_jal(ops: &mut Assembler, rd: u8, imm: i64, pc: u64) {
    let target = (pc as i64).wrapping_add(imm) as u64;
    let ret_addr = pc + 4;

    // Write return address to rd (skip if rd == x0)
    if rd != 0 {
        let rd_off = reg_off(rd);
        dynasm!(ops
            ; mov rax, QWORD ret_addr as i64
            ; mov QWORD [rdi + rd_off], rax
        );
    }

    // Set PC to target
    dynasm!(ops
        ; mov rax, QWORD target as i64
        ; mov QWORD [rdi + pc_off()], rax
    );
    emit_exit(ops);
}

// ── JALR (jump and link register) ───────────────────────────────────────────

/// Emit JALR: `rd = pc + 4; PC = (rs1 + imm) & ~1`.
pub fn emit_jalr(ops: &mut Assembler, rd: u8, rs1: u8, imm: i64, pc: u64) {
    let rs1_off = reg_off(rs1);
    let ret_addr = pc + 4;

    // Compute target = (rs1 + imm) & ~1
    dynasm!(ops
        ; mov rax, QWORD [rdi + rs1_off]
        ; mov rcx, QWORD imm
        ; add rax, rcx
        ; mov rcx, QWORD -2i64            // ~1 mask
        ; and rax, rcx                    // Clear LSB per RISC-V spec
    );

    if rd != 0 {
        let rd_off = reg_off(rd);
        // Save target in rcx, write ret_addr to rd, then set PC from rcx
        dynasm!(ops
            ; mov rcx, rax                 // stash target
            ; mov rax, QWORD ret_addr as i64
            ; mov QWORD [rdi + rd_off], rax
            ; mov rax, rcx                 // restore target
        );
    }

    // Set PC to target
    dynasm!(ops ; mov QWORD [rdi + pc_off()], rax);
    emit_exit(ops);
}

// ── ECALL / EBREAK ──────────────────────────────────────────────────────────

/// Emit ECALL: set PC to current instruction's PC, return EXIT_SYSCALL.
///
/// The engine handles the actual system call dispatch after the block exits.
pub fn emit_ecall(ops: &mut Assembler, pc: u64) {
    dynasm!(ops
        ; mov rax, QWORD pc as i64
        ; mov QWORD [rdi + pc_off()], rax
        ; mov rax, QWORD EXIT_SYSCALL as i64
        ; ret
    );
}

/// Emit EBREAK: set PC to current instruction's PC, return EXIT_EXCEPTION.
pub fn emit_ebreak(ops: &mut Assembler, pc: u64) {
    dynasm!(ops
        ; mov rax, QWORD pc as i64
        ; mov QWORD [rdi + pc_off()], rax
        ; mov rax, QWORD EXIT_EXCEPTION as i64
        ; ret
    );
}

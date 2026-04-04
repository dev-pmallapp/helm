//! Fused instruction pair emitters (AArch64 → x86-64).
//!
//! Emits matched pairs from `fusion::FusedPair` as single, optimised x86-64
//! code sequences. Both instructions in the pair are consumed; the fused
//! emission terminates the block (both F1 and F2 end in a branch).
//!
//! # Register state
//!
//! All fused emitters run with the same register state as single-instruction
//! emitters: `rdi` = flat array, `rsi` = mem ptr, pinned guest registers live.

#![allow(missing_docs)]

use crate::block::EXIT_END_OF_BLOCK;
use crate::dynasm::fusion::FusedPair;
use crate::dynasm::pinned::{emit_pinned_epilogue, load_guest_to_rax};
use crate::regs::{reg_offset, REG_PC, REG_SP, REG_XZR};

use dynasm::dynasm;
use dynasmrt::{x64::Assembler, DynasmApi, DynasmLabelApi};
use helm_arch::aarch64::insn::Instruction;

/// Emit a fused instruction pair.
///
/// Returns `true` (always, since both F1 and F2 are block-terminating).
pub fn emit_fused_pair(ops: &mut Assembler, pair: &FusedPair<'_>) -> bool {
    match pair {
        FusedPair::CmpBranch { cmp, branch } => emit_f1_cmp_branch(ops, cmp, branch),
        FusedPair::SubsBne { subs, bne } => emit_f2_subs_bne(ops, subs, bne),
    }
    true
}

/// F1 fusion: `CMP Xn, #imm` + `B.cond`
///
/// Emits a single `cmp; jcc` sequence. No NZCV word is written — the x86-64
/// flags are tested directly via the appropriate `jcc`.
///
/// Eliminates:
/// - `emit_defer_nzcv_imm` (3 stores to flat array)
/// - `emit_materialize_nzcv` (conditional multi-branch sequence)
fn emit_f1_cmp_branch(ops: &mut Assembler, cmp: &Instruction, branch: &Instruction) {
    let pc_off = reg_offset(REG_PC);
    let rn_slot = if cmp.rn == 31 {
        REG_SP
    } else {
        cmp.rn as usize
    };
    let target = branch.pc.wrapping_add(branch.imm as u64);
    let fallthrough = branch.pc.wrapping_add(4);
    let imm = cmp.imm;

    // Load Xn into rax.
    load_guest_to_rax(ops, rn_slot);

    // Emit the comparison. x86-64 SUB sets SF, ZF, CF, OF exactly matching ARM SUB semantics.
    if i32::try_from(imm).is_ok() {
        dynasm!(ops ; cmp rax, imm as i32);
    } else {
        dynasm!(ops ; mov rcx, QWORD imm ; cmp rax, rcx);
    }

    // Emit jcc based on condition code. If taken, set PC = target and exit.
    // If not taken, fall through to fallthrough path.
    emit_jcc_cond_to_target(ops, branch.cond, target, fallthrough, pc_off, cmp.sf);
}

/// F2 fusion: `SUBS Xd, Xn, #1` + `B.NE`
///
/// Classic loop decrement: subtract 1 from Xn, store in Xd, branch if non-zero.
/// No NZCV word is written — x86-64 `sub; jnz` replaces the pattern entirely.
fn emit_f2_subs_bne(ops: &mut Assembler, subs: &Instruction, bne: &Instruction) {
    let pc_off = reg_offset(REG_PC);
    let rd_slot = if subs.rd == 31 {
        REG_XZR
    } else {
        subs.rd as usize
    };
    let rn_slot = if subs.rn == 31 {
        REG_SP
    } else {
        subs.rn as usize
    };
    let target = bne.pc.wrapping_add(bne.imm as u64);
    let fallthrough = bne.pc.wrapping_add(4);

    // Load Xn, subtract 1, store Xd.
    load_guest_to_rax(ops, rn_slot);
    if subs.sf {
        dynasm!(ops ; sub rax, 1);
    } else {
        dynasm!(ops ; sub eax, 1);
    }
    // Store result (use the appropriate width helper via rax/eax).
    if subs.sf {
        // 64-bit: store full rax.
        use crate::dynasm::pinned::store_rax_to_guest;
        store_rax_to_guest(ops, rd_slot);
    } else {
        // 32-bit: store eax (zero-extends automatically).
        use crate::dynasm::pinned::store_eax_to_guest_32;
        store_eax_to_guest_32(ops, rd_slot);
    }

    // After sub, x86-64 ZF is clear ↔ result != 0 ↔ branch taken.
    // B.NE = cond 1 = Z==0 → taken.
    dynasm!(ops ; jz >not_taken);

    // Taken: set PC = target, exit.
    dynasm!(ops ; mov rax, QWORD target as i64 ; mov QWORD [rdi + pc_off], rax);
    emit_pinned_epilogue(ops);
    dynasm!(ops ; mov rax, QWORD EXIT_END_OF_BLOCK as i64 ; ret);

    // Not taken: set PC = fallthrough, exit.
    dynasm!(ops
        ; not_taken:
        ; mov rax, QWORD fallthrough as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_pinned_epilogue(ops);
    dynasm!(ops ; mov rax, QWORD EXIT_END_OF_BLOCK as i64 ; ret);
}

/// Emit a conditional jump based on an ARM condition code, using x86-64 RFLAGS
/// set by a preceding `cmp` instruction.
///
/// ARM condition codes match x86-64 RFLAGS semantics for `cmp`:
/// - ZF ↔ Z, SF ↔ N, CF ↔ !C (for subtraction), OF ↔ V.
///
/// `sf` = true means 64-bit comparison was used (irrelevant for flag semantics,
/// but used to select the correct writeback path).
fn emit_jcc_cond_to_target(
    ops: &mut Assembler,
    cond: u32,
    target: u64,
    fallthrough: u64,
    pc_off: i32,
    _sf: bool,
) {
    // Each arm: if condition is FALSE, jump to >not_taken.
    // Note: x86-64 `cmp rax, rcx` computes rax - rcx and sets RFLAGS.
    //       ARM `SUBS` computes the same. However, ARM C flag = !borrow,
    //       whereas x86-64 CF = borrow. So ARM C=1 ↔ x86-64 CF=0 (no borrow).
    match cond {
        0 => dynasm!(ops ; jnz >not_taken), // EQ: ZF=1
        1 => dynasm!(ops ; jz >not_taken),  // NE: ZF=0
        2 => dynasm!(ops ; jc >not_taken),  // CS: ARM C=1 → x86 CF=0 → jnc taken; jc→not_taken
        3 => dynasm!(ops ; jnc >not_taken), // CC: ARM C=0 → x86 CF=1 → jc taken; jnc→not_taken
        4 => dynasm!(ops ; jns >not_taken), // MI: SF=1
        5 => dynasm!(ops ; js >not_taken),  // PL: SF=0
        6 => dynasm!(ops ; jno >not_taken), // VS: OF=1
        7 => dynasm!(ops ; jo >not_taken),  // VC: OF=0
        8 => {
            // HI: C=1 && Z=0 → x86: CF=0 && ZF=0 → jc|jz → not_taken
            dynasm!(ops ; jc >not_taken ; jz >not_taken);
        }
        9 => {
            // LS: C=0 || Z=1 → x86: CF=1 || ZF=1 → ja (above) = CF=0 && ZF=0
            // Taken if NOT (CF=0 && ZF=0), i.e. CF=1 or ZF=1.
            dynasm!(ops ; ja >not_taken); // ja = above = ZF=0 && CF=0 → taken means CF|ZF is set
        }
        10 => dynasm!(ops ; jl >not_taken), // GE: N==V → x86 GE = jnl; so LT → not_taken
        11 => dynasm!(ops ; jge >not_taken), // LT: N!=V → x86 LT = jl; GE → not_taken
        12 => dynasm!(ops ; jle >not_taken), // GT: Z=0 && N==V → x86 GT = jg; LE → not_taken
        13 => dynasm!(ops ; jg >not_taken), // LE: Z=1 || N!=V → x86 LE = jle; GT → not_taken
        14 | 15 => {}                       // AL/NV: always taken (no jump)
        _ => {}
    }

    // Taken path: set PC = target, exit block.
    dynasm!(ops ; mov rax, QWORD target as i64 ; mov QWORD [rdi + pc_off], rax);
    emit_pinned_epilogue(ops);
    dynasm!(ops ; mov rax, QWORD EXIT_END_OF_BLOCK as i64 ; ret);

    // Not-taken path.
    dynasm!(ops
        ; not_taken:
        ; mov rax, QWORD fallthrough as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_pinned_epilogue(ops);
    dynasm!(ops ; mov rax, QWORD EXIT_END_OF_BLOCK as i64 ; ret);
}

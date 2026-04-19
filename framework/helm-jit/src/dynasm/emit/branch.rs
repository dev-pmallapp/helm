//! Branch instruction emitters (AArch64 → x86-64).
//!
//! Unconditional/indirect branch emitters terminate the block immediately.
//! Conditional branches only exit on the taken path; fall-through continues in
//! compiled code so the block can cover straight-line hot paths.

#![allow(missing_docs)]
#![allow(clippy::similar_names)]

use crate::block::{PatchSite, EXIT_END_OF_BLOCK, MAX_CHAIN_BUDGET};
use crate::dynasm::lazy_nzcv::emit_materialize_nzcv;
use crate::dynasm::pinned::{emit_pinned_epilogue, load_guest_to_rax, store_rax_to_guest};
use crate::regs::{reg_offset, REG_JIT_RETIRED, REG_PC, REG_X0, REG_XZR};

/// Slot for X30 (link register) — pinned to r14.
const X30_SLOT: usize = REG_X0 + 30;

use dynasm::dynasm;
use dynasmrt::{x64::Assembler, DynasmApi, DynasmLabelApi};
use helm_arch::aarch64::insn::Instruction;

/// Register slot: reg 31 = XZR for data operations.
#[inline]
fn src_slot(reg: u32) -> usize {
    if reg == 31 {
        REG_XZR
    } else {
        reg as usize
    }
}

/// Emit the block exit sequence: flush pinned regs, store exit code, return.
///
/// Called at every block terminating instruction (branches, exceptions).
/// The `emit_pinned_epilogue` call is critical — without it pinned guest
/// registers (in r8–r15, rbx, rbp) would not be written back to the flat array.
fn emit_exit(ops: &mut Assembler, retired_count: u32) {
    emit_pinned_epilogue(ops);
    let retired_off = reg_offset(REG_JIT_RETIRED);
    // Accumulate (not overwrite) so chained blocks sum correctly.
    dynasm!(ops ; add QWORD [rdi + retired_off], retired_count as i32);
    dynasm!(ops
        ; mov rax, QWORD EXIT_END_OF_BLOCK as i64
        ; ret
    );
}

/// Emit a patchable exit slot that can later be rewritten into `jmp rel32`.
fn emit_chainable_exit(
    ops: &mut Assembler,
    patch_sites: &mut Vec<PatchSite>,
    target_pc: u64,
    retired_count: u32,
) {
    emit_pinned_epilogue(ops);
    let retired_off = reg_offset(REG_JIT_RETIRED);
    // Accumulate retired count so chained blocks sum correctly.
    dynasm!(ops ; add QWORD [rdi + retired_off], retired_count as i32);
    // Guard against infinite chained loops: if the accumulated count
    // exceeds MAX_CHAIN_BUDGET, bail out to the runtime immediately.
    dynasm!(ops
        ; mov rax, QWORD [rdi + retired_off]
        ; cmp rax, MAX_CHAIN_BUDGET
        ; jge >bail
        ; mov rax, QWORD EXIT_END_OF_BLOCK as i64
    );
    let patch_offset = ops.offset().0;
    dynasm!(ops
        ; ret
        ; nop
        ; nop
        ; nop
        ; nop
    );
    patch_sites.push(PatchSite {
        byte_offset: patch_offset,
        target_pc,
        linked: false,
    });
    // Budget exceeded: return to runtime for a proper budget check.
    dynasm!(ops
        ; bail:
        ; mov rax, QWORD EXIT_END_OF_BLOCK as i64
        ; ret
    );
}

// ── B (unconditional branch) ────────────────────────────────────────────────

/// Emit `B label` — unconditional PC-relative branch.
pub fn emit_b(ops: &mut Assembler, insn: &Instruction, insn_idx: u32) {
    let pc_off = reg_offset(REG_PC);
    let target = insn.pc.wrapping_add(insn.imm as u64);

    dynasm!(ops
        ; mov rax, QWORD target as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops, insn_idx + 1);
}

// ── BL (branch with link) ──────────────────────────────────────────────────

/// Emit `BL label` — branch with link (saves return address in X30).
pub fn emit_bl(ops: &mut Assembler, insn: &Instruction, insn_idx: u32) {
    let pc_off = reg_offset(REG_PC);
    let target = insn.pc.wrapping_add(insn.imm as u64);
    let ret_addr = insn.pc.wrapping_add(4);

    // Save return address to X30 (pinned to r14).
    dynasm!(ops ; mov rax, QWORD ret_addr as i64);
    store_rax_to_guest(ops, X30_SLOT);

    // Set PC to target.
    dynasm!(ops
        ; mov rax, QWORD target as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_exit(ops, insn_idx + 1);
}

// ── BR (branch to register) ────────────────────────────────────────────────

/// Emit `BR Xn` — branch to address in register.
pub fn emit_br(ops: &mut Assembler, insn: &Instruction, insn_idx: u32) {
    let pc_off = reg_offset(REG_PC);
    load_guest_to_rax(ops, src_slot(insn.rn));
    dynasm!(ops ; mov QWORD [rdi + pc_off], rax);
    emit_exit(ops, insn_idx + 1);
}

// ── BLR (branch with link to register) ─────────────────────────────────────

/// Emit `BLR Xn` — branch with link to register.
pub fn emit_blr(ops: &mut Assembler, insn: &Instruction, insn_idx: u32) {
    let pc_off = reg_offset(REG_PC);
    let ret_addr = insn.pc.wrapping_add(4);

    // Save return address to X30 (pinned to r14).
    dynasm!(ops ; mov rax, QWORD ret_addr as i64);
    store_rax_to_guest(ops, X30_SLOT);

    // Set PC to target from Xn.
    load_guest_to_rax(ops, src_slot(insn.rn));
    dynasm!(ops ; mov QWORD [rdi + pc_off], rax);
    emit_exit(ops, insn_idx + 1);
}

// ── RET (return) ────────────────────────────────────────────────────────────

/// Emit `RET {Xn}` — return to address in register (default X30).
pub fn emit_ret(ops: &mut Assembler, insn: &Instruction, insn_idx: u32) {
    let pc_off = reg_offset(REG_PC);
    load_guest_to_rax(ops, src_slot(insn.rn));
    dynasm!(ops ; mov QWORD [rdi + pc_off], rax);
    emit_exit(ops, insn_idx + 1);
}

// ── B.cond (conditional branch) ─────────────────────────────────────────────

/// Emit `B.cond label` — conditional branch based on NZCV flags.
///
/// Evaluates the 4-bit condition code against the current NZCV value.
/// NZCV is pinned to `rbp` (Rbp). The condition evaluator reads from `rbp`.
pub fn emit_bcond(
    ops: &mut Assembler,
    insn: &Instruction,
    patch_sites: &mut Vec<PatchSite>,
    insn_idx: u32,
) {
    let pc_off = reg_offset(REG_PC);
    let target = insn.pc.wrapping_add(insn.imm as u64);

    // Materialize deferred NZCV if needed (lazy NZCV may have stored FlagOp).
    // If FlagOp==None, rbp is already current and this is a no-op fast path.
    emit_materialize_nzcv(ops);

    // NZCV is pinned to rbp — `emit_cond_check` reads from rbp directly.
    // Evaluate condition and branch
    emit_cond_check(ops, insn.cond);

    // Taken path
    dynasm!(ops
        ; mov rax, QWORD target as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_chainable_exit(ops, patch_sites, target, insn_idx + 1);

    // Not-taken path continues through the rest of the compiled block.
    dynasm!(ops
        ; not_taken:
    );
}

// ── CBZ / CBNZ ──────────────────────────────────────────────────────────────

/// Emit `CBZ Xt, label` — compare and branch on zero.
pub fn emit_cbz(
    ops: &mut Assembler,
    insn: &Instruction,
    patch_sites: &mut Vec<PatchSite>,
    insn_idx: u32,
) {
    let pc_off = reg_offset(REG_PC);
    let rt = src_slot(insn.rd);
    let target = insn.pc.wrapping_add(insn.imm as u64);

    if insn.sf {
        load_guest_to_rax(ops, rt);
        dynasm!(ops ; test rax, rax ; jnz >not_taken);
    } else {
        load_guest_to_rax(ops, rt); // load_guest_to_rax is safe for 32-bit check
        dynasm!(ops ; test eax, eax ; jnz >not_taken);
    }

    // Taken (zero)
    dynasm!(ops ; mov rax, QWORD target as i64 ; mov QWORD [rdi + pc_off], rax);
    emit_chainable_exit(ops, patch_sites, target, insn_idx + 1);

    // Not taken continues through the rest of the compiled block.
    dynasm!(ops
        ; not_taken:
    );
}

/// Emit `CBNZ Xt, label` — compare and branch on non-zero.
pub fn emit_cbnz(
    ops: &mut Assembler,
    insn: &Instruction,
    patch_sites: &mut Vec<PatchSite>,
    insn_idx: u32,
) {
    let pc_off = reg_offset(REG_PC);
    let rt = src_slot(insn.rd);
    let target = insn.pc.wrapping_add(insn.imm as u64);

    if insn.sf {
        load_guest_to_rax(ops, rt);
        dynasm!(ops ; test rax, rax ; jz >not_taken);
    } else {
        load_guest_to_rax(ops, rt);
        dynasm!(ops ; test eax, eax ; jz >not_taken);
    }

    // Taken (non-zero)
    dynasm!(ops ; mov rax, QWORD target as i64 ; mov QWORD [rdi + pc_off], rax);
    emit_chainable_exit(ops, patch_sites, target, insn_idx + 1);

    // Not taken continues through the rest of the compiled block.
    dynasm!(ops
        ; not_taken:
    );
}

// ── TBZ / TBNZ ──────────────────────────────────────────────────────────────

/// Emit `TBZ Xt, #bit, label` — test bit and branch on zero.
pub fn emit_tbz(
    ops: &mut Assembler,
    insn: &Instruction,
    patch_sites: &mut Vec<PatchSite>,
    insn_idx: u32,
) {
    let pc_off = reg_offset(REG_PC);
    let rt = src_slot(insn.rn); // decoder stores Rt in rn for TBZ/TBNZ
    let target = insn.pc.wrapping_add(insn.imm as u64);
    let bit_pos = insn.imm2 as i8;

    load_guest_to_rax(ops, rt);
    dynasm!(ops ; bt rax, bit_pos as i8 ; jc >not_taken);

    // Taken (bit is zero)
    dynasm!(ops ; mov rax, QWORD target as i64 ; mov QWORD [rdi + pc_off], rax);
    emit_chainable_exit(ops, patch_sites, target, insn_idx + 1);

    // Not taken (bit is set) continues through the rest of the compiled block.
    dynasm!(ops
        ; not_taken:
    );
}

/// Emit `TBNZ Xt, #bit, label` — test bit and branch on non-zero.
pub fn emit_tbnz(
    ops: &mut Assembler,
    insn: &Instruction,
    patch_sites: &mut Vec<PatchSite>,
    insn_idx: u32,
) {
    let pc_off = reg_offset(REG_PC);
    let rt = src_slot(insn.rn); // decoder stores Rt in rn for TBZ/TBNZ
    let target = insn.pc.wrapping_add(insn.imm as u64);
    let bit_pos = insn.imm2 as i8;

    load_guest_to_rax(ops, rt);
    dynasm!(ops ; bt rax, bit_pos as i8 ; jnc >not_taken);

    // Taken (bit is set)
    dynasm!(ops ; mov rax, QWORD target as i64 ; mov QWORD [rdi + pc_off], rax);
    emit_chainable_exit(ops, patch_sites, target, insn_idx + 1);

    // Not taken (bit is clear) continues through the rest of the compiled block.
    dynasm!(ops
        ; not_taken:
    );
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

/// Emit a condition code check.
///
/// NZCV is pinned to `rbp` (HostReg::Rbp). We use `ebp` (32-bit view) for
/// bit-test operations. The `bt` instruction only reads the addressed bit;
/// it does not modify `ebp`, so the pinned value is safe throughout.
///
/// If the condition is TRUE, fall through. If FALSE, jump to `>not_taken`.
pub(crate) fn emit_cond_check(ops: &mut Assembler, cond: u32) {
    match cond {
        0 => {
            // EQ: Z==1
            dynasm!(ops ; bt ebp, 30 ; jnc >not_taken);
        }
        1 => {
            // NE: Z==0
            dynasm!(ops ; bt ebp, 30 ; jc >not_taken);
        }
        2 => {
            // CS/HS: C==1
            dynasm!(ops ; bt ebp, 29 ; jnc >not_taken);
        }
        3 => {
            // CC/LO: C==0
            dynasm!(ops ; bt ebp, 29 ; jc >not_taken);
        }
        4 => {
            // MI: N==1
            dynasm!(ops ; bt ebp, 31 ; jnc >not_taken);
        }
        5 => {
            // PL: N==0
            dynasm!(ops ; bt ebp, 31 ; jc >not_taken);
        }
        6 => {
            // VS: V==1
            dynasm!(ops ; bt ebp, 28 ; jnc >not_taken);
        }
        7 => {
            // VC: V==0
            dynasm!(ops ; bt ebp, 28 ; jc >not_taken);
        }
        8 => {
            // HI: C==1 && Z==0
            dynasm!(ops
                ; bt ebp, 29 ; jnc >not_taken  // C must be 1
                ; bt ebp, 30 ; jc >not_taken   // Z must be 0
            );
        }
        9 => {
            // LS: C==0 || Z==1
            dynasm!(ops
                ; bt ebp, 29 ; jnc >taken      // C==0 → taken
                ; bt ebp, 30 ; jnc >not_taken  // Z==0 (and C==1) → not taken
                ; taken:
            );
        }
        10 => {
            // GE: N==V  — use rax as scratch (clobbered by branch emitters)
            dynasm!(ops
                ; mov eax, ebp
                ; shr eax, 31   // N in bit 0
                ; mov ecx, ebp
                ; shr ecx, 28   // V in bit 0
                ; xor eax, ecx
                ; test eax, 1
                ; jnz >not_taken  // N!=V → not taken
            );
        }
        11 => {
            // LT: N!=V
            dynasm!(ops
                ; mov eax, ebp
                ; shr eax, 31
                ; mov ecx, ebp
                ; shr ecx, 28
                ; xor eax, ecx
                ; test eax, 1
                ; jz >not_taken  // N==V → not taken
            );
        }
        12 => {
            // GT: Z==0 && N==V
            dynasm!(ops
                ; bt ebp, 30 ; jc >not_taken  // Z==1 → not taken
                ; mov eax, ebp
                ; shr eax, 31
                ; mov ecx, ebp
                ; shr ecx, 28
                ; xor eax, ecx
                ; test eax, 1
                ; jnz >not_taken  // N!=V → not taken
            );
        }
        13 => {
            // LE: Z==1 || N!=V
            dynasm!(ops
                ; bt ebp, 30 ; jc >taken      // Z==1 → taken
                ; mov eax, ebp
                ; shr eax, 31
                ; mov ecx, ebp
                ; shr ecx, 28
                ; xor eax, ecx
                ; test eax, 1
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

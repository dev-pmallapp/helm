//! Lazy NZCV (deferred flag computation) for the dynasm JIT backend.
//!
//! # Motivation
//!
//! On AArch64, most flag-setting instructions (ADDS, SUBS, ANDS, CMP, CMN)
//! are immediately followed by non-flag-setting instructions, with the flags
//! only actually consumed by a B.cond or CSEL at the end of the sequence.
//! Computing NZCV eagerly (via pushfq + bit extraction) costs ~10 instructions
//! per flag-setting op even when the flags are never read.
//!
//! # How Lazy NZCV Works
//!
//! Instead of computing NZCV immediately, the JIT stores:
//! - `flat[REG_FLAG_OP]`  = `FlagOp` variant (which operation produced the flags)
//! - `flat[REG_FLAG_LHS]` = left operand (rn before the operation)
//! - `flat[REG_FLAG_RHS]` = right operand (immediate or rm value)
//!
//! When a branch instruction needs to read NZCV (via `emit_materialize_nzcv`),
//! it replays the arithmetic on the stored operands and captures the flags then.
//!
//! # Pinning interaction
//!
//! NZCV is pinned to `rbp`. The deferred state lives in the flat array slots
//! 41–43. When flags are materialized, `ebp` is updated with the new NZCV word.
//! The block epilogue then flushes `rbp` to the flat array.
//!
//! # When to use lazy vs eager
//!
//! `emit_defer_nzcv` replaces the `emit_capture_nzcv_*` calls in flag-setting
//! instruction emitters (ADDS, SUBS, ANDS). `emit_materialize_nzcv` is called
//! at the start of every branch emitter that reads NZCV (B.cond).
//!
//! CBZ/CBNZ/TBZ/TBNZ do not read NZCV — they do not need materialization.
//!
//! # Correctness invariant
//!
//! `REG_FLAG_OP = FlagOp::None` means rbp (NZCV) is already up-to-date.
//! Any value other than None means rbp holds stale NZCV; the correct value
//! can be reconstructed from REG_FLAG_LHS/RHS using FlagOp.

#![allow(missing_docs)]

use crate::regs::{reg_offset, FlagOp, REG_FLAG_LHS, REG_FLAG_OP, REG_FLAG_RHS};
use dynasm::dynasm;
use dynasmrt::{x64::Assembler, DynasmApi, DynasmLabelApi};

// ── Defer NZCV computation ──────────────────────────────────────────────────

/// Emit code to defer NZCV computation for a flag-setting instruction.
///
/// Instead of computing NZCV now, stores `flag_op`, `lhs`, and `rhs` in
/// reserved flat array slots. The actual computation is deferred until a
/// branch instruction reads the flags.
///
/// # Arguments
/// - `flag_op`: which operation produced the flags (Add64, Sub64, etc.)
/// - `lhs_slot`: the guest register slot that holds the left operand
/// - `rhs_imm`: the immediate right-hand operand value (for ADDS/SUBS imm)
///   Pass 0 if `rhs_slot` is used instead.
/// - `rhs_slot`: if `Some(slot)`, the right operand comes from a register.
///   If `None`, `rhs_imm` is used.
///
/// # Register state on entry
/// - `rax` = the arithmetic result (already computed; we need lhs BEFORE the op)
///
/// # Register state on exit
/// - `rax` = unchanged (arithmetic result preserved)
/// - `rbp` = marked as stale (set REG_FLAG_OP to non-None value)
///
/// Note: lhs must be passed in as `rax_lhs` because by the time this function
/// is called, rax already holds the result. The caller must pass the pre-operation
/// lhs value.
///
/// This is the simpler form for immediate operations where we can pre-compute
/// what to store. The caller passes the lhs and rhs values directly.
pub fn emit_defer_nzcv_imm(
    ops: &mut Assembler,
    flag_op: FlagOp,
    lhs_in_rcx: bool, // if true, lhs is in rcx; otherwise caller already stored it
    rhs_imm: i64,
) {
    let op_off = reg_offset(REG_FLAG_OP);
    let lhs_off = reg_offset(REG_FLAG_LHS);
    let rhs_off = reg_offset(REG_FLAG_RHS);

    // Store FlagOp to flat[REG_FLAG_OP].
    dynasm!(ops
        ; mov DWORD [rdi + op_off], flag_op as i32
    );

    // Store rhs immediate to flat[REG_FLAG_RHS].
    if i32::try_from(rhs_imm).is_ok() {
        dynasm!(ops ; mov QWORD [rdi + rhs_off], rhs_imm as i32);
    } else {
        dynasm!(ops
            ; push rax
            ; mov rax, QWORD rhs_imm
            ; mov QWORD [rdi + rhs_off], rax
            ; pop rax
        );
    }

    // Store lhs to flat[REG_FLAG_LHS]. Lhs is in rcx (loaded before the arithmetic op).
    if lhs_in_rcx {
        dynasm!(ops ; mov QWORD [rdi + lhs_off], rcx);
    }
    // If lhs was not in rcx, caller must have stored it separately.
}

/// Emit code to defer NZCV for a register-to-register flag-setting operation.
///
/// Caller has already loaded Rn into rax and Rm into rcx; the arithmetic was
/// computed (result in rax). This stores Rn (from rdx — see below) and Rm (rcx).
///
/// Convention: caller must `mov rdx, rax_before` then `add/sub rax, rcx` to
/// preserve both operands. Or more practically, emit with operands passed in:
///   lhs in rdx (the Rn value before the op), rhs in rcx (Rm, possibly shifted).
pub fn emit_defer_nzcv_reg(ops: &mut Assembler, flag_op: FlagOp) {
    let op_off = reg_offset(REG_FLAG_OP);
    let lhs_off = reg_offset(REG_FLAG_LHS);
    let rhs_off = reg_offset(REG_FLAG_RHS);

    dynasm!(ops
        ; mov DWORD [rdi + op_off], flag_op as i32
        ; mov QWORD [rdi + lhs_off], rdx   // Rn (pre-op lhs, loaded by caller into rdx)
        ; mov QWORD [rdi + rhs_off], rcx   // Rm (post-shift rhs)
    );
}

// ── Materialize deferred NZCV ───────────────────────────────────────────────

/// Emit code to materialize NZCV from deferred state if necessary.
///
/// If `flat[REG_FLAG_OP] == FlagOp::None`, NZCV in `rbp` is already current —
/// nothing to do. Otherwise, replay the stored arithmetic operation on the
/// stored operands and update `rbp` with the newly computed NZCV.
///
/// Must be called at the start of every branch emitter that reads NZCV (B.cond).
///
/// # Register state on exit
/// - `rbp` = up-to-date ARM NZCV word
/// - `flat[REG_FLAG_OP]` = FlagOp::None (cleared after materialization)
/// - `rax`, `rcx`, `rdx` = clobbered
pub fn emit_materialize_nzcv(ops: &mut Assembler) {
    let op_off = reg_offset(REG_FLAG_OP);
    let lhs_off = reg_offset(REG_FLAG_LHS);
    let rhs_off = reg_offset(REG_FLAG_RHS);

    // Fast path: if FlagOp::None (0), rbp is already current.
    dynasm!(ops
        ; cmp DWORD [rdi + op_off], 0
        ; je >nzcv_current
    );

    // Load lhs and rhs from the flat array.
    dynasm!(ops
        ; mov rax, QWORD [rdi + lhs_off]   // lhs
        ; mov rcx, QWORD [rdi + rhs_off]   // rhs
        ; mov edx, DWORD [rdi + op_off]    // FlagOp variant
    );

    // Dispatch based on FlagOp. We handle each case with its own arithmetic.
    // FlagOp variants: None=0, Add64=1, Sub64=2, And64=3, Add32=4, Sub32=5, And32=6.
    dynasm!(ops
        ; cmp edx, 1  // Add64
        ; je >do_add64
        ; cmp edx, 2  // Sub64
        ; je >do_sub64
        ; cmp edx, 3  // And64
        ; je >do_and64
        ; cmp edx, 4  // Add32
        ; je >do_add32
        ; cmp edx, 5  // Sub32
        ; je >do_sub32
        ; cmp edx, 6  // And32
        ; je >do_and32
        // Unknown — treat as And64 (sets N/Z, clears C/V)
        ; jmp >do_and64

        ; do_add64:
        ; add rax, rcx
    );
    emit_capture_from_rflags(ops, false);
    dynasm!(ops ; jmp >nzcv_done);

    dynasm!(ops ; do_sub64: ; sub rax, rcx);
    emit_capture_from_rflags(ops, true);
    dynasm!(ops ; jmp >nzcv_done);

    dynasm!(ops ; do_and64: ; and rax, rcx);
    emit_capture_logical_from_rflags(ops);
    dynasm!(ops ; jmp >nzcv_done);

    dynasm!(ops ; do_add32: ; add eax, ecx);
    emit_capture_from_rflags(ops, false);
    dynasm!(ops ; jmp >nzcv_done);

    dynasm!(ops ; do_sub32: ; sub eax, ecx);
    emit_capture_from_rflags(ops, true);
    dynasm!(ops ; jmp >nzcv_done);

    dynasm!(ops ; do_and32: ; and eax, ecx);
    emit_capture_logical_from_rflags(ops);

    dynasm!(ops
        ; nzcv_done:
        // Clear the deferred flag op.
        ; mov DWORD [rdi + op_off], 0

        ; nzcv_current:
    );
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Capture NZCV from RFLAGS into rbp after an arithmetic operation.
/// Uses pushfq to atomically save RFLAGS. rax (result) is preserved.
pub(crate) fn emit_capture_from_rflags(ops: &mut Assembler, is_sub: bool) {
    dynasm!(ops
        ; push rax       // preserve arithmetic result
        ; pushfq         // save RFLAGS atomically
        ; pop  rax       // RFLAGS in rax

        // N = SF = bit7 → bit31
        ; mov ecx, eax ; shr ecx, 7 ; and ecx, 1 ; shl ecx, 31

        // Z = ZF = bit6 → bit30
        ; mov edx, eax ; shr edx, 6 ; and edx, 1 ; shl edx, 30
        ; or  ecx, edx

        // V = OF = bit11 → bit28
        ; mov edx, eax ; shr edx, 11 ; and edx, 1 ; shl edx, 28
        ; or  ecx, edx
    );
    // C = CF = bit0 → bit29. For SUB: invert.
    if is_sub {
        dynasm!(ops
            ; mov edx, eax ; and edx, 1 ; xor edx, 1 ; shl edx, 29 ; or ecx, edx
        );
    } else {
        dynasm!(ops
            ; mov edx, eax ; and edx, 1 ; shl edx, 29 ; or ecx, edx
        );
    }
    dynasm!(ops
        ; pop  rax       // restore arithmetic result
        ; mov  ebp, ecx  // update pinned NZCV
    );
}

/// Capture NZCV for logical ops (C=0, V=0) from RFLAGS into rbp.
pub(crate) fn emit_capture_logical_from_rflags(ops: &mut Assembler) {
    dynasm!(ops
        ; push rax ; pushfq ; pop rax

        ; mov ecx, eax ; shr ecx, 7 ; and ecx, 1 ; shl ecx, 31   // N
        ; mov edx, eax ; shr edx, 6 ; and edx, 1 ; shl edx, 30   // Z
        ; or  ecx, edx   // C=0, V=0 already

        ; pop  rax
        ; mov  ebp, ecx
    );
}

#[cfg(test)]
mod tests {
    use crate::regs::{FlagOp, REG_FLAG_OP};

    #[test]
    fn flag_op_discriminants() {
        // Ensure FlagOp discriminants match what we store/compare in emitted code.
        assert_eq!(FlagOp::None as i32, 0);
        assert_eq!(FlagOp::Add64 as i32, 1);
        assert_eq!(FlagOp::Sub64 as i32, 2);
        assert_eq!(FlagOp::And64 as i32, 3);
        assert_eq!(FlagOp::Add32 as i32, 4);
        assert_eq!(FlagOp::Sub32 as i32, 5);
        assert_eq!(FlagOp::And32 as i32, 6);
    }

    #[test]
    fn flag_op_slots_distinct_from_stash() {
        // Slot 38 is used as stash in ldst.rs — confirm FLAG_OP slots don't overlap.
        assert_ne!(REG_FLAG_OP, 38);
    }
}

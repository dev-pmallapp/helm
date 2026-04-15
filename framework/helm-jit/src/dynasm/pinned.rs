//! Dynasm emit helpers for pinned vs spilled guest registers.
//!
//! Every guest register access in compiled blocks goes through these helpers
//! instead of bare `[rdi + N*8]` memory accesses.  When a register is pinned
//! to a host register (see `regs::DEFAULT_BINDING`) the helper emits a single
//! `mov reg, reg` instruction; otherwise it falls back to the flat-array load.
//!
//! # Caller-saved pinned registers (r8–r11)
//!
//! Registers r8–r11 are **caller-saved** in the System V AMD64 ABI.  Any C
//! helper call (e.g. `jit_mem_read`, `jit_mem_write`) will clobber them.
//! Emitters that call C helpers must push/pop r8–r11 around the call.
//! The convenience helpers `save_caller_saved_pinned` / `restore_caller_saved_pinned`
//! emit these push/pop sequences.
//!
//! # Block prologue / epilogue
//!
//! `emit_pinned_prologue` must be the very first code in every compiled block.
//! `emit_pinned_epilogue` must precede every `ret` instruction.
//!
//! The `run_jit` call site in `helm-engine` does **not** need to save r8–r11
//! before calling a compiled block because the block itself is responsible for
//! saving them via the block-chaining contract: any block that calls a C helper
//! saves/restores r8–r11 around the call.

#![allow(missing_docs)]

use crate::dynasm::lazy_nzcv::emit_materialize_nzcv;
use crate::regs::{pinned_host_reg, reg_offset, HostReg, DEFAULT_BINDING};
use dynasm::dynasm;
use dynasmrt::{x64::Assembler, DynasmApi};

// ── Load / store helpers ────────────────────────────────────────────────────

/// Emit code to load guest register `slot` into x86-64 `rax`.
///
/// If `slot` is pinned: emits `mov rax, <host_reg>` (1 instruction).
/// If `slot` is spilled: emits `mov rax, QWORD [rdi + slot*8]` (1 instruction).
pub fn load_guest_to_rax(ops: &mut Assembler, slot: usize) {
    match pinned_host_reg(slot) {
        Some(HostReg::R8) => dynasm!(ops ; mov rax, r8),
        Some(HostReg::R9) => dynasm!(ops ; mov rax, r9),
        Some(HostReg::R10) => dynasm!(ops ; mov rax, r10),
        Some(HostReg::R11) => dynasm!(ops ; mov rax, r11),
        Some(HostReg::R12) => dynasm!(ops ; mov rax, r12),
        Some(HostReg::R13) => dynasm!(ops ; mov rax, r13),
        Some(HostReg::R14) => dynasm!(ops ; mov rax, r14),
        Some(HostReg::R15) => dynasm!(ops ; mov rax, r15),
        Some(HostReg::Rbx) => dynasm!(ops ; mov rax, rbx),
        Some(HostReg::Rbp) => dynasm!(ops ; mov rax, rbp),
        None => {
            let off = reg_offset(slot);
            dynasm!(ops ; mov rax, QWORD [rdi + off]);
        }
    }
}

/// Emit code to load the 32-bit value of guest register `slot` into `eax`.
///
/// Pinned: `mov eax, <host_reg32>`. Spilled: `mov eax, DWORD [rdi + off]`.
pub fn load_guest_to_eax(ops: &mut Assembler, slot: usize) {
    match pinned_host_reg(slot) {
        Some(HostReg::R8) => dynasm!(ops ; mov eax, r8d),
        Some(HostReg::R9) => dynasm!(ops ; mov eax, r9d),
        Some(HostReg::R10) => dynasm!(ops ; mov eax, r10d),
        Some(HostReg::R11) => dynasm!(ops ; mov eax, r11d),
        Some(HostReg::R12) => dynasm!(ops ; mov eax, r12d),
        Some(HostReg::R13) => dynasm!(ops ; mov eax, r13d),
        Some(HostReg::R14) => dynasm!(ops ; mov eax, r14d),
        Some(HostReg::R15) => dynasm!(ops ; mov eax, r15d),
        Some(HostReg::Rbx) => dynasm!(ops ; mov eax, ebx),
        Some(HostReg::Rbp) => dynasm!(ops ; mov eax, ebp),
        None => {
            let off = reg_offset(slot);
            dynasm!(ops ; mov eax, DWORD [rdi + off]);
        }
    }
}

/// Emit code to load guest register `slot` into x86-64 `rcx`.
///
/// Used for shift-register operands in shift/rotate emitters.
pub fn load_guest_to_rcx(ops: &mut Assembler, slot: usize) {
    match pinned_host_reg(slot) {
        Some(HostReg::R8) => dynasm!(ops ; mov rcx, r8),
        Some(HostReg::R9) => dynasm!(ops ; mov rcx, r9),
        Some(HostReg::R10) => dynasm!(ops ; mov rcx, r10),
        Some(HostReg::R11) => dynasm!(ops ; mov rcx, r11),
        Some(HostReg::R12) => dynasm!(ops ; mov rcx, r12),
        Some(HostReg::R13) => dynasm!(ops ; mov rcx, r13),
        Some(HostReg::R14) => dynasm!(ops ; mov rcx, r14),
        Some(HostReg::R15) => dynasm!(ops ; mov rcx, r15),
        Some(HostReg::Rbx) => dynasm!(ops ; mov rcx, rbx),
        Some(HostReg::Rbp) => dynasm!(ops ; mov rcx, rbp),
        None => {
            let off = reg_offset(slot);
            dynasm!(ops ; mov rcx, QWORD [rdi + off]);
        }
    }
}

/// Emit code to store `rax` into guest register `slot`.
///
/// If `slot` is pinned: `mov <host_reg>, rax`. If spilled: flat-array store.
///
/// Writes to `REG_XZR` are silently discarded (AArch64 XZR is a zero sink).
pub fn store_rax_to_guest(ops: &mut Assembler, slot: usize) {
    if slot == crate::regs::REG_XZR {
        return; // XZR writes are architecturally discarded
    }
    match pinned_host_reg(slot) {
        Some(HostReg::R8) => dynasm!(ops ; mov r8,  rax),
        Some(HostReg::R9) => dynasm!(ops ; mov r9,  rax),
        Some(HostReg::R10) => dynasm!(ops ; mov r10, rax),
        Some(HostReg::R11) => dynasm!(ops ; mov r11, rax),
        Some(HostReg::R12) => dynasm!(ops ; mov r12, rax),
        Some(HostReg::R13) => dynasm!(ops ; mov r13, rax),
        Some(HostReg::R14) => dynasm!(ops ; mov r14, rax),
        Some(HostReg::R15) => dynasm!(ops ; mov r15, rax),
        Some(HostReg::Rbx) => dynasm!(ops ; mov rbx, rax),
        Some(HostReg::Rbp) => dynasm!(ops ; mov rbp, rax),
        None => {
            let off = reg_offset(slot);
            dynasm!(ops ; mov QWORD [rdi + off], rax);
        }
    }
}

/// Emit code to store the 32-bit `eax` into guest register `slot`.
///
/// For 32-bit results: writes `eax` to the low 32 bits and zeros the high 32 bits.
/// Pinned: two instructions (mov low32, zero high32 via movzx or explicit 0).
/// Spilled: two DWORD stores.
///
/// Writes to `REG_XZR` are silently discarded.
pub fn store_eax_to_guest_32(ops: &mut Assembler, slot: usize) {
    if slot == crate::regs::REG_XZR {
        return;
    }
    match pinned_host_reg(slot) {
        Some(HostReg::R8) => {
            // Writing to r8d automatically zero-extends to r8 on x86-64.
            dynasm!(ops ; mov r8d, eax);
        }
        Some(HostReg::R9) => dynasm!(ops ; mov r9d, eax),
        Some(HostReg::R10) => dynasm!(ops ; mov r10d, eax),
        Some(HostReg::R11) => dynasm!(ops ; mov r11d, eax),
        Some(HostReg::R12) => dynasm!(ops ; mov r12d, eax),
        Some(HostReg::R13) => dynasm!(ops ; mov r13d, eax),
        Some(HostReg::R14) => dynasm!(ops ; mov r14d, eax),
        Some(HostReg::R15) => dynasm!(ops ; mov r15d, eax),
        Some(HostReg::Rbx) => dynasm!(ops ; mov ebx, eax),
        Some(HostReg::Rbp) => dynasm!(ops ; mov ebp, eax),
        None => {
            let off = reg_offset(slot);
            dynasm!(ops
                ; mov DWORD [rdi + off], eax
                ; mov DWORD [rdi + off + 4], 0
            );
        }
    }
}

// ── Caller-saved pinned register save/restore ───────────────────────────────
//
// Pinned registers r8–r11 (X0–X3) are caller-saved in the x86-64 ABI.
// Emit push/pop sequences around C helper calls to preserve guest register state.

/// Push r8–r11 (caller-saved pinned regs: X0–X3) before a C helper call.
///
/// Must be paired with `restore_caller_saved_pinned`. The effective address
/// and value operands should be loaded into their final argument registers
/// (rdi/rsi/rdx/rcx per ABI) *after* this call.
pub fn save_caller_saved_pinned(ops: &mut Assembler) {
    dynasm!(ops
        ; push r8
        ; push r9
        ; push r10
        ; push r11
    );
}

/// Pop r8–r11 after a C helper call, restoring pinned guest register state.
pub fn restore_caller_saved_pinned(ops: &mut Assembler) {
    dynasm!(ops
        ; pop r11
        ; pop r10
        ; pop r9
        ; pop r8
    );
}

// ── Block prologue / epilogue ───────────────────────────────────────────────

/// Emit the block entry prologue.
///
/// This must be the first code emitted in every compiled block.
///
/// Actions:
/// 1. Push all callee-saved host registers used as pinned guest registers
///    (rbx, rbp, r12–r15).  r8–r11 are caller-saved and NOT pushed here —
///    the JIT block itself saves/restores them around any C helper calls.
/// 2. Load all 10 pinned guest registers from `[rdi + slot*8]` into host regs.
pub fn emit_pinned_prologue(ops: &mut Assembler) {
    // Push callee-saved regs. r8–r11 are caller-saved and live for the
    // entire block; callers of the compiled block must handle them.
    dynasm!(ops
        ; push rbx
        ; push rbp
        ; push r12
        ; push r13
        ; push r14
        ; push r15
    );

    // Load pinned guest registers from flat array (rdi = flat array ptr).
    for (slot, hreg) in &DEFAULT_BINDING {
        let off = reg_offset(*slot);
        match hreg {
            HostReg::R8 => dynasm!(ops ; mov r8,  QWORD [rdi + off]),
            HostReg::R9 => dynasm!(ops ; mov r9,  QWORD [rdi + off]),
            HostReg::R10 => dynasm!(ops ; mov r10, QWORD [rdi + off]),
            HostReg::R11 => dynasm!(ops ; mov r11, QWORD [rdi + off]),
            HostReg::R12 => dynasm!(ops ; mov r12, QWORD [rdi + off]),
            HostReg::R13 => dynasm!(ops ; mov r13, QWORD [rdi + off]),
            HostReg::R14 => dynasm!(ops ; mov r14, QWORD [rdi + off]),
            HostReg::R15 => dynasm!(ops ; mov r15, QWORD [rdi + off]),
            HostReg::Rbx => dynasm!(ops ; mov rbx, QWORD [rdi + off]),
            HostReg::Rbp => dynasm!(ops ; mov rbp, QWORD [rdi + off]),
        }
    }

    // Re-zero the XZR sentinel slot.  Previous blocks may have written a
    // non-zero value via SUBS-to-XZR (CMP) or similar; without this reset,
    // subsequent reads of XZR (e.g. ORR Xd, XZR, Xm used as MOV) would
    // return a stale non-zero value and corrupt the result.
    let xzr_off = reg_offset(crate::regs::REG_XZR);
    dynasm!(ops ; mov QWORD [rdi + xzr_off], 0);
}

/// Emit the block exit epilogue.
///
/// This must precede every `ret` in a compiled block.
///
/// Actions:
/// 1. Flush all 10 pinned guest registers from host regs back to `[rdi + slot*8]`.
/// 2. Pop callee-saved registers in LIFO order (reverse of prologue push order).
/// 3. Place the exit code in `rax` (EXIT_END_OF_BLOCK = 0 for normal exit).
///
/// Callers that need a non-zero exit code should set `rax` *after* calling this.
pub fn emit_pinned_epilogue(ops: &mut Assembler) {
    // Materialize deferred NZCV into rbp before flushing to flat array.
    // If FlagOp == None, rbp is already current; this is a 2-instruction fast path.
    emit_materialize_nzcv(ops);

    // Flush pinned guest registers to flat array.
    for (slot, hreg) in DEFAULT_BINDING.iter().rev() {
        let off = reg_offset(*slot);
        match hreg {
            HostReg::R8 => dynasm!(ops ; mov QWORD [rdi + off], r8),
            HostReg::R9 => dynasm!(ops ; mov QWORD [rdi + off], r9),
            HostReg::R10 => dynasm!(ops ; mov QWORD [rdi + off], r10),
            HostReg::R11 => dynasm!(ops ; mov QWORD [rdi + off], r11),
            HostReg::R12 => dynasm!(ops ; mov QWORD [rdi + off], r12),
            HostReg::R13 => dynasm!(ops ; mov QWORD [rdi + off], r13),
            HostReg::R14 => dynasm!(ops ; mov QWORD [rdi + off], r14),
            HostReg::R15 => dynasm!(ops ; mov QWORD [rdi + off], r15),
            HostReg::Rbx => dynasm!(ops ; mov QWORD [rdi + off], rbx),
            HostReg::Rbp => dynasm!(ops ; mov QWORD [rdi + off], rbp),
        }
    }

    // Restore callee-saved registers (LIFO: reverse of push order in prologue).
    dynasm!(ops
        ; pop r15
        ; pop r14
        ; pop r13
        ; pop r12
        ; pop rbp
        ; pop rbx
    );
}

// ── Slot-to-slot optimized mov ──────────────────────────────────────────────

/// Emit `mov dest_slot, src_slot` using the most efficient available form.
///
/// If both slots are pinned: `mov hreg_dest, hreg_src` (1 instruction, zero latency).
/// If only dest is pinned: `mov hreg_dest, [rdi + src_off]`.
/// If only src is pinned: `mov [rdi + dst_off], hreg_src`.
/// If neither: `mov rax, [rdi + src_off]; mov [rdi + dst_off], rax`.
pub fn emit_mov_guest_to_guest(ops: &mut Assembler, dst: usize, src: usize) {
    if dst == crate::regs::REG_XZR {
        return; // XZR writes are architecturally discarded
    }
    let dst_pin = pinned_host_reg(dst);
    let src_pin = pinned_host_reg(src);

    match (dst_pin, src_pin) {
        // Both pinned — register to register
        (Some(d), Some(s)) => emit_mov_hreg_to_hreg(ops, d, s),
        // Only dst pinned
        (Some(d), None) => {
            let src_off = reg_offset(src);
            match d {
                HostReg::R8 => dynasm!(ops ; mov r8,  QWORD [rdi + src_off]),
                HostReg::R9 => dynasm!(ops ; mov r9,  QWORD [rdi + src_off]),
                HostReg::R10 => dynasm!(ops ; mov r10, QWORD [rdi + src_off]),
                HostReg::R11 => dynasm!(ops ; mov r11, QWORD [rdi + src_off]),
                HostReg::R12 => dynasm!(ops ; mov r12, QWORD [rdi + src_off]),
                HostReg::R13 => dynasm!(ops ; mov r13, QWORD [rdi + src_off]),
                HostReg::R14 => dynasm!(ops ; mov r14, QWORD [rdi + src_off]),
                HostReg::R15 => dynasm!(ops ; mov r15, QWORD [rdi + src_off]),
                HostReg::Rbx => dynasm!(ops ; mov rbx, QWORD [rdi + src_off]),
                HostReg::Rbp => dynasm!(ops ; mov rbp, QWORD [rdi + src_off]),
            }
        }
        // Only src pinned
        (None, Some(s)) => {
            let dst_off = reg_offset(dst);
            match s {
                HostReg::R8 => dynasm!(ops ; mov QWORD [rdi + dst_off], r8),
                HostReg::R9 => dynasm!(ops ; mov QWORD [rdi + dst_off], r9),
                HostReg::R10 => dynasm!(ops ; mov QWORD [rdi + dst_off], r10),
                HostReg::R11 => dynasm!(ops ; mov QWORD [rdi + dst_off], r11),
                HostReg::R12 => dynasm!(ops ; mov QWORD [rdi + dst_off], r12),
                HostReg::R13 => dynasm!(ops ; mov QWORD [rdi + dst_off], r13),
                HostReg::R14 => dynasm!(ops ; mov QWORD [rdi + dst_off], r14),
                HostReg::R15 => dynasm!(ops ; mov QWORD [rdi + dst_off], r15),
                HostReg::Rbx => dynasm!(ops ; mov QWORD [rdi + dst_off], rbx),
                HostReg::Rbp => dynasm!(ops ; mov QWORD [rdi + dst_off], rbp),
            }
        }
        // Neither pinned — through rax
        (None, None) => {
            let src_off = reg_offset(src);
            let dst_off = reg_offset(dst);
            dynasm!(ops
                ; mov rax, QWORD [rdi + src_off]
                ; mov QWORD [rdi + dst_off], rax
            );
        }
    }
}

/// Emit `mov dst_hreg, src_hreg` for two host registers.
fn emit_mov_hreg_to_hreg(ops: &mut Assembler, dst: HostReg, src: HostReg) {
    if dst == src {
        return; // same pinned register — nothing to do
    }
    // We need a two-register mov. Use rax as scratch if needed.
    // Load src into rax, then store to dst.
    match src {
        HostReg::R8 => dynasm!(ops ; mov rax, r8),
        HostReg::R9 => dynasm!(ops ; mov rax, r9),
        HostReg::R10 => dynasm!(ops ; mov rax, r10),
        HostReg::R11 => dynasm!(ops ; mov rax, r11),
        HostReg::R12 => dynasm!(ops ; mov rax, r12),
        HostReg::R13 => dynasm!(ops ; mov rax, r13),
        HostReg::R14 => dynasm!(ops ; mov rax, r14),
        HostReg::R15 => dynasm!(ops ; mov rax, r15),
        HostReg::Rbx => dynasm!(ops ; mov rax, rbx),
        HostReg::Rbp => dynasm!(ops ; mov rax, rbp),
    }
    match dst {
        HostReg::R8 => dynasm!(ops ; mov r8,  rax),
        HostReg::R9 => dynasm!(ops ; mov r9,  rax),
        HostReg::R10 => dynasm!(ops ; mov r10, rax),
        HostReg::R11 => dynasm!(ops ; mov r11, rax),
        HostReg::R12 => dynasm!(ops ; mov r12, rax),
        HostReg::R13 => dynasm!(ops ; mov r13, rax),
        HostReg::R14 => dynasm!(ops ; mov r14, rax),
        HostReg::R15 => dynasm!(ops ; mov r15, rax),
        HostReg::Rbx => dynasm!(ops ; mov rbx, rax),
        HostReg::Rbp => dynasm!(ops ; mov rbp, rax),
    }
}

/// Returns true if a guest slot is pinned AND its host reg is caller-saved.
///
/// Caller-saved pinned regs must be pushed/popped around C helper calls.
#[inline]
pub fn is_caller_saved_pinned(slot: usize) -> bool {
    matches!(
        pinned_host_reg(slot),
        Some(HostReg::R8 | HostReg::R9 | HostReg::R10 | HostReg::R11)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regs::{REG_NZCV, REG_SP, REG_X0};

    #[test]
    fn pinned_slots_have_host_regs() {
        // X0 is pinned to R8
        assert_eq!(pinned_host_reg(REG_X0), Some(HostReg::R8));
        // X1 is pinned to R9
        assert_eq!(pinned_host_reg(REG_X0 + 1), Some(HostReg::R9));
        // X5 is NOT pinned
        assert_eq!(pinned_host_reg(REG_X0 + 5), None);
        // SP is pinned to R15
        assert_eq!(pinned_host_reg(REG_SP), Some(HostReg::R15));
        // NZCV is pinned to Rbp
        assert_eq!(pinned_host_reg(REG_NZCV), Some(HostReg::Rbp));
    }

    #[test]
    fn caller_saved_pinned_detection() {
        // X0–X3 are pinned to r8–r11 (caller-saved)
        assert!(is_caller_saved_pinned(REG_X0));
        assert!(is_caller_saved_pinned(REG_X0 + 1));
        assert!(is_caller_saved_pinned(REG_X0 + 2));
        assert!(is_caller_saved_pinned(REG_X0 + 3));
        // X4 is pinned to rbx (callee-saved) — NOT caller-saved pinned
        assert!(!is_caller_saved_pinned(REG_X0 + 4));
        // X5 not pinned
        assert!(!is_caller_saved_pinned(REG_X0 + 5));
    }
}

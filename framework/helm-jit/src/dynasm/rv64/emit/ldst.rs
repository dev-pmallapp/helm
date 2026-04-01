//! Load/store instruction emitters (RV64 -> x86-64).
//!
//! Loads and stores call `extern "C"` helper functions (`jit_mem_read`,
//! `jit_mem_write`) to access guest memory. The helper function addresses
//! are embedded directly as immediates in the generated code (same approach
//! as the AArch64 dynasm backend).
//!
//! ## Calling convention for memory helpers
//!
//! ```text
//! jit_mem_read(mem: *mut u8, addr: u64, size: u32, out: *mut u64) -> u64
//! jit_mem_write(mem: *mut u8, addr: u64, val: u64, size: u32) -> u64
//! ```
//!
//! Both return 0 on success, 1 on fault. On fault, the block exits with
//! `EXIT_EXCEPTION`.

#![allow(missing_docs)]

use crate::block::EXIT_EXCEPTION;
use dynasm::dynasm;
use dynasmrt::{x64::Assembler, DynasmApi, DynasmLabelApi};

/// Byte offset of register `r` in the flat register array.
#[inline]
fn reg_off(r: u8) -> i32 {
    i32::from(r) * 8
}

// ── Memory read helper call ─────────────────────────────────────────────────

/// Emit a call to `jit_mem_read` with the effective address in `r8`.
///
/// After this sequence:
/// - On success: loaded value is in `rax`, rdi/rsi restored.
/// - On fault: jumps to `>fault` label (caller must define it).
///
/// Clobbers: rax, rcx, rdx, r8, r9, r10, r11. Preserves rdi, rsi.
fn emit_mem_read(ops: &mut Assembler, size: u32) {
    dynasm!(ops
        // Save rdi (regs ptr) and rsi (mem ptr) on the stack
        ; push rdi
        ; push rsi
        ; sub rsp, 16              // output buffer + alignment (16 for alignment)

        // jit_mem_read(mem, addr, size, out)
        ; mov rdi, rsi             // arg0: mem pointer (was in rsi)
        ; mov rsi, r8              // arg1: address
        ; mov edx, size as i32     // arg2: size
        ; lea rcx, [rsp]           // arg3: output pointer

        ; mov rax, QWORD crate::helpers::jit_mem_read as *const () as i64
        ; call rax

        // Grab result before restoring stack
        ; mov rcx, [rsp]
        ; add rsp, 16
        ; pop rsi
        ; pop rdi

        // Check for fault
        ; test rax, rax
        ; jnz >fault

        ; mov rax, rcx             // loaded value -> rax
    );
}

/// Emit a call to `jit_mem_write` with addr in `r8` and value in `r9`.
///
/// On fault: jumps to `>fault` label.
fn emit_mem_write(ops: &mut Assembler, size: u32) {
    dynasm!(ops
        ; push rdi
        ; push rsi
        ; sub rsp, 8               // alignment (2 pushes + sub = 24; need 16-aligned)

        // jit_mem_write(mem, addr, val, size)
        ; mov rdi, rsi             // arg0: mem pointer
        ; mov rsi, r8              // arg1: address
        ; mov rdx, r9              // arg2: value
        ; mov ecx, size as i32     // arg3: size

        ; mov rax, QWORD crate::helpers::jit_mem_write as *const () as i64
        ; call rax

        ; add rsp, 8
        ; pop rsi
        ; pop rdi

        ; test rax, rax
        ; jnz >fault
    );
}

// ── Load ────────────────────────────────────────────────────────────────────

/// Emit a load: `rd = mem[rs1 + imm]`.
///
/// - `sign_extend`: true for LB/LH/LW, false for LBU/LHU/LWU/LD
/// - `size`: access width in bytes (1, 2, 4, or 8)
pub fn emit_load(ops: &mut Assembler, rd: u8, rs1: u8, imm: i64, size: u32, sign_extend: bool) {
    let rs1_off = reg_off(rs1);

    // Compute effective address: r8 = rs1 + imm
    dynasm!(ops
        ; mov r8, QWORD [rdi + rs1_off]
        ; mov rcx, QWORD imm
        ; add r8, rcx
    );

    // Call memory read helper (result in rax on success)
    emit_mem_read(ops, size);

    // Sign-extend if needed (for LB/LH/LW)
    if sign_extend && size < 8 {
        match size {
            1 => dynasm!(ops ; movsx rax, al),
            2 => dynasm!(ops ; movsx rax, ax),
            4 => dynasm!(ops ; movsxd rax, eax),
            _ => {}
        }
    }

    // Write to rd (skip if rd == 0: x0 is hardwired zero)
    if rd != 0 {
        let rd_off = reg_off(rd);
        dynasm!(ops ; mov QWORD [rdi + rd_off], rax);
    }

    // Jump over fault handler
    dynasm!(ops
        ; jmp >no_fault
        ; fault:
        ; mov rax, QWORD EXIT_EXCEPTION as i64
        ; ret
        ; no_fault:
    );
}

// ── Store ───────────────────────────────────────────────────────────────────

/// Emit a store: `mem[rs1 + imm] = rs2`.
///
/// - `size`: access width in bytes (1, 2, 4, or 8)
pub fn emit_store(ops: &mut Assembler, rs1: u8, rs2: u8, imm: i64, size: u32) {
    let rs1_off = reg_off(rs1);
    let rs2_off = reg_off(rs2);

    // Compute effective address: r8 = rs1 + imm
    dynasm!(ops
        ; mov r8, QWORD [rdi + rs1_off]
        ; mov rcx, QWORD imm
        ; add r8, rcx
    );

    // Load value to store into r9
    dynasm!(ops ; mov r9, QWORD [rdi + rs2_off]);

    // Call memory write helper
    emit_mem_write(ops, size);

    // Jump over fault handler
    dynasm!(ops
        ; jmp >no_fault
        ; fault:
        ; mov rax, QWORD EXIT_EXCEPTION as i64
        ; ret
        ; no_fault:
    );
}

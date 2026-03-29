//! Load/store instruction emitters (AArch64 → x86-64).
//!
//! Loads and stores call extern "C" helper functions (`jit_mem_read`,
//! `jit_mem_write`) to access guest memory. The helpers receive the FlatMem
//! pointer from `rsi`.
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
#![allow(clippy::similar_names)]

use dynasm::dynasm;
use dynasmrt::{DynasmApi, DynasmLabelApi, x64::Assembler};
use helm_arch::aarch64::insn::{Instruction, Opcode};
use crate::block::EXIT_EXCEPTION;
use crate::regs::{reg_offset, REG_SP, REG_PC, REG_XZR};

/// Source/base register offset — reg 31 = SP in addressing context.
#[inline]
fn base_offset(reg: u32) -> i32 {
    if reg == 31 {
        reg_offset(REG_SP)
    } else {
        reg_offset(reg as usize)
    }
}

/// Data register offset — reg 31 = XZR for load/store data.
#[inline]
fn data_offset(reg: u32) -> i32 {
    if reg == 31 {
        reg_offset(REG_XZR)
    } else {
        reg_offset(reg as usize)
    }
}

/// Access size in bytes from the insn.size field or opcode.
fn access_size(insn: &Instruction) -> u32 {
    match insn.opcode {
        Opcode::Ldrb | Opcode::Strb | Opcode::Ldrsb => 1,
        Opcode::Ldrh | Opcode::Strh | Opcode::Ldrsh => 2,
        Opcode::Ldrsw => 4,
        Opcode::Ldr | Opcode::Str => {
            if insn.sf { 8 } else { 4 }
        }
        _ => match insn.size {
            0 => 1,
            1 => 2,
            2 => 4,
            _ => 8,
        },
    }
}

/// Emit a call to `jit_mem_read` and handle the result.
///
/// Before calling: save rdi/rsi, set up args, call, check return.
/// After: the loaded value is in `[rsp - 8]` (written by the helper via the
/// `out` pointer).
///
/// Clobbers: rax, rcx, rdx, r8, r9. Preserves rdi, rsi.
fn emit_mem_read(ops: &mut Assembler, size: u32) {
    // At this point: r8 = effective address, rdi/rsi = regs/mem.
    // Save rdi/rsi on the stack — r10/r11 are caller-saved and get
    // clobbered by the C helper call.
    dynasm!(ops
        ; push rdi            // save regs ptr
        ; push rsi            // save mem ptr
        ; sub rsp, 16         // output buffer + alignment

        // jit_mem_read(mem, addr, size, out)
        ; mov rdi, rsi        // arg1: mem pointer
        ; mov rsi, r8         // arg2: address
        ; mov edx, size as i32 // arg3: size
        ; lea rcx, [rsp]      // arg4: output pointer

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

        ; mov rax, rcx        // loaded value → rax
    );
}

/// Emit a call to `jit_mem_write`.
///
/// Before calling: r8 = effective address, r9 = value to write.
fn emit_mem_write(ops: &mut Assembler, size: u32) {
    dynasm!(ops
        ; push rdi
        ; push rsi
        ; sub rsp, 8          // alignment (3 pushes = 24; need 32 total)

        // jit_mem_write(mem, addr, val, size)
        ; mov rdi, rsi        // arg1: mem pointer
        ; mov rsi, r8         // arg2: address
        ; mov rdx, r9         // arg3: value
        ; mov ecx, size as i32 // arg4: size

        ; mov rax, QWORD crate::helpers::jit_mem_write as *const () as i64
        ; call rax

        ; add rsp, 8
        ; pop rsi
        ; pop rdi

        ; test rax, rax
        ; jnz >fault
    );
}

/// Emit the fault handler with a jump-over on the normal path.
/// rdi is valid (restored before jnz). Stack is balanced.
fn emit_fault_exit(ops: &mut Assembler, insn: &Instruction) {
    let pc_off = reg_offset(REG_PC);
    dynasm!(ops
        // Normal path jumps over the fault handler
        ; jmp >no_fault
        ; fault:
        ; mov rax, QWORD insn.pc as i64
        ; mov QWORD [rdi + pc_off], rax
        ; mov rax, QWORD EXIT_EXCEPTION as i64
        ; ret
        ; no_fault:
    );
}

// ── LDR immediate ───────────────────────────────────────────────────────────

/// Emit load with immediate offset (unsigned, pre-index, or post-index).
///
/// Handles: LDR, LDRB, LDRH, LDRSB, LDRSH, LDRSW.
pub fn emit_ldr_imm(ops: &mut Assembler, insn: &Instruction) {
    let base_off = base_offset(insn.rn);
    let rd_off = data_offset(insn.rd);
    let size = access_size(insn);
    let imm = insn.imm;
    let is_signed = matches!(insn.opcode, Opcode::Ldrsb | Opcode::Ldrsh | Opcode::Ldrsw);

    // Compute effective address
    if insn.pre_index {
        // Pre-index: base += offset, then load from new base
        dynasm!(ops
            ; mov r8, QWORD [rdi + base_off]
            ; mov rcx, QWORD imm as i64
            ; add r8, rcx
            // Write back updated base
            ; mov QWORD [rdi + base_off], r8
        );
    } else if insn.post_index {
        // Post-index: load from base, then base += offset
        dynasm!(ops
            ; mov r8, QWORD [rdi + base_off]
        );
    } else {
        // Unsigned offset (no writeback)
        dynasm!(ops
            ; mov r8, QWORD [rdi + base_off]
            ; mov rcx, QWORD imm as i64
            ; add r8, rcx
        );
    }

    // Call memory read helper
    emit_mem_read(ops, size);

    // Sign-extend if needed
    if is_signed {
        match size {
            1 => dynasm!(ops ; movsx rax, al),
            2 => dynasm!(ops ; movsx rax, ax),
            4 => dynasm!(ops ; movsxd rax, eax),
            _ => {} // 8-byte: no extension needed
        }
    }

    // Store result to destination register
    if insn.sf || is_signed {
        dynasm!(ops ; mov QWORD [rdi + rd_off], rax);
    } else {
        // 32-bit non-signed: zero-extend
        dynasm!(ops
            ; mov DWORD [rdi + rd_off], eax
            ; mov DWORD [rdi + rd_off + 4], 0
        );
    }

    // Post-index writeback
    if insn.post_index {
        dynasm!(ops
            ; mov r8, QWORD [rdi + base_off]
            ; mov rcx, QWORD imm as i64
            ; add r8, rcx
            ; mov QWORD [rdi + base_off], r8
        );
    }

    // Fault handler
    emit_fault_exit(ops, insn);
}

// ── STR immediate ───────────────────────────────────────────────────────────

/// Emit store with immediate offset.
///
/// Handles: STR, STRB, STRH.
pub fn emit_str_imm(ops: &mut Assembler, insn: &Instruction) {
    let base_off = base_offset(insn.rn);
    let rd_off = data_offset(insn.rd);
    let size = access_size(insn);
    let imm = insn.imm;

    // Compute effective address
    if insn.pre_index {
        dynasm!(ops
            ; mov r8, QWORD [rdi + base_off]
            ; mov rcx, QWORD imm as i64
            ; add r8, rcx
            ; mov QWORD [rdi + base_off], r8
        );
    } else if insn.post_index {
        dynasm!(ops
            ; mov r8, QWORD [rdi + base_off]
        );
    } else {
        dynasm!(ops
            ; mov r8, QWORD [rdi + base_off]
            ; mov rcx, QWORD imm as i64
            ; add r8, rcx
        );
    }

    // Load value to store
    dynasm!(ops
        ; mov r9, QWORD [rdi + rd_off]
    );

    // Call memory write helper
    emit_mem_write(ops, size);

    // Post-index writeback
    if insn.post_index {
        dynasm!(ops
            ; mov r8, QWORD [rdi + base_off]
            ; mov rcx, QWORD imm as i64
            ; add r8, rcx
            ; mov QWORD [rdi + base_off], r8
        );
    }

    // Fault handler
    emit_fault_exit(ops, insn);
}

// ── LDP (load pair) ────────────────────────────────────────────────────────

/// Emit `LDP Xt1, Xt2, [Xn, #imm]`.
pub fn emit_ldp(ops: &mut Assembler, insn: &Instruction) {
    let base_off = base_offset(insn.rn);
    let rt1_off = data_offset(insn.rd);
    let rt2_off = data_offset(insn.pair_second);
    let size: u32 = if insn.sf { 8 } else { 4 };
    let imm = insn.imm;

    // Compute base address (with pre-index if applicable)
    if insn.pre_index {
        dynasm!(ops
            ; mov r8, QWORD [rdi + base_off]
            ; mov rcx, QWORD imm as i64
            ; add r8, rcx
            ; mov QWORD [rdi + base_off], r8
        );
    } else if insn.post_index {
        dynasm!(ops
            ; mov r8, QWORD [rdi + base_off]
        );
    } else {
        dynasm!(ops
            ; mov r8, QWORD [rdi + base_off]
            ; mov rcx, QWORD imm as i64
            ; add r8, rcx
        );
    }

    // First load: [addr]
    // Stash base addr in reserved flat-array slot 38 (avoids push/stack misalignment).
    let stash_off = crate::regs::reg_offset(38);
    dynasm!(ops
        ; mov QWORD [rdi + stash_off], r8
    );
    emit_mem_read(ops, size);
    if insn.sf {
        dynasm!(ops ; mov QWORD [rdi + rt1_off], rax);
    } else {
        dynasm!(ops
            ; mov DWORD [rdi + rt1_off], eax
            ; mov DWORD [rdi + rt1_off + 4], 0
        );
    }

    // Second load: [addr + size]
    dynasm!(ops
        ; mov r8, QWORD [rdi + stash_off]
        ; add r8, size as i32
    );
    emit_mem_read(ops, size);
    if insn.sf {
        dynasm!(ops ; mov QWORD [rdi + rt2_off], rax);
    } else {
        dynasm!(ops
            ; mov DWORD [rdi + rt2_off], eax
            ; mov DWORD [rdi + rt2_off + 4], 0
        );
    }

    // Post-index writeback
    if insn.post_index {
        dynasm!(ops
            ; mov r8, QWORD [rdi + base_off]
            ; mov rcx, QWORD imm as i64
            ; add r8, rcx
            ; mov QWORD [rdi + base_off], r8
        );
    }

    emit_fault_exit(ops, insn);
}

// ── STP (store pair) ────────────────────────────────────────────────────────

/// Emit `STP Xt1, Xt2, [Xn, #imm]`.
pub fn emit_stp(ops: &mut Assembler, insn: &Instruction) {
    let base_off = base_offset(insn.rn);
    let rt1_off = data_offset(insn.rd);
    let rt2_off = data_offset(insn.pair_second);
    let size: u32 = if insn.sf { 8 } else { 4 };
    let imm = insn.imm;

    // Compute base address
    if insn.pre_index {
        dynasm!(ops
            ; mov r8, QWORD [rdi + base_off]
            ; mov rcx, QWORD imm as i64
            ; add r8, rcx
            ; mov QWORD [rdi + base_off], r8
        );
    } else if insn.post_index {
        dynasm!(ops
            ; mov r8, QWORD [rdi + base_off]
        );
    } else {
        dynasm!(ops
            ; mov r8, QWORD [rdi + base_off]
            ; mov rcx, QWORD imm as i64
            ; add r8, rcx
        );
    }

    // First store — stash base addr in reserved slot 38
    let stash_off = crate::regs::reg_offset(38);
    dynasm!(ops
        ; mov QWORD [rdi + stash_off], r8
        ; mov r9, QWORD [rdi + rt1_off]
    );
    emit_mem_write(ops, size);

    // Second store at [addr + size]
    dynasm!(ops
        ; mov r8, QWORD [rdi + stash_off]
        ; add r8, size as i32
        ; mov r9, QWORD [rdi + rt2_off]
    );
    emit_mem_write(ops, size);

    // Post-index writeback
    if insn.post_index {
        dynasm!(ops
            ; mov r8, QWORD [rdi + base_off]
            ; mov rcx, QWORD imm as i64
            ; add r8, rcx
            ; mov QWORD [rdi + base_off], r8
        );
    }

    emit_fault_exit(ops, insn);
}

//! System instruction emitters.
//!
//! Most system instructions (SVC, MSR, ERET, WFI, etc.) interact with
//! privileged state and return `None` to force interpreter fallback.
//!
//! Exceptions compiled inline:
//! - **NOP**: no-op.
//! - **MRS DCZID_EL0**: constant load (64-byte block, not prohibited).
//! - **DC ZVA**: 8 sequential 8-byte zero-stores via the memory write helper.

#![allow(missing_docs)]

use crate::dynasm::pinned::{load_guest_to_rax, store_rax_to_guest};
use crate::regs::{reg_offset, REG_JIT_MEM_WRITE, REG_XZR};
use dynasm::dynasm;
use dynasmrt::x64::Assembler;
use dynasmrt::{DynasmApi, DynasmLabelApi};
use helm_arch::aarch64::insn::{Instruction, Opcode};

/// DCZID_EL0 sysreg encoding: op0=3, op1=3, CRn=0, CRm=0, op2=7
const SYSREG_DCZID_EL0: i64 = 0b11_011_0000_0000_111;

/// DCZID_EL0 value: BS=4 (2^(4+2)=64 byte block), DZP=0 (not prohibited).
const DCZID_VALUE: u64 = 0x0000_0004;

/// DC ZVA block size in bytes (must match DCZID_VALUE).
const DC_ZVA_BLOCK_SIZE: u64 = 64;

/// Emit MRS Xd, DCZID_EL0 as a constant load.
fn emit_mrs_dczid(ops: &mut Assembler, insn: &Instruction) -> Option<bool> {
    let rd_slot = if insn.rd == 31 { REG_XZR } else { insn.rd as usize };
    dynasm!(ops ; mov rax, QWORD DCZID_VALUE as i64);
    store_rax_to_guest(ops, rd_slot);
    Some(false)
}

/// Emit DC ZVA Xt: zero 64 bytes at the 64-byte-aligned address in Xt.
///
/// Emits 8 sequential 8-byte zero-writes through the runtime memory-write
/// helper (slot REG_JIT_MEM_WRITE in the flat register array), so this
/// works in both SE and FS modes.
fn emit_dc_zva(ops: &mut Assembler, insn: &Instruction) -> Option<bool> {
    let rd_slot = if insn.rd == 31 { REG_XZR } else { insn.rd as usize };
    let write_off = reg_offset(REG_JIT_MEM_WRITE);
    let stash_off = reg_offset(38); // scratch slot shared with ldst.rs

    // Load guest VA into rax, align to 64-byte boundary, stash in slot 38.
    load_guest_to_rax(ops, rd_slot);
    dynasm!(ops
        ; and rax, !(DC_ZVA_BLOCK_SIZE - 1) as i32
        ; mov QWORD [rdi + stash_off], rax
    );

    // 8 sequential 8-byte zero-writes.
    for i in 0..8 {
        let byte_offset = (i * 8) as i32;

        dynasm!(ops
            ; push rdi
            ; push rsi
            ; push r8
            ; push r9
            ; push r10
            ; push r11
            ; sub rsp, 8  // alignment: 6 pushes (48) + 8 = 56 => RSP 0 mod 16
        );
        dynasm!(ops
            // mem_write(ctx, addr, val, size) -> u64
            ; mov rax, QWORD [rdi + stash_off]
        );
        if byte_offset != 0 {
            dynasm!(ops ; add rax, byte_offset);
        }
        dynasm!(ops
            ; mov rsi, rax           // arg2: address
            ; mov rdi, [rsp + 40]    // arg1: mem/ctx ptr (original rsi)
            ; xor edx, edx           // arg3: value = 0
            ; mov ecx, 8i32          // arg4: size = 8

            ; mov rax, [rsp + 48]    // original rdi (flat regs)
            ; mov rax, QWORD [rax + write_off]
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

    // All writes succeeded -- skip fault exit.
    dynasm!(ops ; jmp >done);

    // Fault path: exit block with EXIT_EXCEPTION.
    dynasm!(ops
        ; fault:
    );
    let pc_off = reg_offset(crate::regs::REG_PC);
    let pc_val = insn.pc as i64;
    if i32::try_from(pc_val).is_ok() {
        dynasm!(ops ; mov QWORD [rdi + pc_off], pc_val as i32);
    } else {
        dynasm!(ops ; mov rax, QWORD pc_val ; mov QWORD [rdi + pc_off], rax);
    }
    crate::dynasm::pinned::emit_pinned_epilogue(ops);
    dynasm!(ops
        ; mov eax, crate::block::EXIT_EXCEPTION as i32
        ; ret
    );

    dynasm!(ops ; done:);
    Some(false)
}

/// Attempt to emit a system instruction.
pub fn emit_system(ops: &mut Assembler, insn: &Instruction) -> Option<bool> {
    match insn.opcode {
        Opcode::Nop => Some(false),
        Opcode::Mrs if insn.imm == SYSREG_DCZID_EL0 => emit_mrs_dczid(ops, insn),
        Opcode::DcZva => emit_dc_zva(ops, insn),
        _ => None,
    }
}

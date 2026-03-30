//! Data-processing instruction emitters (AArch64 → x86-64).
//!
//! All functions take `(ops: &mut Assembler, insn: &Instruction)` and emit
//! x86-64 code that operates on the flat register array pointed to by `rdi`.
//!
//! ## Register usage convention
//! - `rdi` — pointer to flat register array (preserved across the block)
//! - `rsi` — pointer to FlatMem (preserved across the block)
//! - `rax`, `rcx`, `rdx`, `r8` — scratch (freely clobbered)

#![allow(missing_docs)]
#![allow(clippy::similar_names)]

use crate::regs::{reg_offset, REG_NZCV, REG_SP, REG_XZR};
use dynasm::dynasm;
use dynasmrt::{x64::Assembler, DynasmApi};
use helm_arch::aarch64::insn::{Instruction, Opcode};

/// Read a guest register into an x86 scratch register.
/// Handles the SP/XZR distinction: for data-processing, reg 31 = XZR (zero).
/// For address-context instructions, reg 31 = SP (caller should use `load_reg_sp`).
#[inline]
fn src_offset(reg: u32) -> i32 {
    if reg == 31 {
        reg_offset(REG_XZR)
    } else {
        reg_offset(reg as usize)
    }
}

/// Destination offset — same as src_offset for data processing (rd=31 → XZR).
#[inline]
fn dst_offset(reg: u32) -> i32 {
    if reg == 31 {
        reg_offset(REG_XZR)
    } else {
        reg_offset(reg as usize)
    }
}

/// Destination offset in SP context (rd=31 → SP, used by ADD/SUB immediate
/// when they target the stack pointer).
#[inline]
fn dst_offset_sp(reg: u32) -> i32 {
    if reg == 31 {
        reg_offset(REG_SP)
    } else {
        reg_offset(reg as usize)
    }
}

/// Source offset in SP context (rn=31 → SP).
#[inline]
fn src_offset_sp(reg: u32) -> i32 {
    if reg == 31 {
        reg_offset(REG_SP)
    } else {
        reg_offset(reg as usize)
    }
}

// ── ADD / SUB immediate (non-flag-setting) ──────────────────────────────────

/// Emit `ADD/SUB Xd|SP, Xn|SP, #imm{, shift}`.
///
/// Non-flag-setting: rd and rn use SP encoding (reg 31 = SP).
pub fn emit_add_sub_imm(ops: &mut Assembler, insn: &Instruction) {
    let is_sub = insn.opcode == Opcode::SubImm;
    let rn_off = src_offset_sp(insn.rn);
    let rd_off = dst_offset_sp(insn.rd);
    // imm is already shifted by the decoder (shift 0 or 12)
    let imm = insn.imm;

    if insn.sf {
        // 64-bit
        dynasm!(ops
            ; mov rax, QWORD [rdi + rn_off]
        );
        if is_sub {
            dynasm!(ops
                ; mov rcx, QWORD imm as i64
                ; sub rax, rcx
            );
        } else {
            dynasm!(ops
                ; mov rcx, QWORD imm as i64
                ; add rax, rcx
            );
        }
        dynasm!(ops
            ; mov QWORD [rdi + rd_off], rax
        );
    } else {
        // 32-bit: zero-extend result to 64 bits
        dynasm!(ops
            ; mov eax, DWORD [rdi + rn_off]
        );
        if is_sub {
            dynasm!(ops
                ; sub eax, imm as i32
            );
        } else {
            dynasm!(ops
                ; add eax, imm as i32
            );
        }
        // Writing to eax automatically zero-extends to rax on x86-64
        dynasm!(ops
            ; mov DWORD [rdi + rd_off], eax
            ; mov DWORD [rdi + rd_off + 4], 0
        );
    }
}

// ── ADDS / SUBS immediate (flag-setting) ────────────────────────────────────

/// Emit `ADDS/SUBS Xd, Xn, #imm{, shift}`.
///
/// Flag-setting: rd uses XZR encoding (reg 31 = XZR), rn uses XZR encoding
/// (matching interpreter behavior: `read_x` not `read_xsp`).
pub fn emit_adds_subs_imm(ops: &mut Assembler, insn: &Instruction) {
    let is_sub = insn.opcode == Opcode::SubsImm;
    let rn_off = src_offset(insn.rn);
    let rd_off = dst_offset(insn.rd);
    let nzcv_off = reg_offset(REG_NZCV);
    let imm = insn.imm;

    if insn.sf {
        // 64-bit flag-setting
        dynasm!(ops
            ; mov rax, QWORD [rdi + rn_off]
        );
        if is_sub {
            dynasm!(ops
                ; mov rcx, QWORD imm as i64
                ; sub rax, rcx
            );
        } else {
            dynasm!(ops
                ; mov rcx, QWORD imm as i64
                ; add rax, rcx
            );
        }
        // Capture x86 flags → ARM NZCV
        // N = SF (sign flag), Z = ZF, C = CF (inverted for SUB on ARM vs x86),
        // V = OF
        emit_capture_nzcv_64(ops, is_sub, nzcv_off);
        dynasm!(ops
            ; mov QWORD [rdi + rd_off], rax
        );
    } else {
        // 32-bit flag-setting
        dynasm!(ops
            ; mov eax, DWORD [rdi + rn_off]
        );
        if is_sub {
            dynasm!(ops
                ; sub eax, imm as i32
            );
        } else {
            dynasm!(ops
                ; add eax, imm as i32
            );
        }
        emit_capture_nzcv_32(ops, is_sub, nzcv_off);
        dynasm!(ops
            ; mov DWORD [rdi + rd_off], eax
            ; mov DWORD [rdi + rd_off + 4], 0
        );
    }
}

// ── ADD / SUB register (non-flag-setting) ───────────────────────────────────

/// Emit `ADD/SUB Xd, Xn, Xm{, shift #amt}`.
pub fn emit_add_sub_reg(ops: &mut Assembler, insn: &Instruction) {
    let is_sub = insn.opcode == Opcode::SubReg;
    let rn_off = src_offset_sp(insn.rn);
    let rm_off = src_offset(insn.rm);
    let rd_off = dst_offset_sp(insn.rd);

    if insn.sf {
        // Load Rn into rax, Rm into rcx
        dynasm!(ops
            ; mov rax, QWORD [rdi + rn_off]
            ; mov rcx, QWORD [rdi + rm_off]
        );
        emit_apply_shift_64(ops, insn.shift_type, insn.shift_amt);
        if is_sub {
            dynasm!(ops ; sub rax, rcx);
        } else {
            dynasm!(ops ; add rax, rcx);
        }
        dynasm!(ops ; mov QWORD [rdi + rd_off], rax);
    } else {
        dynasm!(ops
            ; mov eax, DWORD [rdi + rn_off]
            ; mov ecx, DWORD [rdi + rm_off]
        );
        emit_apply_shift_32(ops, insn.shift_type, insn.shift_amt);
        if is_sub {
            dynasm!(ops ; sub eax, ecx);
        } else {
            dynasm!(ops ; add eax, ecx);
        }
        dynasm!(ops
            ; mov DWORD [rdi + rd_off], eax
            ; mov DWORD [rdi + rd_off + 4], 0
        );
    }
}

// ── ADDS / SUBS register (flag-setting) ─────────────────────────────────────

/// Emit `ADDS/SUBS Xd, Xn, Xm{, shift #amt}`.
pub fn emit_adds_subs_reg(ops: &mut Assembler, insn: &Instruction) {
    let is_sub = insn.opcode == Opcode::SubsReg;
    let rn_off = src_offset(insn.rn);
    let rm_off = src_offset(insn.rm);
    let rd_off = dst_offset(insn.rd);
    let nzcv_off = reg_offset(REG_NZCV);

    if insn.sf {
        dynasm!(ops
            ; mov rax, QWORD [rdi + rn_off]
            ; mov rcx, QWORD [rdi + rm_off]
        );
        emit_apply_shift_64(ops, insn.shift_type, insn.shift_amt);
        if is_sub {
            dynasm!(ops ; sub rax, rcx);
        } else {
            dynasm!(ops ; add rax, rcx);
        }
        emit_capture_nzcv_64(ops, is_sub, nzcv_off);
        dynasm!(ops ; mov QWORD [rdi + rd_off], rax);
    } else {
        dynasm!(ops
            ; mov eax, DWORD [rdi + rn_off]
            ; mov ecx, DWORD [rdi + rm_off]
        );
        emit_apply_shift_32(ops, insn.shift_type, insn.shift_amt);
        if is_sub {
            dynasm!(ops ; sub eax, ecx);
        } else {
            dynasm!(ops ; add eax, ecx);
        }
        emit_capture_nzcv_32(ops, is_sub, nzcv_off);
        dynasm!(ops
            ; mov DWORD [rdi + rd_off], eax
            ; mov DWORD [rdi + rd_off + 4], 0
        );
    }
}

// ── AND / ORR / EOR immediate (non-flag-setting) ────────────────────────────

/// Emit `AND/ORR/EOR Xd|SP, Xn, #imm`.
///
/// Non-flag-setting logical: rd uses SP encoding (reg 31 = SP).
pub fn emit_logical_imm(ops: &mut Assembler, insn: &Instruction) {
    let rn_off = src_offset(insn.rn);
    let rd_off = dst_offset_sp(insn.rd);
    let imm = insn.imm;

    if insn.sf {
        dynasm!(ops
            ; mov rax, QWORD [rdi + rn_off]
            ; mov rcx, QWORD imm as i64
        );
        match insn.opcode {
            Opcode::AndImm => dynasm!(ops ; and rax, rcx),
            Opcode::OrrImm => dynasm!(ops ; or rax, rcx),
            Opcode::EorImm => dynasm!(ops ; xor rax, rcx),
            _ => unreachable!(),
        }
        dynasm!(ops ; mov QWORD [rdi + rd_off], rax);
    } else {
        dynasm!(ops
            ; mov eax, DWORD [rdi + rn_off]
        );
        match insn.opcode {
            Opcode::AndImm => dynasm!(ops ; and eax, imm as i32),
            Opcode::OrrImm => dynasm!(ops ; or eax, imm as i32),
            Opcode::EorImm => dynasm!(ops ; xor eax, imm as i32),
            _ => unreachable!(),
        }
        dynasm!(ops
            ; mov DWORD [rdi + rd_off], eax
            ; mov DWORD [rdi + rd_off + 4], 0
        );
    }
}

// ── ANDS immediate (flag-setting) ───────────────────────────────────────────

/// Emit `ANDS Xd, Xn, #imm` — flag-setting AND.
pub fn emit_ands_imm(ops: &mut Assembler, insn: &Instruction) {
    let rn_off = src_offset(insn.rn);
    let rd_off = dst_offset(insn.rd);
    let nzcv_off = reg_offset(REG_NZCV);
    let imm = insn.imm;

    if insn.sf {
        dynasm!(ops
            ; mov rax, QWORD [rdi + rn_off]
            ; mov rcx, QWORD imm as i64
            ; and rax, rcx
        );
        // ANDS: C=0, V=0, N/Z from result
        emit_capture_nzcv_logical_64(ops, nzcv_off);
        dynasm!(ops ; mov QWORD [rdi + rd_off], rax);
    } else {
        dynasm!(ops
            ; mov eax, DWORD [rdi + rn_off]
            ; and eax, imm as i32
        );
        emit_capture_nzcv_logical_32(ops, nzcv_off);
        dynasm!(ops
            ; mov DWORD [rdi + rd_off], eax
            ; mov DWORD [rdi + rd_off + 4], 0
        );
    }
}

// ── MOV immediate variants ──────────────────────────────────────────────────
//
// The decoder pre-computes the final value:
//   Movz: insn.imm = imm16 << (hw * 16)          — already shifted
//   Movn: insn.imm = !(imm16 << (hw * 16))       — already inverted+shifted
//   Movk: insn.imm = raw imm16, insn.imm2 = hw   — executor applies shift
//
// Movz and Movn emitters just write insn.imm (with 32-bit masking if !sf).

/// Emit `MOVZ Xd, #imm{, LSL #shift}`.
pub fn emit_movz(ops: &mut Assembler, insn: &Instruction) {
    let rd_off = dst_offset(insn.rd);
    let val = if insn.sf {
        insn.imm as u64
    } else {
        insn.imm as u64 & 0xFFFF_FFFF
    };

    dynasm!(ops
        ; mov rax, QWORD val as i64
        ; mov QWORD [rdi + rd_off], rax
    );
}

/// Emit `MOVK Xd, #imm{, LSL #shift}` — keep other bits.
pub fn emit_movk(ops: &mut Assembler, insn: &Instruction) {
    let rd_off = dst_offset(insn.rd);
    let shift = insn.imm2 * 16;
    let mask = !(0xFFFF_u64 << shift);
    let bits = (insn.imm as u64 & 0xFFFF) << shift;

    dynasm!(ops
        ; mov rax, QWORD [rdi + rd_off]
        ; mov rcx, QWORD mask as i64
        ; and rax, rcx
        ; mov rcx, QWORD bits as i64
        ; or rax, rcx
        ; mov QWORD [rdi + rd_off], rax
    );
}

/// Emit `MOVN Xd, #imm{, LSL #shift}` — value already inverted by decoder.
pub fn emit_movn(ops: &mut Assembler, insn: &Instruction) {
    let rd_off = dst_offset(insn.rd);
    let val = if insn.sf {
        insn.imm as u64
    } else {
        insn.imm as u64 & 0xFFFF_FFFF
    };

    dynasm!(ops
        ; mov rax, QWORD val as i64
        ; mov QWORD [rdi + rd_off], rax
    );
}

// ── Shift helpers ───────────────────────────────────────────────────────────

/// Apply shift to `rcx` (64-bit): shift_type 0=LSL, 1=LSR, 2=ASR, 3=ROR.
fn emit_apply_shift_64(ops: &mut Assembler, shift_type: u32, shift_amt: u32) {
    if shift_amt == 0 {
        return;
    }
    let amt = shift_amt as i8;
    match shift_type {
        0 => dynasm!(ops ; shl rcx, amt),
        1 => dynasm!(ops ; shr rcx, amt),
        2 => dynasm!(ops ; sar rcx, amt),
        3 => dynasm!(ops ; ror rcx, amt),
        _ => unreachable!(),
    }
}

/// Apply shift to `ecx` (32-bit).
fn emit_apply_shift_32(ops: &mut Assembler, shift_type: u32, shift_amt: u32) {
    if shift_amt == 0 {
        return;
    }
    let amt = shift_amt as i8;
    match shift_type {
        0 => dynasm!(ops ; shl ecx, amt),
        1 => dynasm!(ops ; shr ecx, amt),
        2 => dynasm!(ops ; sar ecx, amt),
        3 => dynasm!(ops ; ror ecx, amt),
        _ => unreachable!(),
    }
}

// ── NZCV capture helpers ────────────────────────────────────────────────────
//
// ARM NZCV packing: N=bit31, Z=bit30, C=bit29, V=bit28.
//
// For ADD: x86 CF = ARM C (both set on unsigned overflow).
// For SUB: x86 CF is the INVERSE of ARM C (x86 sets CF on borrow, ARM clears
//          C on borrow).
//
// Strategy: use `setCC` byte instructions to capture all four flags immediately
// after the arithmetic op. `setCC` does NOT modify FLAGS or clobber rax, so
// the result register is preserved. Then assemble the NZCV word from the
// captured bytes.

/// Capture NZCV after a 64-bit ADD/SUB. Result in `rax` is **preserved**.
/// `is_sub`: if true, invert the carry flag.
fn emit_capture_nzcv_64(ops: &mut Assembler, is_sub: bool, nzcv_off: i32) {
    // Immediately capture all four flags via setCC (none modify FLAGS or rax).
    dynasm!(ops
        ; sets  cl          // cl = SF → N
        ; setz  dl          // dl = ZF → Z
        ; seto  r8b         // r8b = OF → V
    );
    if is_sub {
        // ARM C = !x86_CF for subtraction (ARM C=1 means no borrow)
        dynasm!(ops ; setnc r9b); // r9b = !CF → ARM C
    } else {
        dynasm!(ops ; setc  r9b); // r9b = CF → ARM C
    }
    // Assemble NZCV from captured bytes (these ops clobber FLAGS but we're done)
    dynasm!(ops
        ; movzx r10d, cl         // N
        ; shl   r10d, 31
        ; movzx ecx, dl          // Z (reuse ecx now)
        ; shl   ecx, 30
        ; or    r10d, ecx
        ; movzx ecx, r9b         // C
        ; shl   ecx, 29
        ; or    r10d, ecx
        ; movzx ecx, r8b         // V
        ; shl   ecx, 28
        ; or    r10d, ecx
        ; mov   DWORD [rdi + nzcv_off], r10d
    );
}

/// Capture NZCV after a 32-bit ADD/SUB. Result in `eax` is **preserved**.
fn emit_capture_nzcv_32(ops: &mut Assembler, is_sub: bool, nzcv_off: i32) {
    // x86 32-bit ops set FLAGS identically; same capture logic applies.
    emit_capture_nzcv_64(ops, is_sub, nzcv_off);
}

/// Capture NZCV for logical operations (64-bit). C=0, V=0, N/Z from result.
/// Result in `rax` is **preserved**.
fn emit_capture_nzcv_logical_64(ops: &mut Assembler, nzcv_off: i32) {
    // Logical ops (AND/ORR/EOR) set SF and ZF but clear CF and OF on x86.
    // ARM semantics: C=0, V=0, N and Z from result. Capture with setCC:
    dynasm!(ops
        ; sets  cl               // cl = SF → N
        ; setz  dl               // dl = ZF → Z
        ; movzx r10d, cl
        ; shl   r10d, 31
        ; movzx ecx, dl
        ; shl   ecx, 30
        ; or    r10d, ecx
        ; mov   DWORD [rdi + nzcv_off], r10d
    );
}

/// Capture NZCV for logical operations (32-bit). C=0, V=0.
fn emit_capture_nzcv_logical_32(ops: &mut Assembler, nzcv_off: i32) {
    emit_capture_nzcv_logical_64(ops, nzcv_off);
}

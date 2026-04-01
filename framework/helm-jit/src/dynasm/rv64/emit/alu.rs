//! ALU instruction emitters (RV64 -> x86-64).
//!
//! All ALU emitters follow the same pattern:
//! 1. Read source register(s) from `[rdi + rs*8]` into x86-64 scratch registers
//! 2. Perform the operation
//! 3. Write result to `[rdi + rd*8]` (unless rd == 0, since x0 is hardwired zero)
//!
//! Word ops (ADDIW/ADDW/SUBW/SLLIW/SRLIW/SRAIW/SLLW/SRLW/SRAW) operate on
//! the low 32 bits and sign-extend the result to 64 bits.

#![allow(missing_docs)]
#![allow(clippy::similar_names)]

use dynasm::dynasm;
use dynasmrt::{x64::Assembler, DynasmApi, DynasmLabelApi};

/// ALU operation type for shared emitter logic.
#[derive(Clone, Copy)]
pub enum AluOp {
    Add,
    Sub,
    Xor,
    Or,
    And,
}

/// Shift operation type.
#[derive(Clone, Copy)]
pub enum ShiftOp {
    Sll,
    Srl,
    Sra,
}

/// Byte offset of register `r` in the flat register array.
#[inline]
fn reg_off(r: u8) -> i32 {
    i32::from(r) * 8
}

/// Returns true if writes to `rd` should be skipped (x0 is hardwired zero).
#[inline]
fn skip_rd_zero(rd: u8) -> bool {
    rd == 0
}

// ── Register-immediate ALU (64-bit) ─────────────────────────────────────────

/// Emit an ALU-immediate instruction (ADDI, XORI, ORI, ANDI).
///
/// `rd = rs1 <op> imm`  (64-bit)
pub fn emit_alu_imm(ops: &mut Assembler, rd: u8, rs1: u8, imm: i64, op: AluOp) {
    if skip_rd_zero(rd) {
        return;
    }
    let rs1_off = reg_off(rs1);
    let rd_off = reg_off(rd);
    dynasm!(ops ; mov rax, QWORD [rdi + rs1_off]);
    match op {
        AluOp::Add => dynasm!(ops ; mov rcx, QWORD imm ; add rax, rcx),
        AluOp::Sub => dynasm!(ops ; mov rcx, QWORD imm ; sub rax, rcx),
        AluOp::Xor => dynasm!(ops ; mov rcx, QWORD imm ; xor rax, rcx),
        AluOp::Or => dynasm!(ops ; mov rcx, QWORD imm ; or rax, rcx),
        AluOp::And => dynasm!(ops ; mov rcx, QWORD imm ; and rax, rcx),
    }
    dynasm!(ops ; mov QWORD [rdi + rd_off], rax);
}

// ── Register-immediate ALU (32-bit word ops) ────────────────────────────────

/// Emit a 32-bit ALU-immediate instruction (ADDIW).
///
/// `rd = sign_extend_32(rs1[31:0] <op> imm[31:0])`
pub fn emit_alu_imm_w(ops: &mut Assembler, rd: u8, rs1: u8, imm: i64, op: AluOp) {
    if skip_rd_zero(rd) {
        return;
    }
    let rs1_off = reg_off(rs1);
    let rd_off = reg_off(rd);
    dynasm!(ops ; mov eax, DWORD [rdi + rs1_off]);
    match op {
        AluOp::Add => dynasm!(ops ; add eax, imm as i32),
        AluOp::Sub => dynasm!(ops ; sub eax, imm as i32),
        _ => dynasm!(ops ; add eax, imm as i32), // Only ADDIW uses this path
    }
    // Sign-extend 32 -> 64
    dynasm!(ops ; movsxd rax, eax ; mov QWORD [rdi + rd_off], rax);
}

// ── Register-register ALU (64-bit) ──────────────────────────────────────────

/// Emit an ALU register-register instruction (ADD, SUB, XOR, OR, AND).
///
/// `rd = rs1 <op> rs2`  (64-bit)
pub fn emit_alu_reg(ops: &mut Assembler, rd: u8, rs1: u8, rs2: u8, op: AluOp) {
    if skip_rd_zero(rd) {
        return;
    }
    let rs1_off = reg_off(rs1);
    let rs2_off = reg_off(rs2);
    let rd_off = reg_off(rd);
    dynasm!(ops
        ; mov rax, QWORD [rdi + rs1_off]
        ; mov rcx, QWORD [rdi + rs2_off]
    );
    match op {
        AluOp::Add => dynasm!(ops ; add rax, rcx),
        AluOp::Sub => dynasm!(ops ; sub rax, rcx),
        AluOp::Xor => dynasm!(ops ; xor rax, rcx),
        AluOp::Or => dynasm!(ops ; or rax, rcx),
        AluOp::And => dynasm!(ops ; and rax, rcx),
    }
    dynasm!(ops ; mov QWORD [rdi + rd_off], rax);
}

// ── Register-register ALU (32-bit word ops) ─────────────────────────────────

/// Emit a 32-bit ALU register-register instruction (ADDW, SUBW).
///
/// `rd = sign_extend_32(rs1[31:0] <op> rs2[31:0])`
pub fn emit_alu_reg_w(ops: &mut Assembler, rd: u8, rs1: u8, rs2: u8, op: AluOp) {
    if skip_rd_zero(rd) {
        return;
    }
    let rs1_off = reg_off(rs1);
    let rs2_off = reg_off(rs2);
    let rd_off = reg_off(rd);
    dynasm!(ops
        ; mov eax, DWORD [rdi + rs1_off]
        ; mov ecx, DWORD [rdi + rs2_off]
    );
    match op {
        AluOp::Add => dynasm!(ops ; add eax, ecx),
        AluOp::Sub => dynasm!(ops ; sub eax, ecx),
        _ => dynasm!(ops ; add eax, ecx),
    }
    dynasm!(ops ; movsxd rax, eax ; mov QWORD [rdi + rd_off], rax);
}

// ── Shift immediate (64-bit) ────────────────────────────────────────────────

/// Emit a shift-immediate instruction (SLLI, SRLI, SRAI).
///
/// `rd = rs1 << shamt` / `rd = rs1 >> shamt` / `rd = rs1 >>> shamt`
pub fn emit_shift_imm(ops: &mut Assembler, rd: u8, rs1: u8, shamt: u8, op: ShiftOp) {
    if skip_rd_zero(rd) {
        return;
    }
    let rs1_off = reg_off(rs1);
    let rd_off = reg_off(rd);
    dynasm!(ops ; mov rax, QWORD [rdi + rs1_off]);
    match op {
        ShiftOp::Sll => dynasm!(ops ; shl rax, shamt as i8),
        ShiftOp::Srl => dynasm!(ops ; shr rax, shamt as i8),
        ShiftOp::Sra => dynasm!(ops ; sar rax, shamt as i8),
    }
    dynasm!(ops ; mov QWORD [rdi + rd_off], rax);
}

// ── Shift immediate (32-bit word ops) ───────────────────────────────────────

/// Emit a 32-bit shift-immediate instruction (SLLIW, SRLIW, SRAIW).
///
/// Operates on the low 32 bits; result is sign-extended to 64 bits.
pub fn emit_shift_imm_w(ops: &mut Assembler, rd: u8, rs1: u8, shamt: u8, op: ShiftOp) {
    if skip_rd_zero(rd) {
        return;
    }
    let rs1_off = reg_off(rs1);
    let rd_off = reg_off(rd);
    dynasm!(ops ; mov eax, DWORD [rdi + rs1_off]);
    match op {
        ShiftOp::Sll => dynasm!(ops ; shl eax, shamt as i8),
        ShiftOp::Srl => dynasm!(ops ; shr eax, shamt as i8),
        ShiftOp::Sra => dynasm!(ops ; sar eax, shamt as i8),
    }
    dynasm!(ops ; movsxd rax, eax ; mov QWORD [rdi + rd_off], rax);
}

// ── Shift register (64-bit) ─────────────────────────────────────────────────

/// Emit a shift-register instruction (SLL, SRL, SRA).
///
/// Shift amount is the low 6 bits of rs2 (for RV64).
pub fn emit_shift_reg(ops: &mut Assembler, rd: u8, rs1: u8, rs2: u8, op: ShiftOp) {
    if skip_rd_zero(rd) {
        return;
    }
    let rs1_off = reg_off(rs1);
    let rs2_off = reg_off(rs2);
    let rd_off = reg_off(rd);
    // x86 shifts use cl (low 6 bits of rcx for 64-bit ops)
    dynasm!(ops
        ; mov rax, QWORD [rdi + rs1_off]
        ; mov rcx, QWORD [rdi + rs2_off]
    );
    match op {
        ShiftOp::Sll => dynasm!(ops ; shl rax, cl),
        ShiftOp::Srl => dynasm!(ops ; shr rax, cl),
        ShiftOp::Sra => dynasm!(ops ; sar rax, cl),
    }
    dynasm!(ops ; mov QWORD [rdi + rd_off], rax);
}

// ── Shift register (32-bit word ops) ────────────────────────────────────────

/// Emit a 32-bit shift-register instruction (SLLW, SRLW, SRAW).
///
/// Shift amount is the low 5 bits of rs2; result sign-extended to 64 bits.
pub fn emit_shift_reg_w(ops: &mut Assembler, rd: u8, rs1: u8, rs2: u8, op: ShiftOp) {
    if skip_rd_zero(rd) {
        return;
    }
    let rs1_off = reg_off(rs1);
    let rs2_off = reg_off(rs2);
    let rd_off = reg_off(rd);
    dynasm!(ops
        ; mov eax, DWORD [rdi + rs1_off]
        ; mov ecx, DWORD [rdi + rs2_off]
        ; and ecx, 0x1F // Only low 5 bits for 32-bit shift
    );
    match op {
        ShiftOp::Sll => dynasm!(ops ; shl eax, cl),
        ShiftOp::Srl => dynasm!(ops ; shr eax, cl),
        ShiftOp::Sra => dynasm!(ops ; sar eax, cl),
    }
    dynasm!(ops ; movsxd rax, eax ; mov QWORD [rdi + rd_off], rax);
}

// ── Set-less-than immediate ─────────────────────────────────────────────────

/// Emit SLTI / SLTIU: `rd = (rs1 < imm) ? 1 : 0`
pub fn emit_slti(ops: &mut Assembler, rd: u8, rs1: u8, imm: i64, unsigned: bool) {
    if skip_rd_zero(rd) {
        return;
    }
    let rs1_off = reg_off(rs1);
    let rd_off = reg_off(rd);
    dynasm!(ops
        ; mov rax, QWORD [rdi + rs1_off]
        ; mov rcx, QWORD imm
        ; cmp rax, rcx
    );
    // setl/setb reads flags; do NOT xor eax before set (clobbers flags)
    if unsigned {
        dynasm!(ops ; setb al);
    } else {
        dynasm!(ops ; setl al);
    }
    dynasm!(ops ; movzx rax, al ; mov QWORD [rdi + rd_off], rax);
}

// ── Set-less-than register ──────────────────────────────────────────────────

/// Emit SLT / SLTU: `rd = (rs1 < rs2) ? 1 : 0`
pub fn emit_slt_reg(ops: &mut Assembler, rd: u8, rs1: u8, rs2: u8, unsigned: bool) {
    if skip_rd_zero(rd) {
        return;
    }
    let rs1_off = reg_off(rs1);
    let rs2_off = reg_off(rs2);
    let rd_off = reg_off(rd);
    dynasm!(ops
        ; mov rax, QWORD [rdi + rs1_off]
        ; mov rcx, QWORD [rdi + rs2_off]
        ; cmp rax, rcx
    );
    // setl/setb reads flags; xor would clobber them, so use movzx after set
    if unsigned {
        dynasm!(ops ; setb al);
    } else {
        dynasm!(ops ; setl al);
    }
    dynasm!(ops ; movzx rax, al ; mov QWORD [rdi + rd_off], rax);
}

// ── Upper immediate ─────────────────────────────────────────────────────────

/// Emit LUI: `rd = imm` (imm already has upper bits positioned by decoder).
pub fn emit_lui(ops: &mut Assembler, rd: u8, imm: i64) {
    if skip_rd_zero(rd) {
        return;
    }
    let rd_off = reg_off(rd);
    dynasm!(ops
        ; mov rax, QWORD imm
        ; mov QWORD [rdi + rd_off], rax
    );
}

/// Emit AUIPC: `rd = pc + imm`.
pub fn emit_auipc(ops: &mut Assembler, rd: u8, imm: i64, pc: u64) {
    if skip_rd_zero(rd) {
        return;
    }
    let rd_off = reg_off(rd);
    let val = (pc as i64).wrapping_add(imm) as u64;
    dynasm!(ops
        ; mov rax, QWORD val as i64
        ; mov QWORD [rdi + rd_off], rax
    );
}

// ── Multiply ────────────────────────────────────────────────────────────────

/// Emit MUL: `rd = (rs1 * rs2)[63:0]` (low 64 bits of 128-bit product).
pub fn emit_mul(ops: &mut Assembler, rd: u8, rs1: u8, rs2: u8) {
    if skip_rd_zero(rd) {
        return;
    }
    let rs1_off = reg_off(rs1);
    let rs2_off = reg_off(rs2);
    let rd_off = reg_off(rd);
    dynasm!(ops
        ; mov rax, QWORD [rdi + rs1_off]
        ; imul rax, QWORD [rdi + rs2_off]
        ; mov QWORD [rdi + rd_off], rax
    );
}

/// Emit MULW: `rd = sign_extend_32((rs1[31:0] * rs2[31:0])[31:0])`.
pub fn emit_mulw(ops: &mut Assembler, rd: u8, rs1: u8, rs2: u8) {
    if skip_rd_zero(rd) {
        return;
    }
    let rs1_off = reg_off(rs1);
    let rs2_off = reg_off(rs2);
    let rd_off = reg_off(rd);
    dynasm!(ops
        ; mov eax, DWORD [rdi + rs1_off]
        ; imul eax, DWORD [rdi + rs2_off]
        ; movsxd rax, eax
        ; mov QWORD [rdi + rd_off], rax
    );
}

// ── Divide (64-bit) ─────────────────────────────────────────────────────────

/// Emit DIV / DIVU: `rd = rs1 / rs2`.
///
/// RISC-V spec for division by zero:
/// - DIV:  result = -1 (all ones)
/// - DIVU: result = 2^64 - 1 (all ones)
///
/// RISC-V spec for signed overflow (MIN_I64 / -1):
/// - DIV:  result = MIN_I64
pub fn emit_div(ops: &mut Assembler, rd: u8, rs1: u8, rs2: u8, signed: bool) {
    if skip_rd_zero(rd) {
        return;
    }
    let rs1_off = reg_off(rs1);
    let rs2_off = reg_off(rs2);
    let rd_off = reg_off(rd);
    dynasm!(ops
        ; mov rax, QWORD [rdi + rs1_off]
        ; mov rcx, QWORD [rdi + rs2_off]
        ; test rcx, rcx
        ; jz >div_zero
    );
    if signed {
        // Check for signed overflow: MIN_I64 / -1
        dynasm!(ops
            ; mov r8, QWORD i64::MIN
            ; cmp rax, r8
            ; jne >no_overflow
            ; cmp rcx, -1i32
            ; jne >no_overflow
            // Overflow: result = MIN_I64 (already in rax)
            ; jmp >done
            ; no_overflow:
            ; cqo
            ; idiv rcx
        );
    } else {
        dynasm!(ops ; xor edx, edx ; div rcx);
    }
    dynasm!(ops
        ; jmp >done
        ; div_zero:
        ; mov rax, QWORD -1i64
        ; done:
        ; mov QWORD [rdi + rd_off], rax
    );
}

// ── Divide (32-bit word ops) ────────────────────────────────────────────────

/// Emit DIVW / DIVUW: `rd = sign_extend_32(rs1[31:0] / rs2[31:0])`.
pub fn emit_divw(ops: &mut Assembler, rd: u8, rs1: u8, rs2: u8, signed: bool) {
    if skip_rd_zero(rd) {
        return;
    }
    let rs1_off = reg_off(rs1);
    let rs2_off = reg_off(rs2);
    let rd_off = reg_off(rd);
    dynasm!(ops
        ; mov eax, DWORD [rdi + rs1_off]
        ; mov ecx, DWORD [rdi + rs2_off]
        ; test ecx, ecx
        ; jz >div_zero
    );
    if signed {
        // Check for signed overflow: MIN_I32 / -1
        dynasm!(ops
            ; cmp eax, i32::MIN
            ; jne >no_overflow
            ; cmp ecx, -1i32
            ; jne >no_overflow
            // Overflow: result = MIN_I32 (already in eax)
            ; jmp >done_w
            ; no_overflow:
            ; cdq
            ; idiv ecx
        );
    } else {
        dynasm!(ops ; xor edx, edx ; div ecx);
    }
    dynasm!(ops
        ; jmp >done_w
        ; div_zero:
    );
    if signed {
        dynasm!(ops ; mov eax, -1i32);
    } else {
        dynasm!(ops ; mov eax, -1i32); // 0xFFFF_FFFF
    }
    dynasm!(ops
        ; done_w:
        ; movsxd rax, eax
        ; mov QWORD [rdi + rd_off], rax
    );
}

// ── Remainder (64-bit) ──────────────────────────────────────────────────────

/// Emit REM / REMU: `rd = rs1 % rs2`.
///
/// RISC-V spec for remainder by zero:
/// - REM:  result = dividend (rs1)
/// - REMU: result = dividend (rs1)
///
/// RISC-V spec for signed overflow (MIN_I64 % -1):
/// - REM:  result = 0
pub fn emit_rem(ops: &mut Assembler, rd: u8, rs1: u8, rs2: u8, signed: bool) {
    if skip_rd_zero(rd) {
        return;
    }
    let rs1_off = reg_off(rs1);
    let rs2_off = reg_off(rs2);
    let rd_off = reg_off(rd);
    dynasm!(ops
        ; mov rax, QWORD [rdi + rs1_off]
        ; mov rcx, QWORD [rdi + rs2_off]
        ; test rcx, rcx
        ; jz >div_zero
    );
    if signed {
        // Check for signed overflow: MIN_I64 % -1 = 0
        dynasm!(ops
            ; mov r8, QWORD i64::MIN
            ; cmp rax, r8
            ; jne >no_overflow
            ; cmp rcx, -1i32
            ; jne >no_overflow
            ; xor eax, eax  // result = 0
            ; jmp >done
            ; no_overflow:
            ; cqo
            ; idiv rcx
        );
    } else {
        dynasm!(ops ; xor edx, edx ; div rcx);
    }
    dynasm!(ops
        ; mov rax, rdx  // remainder is in rdx
        ; jmp >done
        ; div_zero:
        ; mov rax, QWORD [rdi + rs1_off]  // RV spec: rem(x, 0) = x
        ; done:
        ; mov QWORD [rdi + rd_off], rax
    );
}

// ── Remainder (32-bit word ops) ─────────────────────────────────────────────

/// Emit REMW / REMUW: `rd = sign_extend_32(rs1[31:0] % rs2[31:0])`.
pub fn emit_remw(ops: &mut Assembler, rd: u8, rs1: u8, rs2: u8, signed: bool) {
    if skip_rd_zero(rd) {
        return;
    }
    let rs1_off = reg_off(rs1);
    let rs2_off = reg_off(rs2);
    let rd_off = reg_off(rd);
    dynasm!(ops
        ; mov eax, DWORD [rdi + rs1_off]
        ; mov ecx, DWORD [rdi + rs2_off]
        ; test ecx, ecx
        ; jz >div_zero
    );
    if signed {
        // Check for signed overflow: MIN_I32 % -1 = 0
        dynasm!(ops
            ; cmp eax, i32::MIN
            ; jne >no_overflow
            ; cmp ecx, -1i32
            ; jne >no_overflow
            ; xor eax, eax
            ; jmp >done_w
            ; no_overflow:
            ; cdq
            ; idiv ecx
        );
    } else {
        dynasm!(ops ; xor edx, edx ; div ecx);
    }
    dynasm!(ops
        ; mov eax, edx  // remainder is in edx
        ; jmp >done_w
        ; div_zero:
        ; mov eax, DWORD [rdi + rs1_off]  // RV spec: rem(x, 0) = x
        ; done_w:
        ; movsxd rax, eax
        ; mov QWORD [rdi + rd_off], rax
    );
}

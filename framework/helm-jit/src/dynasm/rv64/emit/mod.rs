//! RV64 instruction emitter dispatch.
//!
//! Routes decoded RISC-V64 instructions to per-category x86-64 code generators:
//! - `alu` : integer arithmetic, logic, shifts, upper-immediate, multiply/divide
//! - `branch` : conditional/unconditional branches, JAL/JALR, ECALL/EBREAK
//! - `ldst` : loads and stores via memory helper functions

#![allow(missing_docs)]

use dynasmrt::x64::Assembler;
use helm_arch::riscv::insn::Instruction;

pub mod alu;
pub mod branch;
pub mod ldst;

// Re-export enum types used by emitter dispatch
pub use alu::{AluOp, ShiftOp};
pub use branch::BranchCond;

/// Emit x86-64 code for one RISC-V64 instruction.
///
/// # Returns
/// - `Some(true)` : instruction emitted, block terminates (branch/ecall/ebreak)
/// - `Some(false)`: instruction emitted, block continues
/// - `None`       : opcode unsupported, block compilation stops here
pub fn emit_rv64_insn(ops: &mut Assembler, insn: &Instruction, pc: u64) -> Option<bool> {
    match insn {
        // ── ALU immediate (64-bit) ──────────────────────────────────────────
        Instruction::ADDI { rd, rs1, imm } => {
            alu::emit_alu_imm(ops, *rd, *rs1, *imm, AluOp::Add);
            Some(false)
        }
        Instruction::SLTI { rd, rs1, imm } => {
            alu::emit_slti(ops, *rd, *rs1, *imm, false);
            Some(false)
        }
        Instruction::SLTIU { rd, rs1, imm } => {
            alu::emit_slti(ops, *rd, *rs1, *imm, true);
            Some(false)
        }
        Instruction::XORI { rd, rs1, imm } => {
            alu::emit_alu_imm(ops, *rd, *rs1, *imm, AluOp::Xor);
            Some(false)
        }
        Instruction::ORI { rd, rs1, imm } => {
            alu::emit_alu_imm(ops, *rd, *rs1, *imm, AluOp::Or);
            Some(false)
        }
        Instruction::ANDI { rd, rs1, imm } => {
            alu::emit_alu_imm(ops, *rd, *rs1, *imm, AluOp::And);
            Some(false)
        }
        Instruction::SLLI { rd, rs1, shamt } => {
            alu::emit_shift_imm(ops, *rd, *rs1, *shamt, ShiftOp::Sll);
            Some(false)
        }
        Instruction::SRLI { rd, rs1, shamt } => {
            alu::emit_shift_imm(ops, *rd, *rs1, *shamt, ShiftOp::Srl);
            Some(false)
        }
        Instruction::SRAI { rd, rs1, shamt } => {
            alu::emit_shift_imm(ops, *rd, *rs1, *shamt, ShiftOp::Sra);
            Some(false)
        }

        // ── ALU register (64-bit) ──────────────────────────────────────────
        Instruction::ADD { rd, rs1, rs2 } => {
            alu::emit_alu_reg(ops, *rd, *rs1, *rs2, AluOp::Add);
            Some(false)
        }
        Instruction::SUB { rd, rs1, rs2 } => {
            alu::emit_alu_reg(ops, *rd, *rs1, *rs2, AluOp::Sub);
            Some(false)
        }
        Instruction::SLL { rd, rs1, rs2 } => {
            alu::emit_shift_reg(ops, *rd, *rs1, *rs2, ShiftOp::Sll);
            Some(false)
        }
        Instruction::SLT { rd, rs1, rs2 } => {
            alu::emit_slt_reg(ops, *rd, *rs1, *rs2, false);
            Some(false)
        }
        Instruction::SLTU { rd, rs1, rs2 } => {
            alu::emit_slt_reg(ops, *rd, *rs1, *rs2, true);
            Some(false)
        }
        Instruction::XOR { rd, rs1, rs2 } => {
            alu::emit_alu_reg(ops, *rd, *rs1, *rs2, AluOp::Xor);
            Some(false)
        }
        Instruction::SRL { rd, rs1, rs2 } => {
            alu::emit_shift_reg(ops, *rd, *rs1, *rs2, ShiftOp::Srl);
            Some(false)
        }
        Instruction::SRA { rd, rs1, rs2 } => {
            alu::emit_shift_reg(ops, *rd, *rs1, *rs2, ShiftOp::Sra);
            Some(false)
        }
        Instruction::OR { rd, rs1, rs2 } => {
            alu::emit_alu_reg(ops, *rd, *rs1, *rs2, AluOp::Or);
            Some(false)
        }
        Instruction::AND { rd, rs1, rs2 } => {
            alu::emit_alu_reg(ops, *rd, *rs1, *rs2, AluOp::And);
            Some(false)
        }

        // ── Word ops (32-bit, sign-extended to 64) ─────────────────────────
        Instruction::ADDIW { rd, rs1, imm } => {
            alu::emit_alu_imm_w(ops, *rd, *rs1, *imm, AluOp::Add);
            Some(false)
        }
        Instruction::SLLIW { rd, rs1, shamt } => {
            alu::emit_shift_imm_w(ops, *rd, *rs1, *shamt, ShiftOp::Sll);
            Some(false)
        }
        Instruction::SRLIW { rd, rs1, shamt } => {
            alu::emit_shift_imm_w(ops, *rd, *rs1, *shamt, ShiftOp::Srl);
            Some(false)
        }
        Instruction::SRAIW { rd, rs1, shamt } => {
            alu::emit_shift_imm_w(ops, *rd, *rs1, *shamt, ShiftOp::Sra);
            Some(false)
        }
        Instruction::ADDW { rd, rs1, rs2 } => {
            alu::emit_alu_reg_w(ops, *rd, *rs1, *rs2, AluOp::Add);
            Some(false)
        }
        Instruction::SUBW { rd, rs1, rs2 } => {
            alu::emit_alu_reg_w(ops, *rd, *rs1, *rs2, AluOp::Sub);
            Some(false)
        }
        Instruction::SLLW { rd, rs1, rs2 } => {
            alu::emit_shift_reg_w(ops, *rd, *rs1, *rs2, ShiftOp::Sll);
            Some(false)
        }
        Instruction::SRLW { rd, rs1, rs2 } => {
            alu::emit_shift_reg_w(ops, *rd, *rs1, *rs2, ShiftOp::Srl);
            Some(false)
        }
        Instruction::SRAW { rd, rs1, rs2 } => {
            alu::emit_shift_reg_w(ops, *rd, *rs1, *rs2, ShiftOp::Sra);
            Some(false)
        }

        // ── Upper immediate ────────────────────────────────────────────────
        Instruction::LUI { rd, imm } => {
            alu::emit_lui(ops, *rd, *imm);
            Some(false)
        }
        Instruction::AUIPC { rd, imm } => {
            alu::emit_auipc(ops, *rd, *imm, pc);
            Some(false)
        }

        // ── Multiply/Divide (64-bit) ───────────────────────────────────────
        Instruction::MUL { rd, rs1, rs2 } => {
            alu::emit_mul(ops, *rd, *rs1, *rs2);
            Some(false)
        }
        Instruction::MULW { rd, rs1, rs2 } => {
            alu::emit_mulw(ops, *rd, *rs1, *rs2);
            Some(false)
        }
        Instruction::DIV { rd, rs1, rs2 } => {
            alu::emit_div(ops, *rd, *rs1, *rs2, true);
            Some(false)
        }
        Instruction::DIVU { rd, rs1, rs2 } => {
            alu::emit_div(ops, *rd, *rs1, *rs2, false);
            Some(false)
        }
        Instruction::DIVW { rd, rs1, rs2 } => {
            alu::emit_divw(ops, *rd, *rs1, *rs2, true);
            Some(false)
        }
        Instruction::DIVUW { rd, rs1, rs2 } => {
            alu::emit_divw(ops, *rd, *rs1, *rs2, false);
            Some(false)
        }
        Instruction::REM { rd, rs1, rs2 } => {
            alu::emit_rem(ops, *rd, *rs1, *rs2, true);
            Some(false)
        }
        Instruction::REMU { rd, rs1, rs2 } => {
            alu::emit_rem(ops, *rd, *rs1, *rs2, false);
            Some(false)
        }
        Instruction::REMW { rd, rs1, rs2 } => {
            alu::emit_remw(ops, *rd, *rs1, *rs2, true);
            Some(false)
        }
        Instruction::REMUW { rd, rs1, rs2 } => {
            alu::emit_remw(ops, *rd, *rs1, *rs2, false);
            Some(false)
        }

        // ── Load/Store ─────────────────────────────────────────────────────
        Instruction::LB { rd, rs1, imm } => {
            ldst::emit_load(ops, *rd, *rs1, *imm, 1, true);
            Some(false)
        }
        Instruction::LH { rd, rs1, imm } => {
            ldst::emit_load(ops, *rd, *rs1, *imm, 2, true);
            Some(false)
        }
        Instruction::LW { rd, rs1, imm } => {
            ldst::emit_load(ops, *rd, *rs1, *imm, 4, true);
            Some(false)
        }
        Instruction::LD { rd, rs1, imm } => {
            ldst::emit_load(ops, *rd, *rs1, *imm, 8, false);
            Some(false)
        }
        Instruction::LBU { rd, rs1, imm } => {
            ldst::emit_load(ops, *rd, *rs1, *imm, 1, false);
            Some(false)
        }
        Instruction::LHU { rd, rs1, imm } => {
            ldst::emit_load(ops, *rd, *rs1, *imm, 2, false);
            Some(false)
        }
        Instruction::LWU { rd, rs1, imm } => {
            ldst::emit_load(ops, *rd, *rs1, *imm, 4, false);
            Some(false)
        }
        Instruction::SB { rs1, rs2, imm } => {
            ldst::emit_store(ops, *rs1, *rs2, *imm, 1);
            Some(false)
        }
        Instruction::SH { rs1, rs2, imm } => {
            ldst::emit_store(ops, *rs1, *rs2, *imm, 2);
            Some(false)
        }
        Instruction::SW { rs1, rs2, imm } => {
            ldst::emit_store(ops, *rs1, *rs2, *imm, 4);
            Some(false)
        }
        Instruction::SD { rs1, rs2, imm } => {
            ldst::emit_store(ops, *rs1, *rs2, *imm, 8);
            Some(false)
        }

        // ── Branches (all terminate the block) ─────────────────────────────
        Instruction::BEQ { rs1, rs2, imm } => {
            branch::emit_branch(ops, *rs1, *rs2, *imm, pc, BranchCond::Eq);
            Some(true)
        }
        Instruction::BNE { rs1, rs2, imm } => {
            branch::emit_branch(ops, *rs1, *rs2, *imm, pc, BranchCond::Ne);
            Some(true)
        }
        Instruction::BLT { rs1, rs2, imm } => {
            branch::emit_branch(ops, *rs1, *rs2, *imm, pc, BranchCond::Lt);
            Some(true)
        }
        Instruction::BGE { rs1, rs2, imm } => {
            branch::emit_branch(ops, *rs1, *rs2, *imm, pc, BranchCond::Ge);
            Some(true)
        }
        Instruction::BLTU { rs1, rs2, imm } => {
            branch::emit_branch(ops, *rs1, *rs2, *imm, pc, BranchCond::Ltu);
            Some(true)
        }
        Instruction::BGEU { rs1, rs2, imm } => {
            branch::emit_branch(ops, *rs1, *rs2, *imm, pc, BranchCond::Geu);
            Some(true)
        }
        Instruction::JAL { rd, imm } => {
            branch::emit_jal(ops, *rd, *imm, pc);
            Some(true)
        }
        Instruction::JALR { rd, rs1, imm } => {
            branch::emit_jalr(ops, *rd, *rs1, *imm, pc);
            Some(true)
        }

        // ── System ─────────────────────────────────────────────────────────
        Instruction::ECALL => {
            branch::emit_ecall(ops, pc);
            Some(true)
        }
        Instruction::EBREAK => {
            branch::emit_ebreak(ops, pc);
            Some(true)
        }

        // FENCE is a no-op on a single-threaded simulator
        Instruction::FENCE { .. } | Instruction::FENCE_I => Some(false),

        // Everything else: unsupported (atomics, FP, CSR, privileged, etc.)
        _ => None,
    }
}

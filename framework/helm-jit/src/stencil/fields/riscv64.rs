//! RISC-V64 instruction → DecodedFields extraction.

#![allow(missing_docs)]

use crate::stencil::types::DecodedFields;
use helm_arch::riscv::insn::Instruction;

/// Extract stencil fields from a decoded RISC-V64 instruction.
///
/// Maps the per-variant fields of the RISC-V `Instruction` enum into the
/// ISA-neutral `DecodedFields` representation. RISC-V `rs1` maps to `rn`,
/// and `rs2` maps to `rm`.
pub fn extract_fields_rv64(insn: &Instruction, pc: u64) -> DecodedFields {
    let mut f = DecodedFields {
        sf: 1, // RV64 always operates on 64-bit registers
        next_pc: pc + 4,
        ..Default::default()
    };

    match *insn {
        // ── ALU immediate (I-type) ─────────────────────────────────────
        Instruction::ADDI { rd, rs1, imm }
        | Instruction::SLTI { rd, rs1, imm }
        | Instruction::SLTIU { rd, rs1, imm }
        | Instruction::XORI { rd, rs1, imm }
        | Instruction::ORI { rd, rs1, imm }
        | Instruction::ANDI { rd, rs1, imm } => {
            f.rd = rd;
            f.rn = rs1;
            f.imm = imm;
        }

        // Shift immediate
        Instruction::SLLI { rd, rs1, shamt }
        | Instruction::SRLI { rd, rs1, shamt }
        | Instruction::SRAI { rd, rs1, shamt } => {
            f.rd = rd;
            f.rn = rs1;
            f.shamt = shamt;
        }

        // ── ALU register (R-type) ──────────────────────────────────────
        Instruction::ADD { rd, rs1, rs2 }
        | Instruction::SUB { rd, rs1, rs2 }
        | Instruction::SLL { rd, rs1, rs2 }
        | Instruction::SLT { rd, rs1, rs2 }
        | Instruction::SLTU { rd, rs1, rs2 }
        | Instruction::XOR { rd, rs1, rs2 }
        | Instruction::SRL { rd, rs1, rs2 }
        | Instruction::SRA { rd, rs1, rs2 }
        | Instruction::OR { rd, rs1, rs2 }
        | Instruction::AND { rd, rs1, rs2 } => {
            f.rd = rd;
            f.rn = rs1;
            f.rm = rs2;
        }

        // ── Word ops (RV64I) ───────────────────────────────────────────
        Instruction::ADDIW { rd, rs1, imm } => {
            f.rd = rd;
            f.rn = rs1;
            f.imm = imm;
        }
        Instruction::SLLIW { rd, rs1, shamt }
        | Instruction::SRLIW { rd, rs1, shamt }
        | Instruction::SRAIW { rd, rs1, shamt } => {
            f.rd = rd;
            f.rn = rs1;
            f.shamt = shamt;
        }
        Instruction::ADDW { rd, rs1, rs2 }
        | Instruction::SUBW { rd, rs1, rs2 }
        | Instruction::SLLW { rd, rs1, rs2 }
        | Instruction::SRLW { rd, rs1, rs2 }
        | Instruction::SRAW { rd, rs1, rs2 } => {
            f.rd = rd;
            f.rn = rs1;
            f.rm = rs2;
        }

        // ── Loads (I-type) ─────────────────────────────────────────────
        Instruction::LB { rd, rs1, imm }
        | Instruction::LH { rd, rs1, imm }
        | Instruction::LW { rd, rs1, imm }
        | Instruction::LD { rd, rs1, imm }
        | Instruction::LBU { rd, rs1, imm }
        | Instruction::LHU { rd, rs1, imm }
        | Instruction::LWU { rd, rs1, imm } => {
            f.rd = rd;
            f.rn = rs1;
            f.imm = imm;
        }

        // ── Stores (S-type) ────────────────────────────────────────────
        Instruction::SB { rs1, rs2, imm }
        | Instruction::SH { rs1, rs2, imm }
        | Instruction::SW { rs1, rs2, imm }
        | Instruction::SD { rs1, rs2, imm } => {
            f.rn = rs1;
            f.rm = rs2;
            f.imm = imm;
        }

        // ── Branches (B-type) ──────────────────────────────────────────
        Instruction::BEQ { rs1, rs2, imm }
        | Instruction::BNE { rs1, rs2, imm }
        | Instruction::BLT { rs1, rs2, imm }
        | Instruction::BGE { rs1, rs2, imm }
        | Instruction::BLTU { rs1, rs2, imm }
        | Instruction::BGEU { rs1, rs2, imm } => {
            f.rn = rs1;
            f.rm = rs2;
            f.imm = imm;
            f.branch_target = (pc as i64 + imm) as u64;
        }

        // ── JAL (J-type) ───────────────────────────────────────────────
        Instruction::JAL { rd, imm } => {
            f.rd = rd;
            f.imm = imm;
            f.branch_target = (pc as i64 + imm) as u64;
        }

        // ── JALR (I-type) ──────────────────────────────────────────────
        Instruction::JALR { rd, rs1, imm } => {
            f.rd = rd;
            f.rn = rs1;
            f.imm = imm;
            // branch_target computed at runtime (rs1 + imm)
        }

        // ── Upper immediate (U-type) ───────────────────────────────────
        Instruction::LUI { rd, imm } => {
            f.rd = rd;
            f.imm = imm;
        }
        Instruction::AUIPC { rd, imm } => {
            f.rd = rd;
            f.imm = imm;
            f.branch_target = pc; // Reuse branch_target for current PC
        }

        // ── Multiply/Divide (R-type, RV64M) ───────────────────────────
        Instruction::MUL { rd, rs1, rs2 }
        | Instruction::MULH { rd, rs1, rs2 }
        | Instruction::MULHSU { rd, rs1, rs2 }
        | Instruction::MULHU { rd, rs1, rs2 }
        | Instruction::DIV { rd, rs1, rs2 }
        | Instruction::DIVU { rd, rs1, rs2 }
        | Instruction::REM { rd, rs1, rs2 }
        | Instruction::REMU { rd, rs1, rs2 } => {
            f.rd = rd;
            f.rn = rs1;
            f.rm = rs2;
        }

        // ── System ─────────────────────────────────────────────────────
        Instruction::ECALL | Instruction::EBREAK => {
            // No fields to extract
        }

        // ── Everything else — not supported by stencil JIT ─────────────
        _ => {}
    }

    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_rv_addi_fields() {
        let insn = Instruction::ADDI {
            rd: 5,
            rs1: 3,
            imm: 42,
        };
        let f = extract_fields_rv64(&insn, 0x1000);
        assert_eq!(f.rd, 5);
        assert_eq!(f.rn, 3);
        assert_eq!(f.imm, 42);
        assert_eq!(f.sf, 1);
        assert_eq!(f.next_pc, 0x1004);
    }

    #[test]
    fn extract_rv_beq_branch_target() {
        let insn = Instruction::BEQ {
            rs1: 1,
            rs2: 2,
            imm: 0x10,
        };
        let f = extract_fields_rv64(&insn, 0x2000);
        assert_eq!(f.rn, 1);
        assert_eq!(f.rm, 2);
        assert_eq!(f.branch_target, 0x2010);
        assert_eq!(f.next_pc, 0x2004);
    }

    #[test]
    fn extract_rv_jal_fields() {
        let insn = Instruction::JAL { rd: 1, imm: 0x100 };
        let f = extract_fields_rv64(&insn, 0x3000);
        assert_eq!(f.rd, 1);
        assert_eq!(f.branch_target, 0x3100);
        assert_eq!(f.next_pc, 0x3004);
    }

    #[test]
    fn extract_rv_sd_fields() {
        let insn = Instruction::SD {
            rs1: 2,
            rs2: 10,
            imm: -8,
        };
        let f = extract_fields_rv64(&insn, 0x4000);
        assert_eq!(f.rn, 2);
        assert_eq!(f.rm, 10);
        assert_eq!(f.imm, -8);
    }
}

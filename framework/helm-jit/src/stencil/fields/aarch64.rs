//! AArch64 instruction → DecodedFields extraction.

#![allow(missing_docs)]

use helm_arch::aarch64::insn::{Instruction, Opcode};
use crate::stencil::types::DecodedFields;

/// Extract stencil fields from a decoded AArch64 instruction.
///
/// Maps the `Instruction` struct fields into the ISA-neutral `DecodedFields`
/// representation used by the stencil compiler for hole patching.
pub fn extract_fields_a64(insn: &Instruction, pc: u64) -> DecodedFields {
    let mut f = DecodedFields {
        rd: insn.rd as u8,
        rn: insn.rn as u8,
        rm: insn.rm as u8,
        ra: insn.ra as u8,
        rt: insn.rd as u8, // For loads/stores, rd == rt in AArch64 encoding
        rt2: insn.pair_second as u8,
        imm: insn.imm,
        simm: 0,
        shamt: insn.shift_amt as u8,
        sf: if insn.sf { 1 } else { 0 },
        shift: insn.shift_type as u8,
        cond: insn.cond as u8,
        branch_target: 0,
        next_pc: pc + 4,
    };

    // Compute branch_target for branch opcodes.
    match insn.opcode {
        Opcode::B | Opcode::Bl => {
            f.branch_target = (pc as i64 + insn.imm) as u64;
        }
        Opcode::BCond | Opcode::Cbz | Opcode::Cbnz | Opcode::Tbz | Opcode::Tbnz => {
            f.branch_target = (pc as i64 + insn.imm) as u64;
            // For CBZ/CBNZ: rt = rd in the encoding
            f.rt = insn.rd as u8;
        }
        _ => {}
    }

    // For MOVK: shamt = hw * 16 (shift position for 16-bit insert)
    if insn.opcode == Opcode::Movk {
        f.shamt = (insn.imm2 as u8) * 16;
        // imm is the raw imm16 value for MOVK
    }

    // For BCond: pass condition code through imm field
    if insn.opcode == Opcode::BCond {
        f.imm = i64::from(insn.cond);
    }

    // For loads/stores: rt = rd, rn = base register, imm = offset
    // The decoder already pre-computes the scaled offset in insn.imm.

    // For RET: rn = X30 by default (decoder sets insn.rn)
    if insn.opcode == Opcode::Ret && insn.rn == 0 {
        f.rn = 30; // Default to X30 if not specified
    }

    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_arch::aarch64::insn::Instruction;

    #[test]
    fn extract_add_imm_fields() {
        let insn = Instruction {
            opcode: Opcode::AddImm,
            rd: 5,
            rn: 3,
            imm: 42,
            sf: true,
            pc: 0x1000,
            ..Instruction::zeroed()
        };
        let f = extract_fields_a64(&insn, 0x1000);
        assert_eq!(f.rd, 5);
        assert_eq!(f.rn, 3);
        assert_eq!(f.imm, 42);
        assert_eq!(f.sf, 1);
        assert_eq!(f.next_pc, 0x1004);
    }

    #[test]
    fn extract_branch_target() {
        let insn = Instruction {
            opcode: Opcode::B,
            imm: 0x100, // branch offset
            pc: 0x2000,
            ..Instruction::zeroed()
        };
        let f = extract_fields_a64(&insn, 0x2000);
        assert_eq!(f.branch_target, 0x2100);
        assert_eq!(f.next_pc, 0x2004);
    }

    #[test]
    fn extract_cbz_fields() {
        let insn = Instruction {
            opcode: Opcode::Cbz,
            rd: 7,
            imm: -0x10,
            sf: true,
            pc: 0x3000,
            ..Instruction::zeroed()
        };
        let f = extract_fields_a64(&insn, 0x3000);
        assert_eq!(f.rt, 7);
        assert_eq!(f.branch_target, 0x2FF0);
    }
}

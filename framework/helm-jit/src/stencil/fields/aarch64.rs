//! AArch64 instruction → DecodedFields extraction.

#![allow(missing_docs)]

use helm_arch::aarch64::insn::{Instruction, Opcode};
use crate::stencil::types::DecodedFields;

/// Compute a 16-bit bitmask for an ARM condition code.
///
/// The bitmask has bit `i` set if the 4-bit NZCV value `i` (where
/// N=bit3, Z=bit2, C=bit1, V=bit0) means "taken" for this condition.
fn cond_to_bitmask(cond: u32) -> u16 {
    let mut mask = 0u16;
    for nzcv_4 in 0u32..16 {
        let n = (nzcv_4 >> 3) & 1;
        let z = (nzcv_4 >> 2) & 1;
        let c = (nzcv_4 >> 1) & 1;
        let v = nzcv_4 & 1;
        let cc = cond >> 1;
        let base = match cc {
            0 => z,                     // EQ/NE
            1 => c,                     // CS/CC
            2 => n,                     // MI/PL
            3 => v,                     // VS/VC
            4 => c & (z ^ 1),           // HI/LS
            5 => (n == v) as u32,        // GE/LT
            6 => ((n == v) as u32) & (z ^ 1), // GT/LE
            7 | _ => 1,                 // AL/NV
        };
        let taken = if (cond & 1) != 0 && cc != 7 { base ^ 1 } else { base };
        if taken != 0 {
            mask |= 1 << nzcv_4;
        }
    }
    mask
}

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

    // For BCond: compute a 16-bit bitmask where bit[nzcv_4bit] = 1 if
    // this NZCV combination means "taken" for this condition code.
    // This avoids the compiler constant-folding the cond code at compile time.
    if insn.opcode == Opcode::BCond {
        f.imm = cond_to_bitmask(insn.cond) as i64;
    }

    // For Csel/Csinc: pass condition code through imm field
    match insn.opcode {
        Opcode::Csel | Opcode::Csinc | Opcode::Csinv | Opcode::Csneg => {
            f.imm = i64::from(insn.cond);
        }
        _ => {}
    }

    // For Adr: pre-compute pc + imm
    if insn.opcode == Opcode::Adr {
        f.imm = (pc as i64 + insn.imm) as i64;
    }

    // For Adrp: pre-compute (pc & ~0xFFF) + (imm << 12)
    // Decoder stores raw 21-bit signed offset; we apply the page shift here.
    if insn.opcode == Opcode::Adrp {
        f.imm = ((pc & !0xFFF) as i64 + (insn.imm << 12)) as i64;
    }

    // For Sbfm/Ubfm: immr in imm, imms in shamt
    // The decoder stores immr in imm and imms in imm2.
    if matches!(insn.opcode, Opcode::Sbfm | Opcode::Ubfm) {
        f.imm = insn.imm;        // immr
        f.shamt = insn.imm2 as u8; // imms
    }

    // For Extr: LSB stored in insn.imm, put into shamt for the stencil
    if insn.opcode == Opcode::Extr {
        f.shamt = insn.imm as u8;
    }

    // For TBZ/TBNZ: bit number in imm2, put into shamt
    if matches!(insn.opcode, Opcode::Tbz | Opcode::Tbnz) {
        f.shamt = insn.imm2 as u8;
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

//! Data-processing immediate (op0 = 100x).

use super::{bit, bits, decode_bit_mask, sext};
use crate::aarch64::insn::{Instruction, Opcode};

pub(super) fn decode_dp_imm(raw: u32, i: &mut Instruction) {
    let sf = bit(raw, 31) != 0;
    i.sf = sf;
    i.rd = bits(raw, 4, 0);
    i.rn = bits(raw, 9, 5);

    // Primary discriminator: bits[28:23] uniquely identifies each DP-IMM sub-group.
    let b28_23 = bits(raw, 28, 23);

    match b28_23 {
        // ADR/ADRP: bits[28:23]=10000x
        0b100000 | 0b100001 => {
            let page = bit(raw, 31) != 0;
            let immlo = bits(raw, 30, 29);
            let immhi = bits(raw, 23, 5);
            i.imm = sext(((immhi << 2) | immlo) as u64, 21);
            i.opcode = if page { Opcode::Adrp } else { Opcode::Adr };
        }

        // ADD/SUB immediate: bits[28:23]=10001x
        0b100010 | 0b100011 => {
            let sub = bit(raw, 30) != 0;
            let setf = bit(raw, 29) != 0;
            let sh = bit(raw, 22);
            let imm12 = bits(raw, 21, 10) as u64;
            i.imm = (imm12 << (sh * 12)) as i64;
            i.opcode = match (sub, setf) {
                (false, false) => Opcode::AddImm,
                (false, true) => Opcode::AddsImm,
                (true, false) => Opcode::SubImm,
                (true, true) => Opcode::SubsImm,
            };
        }

        // Logical immediate: bits[28:23]=100100
        0b100100 => {
            let n = bit(raw, 22);
            let immr = bits(raw, 21, 16);
            let imms = bits(raw, 15, 10);
            if let Some(mask) = decode_bit_mask(n != 0, imms, immr, sf) {
                i.imm = mask as i64;
            } else {
                i.opcode = Opcode::Undefined;
                return;
            }
            i.opcode = match bits(raw, 30, 29) {
                0b00 => Opcode::AndImm,
                0b01 => Opcode::OrrImm,
                0b10 => Opcode::EorImm,
                0b11 => Opcode::AndsImm,
                _ => unreachable!(),
            };
        }

        // Move wide (MOVN/MOVZ/MOVK): bits[28:23]=100101
        0b100101 => {
            let opc = bits(raw, 30, 29);
            let hw = bits(raw, 22, 21);
            let imm16 = bits(raw, 20, 5) as u64;
            match opc {
                0b00 => {
                    i.opcode = Opcode::Movn;
                    i.imm = !((imm16 << (hw * 16)) as i64);
                    i.imm2 = hw as u64;
                }
                0b10 => {
                    i.opcode = Opcode::Movz;
                    i.imm = (imm16 << (hw * 16)) as i64;
                    i.imm2 = hw as u64;
                }
                0b11 => {
                    i.opcode = Opcode::Movk;
                    i.imm = imm16 as i64; // raw imm16, executor applies shift
                    i.imm2 = hw as u64;
                }
                _ => {
                    i.opcode = Opcode::Undefined;
                }
            }
        }

        // Bitfield (SBFM/BFM/UBFM): bits[28:23]=100110
        0b100110 => {
            let opc = bits(raw, 30, 29);
            let immr = bits(raw, 21, 16);
            let imms = bits(raw, 15, 10);
            i.imm = immr as i64;
            i.imm2 = imms as u64;
            i.opcode = match opc {
                0b00 => Opcode::Sbfm,
                0b01 => Opcode::Bfm,
                0b10 => Opcode::Ubfm,
                _ => Opcode::Undefined,
            };
        }

        // EXTR: bits[28:23]=100111
        0b100111 => {
            i.rm = bits(raw, 20, 16);
            i.imm = bits(raw, 15, 10) as i64;
            i.opcode = Opcode::Extr;
        }

        _ => {
            i.opcode = Opcode::Undefined;
        }
    }
}

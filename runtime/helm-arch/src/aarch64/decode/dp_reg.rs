//! Data-processing register (op0 = 0101 / 1101).

use super::{bit, bits};
use crate::aarch64::insn::{Instruction, Opcode};

pub(super) fn decode_dp_reg(raw: u32, i: &mut Instruction) {
    let sf = bit(raw, 31) != 0;
    let _op54 = bits(raw, 30, 29);
    let _s = bit(raw, 29) != 0;
    i.sf = sf;
    i.rd = bits(raw, 4, 0);
    i.rn = bits(raw, 9, 5);
    i.rm = bits(raw, 20, 16);

    // Logical shifted register: bits[28:24]=01010
    if bit(raw, 28) == 0 && bit(raw, 24) == 0 {
        decode_dp_logical_shift(raw, i);
        return;
    }

    let _op = bits(raw, 30, 29);
    let s_bit = bit(raw, 29) != 0;

    // Multiply / divide: bit[28]=1, bit[24]=1
    if bit(raw, 28) == 1 && bit(raw, 24) == 1 {
        decode_dp_mul_div(raw, i);
        return;
    }

    // Conditional select: bits[28:21] = 11010100
    if bits(raw, 28, 21) == 0b1101_0100 {
        decode_dp_condsel(raw, i);
        return;
    }

    // Conditional compare
    if bit(raw, 28) == 1 && bit(raw, 27) == 1 && bits(raw, 24, 21) == 0b0010 {
        decode_dp_condcmp(raw, i);
        return;
    }

    // Data processing (2-source): bits[28:21]=11010110, bit30=0
    if bits(raw, 28, 21) == 0b1101_0110 && bit(raw, 30) == 0 {
        decode_dp_2src(raw, i);
        return;
    }

    // Data processing (1-source): bits[28:21]=11010110, bit30=1
    if bits(raw, 28, 21) == 0b1101_0110 && bit(raw, 30) == 1 {
        decode_dp_1src(raw, i);
        return;
    }

    // ADC / SBC: bits[28:21] = 11010000
    if bits(raw, 28, 21) == 0b1101_0000 {
        let sub = bit(raw, 30) != 0;
        let s = bit(raw, 29) != 0;
        i.opcode = match (sub, s) {
            (false, false) => Opcode::Adc,
            (false, true) => Opcode::Adcs,
            (true, false) => Opcode::Sbc,
            (true, true) => Opcode::Sbcs,
        };
        return;
    }

    // ── FlagM (v8.4) ─────────────────────────────────────────────────────────
    // SETF8/SETF16
    if raw & 0xFFFF_F01F == 0x3A00_000D {
        i.rn = bits(raw, 9, 5);
        i.opcode = if bit(raw, 14) == 0 {
            Opcode::Setf8
        } else {
            Opcode::Setf16
        };
        return;
    }
    // RMIF
    if bits(raw, 31, 21) == 0b1011_1010_000 && bits(raw, 15, 10) == 0b000100 {
        i.rn = bits(raw, 9, 5);
        i.imm = bits(raw, 20, 15) as i64;
        i.imm2 = bits(raw, 3, 0) as u64;
        i.opcode = Opcode::Rmif;
        return;
    }

    // Add/sub register (shifted or extended)
    let extend_mode = bit(raw, 21) != 0;
    let shift_type = bits(raw, 23, 22);
    let shift_amt = bits(raw, 15, 10);
    i.shift_type = shift_type;
    i.shift_amt = shift_amt;

    if extend_mode {
        i.extend_type = bits(raw, 15, 13);
        i.extend_amt = bits(raw, 12, 10);
        i.shift_type = 0;
        i.shift_amt = 0;
    }

    let sub = bit(raw, 30) != 0;
    i.opcode = if extend_mode {
        match (sub, s_bit) {
            (false, false) => Opcode::AddExt,
            (false, true) => Opcode::AddsExt,
            (true, false) => Opcode::SubExt,
            (true, true) => Opcode::SubsExt,
        }
    } else {
        match (sub, s_bit) {
            (false, false) => Opcode::AddReg,
            (false, true) => Opcode::AddsReg,
            (true, false) => Opcode::SubReg,
            (true, true) => Opcode::SubsReg,
        }
    };
}

fn decode_dp_logical_shift(raw: u32, i: &mut Instruction) {
    let opc = bits(raw, 30, 29);
    let n = bit(raw, 21);
    let shift = bits(raw, 23, 22);
    let shift_amt = bits(raw, 15, 10);
    i.shift_type = shift;
    i.shift_amt = shift_amt;

    i.opcode = match (opc, n) {
        (0b00, 0) => Opcode::AndReg,
        (0b00, 1) => Opcode::BicReg,
        (0b01, 0) => Opcode::OrrReg,
        (0b01, 1) => Opcode::OrnReg,
        (0b10, 0) => Opcode::EorReg,
        (0b10, 1) => Opcode::EonReg,
        (0b11, 0) => Opcode::AndsReg,
        (0b11, 1) => Opcode::BicsReg,
        _ => Opcode::Undefined,
    };
}

fn decode_dp_mul_div(raw: u32, i: &mut Instruction) {
    let _op31 = bits(raw, 31, 29);
    let op1 = bits(raw, 23, 21);
    let ra = bits(raw, 14, 10);
    let o0 = bit(raw, 15);
    i.ra = ra;

    match op1 {
        0b000 => {
            i.opcode = if o0 == 0 { Opcode::Madd } else { Opcode::Msub };
        }
        0b001 => {
            i.opcode = if o0 == 0 {
                Opcode::Smaddl
            } else {
                Opcode::Smsubl
            };
        }
        0b010 => {
            i.opcode = Opcode::Smulh;
        }
        0b101 => {
            i.opcode = if o0 == 0 {
                Opcode::Umaddl
            } else {
                Opcode::Umsubl
            };
        }
        0b110 => {
            i.opcode = Opcode::Umulh;
        }
        _ => {
            if bit(raw, 10) == 1 {
                i.opcode = if bit(raw, 29) == 0 {
                    Opcode::Udiv
                } else {
                    Opcode::Sdiv
                };
            } else {
                let op2 = bits(raw, 12, 10);
                i.opcode = match op2 {
                    0b010 => Opcode::Lsl,
                    0b011 => Opcode::Lsr,
                    0b100 => Opcode::Asr,
                    0b110 => Opcode::Ror,
                    _ => Opcode::Undefined,
                };
            }
        }
    }
}

fn decode_dp_condsel(raw: u32, i: &mut Instruction) {
    let op2 = bits(raw, 11, 10);
    let op = bit(raw, 30);
    let _s = bit(raw, 29);
    i.cond = bits(raw, 15, 12);

    i.opcode = match (op, op2) {
        (0, 0b00) => Opcode::Csel,
        (0, 0b01) => Opcode::Csinc,
        (1, 0b00) => Opcode::Csinv,
        (1, 0b01) => Opcode::Csneg,
        _ => Opcode::Undefined,
    };
}

fn decode_dp_condcmp(raw: u32, i: &mut Instruction) {
    let _o2 = bit(raw, 10);
    let nzcv = bits(raw, 3, 0);
    i.nzcv_imm = nzcv;
    i.cond = bits(raw, 15, 12);
    i.rm = bits(raw, 20, 16);
    let imm5 = bits(raw, 20, 16);
    let use_imm = bit(raw, 11) != 0;
    if use_imm {
        i.imm = imm5 as i64;
    }

    let sub = bit(raw, 30) != 0;
    i.opcode = match (sub, use_imm) {
        (false, false) => Opcode::Ccmn,
        (false, true) => Opcode::Ccmn,
        (true, false) => Opcode::Ccmp,
        (true, true) => Opcode::Ccmp,
    };
}

fn decode_dp_2src(raw: u32, i: &mut Instruction) {
    let op2 = bits(raw, 15, 10);
    i.rn = bits(raw, 9, 5);
    i.rm = bits(raw, 20, 16);
    i.opcode = match op2 {
        0b000010 => Opcode::Udiv,
        0b000011 => Opcode::Sdiv,
        0b001000 => Opcode::Lsl,
        0b001001 => Opcode::Lsr,
        0b001010 => Opcode::Asr,
        0b001011 => Opcode::Ror,
        0b010000 => {
            i.size = 0;
            Opcode::Crc32
        }
        0b010001 => {
            i.size = 1;
            Opcode::Crc32
        }
        0b010010 => {
            i.size = 2;
            Opcode::Crc32
        }
        0b010011 => {
            i.size = 3;
            Opcode::Crc32
        }
        0b010100 => {
            i.size = 0;
            Opcode::Crc32c
        }
        0b010101 => {
            i.size = 1;
            Opcode::Crc32c
        }
        0b010110 => {
            i.size = 2;
            Opcode::Crc32c
        }
        0b010111 => {
            i.size = 3;
            Opcode::Crc32c
        }
        _ => Opcode::Undefined,
    };
}

fn decode_dp_1src(raw: u32, i: &mut Instruction) {
    let opcode2 = bits(raw, 20, 16);
    let op2 = bits(raw, 15, 10);

    i.rn = bits(raw, 9, 5);

    // PAC register-form instructions: opcode2 = 00001, sf=1
    if opcode2 == 0b00001 && i.sf {
        i.opcode = match op2 {
            // PACIA/PACIB/PACDA/PACDB Xd, Xn (context in Rn)
            0b000000 => Opcode::PacReg, // PACIA
            0b000001 => Opcode::PacReg, // PACIB
            0b000010 => Opcode::PacReg, // PACDA
            0b000011 => Opcode::PacReg, // PACDB
            // AUTIA/AUTIB/AUTDA/AUTDB Xd, Xn (context in Rn)
            0b000100 => Opcode::AutReg, // AUTIA
            0b000101 => Opcode::AutReg, // AUTIB
            0b000110 => Opcode::AutReg, // AUTDA
            0b000111 => Opcode::AutReg, // AUTDB
            // PACIZA/PACIZB/PACDZA/PACDZB Xd (zero context, Rn=11111)
            0b001000 => Opcode::PacRegZ, // PACIZA
            0b001001 => Opcode::PacRegZ, // PACIZB
            0b001010 => Opcode::PacRegZ, // PACDZA
            0b001011 => Opcode::PacRegZ, // PACDZB
            // AUTIZA/AUTIZB/AUTDZA/AUTDZB Xd (zero context, Rn=11111)
            0b001100 => Opcode::AutRegZ, // AUTIZA
            0b001101 => Opcode::AutRegZ, // AUTIZB
            0b001110 => Opcode::AutRegZ, // AUTDZA
            0b001111 => Opcode::AutRegZ, // AUTDZB
            // XPACI/XPACD Xd
            0b010000 => Opcode::Xpac, // XPACI
            0b010001 => Opcode::Xpac, // XPACD
            _ => Opcode::Undefined,
        };
        return;
    }

    i.opcode = match op2 {
        0b000000 => Opcode::Rbit,
        0b000001 => Opcode::Rev16,
        0b000010 => {
            if i.sf {
                Opcode::Rev32
            } else {
                Opcode::Rev
            }
        }
        0b000011 => Opcode::Rev,
        0b000100 => Opcode::Clz,
        0b000101 => Opcode::Cls,
        _ => Opcode::Undefined,
    };
}

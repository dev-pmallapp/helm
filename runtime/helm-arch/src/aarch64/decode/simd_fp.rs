//! SIMD / FP (op0 = 0111 / 1111).

use super::{bit, bits};
use crate::aarch64::insn::{Instruction, Opcode};

pub(super) fn decode_simd_fp(raw: u32, i: &mut Instruction) {
    i.rd = bits(raw, 4, 0);
    i.rn = bits(raw, 9, 5);
    i.rm = bits(raw, 20, 16);

    let ptype = bits(raw, 23, 22);
    i.ftype = ptype;

    // Scalar FP data processing: bits[28:24] = 0b11110
    if bits(raw, 28, 24) == 0b11110 {
        decode_fp_data(raw, i);
        return;
    }

    // Advanced SIMD — dispatch by encoding groups
    let q = bit(raw, 30);
    let u = bit(raw, 29);
    i.sf = q != 0;
    i.size = bits(raw, 23, 22);

    // SIMD three-same: bits[28:24]=01110, bit21=1, bit10=1
    if bits(raw, 28, 24) == 0b01110 && bit(raw, 21) == 1 && bit(raw, 10) == 1 {
        let opcode5 = bits(raw, 15, 11);
        i.opcode = match (u, opcode5) {
            (0, 0b10000) => Opcode::SimdAdd,
            (1, 0b10000) => Opcode::SimdSub,
            (0, 0b10011) => Opcode::SimdMul,
            (0, 0b00011) => Opcode::SimdAnd,
            (0, 0b00111) => Opcode::SimdOrr,
            (1, 0b00011) => Opcode::SimdEor,
            (1, 0b00111) => Opcode::SimdBsl,
            (0, 0b01100) => Opcode::SimdSmax,
            (1, 0b01100) => Opcode::SimdUmax,
            (0, 0b01101) => Opcode::SimdSmin,
            (1, 0b01101) => Opcode::SimdUmin,
            (0, 0b10001) => Opcode::SimdCmtst,
            (1, 0b10001) => Opcode::SimdCmeq,
            (0, 0b00110) => Opcode::SimdCmgt,
            (1, 0b00110) => Opcode::SimdCmge,
            (0, 0b01000) => Opcode::SimdAddp,
            (0, 0b10100) => Opcode::SimdSmaxp,
            (1, 0b10100) => Opcode::SimdUmaxp,
            _ => Opcode::SimdOther,
        };
        return;
    }

    // SIMD two-reg misc + across-lanes
    if bits(raw, 28, 24) == 0b01110 && bit(raw, 21) == 1 && bits(raw, 11, 10) == 0b10 {
        let opcode5 = bits(raw, 16, 12);
        i.opcode = match (u, opcode5) {
            (0, 0b01000) => Opcode::SimdCmgt0,
            (0, 0b01001) => Opcode::SimdCmeq0,
            (0, 0b01010) => Opcode::SimdCmlt0,
            (1, 0b01000) => Opcode::SimdCmge0,
            (1, 0b01001) => Opcode::SimdCmle0,
            (1, 0b00101) => Opcode::SimdNot,
            (1, 0b01011) => Opcode::SimdNeg,
            (0, 0b01011) => Opcode::SimdAbs,
            (0, 0b00100) => Opcode::SimdClz,
            (0, 0b00101) => Opcode::SimdCnt,
            (1, 0b00000) => Opcode::SimdRev64,
            (1, 0b01010) => Opcode::SimdUmaxv,
            (1, 0b11010) => Opcode::SimdUminv,
            (0, 0b10010) => Opcode::SimdXtn,
            (0, 0b11011) => Opcode::SimdAddv,
            _ => Opcode::SimdOther,
        };
        return;
    }

    // SIMD copy (DUP, INS, UMOV, SMOV)
    if bits(raw, 28, 24) == 0b01110 && bit(raw, 21) == 0 {
        let imm4 = bits(raw, 14, 11);
        i.imm = bits(raw, 20, 16) as i64;
        i.opcode = match (u, imm4) {
            (0, 0b0000) => Opcode::SimdDup,
            (0, 0b0001) => Opcode::SimdDup,
            (0, 0b0011) => Opcode::SimdIns,
            (0, 0b0101) => Opcode::SimdSmov,
            (0, 0b0111) => Opcode::SimdUmov,
            _ => Opcode::SimdOther,
        };
        return;
    }

    // SIMD modified immediate (MOVI/MVNI/ORR/BIC immediate).
    if bits(raw, 28, 24) == 0b01111 {
        let abc = bits(raw, 18, 16);
        let cmode = bits(raw, 15, 12);
        let defgh = bits(raw, 9, 5);
        i.imm = ((abc << 5) | defgh) as i64;
        i.opcode = if (cmode & 1) == 1 && cmode < 12 {
            Opcode::SimdOrrImm
        } else {
            Opcode::SimdMovi
        };
        return;
    }

    // SIMD shift by immediate
    if bits(raw, 28, 23) == 0b011110 {
        let opcode5 = bits(raw, 15, 11);
        i.imm = bits(raw, 22, 16) as i64;
        i.opcode = match (u, opcode5) {
            (0, 0b00000) => Opcode::SimdSshr,
            (1, 0b00000) => Opcode::SimdUshr,
            (0, 0b01010) => Opcode::SimdShl,
            _ => Opcode::SimdOther,
        };
        return;
    }

    // v8.4 Dot Product (SDOT/UDOT)
    if bits(raw, 28, 24) == 0b01110 && bit(raw, 21) == 0 && bits(raw, 15, 12) == 0b1001 {
        i.ra = bits(raw, 14, 10);
        i.opcode = if u == 0 { Opcode::Sdot } else { Opcode::Udot };
        return;
    }

    // v8.3 FCMA: FCADD
    if bits(raw, 28, 24) == 0b01110 && bit(raw, 21) == 0 && bits(raw, 15, 10) == 0b110010 {
        i.imm = bits(raw, 12, 12) as i64;
        i.opcode = Opcode::Fcadd;
        return;
    }
    // FCMLA
    if bits(raw, 28, 24) == 0b01110
        && bit(raw, 21) == 0
        && bits(raw, 15, 14) == 0b11
        && bits(raw, 11, 10) == 0b01
    {
        i.ra = bits(raw, 14, 10);
        i.imm = bits(raw, 13, 12) as i64;
        i.opcode = Opcode::Fcmla;
        return;
    }

    // Crypto stubs
    if bits(raw, 28, 23) == 0b110010 {
        i.opcode = Opcode::Sha3;
        return;
    }
    if bits(raw, 28, 24) == 0b11001 && bit(raw, 21) == 1 {
        i.opcode = Opcode::Sha512;
        return;
    }
    if bits(raw, 28, 24) == 0b11001 && bit(raw, 21) == 0 && bits(raw, 23, 22) == 0b00 {
        i.opcode = Opcode::Sm3;
        return;
    }
    if bits(raw, 28, 24) == 0b11001 && bit(raw, 21) == 0 && bits(raw, 23, 22) == 0b10 {
        i.opcode = Opcode::Sm4;
        return;
    }

    i.opcode = Opcode::SimdOther;
}

fn decode_fp_data(raw: u32, i: &mut Instruction) {
    let ptype = bits(raw, 23, 22);
    i.sf = bit(raw, 31) != 0;
    i.ftype = ptype;
    i.rd = bits(raw, 4, 0);
    i.rn = bits(raw, 9, 5);
    i.rm = bits(raw, 20, 16);

    // All subgroups below have bits[28:24]=11110 (already checked by caller).
    // bit[21] is always 1 for most scalar FP encodings in this group.

    // FJCVTZS (v8.3 JSCVT): ptype=01, bits[21:16]=011110, bits[15:10]=000000
    if ptype == 0b01 && bits(raw, 21, 16) == 0b011110 && bits(raw, 15, 10) == 0b000000 {
        i.opcode = Opcode::Fjcvtzs;
        return;
    }

    // ── FP<->integer conversions: bit[21]=1, bits[15:10]=000000 ─────────
    // Includes SCVTF, UCVTF, FCVTZS, FCVTZU, FCVTNS, FMOV GPR, etc.
    if bit(raw, 21) == 1 && bits(raw, 15, 10) == 0 {
        let rmode = bits(raw, 20, 19);
        let opcode3 = bits(raw, 18, 16);
        i.opcode = match (rmode, opcode3) {
            (0b00, 0b000) => Opcode::FcvtnsGpr,
            (0b00, 0b001) => Opcode::FcvtnuGpr,
            (0b00, 0b010) => Opcode::ScvtfGpr,
            (0b00, 0b011) => Opcode::UcvtfGpr,
            (0b00, 0b100) => Opcode::FcvtasGpr,
            (0b00, 0b101) => Opcode::FcvtauGpr,
            (0b00, 0b110) | (0b01, 0b110) => Opcode::FmovGpr,
            (0b00, 0b111) | (0b01, 0b111) => Opcode::FmovGpr,
            (0b01, 0b000) => Opcode::FcvtpsGpr,
            (0b01, 0b001) => Opcode::FcvtpuGpr,
            (0b10, 0b000) => Opcode::FcvtmsGpr,
            (0b10, 0b001) => Opcode::FcvtmuGpr,
            (0b11, 0b000) => Opcode::FcvtzsGpr,
            (0b11, 0b001) => Opcode::FcvtzuGpr,
            _ => Opcode::Fcvt,
        };
        return;
    }

    // ── FP data-processing (1 source): bit[21]=1, bits[14:10]=10000 ────
    if bit(raw, 21) == 1 && bits(raw, 14, 10) == 0b10000 {
        let opcode1 = bits(raw, 20, 15);
        i.opcode = match opcode1 {
            0b000000 => Opcode::FmovReg,
            0b000001 => Opcode::Fabs,
            0b000010 => Opcode::Fneg,
            0b000011 => Opcode::Fsqrt,
            // FCVT between FP sizes (S<->D, S<->H, D<->H)
            _ => Opcode::Fcvt,
        };
        return;
    }

    // ── FP compare: bit[21]=1, bits[15:14]=00, bits[13:10]=1000/1001 ───
    if bit(raw, 21) == 1 && bits(raw, 15, 14) == 0 {
        match bits(raw, 13, 10) {
            0b1000 => { i.opcode = Opcode::Fcmp; return; }
            0b1001 => { i.opcode = Opcode::Fcmpe; return; }
            _ => {}
        }
    }

    // ── ADDP (scalar): bit[30]=1, ptype=3, bits[15:10]=101110 ──────────
    if bit(raw, 30) == 1 && ptype == 3 && bits(raw, 15, 10) == 0b101110 {
        i.opcode = Opcode::ScalarAddp;
        return;
    }

    // ── FMOV (immediate): bit[21]=1, bits[12:10]=100 ──────────────────
    if bit(raw, 21) == 1 && bits(raw, 12, 10) == 0b100 {
        let imm8 = bits(raw, 20, 13);
        i.imm = imm8 as i64;
        i.opcode = Opcode::FmovImm;
        return;
    }

    // ── FP conditional compare: bit[21]=1, bits[11:10]=01 ─────────────
    if bit(raw, 21) == 1 && bits(raw, 11, 10) == 0b01 {
        i.cond = bits(raw, 15, 12);
        i.nzcv_imm = bits(raw, 3, 0);
        i.opcode = if bit(raw, 4) == 0 { Opcode::Fccmp } else { Opcode::Fccmpe };
        return;
    }

    // ── FP conditional select: bit[21]=1, bits[11:10]=11 ──────────────
    if bit(raw, 21) == 1 && bits(raw, 11, 10) == 0b11 {
        i.cond = bits(raw, 15, 12);
        i.opcode = Opcode::Fsel;
        return;
    }

    // ── FP data-processing (2 source): bit[21]=1, bits[11:10]=10 ──────
    if bit(raw, 21) == 1 && bits(raw, 11, 10) == 0b10 {
        i.fp_rounding = ptype;
        i.opcode = match bits(raw, 15, 12) {
            0b0000 => Opcode::Fmul,
            0b0001 => Opcode::Fdiv,
            0b0010 => Opcode::Fadd,
            0b0011 => Opcode::Fsub,
            0b0100 => Opcode::Fmax,
            0b0101 => Opcode::Fmin,
            0b0110 => Opcode::Fmaxnm,
            0b0111 => Opcode::Fminnm,
            0b1000 => Opcode::Fnmul,
            _ => Opcode::Fcvt,
        };
        return;
    }

    // Fallback
    i.opcode = Opcode::Fcvt;
}

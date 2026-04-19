//! Load/Store (op0 = 0x00, 0x10, 0x11, 0x01).

use super::{bit, bits, sext};
use crate::aarch64::insn::{Instruction, Opcode};

pub(super) fn decode_ldst(raw: u32, i: &mut Instruction) {
    let size = bits(raw, 31, 30);
    let v = bit(raw, 26); // FP/SIMD?
    let opc = bits(raw, 23, 22);
    i.size = size;
    i.rd = bits(raw, 4, 0); // Rt
    i.rn = bits(raw, 9, 5); // Rn (base)

    // ── Load literal (PC-relative): bits[29:27]=011, bit24=0 ──────────────
    if bits(raw, 29, 27) == 0b011 && bit(raw, 24) == 0 {
        let imm19 = bits(raw, 23, 5);
        i.imm = sext((imm19 << 2) as u64, 21);
        i.sf = size == 1; // 0b01 → 64-bit Xt
        if v == 1 {
            i.opcode = Opcode::LdrSimd; // FP/SIMD literal
            i.ftype = size; // 0=S,1=D,2=Q
        } else {
            i.opcode = match size {
                0b00 | 0b01 => Opcode::LdrLit, // LDR Wt/Xt, label
                0b10 => Opcode::LdrswLit,      // LDRSW Xt, label
                0b11 => Opcode::Prfm,          // PRFM literal
                _ => Opcode::Undefined,
            };
        }
        return;
    }

    // ── PRFM (prefetch memory): size=11, V=0, opc=10, unsigned-offset ─────
    if size == 0b11 && v == 0 && opc == 0b10 && bit(raw, 24) == 1 {
        i.opcode = Opcode::Prfm;
        return;
    }

    // ── LDP / STP: bits[29:27] = 101 ──────────────────────────────────────
    if bits(raw, 29, 27) == 0b101 {
        decode_ldst_pair(raw, i, v);
        return;
    }

    // ── Exclusive / ordered: bits[29:24] = 0b001000 ──────────────────────
    if bits(raw, 29, 24) == 0b001000 {
        decode_ldst_exclusive(raw, i);
        return;
    }

    // ── LSE atomics: bits[29:24]=111000, bit21=1, bits[11:10]=00 ──────────
    if bits(raw, 29, 24) == 0b111000 && bit(raw, 21) == 1 && bits(raw, 11, 10) == 0b00 {
        decode_ldst_atomic(raw, i);
        return;
    }

    // ── SIMD/FP load/store (V=1) ──────────────────────────────────────────
    if v == 1 {
        // bit[29]=0 with V=1 → AdvSIMD structure ld/st (LD1/ST1 etc.)
        // bit[29]=1 with V=1 → scalar FP/SIMD ld/st (LDR/STR Bn/Hn/Sn/Dn/Qn)
        if bit(raw, 29) == 0 {
            decode_ldst_simd_struct(raw, i);
        } else {
            decode_ldst_simd(raw, i);
        }
        return;
    }

    // ── PRFM (prefetch memory): pre/post/unscaled/register-offset forms ────
    if size == 0b11 && v == 0 && opc == 0b10 && bit(raw, 24) == 0 {
        i.opcode = Opcode::Prfm;
        return;
    }

    // ── Register offset: bits[24] = 0, bit[21] = 1, bits[11:10]=10 ───────
    if bit(raw, 24) == 0 && bit(raw, 21) == 1 && bits(raw, 11, 10) == 0b10 {
        decode_ldst_reg_offset(raw, i);
        return;
    }

    // ── RCPC2 (v8.4): LDAPUR / STLUR — load/store-release unscaled imm ────
    if bits(raw, 29, 24) == 0b011001 && bit(raw, 21) == 0 && bits(raw, 11, 10) == 0b00 {
        // LDAPUR family
        let imm9 = bits(raw, 20, 12);
        i.imm = sext(imm9 as u64, 9);
        i.sf = size == 3;
        i.opcode = match size {
            0 => Opcode::LdapurB,
            1 => Opcode::LdapurH,
            _ => Opcode::Ldapur,
        };
        return;
    }
    if bits(raw, 29, 24) == 0b011000 && bit(raw, 21) == 0 && bits(raw, 11, 10) == 0b00 {
        // STLUR family
        let imm9 = bits(raw, 20, 12);
        i.imm = sext(imm9 as u64, 9);
        i.sf = size == 3;
        i.opcode = match size {
            0 => Opcode::StlurB,
            1 => Opcode::StlurH,
            _ => Opcode::Stlur,
        };
        return;
    }

    // ── Unscaled immediate (LDUR/STUR): bit24=0, bit21=0, bits[11:10]=00 ──
    if bit(raw, 24) == 0 && bit(raw, 21) == 0 && bits(raw, 11, 10) == 0b00 {
        let imm9 = bits(raw, 20, 12);
        i.imm = sext(imm9 as u64, 9);
        let store = opc & 1 == 0;
        let signed = opc & 2 != 0;
        i.signed_load = signed;
        decode_ldst_size_opcode(size, store, signed, true, i);
        set_ldst_sf(i, size, opc, signed);
        return;
    }

    // ── Pre/post-index: bits[24] = 0 ──────────────────────────────────────
    if bit(raw, 24) == 0 {
        let imm9 = bits(raw, 20, 12);
        i.imm = sext(imm9 as u64, 9);
        i.post_index = bit(raw, 11) == 0;
        i.pre_index = bit(raw, 11) != 0;
        let store = opc & 1 == 0;
        let signed = opc & 2 != 0;
        i.signed_load = signed;
        decode_ldst_size_opcode(size, store, signed, false, i);
        set_ldst_sf(i, size, opc, signed);
        return;
    }

    // ── Unsigned offset (most common): bits[24] = 1 ───────────────────────
    let imm12 = bits(raw, 21, 10) as u64;
    i.imm = (imm12 << size) as i64; // scaled by access size
    let store = bit(raw, 22) == 0;
    let signed = bit(raw, 23) != 0;
    i.signed_load = signed;
    decode_ldst_size_opcode(size, store, signed, false, i);
    set_ldst_sf(i, size, opc, signed);
}

/// Set `insn.sf` for load/store based on access size and opc.
fn set_ldst_sf(i: &mut Instruction, size: u32, opc: u32, signed: bool) {
    if signed {
        i.sf = (opc & 1) == 0;
    } else {
        i.sf = size == 3;
    }
}

fn decode_ldst_size_opcode(
    size: u32,
    store: bool,
    signed: bool,
    unscaled: bool,
    i: &mut Instruction,
) {
    i.opcode = match (size, store, signed, unscaled) {
        // Byte
        (0, true, false, false) => Opcode::Strb,
        (0, true, false, true) => Opcode::Sturb,
        (0, false, false, false) => Opcode::Ldrb,
        (0, false, false, true) => Opcode::Ldurb,
        (0, false, true, false) => Opcode::Ldrsb,
        (0, false, true, true) => Opcode::Ldursb,
        (0, true, true, _) => Opcode::Ldrsb,
        // Halfword
        (1, true, false, false) => Opcode::Strh,
        (1, true, false, true) => Opcode::Sturh,
        (1, false, false, false) => Opcode::Ldrh,
        (1, false, false, true) => Opcode::Ldurh,
        (1, false, true, false) => Opcode::Ldrsh,
        (1, false, true, true) => Opcode::Ldursh,
        (1, true, true, _) => Opcode::Ldrsh,
        // Word
        (2, true, false, false) => Opcode::Str,
        (2, true, false, true) => Opcode::Stur,
        (2, false, false, false) => Opcode::Ldr,
        (2, false, false, true) => Opcode::Ldur,
        (2, false, true, false) => Opcode::Ldrsw,
        (2, false, true, true) => Opcode::Ldursw,
        (2, true, true, _) => Opcode::Ldrsw,
        // Doubleword
        (3, true, _, _) => {
            if unscaled {
                Opcode::Stur
            } else {
                Opcode::Str
            }
        }
        (3, false, _, _) => {
            if unscaled {
                Opcode::Ldur
            } else {
                Opcode::Ldr
            }
        }
        _ => Opcode::Undefined,
    };
}

fn decode_ldst_reg_offset(raw: u32, i: &mut Instruction) {
    let size = bits(raw, 31, 30);
    let opc = bits(raw, 23, 22);
    let rm = bits(raw, 20, 16);
    let option = bits(raw, 15, 13);
    let s = bit(raw, 12);
    i.rm = rm;
    i.extend_type = option;
    i.extend_amt = if s != 0 { size } else { 0 };
    i.size = size;
    let store = opc & 1 == 0;
    let signed = opc & 2 != 0;
    decode_ldst_size_opcode(size, store, signed, false, i);
    set_ldst_sf(i, size, opc, signed);
}

fn decode_ldst_pair(raw: u32, i: &mut Instruction, v: u32) {
    let opc = bits(raw, 31, 30);
    let l = bit(raw, 22);
    let imm7 = bits(raw, 21, 15);
    let rt2 = bits(raw, 14, 10);
    let rn = bits(raw, 9, 5);
    let rt = bits(raw, 4, 0);

    i.rd = rt;
    i.pair_second = rt2;
    i.rn = rn;

    if v == 1 {
        let scale = match opc {
            0b00 => 2u32,
            0b01 => 3,
            0b10 => 4,
            _ => 2,
        };
        i.imm = sext(imm7 as u64, 7) << scale;
        i.ftype = opc;
        i.sf = opc >= 1;
    } else {
        let scale = if opc == 0b10 { 3u32 } else { 2u32 };
        i.imm = sext(imm7 as u64, 7) << scale;
        i.sf = opc == 0b10;
        i.signed_load = opc == 0b01;
    }

    let pre = bits(raw, 24, 23) == 0b11;
    let post = bits(raw, 24, 23) == 0b01;
    i.pre_index = pre;
    i.post_index = post;

    if v == 1 {
        i.opcode = if l != 0 {
            Opcode::LdpSimd
        } else {
            Opcode::StpSimd
        };
    } else {
        i.opcode = if l != 0 { Opcode::Ldp } else { Opcode::Stp };
    }
}

/// AdvSIMD load/store multiple/single structures.
/// Encoding: bit[29]=0, bit[26]=1 (V=1).
///   bit[24]=0 in sub-group → multiple structures
///   bit[24]=1 in sub-group → single structure (element)
fn decode_ldst_simd_struct(raw: u32, i: &mut Instruction) {
    let q = bit(raw, 30);
    let l = bit(raw, 22); // 0 = store, 1 = load
    let opcode3 = bits(raw, 15, 13);
    let s = bit(raw, 12);
    let sz = bits(raw, 11, 10);

    // bits[29:24] sub-group:
    //   0_01100 → multiple, no post-index
    //   0_01101 → single, no post-index
    //   0_11100 → multiple, post-index
    //   0_11101 → single, post-index
    let bits_29_24 = bits(raw, 29, 24);
    let is_single_elem = (bits_29_24 & 1) != 0;

    if !is_single_elem {
        // Multiple-structure form: existing stub opcodes.
        i.opcode = if l != 0 {
            Opcode::SimdLd1
        } else {
            Opcode::SimdSt1
        };
        return;
    }

    // Single-structure: determine element size and lane index.
    // Encode in imm: (esize_log2 << 8) | (index << 4)
    let (esize_log2, index) = match opcode3 {
        0b000 => (0u32, (q << 3) | (s << 2) | sz),     // B
        0b010 => (1, (q << 2) | (s << 1) | (sz >> 1)), // H
        0b100 if s == 0 && (sz & 1) == 0 => (2, (q << 1) | (sz >> 1)), // S
        0b100 if s == 0 && sz == 0b01 => (3, q),       // D
        _ => {
            // 3-register or unsupported → stub
            i.opcode = if l != 0 {
                Opcode::SimdLd1
            } else {
                Opcode::SimdSt1
            };
            return;
        }
    };

    i.sf = q != 0;
    i.imm = ((esize_log2 as i64) << 8) | ((index as i64) << 4);
    i.rn = bits(raw, 9, 5);
    i.rd = bits(raw, 4, 0);
    i.opcode = if l != 0 {
        Opcode::SimdLd1
    } else {
        Opcode::SimdSt1
    };
}

fn decode_ldst_simd(raw: u32, i: &mut Instruction) {
    let size = bits(raw, 31, 30);
    let opc = bits(raw, 23, 22);
    i.rd = bits(raw, 4, 0);
    i.rn = bits(raw, 9, 5);
    let is_128 = size == 0b00 && (opc & 0b10) != 0;
    i.ftype = if is_128 { 4 } else { size };

    let is_load = (opc & 1) != 0;

    if bit(raw, 24) == 1 {
        let imm12 = bits(raw, 21, 10) as u64;
        let scale = if is_128 { 4u32 } else { size };
        i.imm = (imm12 << scale) as i64;
        i.opcode = if is_load {
            Opcode::LdrSimd
        } else {
            Opcode::StrSimd
        };
        return;
    }

    if bit(raw, 24) == 0 && bit(raw, 21) == 1 && bits(raw, 11, 10) == 0b10 {
        i.rm = bits(raw, 20, 16);
        let option = bits(raw, 15, 13);
        let s_bit = bit(raw, 12);
        i.extend_type = option;
        i.extend_amt = if s_bit != 0 {
            if is_128 {
                4
            } else {
                size
            }
        } else {
            0
        };
        i.opcode = if is_load {
            Opcode::LdrSimd
        } else {
            Opcode::StrSimd
        };
        i.imm = i64::MIN;
        return;
    }

    if bits(raw, 11, 10) == 0b00 && bit(raw, 21) == 0 {
        let imm9 = bits(raw, 20, 12);
        i.imm = sext(imm9 as u64, 9);
        i.opcode = if is_load {
            Opcode::LdurSimd
        } else {
            Opcode::SturSimd
        };
        return;
    }

    if bit(raw, 24) == 0 {
        let imm9 = bits(raw, 20, 12);
        i.imm = sext(imm9 as u64, 9);
        i.pre_index = bit(raw, 11) != 0;
        i.post_index = bit(raw, 11) == 0;
        i.opcode = if is_load {
            Opcode::LdrSimd
        } else {
            Opcode::StrSimd
        };
        return;
    }

    i.opcode = if is_load {
        Opcode::LdrSimd
    } else {
        Opcode::StrSimd
    };
}

fn decode_ldst_atomic(raw: u32, i: &mut Instruction) {
    let size = bits(raw, 31, 30);
    let a = bit(raw, 23);
    let r = bit(raw, 22);
    let rs = bits(raw, 20, 16);
    let o3 = bit(raw, 15);
    let opc = bits(raw, 14, 12);
    let rn = bits(raw, 9, 5);
    let rt = bits(raw, 4, 0);

    i.rd = rt;
    i.rn = rn;
    i.rm = rs;
    i.size = size;
    i.sf = size == 3;
    i.acquire = a != 0;
    i.release = r != 0;

    if o3 == 0 && opc == 0b100 && rs == 31 && r == 0 {
        i.opcode = match size {
            0 => Opcode::Ldaprb,
            1 => Opcode::Ldaprh,
            _ => Opcode::Ldapr,
        };
        return;
    }

    if o3 == 1 {
        i.opcode = Opcode::Swp;
        return;
    }

    i.opcode = match opc {
        0b000 => Opcode::Ldadd,
        0b001 => Opcode::Ldclr,
        0b010 => Opcode::Ldeor,
        0b011 => Opcode::Ldset,
        0b100 => Opcode::LdSmax,
        0b101 => Opcode::LdSmin,
        0b110 => Opcode::LdUmax,
        0b111 => Opcode::LdUmin,
        _ => Opcode::Undefined,
    };
}

fn decode_ldst_exclusive(raw: u32, i: &mut Instruction) {
    let o2 = bit(raw, 23);
    let l = bit(raw, 22);
    let o1 = bit(raw, 21);
    let rs = bits(raw, 20, 16);
    let o0 = bit(raw, 15);
    let rt2 = bits(raw, 14, 10);
    let rn = bits(raw, 9, 5);
    let rt = bits(raw, 4, 0);
    i.rd = rt;
    i.rn = rn;
    i.rm = rs;
    i.pair_second = rt2;
    i.sf = bit(raw, 30) != 0;
    i.size = bits(raw, 31, 30);

    if o2 == 1 && o1 == 0 && o0 == 1 && rs == 31 && rt2 == 31 {
        i.acquire = l == 1;
        i.release = l == 0;
        i.opcode = if l == 1 { Opcode::Ldar } else { Opcode::Stlr };
        return;
    }

    if o2 == 1 {
        i.acquire = o1 != 0;
        i.release = o0 != 0;
        if rt2 == 31 {
            i.opcode = Opcode::Cas;
        } else {
            i.opcode = Opcode::Casp;
        }
        return;
    }

    if raw & 0xFFFFF0FF == 0xD503305F {
        i.opcode = Opcode::Clrex;
        return;
    }

    i.acquire = o0 != 0;
    i.release = o0 != 0;
    i.opcode = match (o1, l, o0) {
        (0, 0, 0) => Opcode::Stxr,
        (0, 0, 1) => Opcode::Stlxr,
        (0, 1, 0) => Opcode::Ldxr,
        (0, 1, 1) => Opcode::Ldaxr,
        (1, 0, 0) => Opcode::Stxp,
        (1, 0, 1) => Opcode::Stlxp,
        (1, 1, 0) => Opcode::Ldxp,
        (1, 1, 1) => Opcode::Ldaxp,
        _ => unreachable!(),
    };
}

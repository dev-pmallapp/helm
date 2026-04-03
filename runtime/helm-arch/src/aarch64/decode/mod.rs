//! AArch64 32-bit fixed-width instruction decoder.
//!
//! Top-level dispatch on bits [28:25] (op0), then per-group decoders.
//! Returns an [`Instruction`] struct with all relevant fields populated.
//!
//! Reference: ARM DDI 0487 (AArch64 Architecture Reference Manual), C4.

mod branch_sys;
mod dp_imm;
mod dp_reg;
mod ldst;
mod simd_fp;

use super::insn::{Instruction, Opcode};
use crate::DecodeError;

// ── Bit helpers ───────────────────────────────────────────────────────────────

#[inline(always)]
pub(super) fn bit(v: u32, pos: u32) -> u32 {
    (v >> pos) & 1
}
#[inline(always)]
pub(super) fn bits(v: u32, hi: u32, lo: u32) -> u32 {
    (v >> lo) & ((1 << (hi - lo + 1)) - 1)
}
#[inline(always)]
pub(super) fn sext(v: u64, bits_wide: u32) -> i64 {
    let shift = 64 - bits_wide;
    ((v as i64) << shift) >> shift
}

pub fn decode(raw: u32, pc: u64) -> Result<Instruction, DecodeError> {
    let op0 = bits(raw, 28, 25);
    let mut insn = Instruction::zeroed();
    insn.raw = raw;
    insn.pc = pc;

    match op0 {
        0b1000 | 0b1001 => dp_imm::decode_dp_imm(raw, &mut insn),
        0b1010 | 0b1011 => branch_sys::decode_branch_sys(raw, &mut insn),
        0b0100 | 0b0110 | 0b1100 | 0b1110 => ldst::decode_ldst(raw, &mut insn),
        0b0101 | 0b1101 => dp_reg::decode_dp_reg(raw, &mut insn),
        0b0111 | 0b1111 => simd_fp::decode_simd_fp(raw, &mut insn),
        _ => {
            insn.opcode = Opcode::Undefined;
        }
    }

    if insn.opcode == Opcode::Undefined {
        return Err(DecodeError::Unknown { raw, pc });
    }
    Ok(insn)
}

// ── Bit-mask decode helper (N:immr:imms → mask) ───────────────────────────────

pub(super) fn decode_bit_mask(n: bool, imms: u32, immr: u32, sf: bool) -> Option<u64> {
    // ARM ARM C.6 / DecodeBitMasks.
    // len = highest set bit of (N:NOT(imms)[5:0]), must be >= 1.
    let combined = if n { 0x40u32 } else { 0 } | ((!imms) & 0x3F);
    if combined == 0 {
        return None; // len < 1 ⟹ undefined
    }
    let len = 31 - combined.leading_zeros(); // highest set bit position
    if len < 1 {
        return None;
    }
    let levels = (1u32 << len) - 1;
    let s = imms & levels;
    let r = immr & levels;
    let esize = 1u64 << len;
    // welem = ZeroExtend(Ones(S+1), esize)
    let welem: u64 = if s + 1 >= 64 {
        u64::MAX
    } else {
        (1u64 << (s + 1)) - 1
    };
    let emask: u64 = if esize >= 64 {
        u64::MAX
    } else {
        (1u64 << esize) - 1
    };
    // ROR(welem, R) within esize
    let rotated = if r == 0 {
        welem
    } else if esize >= 64 {
        welem.rotate_right(r)
    } else {
        ((welem >> r) | (welem << (esize as u32 - r))) & emask
    };
    // Replicate element across 64 bits
    let rsz: u64 = if sf { 64 } else { 32 };
    let mut mask = 0u64;
    let mut pos = 0u64;
    while pos < rsz {
        mask |= rotated << pos;
        pos += esize;
    }
    Some(mask)
}

//! AArch64 execute — simd group.
#![allow(unused_imports, unused_variables)]
use super::helpers::*;
use crate::aarch64::arch_state::Aarch64ArchState;
use crate::aarch64::exception;
use crate::aarch64::insn::{Instruction, Opcode};
use helm_core::{AccessType, HartException, MemFault, MemInterface};
#[allow(unused_imports)]
use helm_diag::{sim_stub, sim_warn};

#[allow(clippy::too_many_lines)]
pub(super) fn exec_simd(
    insn: &Instruction,
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
) -> Result<bool, HartException> {
    use Opcode::*;
    let pc_written = false;
    match insn.opcode {
        // ── SIMD DUP (replicate scalar to vector) ─────────────────────────
        SimdDup => {
            // insn.imm holds imm5 which encodes element size + index
            let imm5 = insn.imm as u32;
            if imm5 & 1 != 0 {
                // Byte: DUP Vd.xB, Wn — replicate lowest byte
                let b = a.read_x(insn.rn) as u8;
                let mut val = 0u128;
                for i in 0..16 {
                    val |= (b as u128) << (i * 8);
                }
                a.v[insn.rd as usize] = if insn.sf {
                    val
                } else {
                    val & ((1u128 << 64) - 1)
                };
            } else if imm5 & 2 != 0 {
                // Halfword
                let h = a.read_x(insn.rn) as u16;
                let mut val = 0u128;
                for i in 0..8 {
                    val |= (h as u128) << (i * 16);
                }
                a.v[insn.rd as usize] = if insn.sf {
                    val
                } else {
                    val & ((1u128 << 64) - 1)
                };
            } else if imm5 & 4 != 0 {
                // Word
                let w = a.read_x(insn.rn) as u32;
                let mut val = 0u128;
                for i in 0..4 {
                    val |= (w as u128) << (i * 32);
                }
                a.v[insn.rd as usize] = if insn.sf {
                    val
                } else {
                    val & ((1u128 << 64) - 1)
                };
            } else if imm5 & 8 != 0 {
                // Doubleword
                let d = a.read_x(insn.rn);
                let val = (d as u128) | ((d as u128) << 64);
                a.v[insn.rd as usize] = if insn.sf { val } else { d as u128 };
            }
        }

        // ── SIMD UMOV / SMOV (move element to GPR) ──────────────────────
        SimdUmov => {
            let imm5 = insn.imm as u32;
            let v = a.v[insn.rn as usize];
            if imm5 & 8 != 0 {
                // 64-bit element
                let idx = (imm5 >> 4) & 1;
                let val = if idx == 0 { v as u64 } else { (v >> 64) as u64 };
                a.write_x(insn.rd, val);
            } else if imm5 & 4 != 0 {
                let idx = (imm5 >> 3) & 3;
                let val = ((v >> (idx * 32)) & 0xFFFF_FFFF) as u64;
                a.write_x(insn.rd, val);
            } else if imm5 & 2 != 0 {
                let idx = (imm5 >> 2) & 7;
                let val = ((v >> (idx * 16)) & 0xFFFF) as u64;
                a.write_x(insn.rd, val);
            } else if imm5 & 1 != 0 {
                let idx = (imm5 >> 1) & 15;
                let val = ((v >> (idx * 8)) & 0xFF) as u64;
                a.write_x(insn.rd, val);
            }
        }
        SimdSmov => {
            let imm5 = insn.imm as u32;
            let v = a.v[insn.rn as usize];
            if imm5 & 4 != 0 {
                let idx = (imm5 >> 3) & 3;
                let val = ((v >> (idx * 32)) & 0xFFFF_FFFF) as u64;
                a.write_x(insn.rd, val as i32 as i64 as u64);
            } else if imm5 & 2 != 0 {
                let idx = (imm5 >> 2) & 7;
                let val = ((v >> (idx * 16)) & 0xFFFF) as u64;
                a.write_x(insn.rd, val as i16 as i64 as u64);
            } else if imm5 & 1 != 0 {
                let idx = (imm5 >> 1) & 15;
                let val = ((v >> (idx * 8)) & 0xFF) as u64;
                a.write_x(insn.rd, val as i8 as i64 as u64);
            }
        }

        // ── SIMD INS (insert element from GPR or element) ───────────────
        SimdIns => {
            let imm5 = insn.imm as u32;
            let val = a.read_x(insn.rn);
            let v = &mut a.v[insn.rd as usize];
            if imm5 & 1 != 0 {
                let idx = (imm5 >> 1) & 15;
                let mask = !(0xFFu128 << (idx * 8));
                *v = (*v & mask) | ((val as u128 & 0xFF) << (idx * 8));
            } else if imm5 & 2 != 0 {
                let idx = (imm5 >> 2) & 7;
                let mask = !(0xFFFFu128 << (idx * 16));
                *v = (*v & mask) | ((val as u128 & 0xFFFF) << (idx * 16));
            } else if imm5 & 4 != 0 {
                let idx = (imm5 >> 3) & 3;
                let mask = !(0xFFFF_FFFFu128 << (idx * 32));
                *v = (*v & mask) | ((val as u128 & 0xFFFF_FFFF) << (idx * 32));
            } else if imm5 & 8 != 0 {
                let idx = (imm5 >> 4) & 1;
                let mask = !(0xFFFF_FFFF_FFFF_FFFFu128 << (idx * 64));
                *v = (*v & mask) | ((val as u128) << (idx * 64));
            }
        }

        // ── SIMD MOVI (move immediate to vector) ────────────────────────
        SimdMovi => {
            // Simplified: set all bytes to the immediate value
            let imm8 = insn.imm as u8;
            let mut val = 0u128;
            for i in 0..16 {
                val |= (imm8 as u128) << (i * 8);
            }
            a.v[insn.rd as usize] = if insn.sf {
                val
            } else {
                val & ((1u128 << 64) - 1)
            };
        }

        // ── SIMD integer lane-wise arithmetic ────────────────────────────
        SimdAdd | SimdSub | SimdMul => {
            let bytes = if insn.sf { 16usize } else { 8 };
            let esize = 1usize << insn.size;
            let ebits = esize * 8;
            let emask = if ebits == 128 {
                u128::MAX
            } else {
                (1u128 << ebits) - 1
            };
            let vn = a.v[insn.rn as usize];
            let vm = a.v[insn.rm as usize];
            let mut result = 0u128;

            for lane in 0..(bytes / esize) {
                let shift = lane * ebits;
                let lhs = (vn >> shift) & emask;
                let rhs = (vm >> shift) & emask;
                let lane_val = match insn.opcode {
                    SimdAdd => lhs.wrapping_add(rhs) & emask,
                    SimdSub => lhs.wrapping_sub(rhs) & emask,
                    SimdMul => lhs.wrapping_mul(rhs) & emask,
                    _ => unreachable!(),
                };
                result |= lane_val << shift;
            }

            a.v[insn.rd as usize] = result;
        }

        // ── SIMD compare against zero ────────────────────────────────────
        SimdCmgt0 | SimdCmeq0 | SimdCmlt0 | SimdCmge0 | SimdCmle0 => {
            let bytes = if insn.sf { 16usize } else { 8 };
            let esize = 1usize << insn.size;
            let ebits = esize * 8;
            let emask = if ebits == 128 {
                u128::MAX
            } else {
                (1u128 << ebits) - 1
            };
            let src = a.v[insn.rn as usize];
            let mut result = 0u128;

            for lane in 0..(bytes / esize) {
                let shift = lane * ebits;
                let ea = (src >> shift) & emask;
                let sign = ea >> (ebits - 1);
                let lane_val = match insn.opcode {
                    SimdCmgt0 => sign == 0 && ea != 0,
                    SimdCmeq0 => ea == 0,
                    SimdCmlt0 => sign != 0,
                    SimdCmge0 => sign == 0,
                    SimdCmle0 => sign != 0 || ea == 0,
                    _ => unreachable!(),
                };
                if lane_val {
                    result |= emask << shift;
                }
            }

            a.v[insn.rd as usize] = result;
        }

        // ── SIMD CMEQ (bytewise compare equal) ────────────────────────────
        SimdCmeq => {
            let vn = a.v[insn.rn as usize];
            let vm = a.v[insn.rm as usize];
            let bytes = if insn.sf { 16 } else { 8 }; // Q bit
            let mut result = 0u128;
            for i in 0..bytes {
                let bn = ((vn >> (i * 8)) & 0xFF) as u8;
                let bm = ((vm >> (i * 8)) & 0xFF) as u8;
                if bn == bm {
                    result |= 0xFFu128 << (i * 8);
                }
            }
            a.v[insn.rd as usize] = result;
        }

        // ── SIMD UMAXV (unsigned max across vector bytes) ────────────────
        SimdUmaxv => {
            let vn = a.v[insn.rn as usize];
            let bytes = if insn.sf { 16 } else { 8 }; // Q bit
            let mut max_val = 0u8;
            for i in 0..bytes {
                let b = ((vn >> (i * 8)) & 0xFF) as u8;
                if b > max_val {
                    max_val = b;
                }
            }
            // Result goes into the lowest element of Vd, rest zeroed
            a.v[insn.rd as usize] = max_val as u128;
        }

        // ── SIMD UMINV (unsigned min across vector bytes) ────────────────
        SimdUminv => {
            let vn = a.v[insn.rn as usize];
            let bytes = if insn.sf { 16 } else { 8 };
            let mut min_val = 0xFFu8;
            for i in 0..bytes {
                let b = ((vn >> (i * 8)) & 0xFF) as u8;
                if b < min_val {
                    min_val = b;
                }
            }
            a.v[insn.rd as usize] = min_val as u128;
        }

        // ── SIMD CMGT/CMGE/CMHI/CMHS (bytewise compare) ─────────────────
        SimdCmgt => {
            let vn = a.v[insn.rn as usize];
            let vm = a.v[insn.rm as usize];
            let bytes = if insn.sf { 16 } else { 8 };
            let mut result = 0u128;
            for i in 0..bytes {
                let bn = ((vn >> (i * 8)) & 0xFF) as i8;
                let bm = ((vm >> (i * 8)) & 0xFF) as i8;
                if bn > bm {
                    result |= 0xFFu128 << (i * 8);
                }
            }
            a.v[insn.rd as usize] = result;
        }
        SimdCmge => {
            let vn = a.v[insn.rn as usize];
            let vm = a.v[insn.rm as usize];
            let bytes = if insn.sf { 16 } else { 8 };
            let mut result = 0u128;
            for i in 0..bytes {
                let bn = ((vn >> (i * 8)) & 0xFF) as i8;
                let bm = ((vm >> (i * 8)) & 0xFF) as i8;
                if bn >= bm {
                    result |= 0xFFu128 << (i * 8);
                }
            }
            a.v[insn.rd as usize] = result;
        }
        SimdCmhi => {
            let vn = a.v[insn.rn as usize];
            let vm = a.v[insn.rm as usize];
            let bytes = if insn.sf { 16 } else { 8 };
            let mut result = 0u128;
            for i in 0..bytes {
                let bn = ((vn >> (i * 8)) & 0xFF) as u8;
                let bm = ((vm >> (i * 8)) & 0xFF) as u8;
                if bn > bm {
                    result |= 0xFFu128 << (i * 8);
                }
            }
            a.v[insn.rd as usize] = result;
        }
        SimdCmhs => {
            let vn = a.v[insn.rn as usize];
            let vm = a.v[insn.rm as usize];
            let bytes = if insn.sf { 16 } else { 8 };
            let mut result = 0u128;
            for i in 0..bytes {
                let bn = ((vn >> (i * 8)) & 0xFF) as u8;
                let bm = ((vm >> (i * 8)) & 0xFF) as u8;
                if bn >= bm {
                    result |= 0xFFu128 << (i * 8);
                }
            }
            a.v[insn.rd as usize] = result;
        }

        // ── SIMD AND/ORR/EOR/BIC (bitwise) ──────────────────────────────
        SimdAnd => {
            a.v[insn.rd as usize] = a.v[insn.rn as usize] & a.v[insn.rm as usize];
            if !insn.sf {
                a.v[insn.rd as usize] &= (1u128 << 64) - 1;
            }
        }
        SimdOrr => {
            a.v[insn.rd as usize] = a.v[insn.rn as usize] | a.v[insn.rm as usize];
            if !insn.sf {
                a.v[insn.rd as usize] &= (1u128 << 64) - 1;
            }
        }
        SimdEor => {
            a.v[insn.rd as usize] = a.v[insn.rn as usize] ^ a.v[insn.rm as usize];
            if !insn.sf {
                a.v[insn.rd as usize] &= (1u128 << 64) - 1;
            }
        }
        SimdBic => {
            a.v[insn.rd as usize] = a.v[insn.rn as usize] & !a.v[insn.rm as usize];
            if !insn.sf {
                a.v[insn.rd as usize] &= (1u128 << 64) - 1;
            }
        }

        // ── SIMD NOT (bitwise) ──────────────────────────────────────────
        SimdNot => {
            let mask = if insn.sf {
                u128::MAX
            } else {
                (1u128 << 64) - 1
            };
            a.v[insn.rd as usize] = !a.v[insn.rn as usize] & mask;
        }

        // ── SIMD signed unary arithmetic ─────────────────────────────────
        SimdAbs | SimdNeg => {
            let bytes = if insn.sf { 16usize } else { 8 };
            let esize = 1usize << insn.size;
            let ebits = esize * 8;
            let emask = if ebits == 128 {
                u128::MAX
            } else {
                (1u128 << ebits) - 1
            };
            let src = a.v[insn.rn as usize];
            let mut result = 0u128;

            for lane in 0..(bytes / esize) {
                let shift = lane * ebits;
                let ea = (src >> shift) & emask;
                let sign = ea >> (ebits - 1);
                let signed = ea as i128 - if sign != 0 { 1i128 << ebits } else { 0 };
                let lane_val = match insn.opcode {
                    SimdAbs => (signed.unsigned_abs() as u128) & emask,
                    SimdNeg => ((-signed) as u128) & emask,
                    _ => unreachable!(),
                };
                result |= lane_val << shift;
            }

            a.v[insn.rd as usize] = result;
        }

        // ── SIMD USHR (unsigned shift right by immediate) ────────────────
        SimdUshr => {
            // insn.imm encodes immh:immb; shift = esize - (immh:immb - esize)
            // For V.2D (size=11): esize=64, shift = 128 - imm (where imm = immh:immb)
            let imm = insn.imm as u32;
            // Determine element size from immh field (bits[22:19] of original imm)
            let immh = (imm >> 3) & 0xF;
            let (esize, emask, shift_amt) = if immh >= 8 {
                let s = 128 - imm;
                (64usize, 0xFFFF_FFFF_FFFF_FFFFu128, s as usize)
            } else if immh >= 4 {
                let s = 64 - imm;
                (32, 0xFFFF_FFFFu128, s as usize)
            } else if immh >= 2 {
                let s = 32 - imm;
                (16, 0xFFFFu128, s as usize)
            } else {
                let s = 16 - imm;
                (8, 0xFFu128, s as usize)
            };
            let vn = a.v[insn.rn as usize];
            let lanes = if insn.sf { 128 / esize } else { 64 / esize };
            let mut result = 0u128;
            for lane in 0..lanes {
                let e = (vn >> (lane * esize)) & emask;
                let shifted = if shift_amt >= esize {
                    0
                } else {
                    e >> shift_amt
                };
                result |= (shifted & emask) << (lane * esize);
            }
            a.v[insn.rd as usize] = result;
        }

        // ── Catch-all SIMD — silently skip unimplemented ─────────────────
        SimdOther | SimdLd1 | SimdSt1 | FcvtzsVec | FcvtzuVec | SimdMvni | SimdFmov | SimdCmtst
        | SimdAddp | SimdAddv | SimdSshl | SimdUshl | SimdSshr | SimdShl | SimdTbl | SimdTbx
        | SimdZip1 | SimdZip2 | SimdUzp1 | SimdUzp2 | SimdTrn1 | SimdTrn2 | SimdExt | SimdRev64
        | SimdRev32 | SimdRev16 | SimdCnt | SimdClz | SimdSxtl | SimdUxtl | SimdSmin | SimdUmin
        | SimdSmax | SimdUmax | SimdFadd | SimdFsub | SimdFmul | SimdFdiv | SimdFabs | SimdFneg
        | SimdFsqrt | SimdFcmeq | SimdFcmgt | SimdFcmge | SimdFcvtzs | SimdFcvtzu | SimdScvtf
        | SimdUcvtf | SimdFrintm | SimdFrintn | SimdFrintp | SimdFrintz | SimdLd2 | SimdSt2
        | SimdLd3 | SimdSt3 | SimdLd4 | SimdSt4 | SimdLd1r | SimdBif | SimdBit | SimdBsl
        | SimdOrrImm => {
            // Unimplemented SIMD — silently skip for Phase 0
        }
        // ── Scalar ADDP: ADDP Dd, Vn.2D ─────────────────────────────────────
        ScalarAddp => {
            let vn = a.v[insn.rn as usize];
            let lo = vn as u64;
            let hi = (vn >> 64) as u64;
            a.v[insn.rd as usize] = lo.wrapping_add(hi) as u128;
        }

        // ── v8.4 Dot Product (SDOT / UDOT) ─────────────────────────────────
        Sdot | Udot => {
            // Each 128-bit (sf=true, 4 lanes) or 64-bit (sf=false, 2 lanes) vector:
            // For each 32-bit accumulator lane i, sum 4 signed/unsigned byte products.
            let vn = a.v[insn.rn as usize];
            let vm = a.v[insn.rm as usize];
            let mut vd = a.v[insn.rd as usize];
            let lanes: usize = if insn.sf { 4 } else { 2 };
            let signed = matches!(insn.opcode, Sdot);
            for lane in 0..lanes {
                let bit_off = lane * 32;
                let mut acc = ((vd >> bit_off) & 0xFFFF_FFFF) as u32;
                for b in 0..4usize {
                    let byte_off = bit_off + b * 8;
                    let na = ((vn >> byte_off) & 0xFF) as u32;
                    let ma = ((vm >> byte_off) & 0xFF) as u32;
                    let prod = if signed {
                        ((na as i8 as i32) * (ma as i8 as i32)) as u32
                    } else {
                        na.wrapping_mul(ma)
                    };
                    acc = acc.wrapping_add(prod);
                }
                let clear_mask = !(0xFFFF_FFFFu128 << bit_off);
                vd = (vd & clear_mask) | ((acc as u128) << bit_off);
            }
            // Zero upper 64 bits for 64-bit form
            if !insn.sf {
                vd &= 0xFFFF_FFFF_FFFF_FFFF;
            }
            a.v[insn.rd as usize] = vd;
        }

        // ── v8.3 FCMA: FCADD (complex FP add with rotation) ─────────────────
        Fcadd => {
            // Stub: complex add is rare in boot path; implement as NOP on result vector
            // A proper impl would do: Vd[i] += rot90(Vn[i])
            // For now: silently skip (no crash needed; BTI/PSCI is the boot blocker)
        }

        // ── v8.3 FCMA: FCMLA (complex FP multiply-accumulate) ───────────────
        Fcmla => {
            // Stub: complex mul-accumulate; skip silently
        }

        // ── Crypto stubs: raise IllegalInstruction ───────────────────────────
        Sha3 | Sha512 | Sm3 | Sm4 => {
            return Err(HartException::IllegalInstruction {
                pc: a.pc,
                raw: insn.raw,
            });
        }

        _ => unreachable!("wrong dispatch to simd"),
    }
    Ok(pc_written)
}

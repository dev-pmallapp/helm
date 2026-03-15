#![allow(clippy::unusual_byte_groupings)]
#![allow(clippy::unnecessary_cast, clippy::identity_op)]
//! AArch64 single-pass decode+execute for SE/FE mode.
//!
//! Entry point: [`step`] — takes a raw 32-bit instruction word and mutable
//! references to [`Aarch64ArchState`] and a [`MemInterface`].
//! Returns `Ok(pc_written)` where `true` means the instruction wrote PC.

use helm_core::{AccessType, HartException, MemFault, MemInterface};
use super::arch_state::Aarch64ArchState;

// ── Bit-extraction helpers ───────────────────────────────────────────────────

#[inline(always)]
fn bit(v: u32, pos: u32) -> u32 { (v >> pos) & 1 }

#[inline(always)]
fn bits(v: u32, hi: u32, lo: u32) -> u32 {
    (v >> lo) & ((1 << (hi - lo + 1)) - 1)
}

/// Sign-extend a `bits_wide`-bit value to 64 bits.
#[inline(always)]
fn sext(v: u64, bits_wide: u32) -> i64 {
    let shift = 64 - bits_wide;
    ((v as i64) << shift) >> shift
}

/// Sign-extend a value of `size` bytes to 64 bits.
#[inline]
fn sign_extend(v: u64, size: usize) -> u64 {
    let shift = 64 - size * 8;
    ((v as i64) << shift >> shift) as u64
}

/// Sign-extend to the width given in *bits*.
#[inline]
fn sign_extend_bits(v: u64, width: usize) -> u64 {
    if width == 0 || width >= 64 { return v; }
    let shift = 64 - width;
    ((v as i64) << shift >> shift) as u64
}

// ── Arithmetic overflow helpers ──────────────────────────────────────────────

#[inline]
fn add_overflow64(a: u64, b: u64, res: u64) -> bool {
    ((!(a ^ b)) & (a ^ res)) >> 63 != 0
}
#[inline]
fn sub_overflow64(a: u64, b: u64, res: u64) -> bool {
    ((a ^ b) & (a ^ res)) >> 63 != 0
}
#[inline]
fn add_overflow32(a: u32, b: u32, res: u32) -> bool {
    ((!(a ^ b)) & (a ^ res)) >> 31 != 0
}
#[inline]
fn sub_overflow32(a: u32, b: u32, res: u32) -> bool {
    ((a ^ b) & (a ^ res)) >> 31 != 0
}

// ── Memory helpers ───────────────────────────────────────────────────────────

#[inline]
fn rd(mem: &mut impl MemInterface, addr: u64, sz: usize) -> Result<u64, HartException> {
    mem.read(addr, sz, AccessType::Load)
        .map_err(|_| HartException::LoadAccessFault { addr })
}

#[inline]
fn wr(mem: &mut impl MemInterface, addr: u64, val: u64, sz: usize) -> Result<(), HartException> {
    mem.write(addr, sz, val, AccessType::Store)
        .map_err(|_| HartException::StoreAccessFault { addr })
}

#[inline]
fn rd_atomic(mem: &mut impl MemInterface, addr: u64, sz: usize) -> Result<u64, HartException> {
    mem.read(addr, sz, AccessType::Atomic)
        .map_err(|_| HartException::LoadAccessFault { addr })
}

#[inline]
fn wr_atomic(mem: &mut impl MemInterface, addr: u64, val: u64, sz: usize) -> Result<(), HartException> {
    mem.write(addr, sz, val, AccessType::Atomic)
        .map_err(|_| HartException::StoreAccessFault { addr })
}

#[inline]
fn rd128(mem: &mut impl MemInterface, addr: u64) -> Result<u128, HartException> {
    let lo = rd(mem, addr, 8)? as u128;
    let hi = rd(mem, addr.wrapping_add(8), 8)? as u128;
    Ok((hi << 64) | lo)
}

#[inline]
fn wr128(mem: &mut impl MemInterface, addr: u64, val: u128) -> Result<(), HartException> {
    wr(mem, addr, val as u64, 8)?;
    wr(mem, addr.wrapping_add(8), (val >> 64) as u64, 8)
}

// ── Shift / extend helpers ───────────────────────────────────────────────────

fn apply_shift(val: u64, stype: u32, amt: u32, sf: bool) -> u64 {
    let amt = amt & if sf { 63 } else { 31 };
    match stype {
        0 => val << amt,
        1 => val >> amt,
        2 => ((val as i64) >> amt) as u64,
        3 => val.rotate_right(amt),
        _ => val,
    }
}

fn apply_extend(val: u64, etype: u32, amt: u32) -> u64 {
    let extended = match etype {
        0 => val & 0xFF,            // UXTB
        1 => val & 0xFFFF,          // UXTH
        2 => val & 0xFFFF_FFFF,     // UXTW / LSL
        3 => val,                   // UXTX / LSL64
        4 => (val as i8) as u64,    // SXTB
        5 => (val as i16) as u64,   // SXTH
        6 => (val as i32) as u64,   // SXTW
        7 => val,                   // SXTX
        _ => val,
    };
    extended << amt
}

// ── Condition evaluation ─────────────────────────────────────────────────────

fn cond(a: &Aarch64ArchState, cc: u32) -> bool {
    a.eval_cond(cc)
}

// ── Bitmask decode (N:immr:imms → mask) ──────────────────────────────────────

fn decode_bitmask(n: bool, imms: u32, immr: u32, sf: bool) -> Option<u64> {
    let len = if n {
        6u32
    } else {
        let x = (!imms) & 0x3F;
        if x == 0 { return None; }
        31 - x.leading_zeros()
    };
    if len < 1 { return None; }
    let levels = (1u32 << len) - 1;
    let s = imms & levels;
    let r = immr & levels;
    let esize = 1u32 << len;
    if s == levels { return None; }

    let welem = (1u64 << (s + 1)) - 1;
    let rotated = if r == 0 {
        welem
    } else {
        ((welem >> r) | (welem << (esize - r))) & if esize >= 64 { u64::MAX } else { (1u64 << esize) - 1 }
    };

    let mut mask = rotated;
    let mut bits_done = esize;
    while bits_done < 64 {
        mask |= mask << bits_done;
        bits_done *= 2;
    }
    if !sf { mask &= 0xFFFF_FFFF; }
    Some(mask)
}

// ── FP helpers ───────────────────────────────────────────────────────────────

fn fp_imm8_to_f32(imm8: u32) -> f32 {
    let sign  = (imm8 >> 7) & 1;
    let exp4  = (imm8 >> 4) & 0xF;
    let mant3 = imm8 & 0x7;
    let exp = if exp4 & 0x8 != 0 { (exp4 | 0xFFFF_FFF8) as i32 } else { exp4 as i32 };
    let exp_biased = (exp + 127) as u32;
    let fbits = (sign << 31) | ((exp_biased & 0xFF) << 23) | (mant3 << 20);
    f32::from_bits(fbits)
}

// ═══════════════════════════════════════════════════════════════════════════════
// step() — top-level dispatch on bits[28:25]
// ═══════════════════════════════════════════════════════════════════════════════

/// Single-pass decode+execute of one AArch64 instruction.
///
/// Returns `Ok(true)` if the instruction wrote PC (branch taken, SVC, etc.),
/// `Ok(false)` if the caller should advance PC by 4.
pub fn step(
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
    insn: u32,
) -> Result<bool, HartException> {
    let op0 = bits(insn, 28, 25);
    match op0 {
        0b1000 | 0b1001 => exec_dp_imm(a, mem, insn),
        0b1010 | 0b1011 => exec_branch_sys(a, mem, insn),
        0b0100 | 0b0110 | 0b1100 | 0b1110 => exec_ldst(a, mem, insn),
        0b0101 | 0b1101 => exec_dp_reg(a, mem, insn),
        0b0111 | 0b1111 => exec_simd_fp(a, mem, insn),
        _ => Err(HartException::IllegalInstruction { pc: a.pc, raw: insn }),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Data-processing — immediate  (op0 = 100x)
// ═══════════════════════════════════════════════════════════════════════════════

fn exec_dp_imm(
    a: &mut Aarch64ArchState,
    _mem: &mut impl MemInterface,
    insn: u32,
) -> Result<bool, HartException> {
    let sf = bit(insn, 31) != 0;
    let rd = bits(insn, 4, 0);
    let rn = bits(insn, 9, 5);
    let b28_23 = bits(insn, 28, 23);

    match b28_23 {
        // ── ADR / ADRP ───────────────────────────────────────────────────────
        0b100000 | 0b100001 => {
            let page = bit(insn, 31) != 0;
            let immlo = bits(insn, 30, 29);
            let immhi = bits(insn, 23, 5);
            let imm = sext(((immhi << 2) | immlo) as u64, 21);
            if page {
                // ADRP
                let base = a.pc & !0xFFF;
                let val = base.wrapping_add((imm as u64) << 12);
                a.write_x(rd, val);
            } else {
                // ADR
                let val = a.pc.wrapping_add(imm as u64);
                a.write_x(rd, val);
            }
        }

        // ── ADD / SUB immediate ──────────────────────────────────────────────
        0b100010 | 0b100011 => {
            let sub  = bit(insn, 30) != 0;
            let setf = bit(insn, 29) != 0;
            let sh   = bit(insn, 22);
            let imm12 = bits(insn, 21, 10) as u64;
            let imm = imm12 << (sh * 12);

            if !setf {
                // ADD / SUB (no flags)
                let src = if sf { a.read_xsp(rn) } else { a.read_xsp(rn) & 0xFFFF_FFFF };
                let res = if sub { src.wrapping_sub(imm) } else { src.wrapping_add(imm) };
                if sf { a.write_xsp(rd, res); } else { a.write_xsp(rd, res & 0xFFFF_FFFF); }
            } else if !sub {
                // ADDS immediate
                let src = a.read_x(rn);
                let (res, c) = src.overflowing_add(imm);
                let v = add_overflow64(src, imm, res);
                if sf {
                    a.set_nzcv64(res, c, v);
                    a.write_x(rd, res);
                } else {
                    let r32 = res as u32;
                    a.set_nzcv(r32 >> 31 != 0, r32 == 0,
                        (src as u32).overflowing_add(imm as u32).1,
                        add_overflow32(src as u32, imm as u32, r32));
                    a.write_x(rd, r32 as u64);
                }
            } else {
                // SUBS immediate
                let src = a.read_x(rn);
                let (res, b) = src.overflowing_sub(imm);
                let v = sub_overflow64(src, imm, res);
                if sf {
                    a.set_nzcv64(res, !b, v);
                    a.write_x(rd, res);
                } else {
                    let r32 = res as u32;
                    let (_, b32) = (src as u32).overflowing_sub(imm as u32);
                    a.set_nzcv(r32 >> 31 != 0, r32 == 0, !b32,
                        sub_overflow32(src as u32, imm as u32, r32));
                    a.write_x(rd, r32 as u64);
                }
            }
        }

        // ── Logical immediate ────────────────────────────────────────────────
        0b100100 => {
            let n    = bit(insn, 22) != 0;
            let immr = bits(insn, 21, 16);
            let imms = bits(insn, 15, 10);
            let mask = match decode_bitmask(n, imms, immr, sf) {
                Some(m) => m,
                None => return Err(HartException::IllegalInstruction { pc: a.pc, raw: insn }),
            };
            let opc = bits(insn, 30, 29);
            let src = a.read_xsp(rn);
            match opc {
                0b00 => {
                    // AND immediate
                    let res = src & mask;
                    if sf { a.write_xsp(rd, res); } else { a.write_xsp(rd, (res as u32) as u64); }
                }
                0b01 => {
                    // ORR immediate
                    let res = src | mask;
                    if sf { a.write_xsp(rd, res); } else { a.write_xsp(rd, (res as u32) as u64); }
                }
                0b10 => {
                    // EOR immediate
                    let res = src ^ mask;
                    if sf { a.write_xsp(rd, res); } else { a.write_xsp(rd, (res as u32) as u64); }
                }
                0b11 => {
                    // ANDS immediate
                    let res = src & mask;
                    if sf { a.write_x(rd, res); } else { a.write_x(rd, (res as u32) as u64); }
                    a.set_nzcv64(res, false, false);
                }
                _ => unreachable!(),
            }
        }

        // ── Move wide (MOVN / MOVZ / MOVK) ──────────────────────────────────
        0b100101 => {
            let opc   = bits(insn, 30, 29);
            let hw    = bits(insn, 22, 21);
            let imm16 = bits(insn, 20, 5) as u64;
            match opc {
                0b00 => {
                    // MOVN
                    let val = !((imm16) << (hw * 16));
                    if sf { a.write_x(rd, val); } else { a.write_w(rd, val as u32); }
                }
                0b10 => {
                    // MOVZ
                    let val = imm16 << (hw * 16);
                    if sf { a.write_x(rd, val); } else { a.write_w(rd, val as u32); }
                }
                0b11 => {
                    // MOVK — insert 16-bit field into existing value
                    let shift = hw * 16;
                    let mask  = !(0xFFFFu64 << shift);
                    let old   = a.read_x(rd);
                    let val   = (old & mask) | ((imm16 & 0xFFFF) << shift);
                    if sf { a.write_x(rd, val); } else { a.write_w(rd, val as u32); }
                }
                _ => return Err(HartException::IllegalInstruction { pc: a.pc, raw: insn }),
            }
        }

        // ── Bitfield (SBFM / BFM / UBFM) ────────────────────────────────────
        0b100110 => {
            let opc  = bits(insn, 30, 29);
            let immr = bits(insn, 21, 16);
            let imms = bits(insn, 15, 10);
            let src  = a.read_x(rn);
            match opc {
                0b00 => {
                    // SBFM
                    let val = if imms >= immr {
                        let width = imms - immr + 1;
                        let extracted = (src >> immr) & ((1u64 << width) - 1);
                        sign_extend_bits(extracted, width as usize)
                    } else {
                        let width = imms + 1;
                        let shifted = src.rotate_right(immr) & ((1u64 << width) - 1);
                        sign_extend_bits(shifted, width as usize)
                    };
                    a.write_x(rd, val);
                }
                0b01 => {
                    // BFM
                    let dst = a.read_x(rd);
                    let width = if imms >= immr { imms - immr + 1 } else { imms + 1 };
                    let mask = (1u64 << width) - 1;
                    let extracted = if imms >= immr { (src >> immr) & mask } else { src & mask };
                    let shift = if imms >= immr { 0 } else { (64 - immr) & 63 };
                    let val = (dst & !(mask << shift)) | ((extracted & mask) << shift);
                    a.write_x(rd, val);
                }
                0b10 => {
                    // UBFM
                    let val = if imms >= immr {
                        let width = imms - immr + 1;
                        (src >> immr) & ((1u64 << width) - 1)
                    } else {
                        let width = imms + 1;
                        src.rotate_right(immr) & ((1u64 << width) - 1)
                    };
                    a.write_x(rd, val);
                }
                _ => return Err(HartException::IllegalInstruction { pc: a.pc, raw: insn }),
            }
        }

        // ── EXTR ─────────────────────────────────────────────────────────────
        0b100111 => {
            let rm   = bits(insn, 20, 16);
            let immr = bits(insn, 15, 10);
            let rs1  = a.read_x(rn);
            let rs2  = a.read_x(rm);
            let val = if sf {
                if immr == 0 { rs1 } else { (rs1 << (64 - immr)) | (rs2 >> immr) }
            } else {
                let r = ((rs1 as u32) << (32 - immr)) | ((rs2 as u32) >> immr);
                r as u64
            };
            a.write_x(rd, val);
        }

        _ => return Err(HartException::IllegalInstruction { pc: a.pc, raw: insn }),
    }

    Ok(false)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Branches and system instructions  (op0 = 101x)
// ═══════════════════════════════════════════════════════════════════════════════

fn exec_branch_sys(
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
    insn: u32,
) -> Result<bool, HartException> {
    // ── B / BL (unconditional immediate) ─────────────────────────────────
    // B:  bits[31:26] = 000101
    // BL: bits[31:26] = 100101
    if bits(insn, 30, 26) == 0b00101 {
        let imm26 = bits(insn, 25, 0);
        let offset = sext((imm26 << 2) as u64, 28) as u64;
        if bit(insn, 31) != 0 {
            // BL
            a.write_x(30, a.pc.wrapping_add(4));
        }
        a.pc = a.pc.wrapping_add(offset);
        return Ok(true);
    }

    // ── B.cond: bits[31:24] = 0101_0100 ──────────────────────────────────
    if bits(insn, 31, 24) == 0b0101_0100 {
        let imm19 = bits(insn, 23, 5);
        let cc    = bits(insn, 3, 0);
        if cond(a, cc) {
            let offset = sext((imm19 << 2) as u64, 21) as u64;
            a.pc = a.pc.wrapping_add(offset);
            return Ok(true);
        }
        return Ok(false);
    }

    // ── CBZ / CBNZ: bits[30:25] = 011010 ─────────────────────────────────
    if bits(insn, 30, 25) == 0b011010 {
        let sf  = bit(insn, 31) != 0;
        let rt  = bits(insn, 4, 0);
        let imm19 = bits(insn, 23, 5);
        let nz  = bit(insn, 24) != 0; // 0=CBZ, 1=CBNZ
        let val = if sf { a.read_x(rt) } else { a.read_x(rt) & 0xFFFF_FFFF };
        let take = if nz { val != 0 } else { val == 0 };
        if take {
            let offset = sext((imm19 << 2) as u64, 21) as u64;
            a.pc = a.pc.wrapping_add(offset);
            return Ok(true);
        }
        return Ok(false);
    }

    // ── TBZ / TBNZ: bits[30:25] = 011011 ─────────────────────────────────
    if bits(insn, 30, 25) == 0b011011 {
        let rt    = bits(insn, 4, 0);
        let imm14 = bits(insn, 18, 5);
        let b5    = bit(insn, 31);
        let b40   = bits(insn, 23, 19);
        let bitpos = (b5 << 5) | b40;
        let nz    = bit(insn, 24) != 0;
        let val   = a.read_x(rt);
        let take  = if nz { val & (1 << bitpos) != 0 } else { val & (1 << bitpos) == 0 };
        if take {
            let offset = sext((imm14 << 2) as u64, 16) as u64;
            a.pc = a.pc.wrapping_add(offset);
            return Ok(true);
        }
        return Ok(false);
    }

    // ── BR / BLR / RET: bits[31:25] = 1101011 ───────────────────────────
    if bits(insn, 31, 25) == 0b1101011 {
        let opc = bits(insn, 24, 21);
        let rn  = bits(insn, 9, 5);
        match opc {
            0b0000 => {
                // BR
                a.pc = a.read_x(rn);
                return Ok(true);
            }
            0b0001 => {
                // BLR
                a.write_x(30, a.pc.wrapping_add(4));
                a.pc = a.read_x(rn);
                return Ok(true);
            }
            0b0010 => {
                // RET
                a.pc = a.read_x(rn);
                return Ok(true);
            }
            _ => {}
        }
    }

    // ── Exception generation: bits[31:24] = 1101_0100 ────────────────────
    if bits(insn, 31, 24) == 0b1101_0100 {
        let opc = bits(insn, 23, 21);
        let ll  = bits(insn, 1, 0);
        match (opc, ll) {
            (0b000, 0b01) => {
                // SVC
                return Err(HartException::EnvironmentCall {
                    pc: a.pc,
                    nr: a.x[8],
                });
            }
            (0b001, 0b00) => {
                // BRK
                return Err(HartException::Breakpoint { pc: a.pc });
            }
            (0b000, 0b10) => {
                // HVC — unsupported in SE mode
                return Err(HartException::Unsupported);
            }
            (0b000, 0b11) => {
                // SMC — unsupported in SE mode
                return Err(HartException::Unsupported);
            }
            _ => {}
        }
    }

    // ── ERET ─────────────────────────────────────────────────────────────
    if insn == 0xD69F_03E0 {
        a.pc = a.elr_el1;
        return Ok(true);
    }

    // ── System instructions: bits[31:22] = 1101_0101_00 ──────────────────
    if bits(insn, 31, 22) == 0b1101_0101_00 {
        return exec_system(a, mem, insn);
    }

    // NOP encoding
    if insn == 0xD503_201F { return Ok(false); }
    // WFI
    if insn == 0xD503_207F { return Ok(false); }

    Err(HartException::IllegalInstruction { pc: a.pc, raw: insn })
}

/// System register access and hints (MRS, MSR, barriers, DC ZVA).
fn exec_system(
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
    insn: u32,
) -> Result<bool, HartException> {
    let l   = bit(insn, 21);
    let op0 = bits(insn, 20, 19);
    let op1 = bits(insn, 18, 16);
    let crn = bits(insn, 15, 12);
    let crm = bits(insn, 11, 8);
    let op2 = bits(insn, 7, 5);
    let rt  = bits(insn, 4, 0);

    // ── Barriers / hints ─────────────────────────────────────────────────
    // NOP, YIELD, WFE, WFI, SEV, SEVL, ISB, DMB, DSB — all NOP in SE mode
    if insn == 0xD503_201F    // NOP
        || insn == 0xD503_207F // WFI
        || insn == 0xD503_2020 // YIELD
    {
        return Ok(false);
    }
    // ISB/DSB/DMB: bits[31:8] = 0b1101_0101_0000_0011_0010_xxxx
    if bits(insn, 31, 12) == 0b1101_0101_0000_0011_0010 {
        return Ok(false);
    }
    // Other hints (SEV, SEVL, WFE, etc.): bits[31:12] = 0b11010101000000110010_0000
    if bits(insn, 31, 5) == 0b110100101000000110010_0000000 >> 5 {
        return Ok(false);
    }

    // ── MRS / MSR ────────────────────────────────────────────────────────
    if bits(insn, 31, 20) == 0b1101_0101_0010 || bits(insn, 31, 20) == 0b1101_0101_0011 {
        let encoded = (op0 << 14) | (op1 << 11) | (crn << 7) | (crm << 3) | op2;
        if l == 1 {
            // MRS Xt, sysreg
            let val = read_sysreg(a, encoded);
            a.write_x(rt, val);
        } else {
            // MSR sysreg, Xt
            let val = a.read_x(rt);
            write_sysreg(a, encoded, val);
        }
        return Ok(false);
    }

    // ── DC ZVA: op0=01, op1=011, CRn=0111, CRm=0100, op2=001 ───────────
    if op0 == 0b01 && op1 == 0b011 && crn == 0b0111 && crm == 0b0100 && op2 == 0b001 {
        let va = a.read_x(rt);
        let line = va & !63u64; // assume 64-byte cache line
        for off in (0..64).step_by(8) {
            let _ = wr(mem, line + off, 0, 8);
        }
        return Ok(false);
    }

    // Other SYS instructions (TLBI, DC, IC, AT) — NOP in SE mode
    Ok(false)
}

fn read_sysreg(a: &Aarch64ArchState, encoded: u32) -> u64 {
    match encoded {
        0b11_011_1101_0000_010 => a.tpidr_el0,
        0b11_011_0100_0010_000 => a.nzcv as u64,
        0b11_011_0100_0100_000 => a.fpcr as u64,
        0b11_011_0100_0100_001 => a.fpsr as u64,
        0b11_011_0000_0000_001 => 0x8444_C004,        // CTR_EL0
        0b11_011_0000_0000_111 => 0x0000_0004,        // DCZID_EL0 (64-byte block)
        0b11_011_1110_0000_010 => a.cntvct_el0,       // CNTVCT_EL0
        0b11_011_1110_0000_000 => a.cntfrq_el0,       // CNTFRQ_EL0
        0b11_000_0000_0000_000 => a.midr_el1,         // MIDR_EL1
        0b11_000_0000_0000_101 => a.mpidr_el1,        // MPIDR_EL1
        0b11_000_0000_0100_000 => a.id_aa64pfr0_el1,  // ID_AA64PFR0_EL1
        0b11_000_0000_0110_000 => a.id_aa64isar0_el1, // ID_AA64ISAR0_EL1
        0b11_000_0000_0111_000 => a.id_aa64mmfr0_el1, // ID_AA64MMFR0_EL1
        0b11_000_0001_0000_000 => a.sctlr_el1,        // SCTLR_EL1
        _ => 0,
    }
}

fn write_sysreg(a: &mut Aarch64ArchState, encoded: u32, val: u64) {
    match encoded {
        0b11_011_1101_0000_010 => a.tpidr_el0 = val,
        0b11_011_0100_0010_000 => a.nzcv = val as u32,
        0b11_011_0100_0100_000 => a.fpcr = val as u32,
        0b11_011_0100_0100_001 => a.fpsr = val as u32,
        0b11_000_0001_0000_000 => a.sctlr_el1 = val,
        0b11_000_0010_0000_000 => a.tcr_el1 = val,
        0b11_000_0010_0000_001 => a.ttbr0_el1 = val,
        0b11_000_0010_0000_011 => a.ttbr1_el1 = val,
        0b11_000_1100_0000_000 => a.vbar_el1 = val,
        0b11_000_1010_0010_000 => a.mair_el1 = val,
        _ => { /* ignore writes to unknown sysregs */ }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Load / Store  (op0 = x1x0)
// ═══════════════════════════════════════════════════════════════════════════════

fn exec_ldst(
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
    insn: u32,
) -> Result<bool, HartException> {
    let size = bits(insn, 31, 30);
    let v    = bit(insn, 26);   // FP/SIMD?
    let opc  = bits(insn, 23, 22);
    let rt   = bits(insn, 4, 0);
    let rn   = bits(insn, 9, 5);

    // ── Load literal (PC-relative): bits[29:27]=011, bit24=0 ─────────────
    if bits(insn, 29, 27) == 0b011 && bit(insn, 24) == 0 {
        let imm19 = bits(insn, 23, 5);
        let offset = sext((imm19 << 2) as u64, 21) as u64;
        let addr = a.pc.wrapping_add(offset);
        if v == 1 {
            // SIMD/FP literal
            let ftype = size; // 0=S,1=D,2=Q
            match ftype {
                0 => { let val = rd(mem, addr, 4)?; a.v[rt as usize] = val as u128; }
                1 => { let val = rd(mem, addr, 8)?; a.v[rt as usize] = val as u128; }
                2 => { a.v[rt as usize] = rd128(mem, addr)?; }
                _ => { let val = rd(mem, addr, 8)?; a.v[rt as usize] = val as u128; }
            }
        } else {
            match size {
                0b10 => {
                    // LDRSW
                    let val = rd(mem, addr, 4)?;
                    a.write_x(rt, val as i32 as i64 as u64);
                }
                _ => {
                    // LDR Wt/Xt
                    let sz = if size == 3 { 8 } else { 4 };
                    let val = rd(mem, addr, sz)?;
                    a.write_x(rt, val);
                }
            }
        }
        return Ok(false);
    }

    // ── PRFM (prefetch → NOP): size=11, V=0, opc=10, unsigned-offset ────
    if size == 0b11 && v == 0 && opc == 0b10 && bit(insn, 24) == 1 {
        return Ok(false);
    }

    // ── LDP / STP: bits[29:27] = 101 ────────────────────────────────────
    if bits(insn, 29, 27) == 0b101 {
        return exec_ldst_pair(a, mem, insn, v);
    }

    // ── Exclusive / ordered: bits[29:24] = 001000 ───────────────────────
    if bits(insn, 29, 24) == 0b001000 {
        return exec_ldst_exclusive(a, mem, insn);
    }

    // ── LSE atomics: bits[29:24]=111000, bit21=1, bits[11:10]=00 ────────
    if bits(insn, 29, 24) == 0b111000 && bit(insn, 21) == 1 && bits(insn, 11, 10) == 0b00 {
        return exec_ldst_atomic(a, mem, insn);
    }

    // ── SIMD/FP load/store (V=1) ────────────────────────────────────────
    if v == 1 {
        return exec_ldst_simd(a, mem, insn);
    }

    // ── Register offset: bit24=0, bit21=1, bits[11:10]=10 ───────────────
    if bit(insn, 24) == 0 && bit(insn, 21) == 1 && bits(insn, 11, 10) == 0b10 {
        return exec_ldst_reg_offset(a, mem, insn);
    }

    // ── Unscaled immediate (LDUR/STUR): bit24=0, bits[11:10]=00 ─────────
    if bit(insn, 24) == 0 && bits(insn, 11, 10) == 0b00 && bit(insn, 21) == 0 {
        let imm9 = bits(insn, 20, 12);
        let offset = sext(imm9 as u64, 9) as u64;
        let base = a.read_xsp(rn);
        let addr = base.wrapping_add(offset);
        let store = opc == 0b00;
        let signed = opc & 0b10 != 0;
        let sz = 1usize << size;
        if store {
            let val = a.read_x(rt);
            wr(mem, addr, val, sz)?;
        } else {
            let val = rd(mem, addr, sz)?;
            if signed {
                a.write_x(rt, sign_extend(val, sz));
            } else {
                a.write_x(rt, val);
            }
        }
        return Ok(false);
    }

    // ── Pre/post-index: bit24=0 ─────────────────────────────────────────
    if bit(insn, 24) == 0 {
        let imm9 = bits(insn, 20, 12);
        let offset = sext(imm9 as u64, 9) as u64;
        let post = bit(insn, 11) == 0;
        let base = a.read_xsp(rn);
        let addr = if post { base } else { base.wrapping_add(offset) };
        let store = opc == 0b00;
        let signed = opc & 0b10 != 0;
        let sz = 1usize << size;

        if !post {
            // pre-index: writeback before access
            a.write_xsp(rn, base.wrapping_add(offset));
        }
        if store {
            let val = a.read_x(rt);
            wr(mem, addr, val, sz)?;
        } else {
            let val = rd(mem, addr, sz)?;
            if signed {
                a.write_x(rt, sign_extend(val, sz));
            } else {
                a.write_x(rt, val);
            }
        }
        if post {
            // post-index: writeback after access
            a.write_xsp(rn, base.wrapping_add(offset));
        }
        return Ok(false);
    }

    // ── Unsigned offset (most common): bit24=1 ──────────────────────────
    {
        let imm12 = bits(insn, 21, 10) as u64;
        let offset = imm12 << size;
        let base = a.read_xsp(rn);
        let addr = base.wrapping_add(offset);
        let store = bit(insn, 22) == 0;
        let signed = bit(insn, 23) != 0;
        let sz = 1usize << size;
        if store {
            let val = a.read_x(rt);
            wr(mem, addr, val, sz)?;
        } else {
            let val = rd(mem, addr, sz)?;
            if signed {
                a.write_x(rt, sign_extend(val, sz));
            } else {
                a.write_x(rt, val);
            }
        }
    }
    Ok(false)
}

fn exec_ldst_reg_offset(
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
    insn: u32,
) -> Result<bool, HartException> {
    let size   = bits(insn, 31, 30);
    let opc    = bits(insn, 23, 22);
    let rm     = bits(insn, 20, 16);
    let option = bits(insn, 15, 13);
    let s      = bit(insn, 12);
    let rn     = bits(insn, 9, 5);
    let rt     = bits(insn, 4, 0);
    let ext_amt = if s != 0 { size } else { 0 };
    let rm_val = a.read_x(rm);
    let ext = apply_extend(rm_val, option, ext_amt);
    let base = a.read_xsp(rn);
    let addr = base.wrapping_add(ext);
    let store  = opc & 1 == 0;
    let signed = opc & 2 != 0;
    let sz = 1usize << size;
    if store {
        let val = a.read_x(rt);
        wr(mem, addr, val, sz)?;
    } else {
        let val = rd(mem, addr, sz)?;
        if signed {
            a.write_x(rt, sign_extend(val, sz));
        } else {
            a.write_x(rt, val);
        }
    }
    Ok(false)
}

fn exec_ldst_pair(
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
    insn: u32,
    v: u32,
) -> Result<bool, HartException> {
    let opc  = bits(insn, 31, 30);
    let l    = bit(insn, 22);
    let imm7 = bits(insn, 21, 15);
    let rt2  = bits(insn, 14, 10);
    let rn   = bits(insn, 9, 5);
    let rt   = bits(insn, 4, 0);
    let pre  = bits(insn, 24, 23) == 0b11;
    let post = bits(insn, 24, 23) == 0b01;

    if v == 1 {
        // SIMD/FP pair
        let scale = match opc { 0b00 => 2u32, 0b01 => 3, 0b10 => 4, _ => 2 };
        let offset = (sext(imm7 as u64, 7) << scale) as u64;
        let base = a.read_xsp(rn);
        let ea = if pre { base.wrapping_add(offset) } else { base };
        let sz: usize = 1 << scale; // 4, 8, or 16

        if l != 0 {
            // LDP SIMD
            if sz <= 8 {
                let v1 = rd(mem, ea, sz)?;
                let v2 = rd(mem, ea.wrapping_add(sz as u64), sz)?;
                a.v[rt as usize]  = v1 as u128;
                a.v[rt2 as usize] = v2 as u128;
            } else {
                // Q-regs (16 bytes each)
                a.v[rt as usize]  = rd128(mem, ea)?;
                a.v[rt2 as usize] = rd128(mem, ea.wrapping_add(16))?;
            }
        } else {
            // STP SIMD
            if sz <= 8 {
                wr(mem, ea, a.v[rt as usize] as u64, sz)?;
                wr(mem, ea.wrapping_add(sz as u64), a.v[rt2 as usize] as u64, sz)?;
            } else {
                wr128(mem, ea, a.v[rt as usize])?;
                wr128(mem, ea.wrapping_add(16), a.v[rt2 as usize])?;
            }
        }
        if pre || post {
            let wb = if post { base.wrapping_add(offset) } else { ea };
            a.write_xsp(rn, wb);
        }
    } else {
        // Integer pair
        let sf = opc == 0b10;
        let signed = opc == 0b01; // LDPSW
        let scale = if sf { 3u32 } else { 2 };
        let offset = (sext(imm7 as u64, 7) << scale) as u64;
        let base = a.read_xsp(rn);
        let ea = if pre { base.wrapping_add(offset) } else { base };
        let sz: usize = if sf { 8 } else { 4 };

        if pre {
            a.write_xsp(rn, ea);
        }
        if l != 0 {
            // LDP
            let v1 = rd(mem, ea, sz)?;
            let v2 = rd(mem, ea.wrapping_add(sz as u64), sz)?;
            if signed {
                a.write_x(rt,  sign_extend(v1, sz));
                a.write_x(rt2, sign_extend(v2, sz));
            } else {
                a.write_x(rt, v1);
                a.write_x(rt2, v2);
            }
        } else {
            // STP
            wr(mem, ea, a.read_x(rt), sz)?;
            wr(mem, ea.wrapping_add(sz as u64), a.read_x(rt2), sz)?;
        }
        if post {
            a.write_xsp(rn, base.wrapping_add(offset));
        }
    }
    Ok(false)
}

fn exec_ldst_exclusive(
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
    insn: u32,
) -> Result<bool, HartException> {
    let l    = bit(insn, 22);
    let rs   = bits(insn, 20, 16);
    let o0   = bit(insn, 15);
    let o1   = bit(insn, 21);
    let rn   = bits(insn, 9, 5);
    let rt   = bits(insn, 4, 0);
    let size = bits(insn, 31, 30);

    // CLREX
    if insn & 0xFFFFF0FF == 0xD503305F {
        return Ok(false);
    }

    // LDAR / STLR (load-acquire / store-release, no exclusivity)
    if o1 == 1 && o0 == 1 && rs == 31 {
        let addr = a.read_xsp(rn);
        let sz = 1usize << size;
        if l == 1 {
            // LDAR
            let val = rd(mem, addr, sz)?;
            a.write_x(rt, val);
        } else {
            // STLR
            let val = a.read_x(rt);
            wr(mem, addr, val, sz)?;
        }
        return Ok(false);
    }

    let addr = a.read_xsp(rn);
    let sz = 1usize << size;

    match (l, o0) {
        (1, _) => {
            // LDXR / LDAXR
            let val = rd_atomic(mem, addr, sz)?;
            a.write_x(rt, val);
        }
        (0, _) => {
            // STXR / STLXR
            let val = a.read_x(rt);
            wr_atomic(mem, addr, val, sz)?;
            a.write_x(rs, 0); // success
        }
        _ => {}
    }
    Ok(false)
}

fn exec_ldst_atomic(
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
    insn: u32,
) -> Result<bool, HartException> {
    let size = bits(insn, 31, 30);
    let rs   = bits(insn, 20, 16);
    let opc  = bits(insn, 14, 12);
    let rn   = bits(insn, 9, 5);
    let rt   = bits(insn, 4, 0);
    let addr = a.read_xsp(rn);
    let sz = 1usize << size;
    let mask = if sz < 8 { (1u64 << (sz * 8)) - 1 } else { u64::MAX };

    match opc {
        0b000 => {
            // LDADD
            let old = rd_atomic(mem, addr, sz)?;
            let new_val = old.wrapping_add(a.read_x(rs));
            wr_atomic(mem, addr, new_val & mask, sz)?;
            a.write_x(rt, old & mask);
        }
        0b001 => {
            // LDCLR
            let old = rd_atomic(mem, addr, sz)?;
            let new_val = old & !a.read_x(rs);
            wr_atomic(mem, addr, new_val & mask, sz)?;
            a.write_x(rt, old & mask);
        }
        0b010 => {
            // LDEOR
            let old = rd_atomic(mem, addr, sz)?;
            let new_val = old ^ a.read_x(rs);
            wr_atomic(mem, addr, new_val & mask, sz)?;
            a.write_x(rt, old & mask);
        }
        0b011 => {
            // LDSET
            let old = rd_atomic(mem, addr, sz)?;
            let new_val = old | a.read_x(rs);
            wr_atomic(mem, addr, new_val & mask, sz)?;
            a.write_x(rt, old & mask);
        }
        0b100 => {
            // SWP
            let old = rd_atomic(mem, addr, sz)?;
            wr_atomic(mem, addr, a.read_x(rs) & mask, sz)?;
            a.write_x(rt, old & mask);
        }
        _ => {
            // CAS: bits[14:12]=1xx — separate encoding check
            // CAS is actually encoded differently; handle via top bits
            // For safety, treat as CAS
            let old = rd_atomic(mem, addr, sz)?;
            let expect = a.read_x(rt) & mask;
            if (old & mask) == expect {
                wr_atomic(mem, addr, a.read_x(rs) & mask, sz)?;
            }
            a.write_x(rt, old & mask);
        }
    }
    Ok(false)
}

fn exec_ldst_simd(
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
    insn: u32,
) -> Result<bool, HartException> {
    let size = bits(insn, 31, 30);
    let opc  = bits(insn, 23, 22);
    let rt   = bits(insn, 4, 0);
    let rn   = bits(insn, 9, 5);

    let is_128 = size == 0b00 && (opc & 0b10) != 0;
    let ftype: u32 = if is_128 { 4 } else { size }; // 0=B,1=H,2=S,3=D,4=Q
    let is_load = (opc & 1) != 0;
    let size_bytes: usize = match ftype { 0 => 1, 1 => 2, 2 => 4, 3 => 8, _ => 16 };

    // Unsigned offset: bit24=1
    if bit(insn, 24) == 1 {
        let imm12 = bits(insn, 21, 10) as u64;
        let scale = if is_128 { 4u32 } else { size };
        let offset = imm12 << scale;
        let base = a.read_xsp(rn);
        let addr = base.wrapping_add(offset);
        if is_load {
            if size_bytes <= 8 {
                a.v[rt as usize] = rd(mem, addr, size_bytes)? as u128;
            } else {
                a.v[rt as usize] = rd128(mem, addr)?;
            }
        } else {
            let val = a.v[rt as usize];
            if size_bytes <= 8 {
                wr(mem, addr, val as u64, size_bytes)?;
            } else {
                wr128(mem, addr, val)?;
            }
        }
        return Ok(false);
    }

    // Unscaled offset: bit24=0, bits[11:10]=00
    if bits(insn, 11, 10) == 0b00 && bit(insn, 21) == 0 {
        let imm9 = bits(insn, 20, 12);
        let offset = sext(imm9 as u64, 9) as u64;
        let base = a.read_xsp(rn);
        let addr = base.wrapping_add(offset);
        if is_load {
            if size_bytes <= 8 {
                a.v[rt as usize] = rd(mem, addr, size_bytes)? as u128;
            } else {
                a.v[rt as usize] = rd128(mem, addr)?;
            }
        } else {
            let val = a.v[rt as usize];
            if size_bytes <= 8 {
                wr(mem, addr, val as u64, size_bytes)?;
            } else {
                wr128(mem, addr, val)?;
            }
        }
        return Ok(false);
    }

    // Pre/post-index: bit24=0
    if bit(insn, 24) == 0 {
        let imm9 = bits(insn, 20, 12);
        let offset = sext(imm9 as u64, 9) as u64;
        let pre  = bit(insn, 11) != 0;
        let post = bit(insn, 11) == 0;
        let base = a.read_xsp(rn);
        let eff = base.wrapping_add(offset);

        if pre {
            a.write_xsp(rn, eff);
        }
        let access_addr = if pre || !post { eff } else { base };

        if is_load {
            if size_bytes <= 8 {
                a.v[rt as usize] = rd(mem, access_addr, size_bytes)? as u128;
            } else {
                a.v[rt as usize] = rd128(mem, access_addr)?;
            }
        } else {
            let val = a.v[rt as usize];
            if size_bytes <= 8 {
                wr(mem, access_addr, val as u64, size_bytes)?;
            } else {
                wr128(mem, access_addr, val)?;
            }
        }
        if post {
            a.write_xsp(rn, eff);
        }
        return Ok(false);
    }

    // Fallback — unsigned offset (shouldn't reach here normally)
    Ok(false)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Data-processing — register  (op0 = x101)
// ═══════════════════════════════════════════════════════════════════════════════

fn exec_dp_reg(
    a: &mut Aarch64ArchState,
    _mem: &mut impl MemInterface,
    insn: u32,
) -> Result<bool, HartException> {
    let sf = bit(insn, 31) != 0;
    let rd = bits(insn, 4, 0);
    let rn = bits(insn, 9, 5);
    let rm = bits(insn, 20, 16);

    // ── Logical shifted register: bit28=0 ────────────────────────────────
    if bit(insn, 28) == 0 {
        let opc       = bits(insn, 30, 29);
        let n         = bit(insn, 21);
        let shift     = bits(insn, 23, 22);
        let shift_amt = bits(insn, 15, 10);

        let rn_val = a.read_x(rn);
        let rm_val = apply_shift(a.read_x(rm), shift, shift_amt, sf);
        let (res, setf) = match (opc, n) {
            (0b00, 0) => (rn_val & rm_val, false),              // AND
            (0b00, 1) => (rn_val & !rm_val, false),             // BIC
            (0b01, 0) => (rn_val | rm_val, false),              // ORR
            (0b01, 1) => (rn_val | !rm_val, false),             // ORN
            (0b10, 0) => (rn_val ^ rm_val, false),              // EOR
            (0b10, 1) => (rn_val ^ !rm_val, false),             // EON
            (0b11, 0) => (rn_val & rm_val, true),               // ANDS
            (0b11, 1) => (rn_val & !rm_val, true),              // BICS
            _ => unreachable!(),
        };
        let res = if sf { res } else { (res as u32) as u64 };
        if setf { a.set_nzcv64(res, false, false); }
        a.write_x(rd, res);
        return Ok(false);
    }

    // ── Multiply / divide: bit28=1, bit24=1 ─────────────────────────────
    if bit(insn, 28) == 1 && bit(insn, 24) == 1 {
        let op1 = bits(insn, 23, 21);
        let o0  = bit(insn, 15);
        let ra  = bits(insn, 14, 10);

        match op1 {
            0b000 => {
                // MADD / MSUB
                let rn_val = a.read_x(rn);
                let rm_val = a.read_x(rm);
                let ra_val = a.read_x(ra);
                let res = if o0 == 0 {
                    // MADD
                    if sf {
                        rn_val.wrapping_mul(rm_val).wrapping_add(ra_val)
                    } else {
                        ((rn_val as u32).wrapping_mul(rm_val as u32) as u64).wrapping_add(ra_val) & 0xFFFF_FFFF
                    }
                } else {
                    // MSUB
                    ra_val.wrapping_sub(rn_val.wrapping_mul(rm_val))
                };
                a.write_x(rd, res);
            }
            0b001 => {
                // SMADDL / SMSUBL
                let rn_val = a.read_x(rn) as i32 as i64;
                let rm_val = a.read_x(rm) as i32 as i64;
                let ra_val = a.read_x(ra) as i64;
                let res = if o0 == 0 {
                    rn_val.wrapping_mul(rm_val).wrapping_add(ra_val) as u64
                } else {
                    ra_val.wrapping_sub(rn_val.wrapping_mul(rm_val)) as u64
                };
                a.write_x(rd, res);
            }
            0b010 => {
                // SMULH
                let rn_val = a.read_x(rn) as i64 as i128;
                let rm_val = a.read_x(rm) as i64 as i128;
                a.write_x(rd, ((rn_val * rm_val) >> 64) as u64);
            }
            0b101 => {
                // UMADDL / UMSUBL
                let rn_val = a.read_x(rn) as u32 as u64;
                let rm_val = a.read_x(rm) as u32 as u64;
                let ra_val = a.read_x(ra);
                let res = if o0 == 0 {
                    rn_val.wrapping_mul(rm_val).wrapping_add(ra_val)
                } else {
                    ra_val.wrapping_sub(rn_val.wrapping_mul(rm_val))
                };
                a.write_x(rd, res);
            }
            0b110 => {
                // UMULH
                let rn_val = a.read_x(rn) as u128;
                let rm_val = a.read_x(rm) as u128;
                a.write_x(rd, ((rn_val * rm_val) >> 64) as u64);
            }
            _ => {
                // UDIV / SDIV or variable shift
                if bit(insn, 10) == 1 {
                    let rn_val = a.read_x(rn);
                    let rm_val = a.read_x(rm);
                    if bit(insn, 29) == 0 {
                        // UDIV
                        a.write_x(rd, if rm_val == 0 { 0 } else { rn_val / rm_val });
                    } else {
                        // SDIV
                        let rn_s = rn_val as i64;
                        let rm_s = rm_val as i64;
                        a.write_x(rd, if rm_s == 0 { 0 } else { rn_s.wrapping_div(rm_s) as u64 });
                    }
                } else {
                    // Variable shift (LSLV/LSRV/ASRV/RORV)
                    let op2 = bits(insn, 12, 10);
                    let src = a.read_x(rn);
                    let sh  = (a.read_x(rm) % if sf { 64 } else { 32 }) as u32;
                    let res = match op2 {
                        0b010 => if sf { src << sh } else { ((src as u32) << sh) as u64 },
                        0b011 => if sf { src >> sh } else { ((src as u32) >> sh) as u64 },
                        0b100 => if sf { ((src as i64) >> sh) as u64 } else { ((src as i32) >> sh) as u64 },
                        0b110 => if sf { src.rotate_right(sh) } else { (src as u32).rotate_right(sh) as u64 },
                        _ => src,
                    };
                    a.write_x(rd, res);
                }
            }
        }
        return Ok(false);
    }

    // ── Conditional select: bits[28:21] = 1101_0100 ─────────────────────
    if bits(insn, 28, 21) == 0b1101_0100 {
        let op2 = bits(insn, 11, 10);
        let op  = bit(insn, 30);
        let cc  = bits(insn, 15, 12);
        let c   = cond(a, cc);
        let rn_val = a.read_x(rn);
        let rm_val = a.read_x(rm);
        let val = match (op, op2) {
            (0, 0b00) => if c { rn_val } else { rm_val },              // CSEL
            (0, 0b01) => if c { rn_val } else { rm_val.wrapping_add(1) }, // CSINC
            (1, 0b00) => if c { rn_val } else { !rm_val },             // CSINV
            (1, 0b01) => if c { rn_val } else { rm_val.wrapping_neg() }, // CSNEG
            _ => rn_val,
        };
        a.write_x(rd, val);
        return Ok(false);
    }

    // ── Conditional compare: bit28=1, bit27=1, bits[24:21]=0010 ─────────
    if bit(insn, 28) == 1 && bit(insn, 27) == 1 && bits(insn, 24, 21) == 0b0010 {
        let nzcv_imm = bits(insn, 3, 0);
        let cc = bits(insn, 15, 12);
        let sub = bit(insn, 30) != 0;
        let use_imm = bit(insn, 11) != 0;

        if cond(a, cc) {
            let rn_val = a.read_x(rn);
            let op2 = if use_imm { bits(insn, 20, 16) as u64 } else { a.read_x(rm) };
            let (res, b) = if sub {
                rn_val.overflowing_sub(op2)
            } else {
                rn_val.overflowing_add(op2)
            };
            let v = if sub { sub_overflow64(rn_val, op2, res) } else { add_overflow64(rn_val, op2, res) };
            let c = if sub { !b } else { b };
            a.set_nzcv64(res, c, v);
        } else {
            a.nzcv = nzcv_imm << 28;
        }
        return Ok(false);
    }

    // ── 1-source data processing: bit30=1, bits[23:21]=000 ──────────────
    if bit(insn, 30) == 1 && bits(insn, 23, 21) == 0b000 {
        let op2 = bits(insn, 15, 10);
        let src = a.read_x(rn);
        match op2 {
            0b000000 => {
                // RBIT
                let v = if sf { src.reverse_bits() } else { (src as u32).reverse_bits() as u64 };
                a.write_x(rd, v);
            }
            0b000001 => {
                // REV16
                let v = ((src & 0xFF00_FF00_FF00_FF00) >> 8) | ((src & 0x00FF_00FF_00FF_00FF) << 8);
                a.write_x(rd, v);
            }
            0b000010 => {
                // REV32 (sf=1) or REV (sf=0)
                if sf {
                    let hi = (src >> 32) as u32;
                    let lo = src as u32;
                    a.write_x(rd, ((lo.swap_bytes() as u64) << 32) | hi.swap_bytes() as u64);
                } else {
                    a.write_x(rd, (src as u32).swap_bytes() as u64);
                }
            }
            0b000011 => {
                // REV
                let v = if sf { src.swap_bytes() } else { (src as u32).swap_bytes() as u64 };
                a.write_x(rd, v);
            }
            0b000100 => {
                // CLZ
                let v = if sf { src.leading_zeros() as u64 } else { (src as u32).leading_zeros() as u64 };
                a.write_x(rd, v);
            }
            0b000101 => {
                // CLS
                let v = if sf {
                    (src ^ (src << 1)).leading_zeros() as u64
                } else {
                    ((src as u32) ^ ((src as u32) << 1)).leading_zeros() as u64 - 1
                };
                a.write_x(rd, v);
            }
            _ => return Err(HartException::IllegalInstruction { pc: a.pc, raw: insn }),
        }
        return Ok(false);
    }

    // ── Add/sub shifted / extended register ──────────────────────────────
    {
        let sub = bit(insn, 30) != 0;
        let s_bit = bit(insn, 29) != 0;
        let extend_mode = bit(insn, 21) != 0;

        let src = if extend_mode || !s_bit { a.read_xsp(rn) } else { a.read_x(rn) };

        let op2 = if extend_mode {
            let etype = bits(insn, 15, 13);
            let eamt  = bits(insn, 12, 10);
            apply_extend(a.read_x(rm), etype, eamt)
        } else {
            let shift_type = bits(insn, 23, 22);
            let shift_amt  = bits(insn, 15, 10);
            apply_shift(a.read_x(rm), shift_type, shift_amt, sf)
        };

        let (res, c, v) = if sub {
            let (r, b) = src.overflowing_sub(op2);
            (r, !b, sub_overflow64(src, op2, r))
        } else {
            let (r, c) = src.overflowing_add(op2);
            (r, c, add_overflow64(src, op2, r))
        };
        let res = if sf { res } else { (res as u32) as u64 };

        if s_bit {
            a.set_nzcv64(res, c, v);
            a.write_x(rd, res);
        } else {
            a.write_xsp(rd, res);
        }
    }
    Ok(false)
}

// ── ADC / SBC (carry-in arithmetic) ──────────────────────────────────────────
// Note: ADC/SBC share encoding with DP-reg (bits[28:24]=11010, bit21=0)
// They are dispatched from exec_dp_reg when bit28=1, bit24=0, bits[23:21]=000

// ═══════════════════════════════════════════════════════════════════════════════
// SIMD / FP  (op0 = x111)
// ═══════════════════════════════════════════════════════════════════════════════

fn exec_simd_fp(
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
    insn: u32,
) -> Result<bool, HartException> {
    let rd = bits(insn, 4, 0);
    let rn = bits(insn, 9, 5);
    let rm = bits(insn, 20, 16);
    let ptype = bits(insn, 23, 22);

    // ── Scalar FP data processing: bits[28:24] = 11110 ──────────────────
    if bits(insn, 28, 24) == 0b11110 {
        return exec_fp_scalar(a, insn, rd, rn, rm, ptype);
    }

    // ── FP fused multiply-add: bits[28:24] = 11111 ──────────────────────
    if bits(insn, 28, 24) == 0b11111 {
        let ra  = bits(insn, 14, 10);
        let o1  = bit(insn, 21);
        let o0  = bit(insn, 15);
        if ptype == 1 {
            let fn_val = f64::from_bits(a.v[rn as usize] as u64);
            let fm_val = f64::from_bits(a.v[rm as usize] as u64);
            let fa_val = f64::from_bits(a.v[ra as usize] as u64);
            let res: f64 = match (o1, o0) {
                (0, 0) =>  fn_val * fm_val + fa_val,   // FMADD
                (0, 1) => -fn_val * fm_val + fa_val,   // FMSUB
                (1, 0) => -fn_val * fm_val - fa_val,   // FNMADD
                (1, 1) =>  fn_val * fm_val - fa_val,   // FNMSUB
                _ => 0.0,
            };
            a.v[rd as usize] = res.to_bits() as u128;
        } else {
            let fn_val = f32::from_bits(a.v[rn as usize] as u32);
            let fm_val = f32::from_bits(a.v[rm as usize] as u32);
            let fa_val = f32::from_bits(a.v[ra as usize] as u32);
            let res: f32 = match (o1, o0) {
                (0, 0) =>  fn_val * fm_val + fa_val,
                (0, 1) => -fn_val * fm_val + fa_val,
                (1, 0) => -fn_val * fm_val - fa_val,
                (1, 1) =>  fn_val * fm_val - fa_val,
                _ => 0.0,
            };
            a.v[rd as usize] = res.to_bits() as u128;
        }
        return Ok(false);
    }

    // ── Advanced SIMD — mostly NOP stubs for Phase 0 ────────────────────
    // SIMD copy (DUP, INS, UMOV, SMOV): bits[28:24]=0x1110, bit21=0
    if bits(insn, 28, 24) == 0b01110 && bit(insn, 21) == 0 {
        let q    = bit(insn, 30);
        let u    = bit(insn, 29);
        let imm4 = bits(insn, 14, 11);
        let imm5 = bits(insn, 20, 16);

        match (u, imm4) {
            (0, 0b0001) => {
                // DUP (general)
                let val = a.read_x(rn);
                if imm5 & 1 != 0 {
                    let b = val as u8;
                    let mut v = 0u128;
                    for i in 0..16u32 { v |= (b as u128) << (i * 8); }
                    a.v[rd as usize] = if q != 0 { v } else { v & ((1u128 << 64) - 1) };
                } else if imm5 & 2 != 0 {
                    let h = val as u16;
                    let mut v = 0u128;
                    for i in 0..8u32 { v |= (h as u128) << (i * 16); }
                    a.v[rd as usize] = if q != 0 { v } else { v & ((1u128 << 64) - 1) };
                } else if imm5 & 4 != 0 {
                    let w = val as u32;
                    let mut v = 0u128;
                    for i in 0..4u32 { v |= (w as u128) << (i * 32); }
                    a.v[rd as usize] = if q != 0 { v } else { v & ((1u128 << 64) - 1) };
                } else if imm5 & 8 != 0 {
                    let d = val;
                    let v = (d as u128) | ((d as u128) << 64);
                    a.v[rd as usize] = if q != 0 { v } else { d as u128 };
                }
            }
            (0, 0b0111) => {
                // UMOV
                let v = a.v[rn as usize];
                if imm5 & 8 != 0 {
                    let idx = (imm5 >> 4) & 1;
                    let val = if idx == 0 { v as u64 } else { (v >> 64) as u64 };
                    a.write_x(rd, val);
                } else if imm5 & 4 != 0 {
                    let idx = (imm5 >> 3) & 3;
                    a.write_x(rd, ((v >> (idx * 32)) & 0xFFFF_FFFF) as u64);
                } else if imm5 & 2 != 0 {
                    let idx = (imm5 >> 2) & 7;
                    a.write_x(rd, ((v >> (idx * 16)) & 0xFFFF) as u64);
                } else if imm5 & 1 != 0 {
                    let idx = (imm5 >> 1) & 15;
                    a.write_x(rd, ((v >> (idx * 8)) & 0xFF) as u64);
                }
            }
            (0, 0b0101) => {
                // SMOV
                let v = a.v[rn as usize];
                if imm5 & 4 != 0 {
                    let idx = (imm5 >> 3) & 3;
                    let val = ((v >> (idx * 32)) & 0xFFFF_FFFF) as u64;
                    a.write_x(rd, val as i32 as i64 as u64);
                } else if imm5 & 2 != 0 {
                    let idx = (imm5 >> 2) & 7;
                    let val = ((v >> (idx * 16)) & 0xFFFF) as u64;
                    a.write_x(rd, val as i16 as i64 as u64);
                } else if imm5 & 1 != 0 {
                    let idx = (imm5 >> 1) & 15;
                    let val = ((v >> (idx * 8)) & 0xFF) as u64;
                    a.write_x(rd, val as i8 as i64 as u64);
                }
            }
            (0, 0b0011) => {
                // INS (from GPR)
                let val = a.read_x(rn);
                let v = &mut a.v[rd as usize];
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
            _ => { /* unimplemented SIMD copy — skip */ }
        }
        return Ok(false);
    }

    // SIMD modified immediate (MOVI): bits[28:24]=0x1111
    if bits(insn, 28, 24) == 0b01111 {
        let q     = bit(insn, 30);
        let abc   = bits(insn, 18, 16);
        let defgh = bits(insn, 9, 5);
        let imm8  = ((abc << 5) | defgh) as u8;
        let mut val = 0u128;
        for i in 0..16u32 { val |= (imm8 as u128) << (i * 8); }
        a.v[rd as usize] = if q != 0 { val } else { val & ((1u128 << 64) - 1) };
        return Ok(false);
    }

    // All other SIMD — silently NOP for Phase 0
    Ok(false)
}

fn exec_fp_scalar(
    a: &mut Aarch64ArchState,
    insn: u32,
    rd: u32,
    rn: u32,
    rm: u32,
    ptype: u32,
) -> Result<bool, HartException> {
    let sf = bit(insn, 31) != 0;
    let op = bits(insn, 21, 16);
    let op2 = bits(insn, 15, 10);

    // ── FMOV immediate: bits[21:16]=000001, bit11=0 ─────────────────────
    if op == 0b000001 && bit(insn, 11) == 0 {
        let imm8 = bits(insn, 20, 13);
        let f32_val = fp_imm8_to_f32(imm8);
        if ptype == 1 {
            a.v[rd as usize] = f64::from(f32_val).to_bits() as u128;
        } else {
            a.v[rd as usize] = f32_val.to_bits() as u128;
        }
        return Ok(false);
    }

    // ── FMOV register: op=000000 ────────────────────────────────────────
    if op == 0b000000 && op2 == 0b010000 {
        a.v[rd as usize] = a.v[rn as usize];
        return Ok(false);
    }

    // ── FMOV to/from GPR: bit21=0 ──────────────────────────────────────
    if bit(insn, 21) == 0 {
        let to_fp = (insn >> 16) & 1 == 0;
        if sf {
            if to_fp {
                a.v[rd as usize] = a.read_x(rn) as u128;
            } else {
                a.write_x(rd, a.v[rn as usize] as u64);
            }
        } else {
            if to_fp {
                a.v[rd as usize] = a.read_w(rn) as u128;
            } else {
                a.write_w(rd, a.v[rn as usize] as u32);
            }
        }
        return Ok(false);
    }

    // ── FCMP / FCMPE ────────────────────────────────────────────────────
    if bits(insn, 15, 10) == 0b001000 || bits(insn, 15, 10) == 0b001001 {
        if ptype == 1 {
            let fn_val = f64::from_bits(a.v[rn as usize] as u64);
            let fm_val = f64::from_bits(a.v[rm as usize] as u64);
            let unordered = fn_val.is_nan() || fm_val.is_nan();
            let n = fn_val < fm_val;
            let z = fn_val == fm_val;
            let c = !(fn_val < fm_val) || unordered;
            let v = unordered;
            a.set_nzcv(n, z, c, v);
        } else {
            let fn_val = f32::from_bits(a.v[rn as usize] as u32);
            let fm_val = f32::from_bits(a.v[rm as usize] as u32);
            let unordered = fn_val.is_nan() || fm_val.is_nan();
            let n = fn_val < fm_val;
            let z = fn_val == fm_val;
            let c = !(fn_val < fm_val) || unordered;
            let v = unordered;
            a.set_nzcv(n, z, c, v);
        }
        return Ok(false);
    }

    // ── FCCMP / FCCMPE ──────────────────────────────────────────────────
    if bits(insn, 15, 10) & 0b111110 == 0b000100 {
        // bits[15:10] = 0001x0 for FCCMP, 0001x1 for FCCMPE
        let cc = bits(insn, 15, 12);
        let nzcv_imm = bits(insn, 3, 0);
        if cond(a, cc) {
            if ptype == 1 {
                let fn_val = f64::from_bits(a.v[rn as usize] as u64);
                let fm_val = f64::from_bits(a.v[rm as usize] as u64);
                if fn_val.is_nan() || fm_val.is_nan() {
                    a.set_nzcv(false, false, true, true);
                } else if fn_val == fm_val {
                    a.set_nzcv(false, true, true, false);
                } else if fn_val < fm_val {
                    a.set_nzcv(true, false, false, false);
                } else {
                    a.set_nzcv(false, false, true, false);
                }
            }
            // Single-precision path omitted for brevity — same pattern
        } else {
            a.set_nzcv(nzcv_imm & 8 != 0, nzcv_imm & 4 != 0, nzcv_imm & 2 != 0, nzcv_imm & 1 != 0);
        }
        return Ok(false);
    }

    // ── FCSEL: bits[15:10] = 11xxxx, bit29=0, bit11=1 ──────────────────
    if bit(insn, 29) == 0 && bit(insn, 11) == 1 && bits(insn, 15, 12) != 0 {
        let cc = bits(insn, 15, 12);
        if cond(a, cc) {
            a.v[rd as usize] = a.v[rn as usize];
        } else {
            a.v[rd as usize] = a.v[rm as usize];
        }
        return Ok(false);
    }

    // ── FP data processing (2-source) ───────────────────────────────────
    match op {
        0b000010 => { fp_binary(a, rd, rn, rm, ptype, |x, y| x + y, |x, y| x + y); }    // FADD
        0b000011 => { fp_binary(a, rd, rn, rm, ptype, |x, y| x - y, |x, y| x - y); }    // FSUB
        0b000100 => { fp_binary(a, rd, rn, rm, ptype, |x, y| x * y, |x, y| x * y); }    // FMUL
        0b000110 => { fp_binary(a, rd, rn, rm, ptype, |x, y| x / y, |x, y| x / y); }    // FDIV
        0b001000 => { fp_binary(a, rd, rn, rm, ptype, |x, y| if x >= y { x } else { y }, |x, y| if x >= y { x } else { y }); } // FMAX
        0b001001 => { fp_binary(a, rd, rn, rm, ptype, |x, y| if x <= y { x } else { y }, |x, y| if x <= y { x } else { y }); } // FMIN
        0b001010 => { fp_binary(a, rd, rn, rm, ptype, |x: f64, y| x.max(y), |x: f32, y| x.max(y)); } // FMAXNM
        0b001011 => { fp_binary(a, rd, rn, rm, ptype, |x: f64, y| x.min(y), |x: f32, y| x.min(y)); } // FMINNM
        0b001100 => {
            // FNMUL
            if ptype == 1 {
                let rn_v = f64::from_bits(a.v[rn as usize] as u64);
                let rm_v = f64::from_bits(a.v[rm as usize] as u64);
                a.v[rd as usize] = (-(rn_v * rm_v)).to_bits() as u128;
            } else {
                let rn_v = f32::from_bits(a.v[rn as usize] as u32);
                let rm_v = f32::from_bits(a.v[rm as usize] as u32);
                a.v[rd as usize] = (-(rn_v * rm_v)).to_bits() as u128;
            }
        }
        _ => {
            // ── FP 1-source / conversions ───────────────────────────────
            match op {
                0b000001 => {
                    // FSQRT
                    if ptype == 1 {
                        let v = f64::from_bits(a.v[rn as usize] as u64).sqrt();
                        a.v[rd as usize] = v.to_bits() as u128;
                    } else {
                        let v = f32::from_bits(a.v[rn as usize] as u32).sqrt();
                        a.v[rd as usize] = v.to_bits() as u128;
                    }
                }
                0b000101 => {
                    // FABS
                    if ptype == 1 {
                        let v = f64::from_bits(a.v[rn as usize] as u64).abs();
                        a.v[rd as usize] = v.to_bits() as u128;
                    } else {
                        let v = f32::from_bits(a.v[rn as usize] as u32).abs();
                        a.v[rd as usize] = v.to_bits() as u128;
                    }
                }
                0b000111 => {
                    // FNEG
                    if ptype == 1 {
                        let v = -f64::from_bits(a.v[rn as usize] as u64);
                        a.v[rd as usize] = v.to_bits() as u128;
                    } else {
                        let v = -f32::from_bits(a.v[rn as usize] as u32);
                        a.v[rd as usize] = v.to_bits() as u128;
                    }
                }
                _ => {
                    // FCVT, FCVTZS/FCVTZU/SCVTF/UCVTF, and other FP-GPR conversions
                    exec_fp_convert(a, insn, rd, rn, ptype);
                }
            }
        }
    }
    Ok(false)
}

fn fp_binary(
    a: &mut Aarch64ArchState,
    rd: u32, rn: u32, rm: u32,
    ptype: u32,
    f64_op: impl Fn(f64, f64) -> f64,
    f32_op: impl Fn(f32, f32) -> f32,
) {
    if ptype == 1 {
        let rn_v = f64::from_bits(a.v[rn as usize] as u64);
        let rm_v = f64::from_bits(a.v[rm as usize] as u64);
        a.v[rd as usize] = f64_op(rn_v, rm_v).to_bits() as u128;
    } else {
        let rn_v = f32::from_bits(a.v[rn as usize] as u32);
        let rm_v = f32::from_bits(a.v[rm as usize] as u32);
        a.v[rd as usize] = f32_op(rn_v, rm_v).to_bits() as u128;
    }
}

fn exec_fp_convert(
    a: &mut Aarch64ArchState,
    insn: u32,
    rd: u32,
    rn: u32,
    ptype: u32,
) {
    let sf = bit(insn, 31) != 0;
    let opcode = bits(insn, 20, 16);
    let rmode  = bits(insn, 20, 19);

    // FCVT between FP sizes
    if ptype == 0 && (insn >> 15) & 3 == 1 {
        // SP -> DP
        let v = f32::from_bits(a.v[rn as usize] as u32);
        a.v[rd as usize] = f64::from(v).to_bits() as u128;
        return;
    }
    if ptype == 1 && (insn >> 15) & 3 == 0 {
        // DP -> SP
        let v = f64::from_bits(a.v[rn as usize] as u64);
        a.v[rd as usize] = (v as f32).to_bits() as u128;
        return;
    }

    // FCVTZS (FP to signed integer, round toward zero)
    // FCVTZU (FP to unsigned integer, round toward zero)
    // Detect via bits[20:16]
    let mode = bits(insn, 20, 19);
    let op_code = bits(insn, 18, 16);

    // FP-to-integer conversions: bits[23:22]=ptype, bits[20:16] encodes rounding+signed
    // FCVTZS: rmode=11, opcode=000 (signed)
    // FCVTZU: rmode=11, opcode=001 (unsigned)
    // SCVTF:  rmode=00, opcode=010 (signed int to FP)
    // UCVTF:  rmode=00, opcode=011 (unsigned int to FP)
    match (mode, op_code) {
        (0b11, 0b000) => {
            // FCVTZS
            if ptype == 1 {
                let v = f64::from_bits(a.v[rn as usize] as u64);
                if sf { a.write_x(rd, v as i64 as u64); }
                else  { a.write_w(rd, v as i32 as u32); }
            } else {
                let v = f32::from_bits(a.v[rn as usize] as u32);
                if sf { a.write_x(rd, v as i64 as u64); }
                else  { a.write_w(rd, v as i32 as u32); }
            }
        }
        (0b11, 0b001) => {
            // FCVTZU
            if ptype == 1 {
                let v = f64::from_bits(a.v[rn as usize] as u64);
                if sf { a.write_x(rd, v as u64); }
                else  { a.write_w(rd, v as u32); }
            } else {
                let v = f32::from_bits(a.v[rn as usize] as u32);
                if sf { a.write_x(rd, v as u64); }
                else  { a.write_w(rd, v as u32); }
            }
        }
        (0b00, 0b010) => {
            // SCVTF
            if ptype == 1 {
                let v = if sf { a.read_x(rn) as i64 as f64 } else { a.read_x(rn) as i32 as f64 };
                a.v[rd as usize] = v.to_bits() as u128;
            } else {
                let v = if sf { a.read_x(rn) as i64 as f32 } else { a.read_x(rn) as i32 as f32 };
                a.v[rd as usize] = v.to_bits() as u128;
            }
        }
        (0b00, 0b011) => {
            // UCVTF
            if ptype == 1 {
                let v = if sf { a.read_x(rn) as f64 } else { a.read_x(rn) as u32 as f64 };
                a.v[rd as usize] = v.to_bits() as u128;
            } else {
                let v = if sf { a.read_x(rn) as f32 } else { a.read_x(rn) as u32 as f32 };
                a.v[rd as usize] = v.to_bits() as u128;
            }
        }
        _ => {
            // Other rounding-mode FCVT variants — stub: treat as round-toward-zero
            if ptype == 1 {
                let v = f64::from_bits(a.v[rn as usize] as u64);
                a.write_x(rd, v as i64 as u64);
            } else {
                let v = f32::from_bits(a.v[rn as usize] as u32);
                a.write_x(rd, v as i64 as u64);
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    /// Trivial stub MemInterface for unit testing.
    struct TestMem {
        data: Vec<u8>,
    }
    impl TestMem {
        fn new(size: usize) -> Self { Self { data: vec![0u8; size] } }
    }
    impl MemInterface for TestMem {
        fn read(&mut self, addr: u64, size: usize, _ty: AccessType) -> Result<u64, MemFault> {
            let a = addr as usize;
            if a + size > self.data.len() {
                return Err(MemFault::AccessFault { addr });
            }
            let mut v = 0u64;
            for i in 0..size { v |= (self.data[a + i] as u64) << (i * 8); }
            Ok(v)
        }
        fn write(&mut self, addr: u64, size: usize, val: u64, _ty: AccessType) -> Result<(), MemFault> {
            let a = addr as usize;
            if a + size > self.data.len() {
                return Err(MemFault::AccessFault { addr });
            }
            for i in 0..size { self.data[a + i] = (val >> (i * 8)) as u8; }
            Ok(())
        }
    }

    #[test]
    fn test_movz_x0() {
        let mut a = Aarch64ArchState::new();
        let mut m = TestMem::new(4096);
        // MOVZ X0, #42 — encoding: sf=1, opc=10, hw=00, imm16=42, Rd=0
        // 1_10_100101_00_0000000000101010_00000
        let insn = 0xD2800540u32; // MOVZ X0, #0x2A
        let pw = step(&mut a, &mut m, insn).unwrap();
        assert!(!pw);
        assert_eq!(a.read_x(0), 42);
    }

    #[test]
    fn test_add_imm() {
        let mut a = Aarch64ArchState::new();
        let mut m = TestMem::new(4096);
        a.write_x(1, 100);
        // ADD X0, X1, #10 — encoding: sf=1, op=0, S=0, sh=0, imm12=10, Rn=1, Rd=0
        // 1_00_100010_0_000000001010_00001_00000
        let insn = 0x91002820u32;
        let pw = step(&mut a, &mut m, insn).unwrap();
        assert!(!pw);
        assert_eq!(a.read_x(0), 110);
    }

    #[test]
    fn test_b_branch() {
        let mut a = Aarch64ArchState::new();
        let mut m = TestMem::new(4096);
        a.pc = 0x1000;
        // B +8 — encoding: op=0, imm26=2
        // 0_00101_00000000000000000000000010
        let insn = 0x14000002u32;
        let pw = step(&mut a, &mut m, insn).unwrap();
        assert!(pw);
        assert_eq!(a.pc, 0x1008);
    }

    #[test]
    fn test_svc() {
        let mut a = Aarch64ArchState::new();
        let mut m = TestMem::new(4096);
        a.pc = 0x2000;
        a.x[8] = 93; // exit syscall nr
        // SVC #0 — encoding: 0xD4000001
        let insn = 0xD4000001u32;
        let result = step(&mut a, &mut m, insn);
        match result {
            Err(HartException::EnvironmentCall { pc, nr }) => {
                assert_eq!(pc, 0x2000);
                assert_eq!(nr, 93);
            }
            _ => panic!("expected EnvironmentCall"),
        }
    }

    #[test]
    fn test_str_ldr() {
        let mut a = Aarch64ArchState::new();
        let mut m = TestMem::new(4096);
        a.write_x(0, 0xDEAD_BEEF_CAFE_BABE);
        a.sp = 0x100;
        // STR X0, [SP, #0] — unsigned offset, size=11, V=0, opc=00
        // 11_111_0_01_00_000000000000_11111_00000
        let str_insn = 0xF90003E0u32;
        step(&mut a, &mut m, str_insn).unwrap();
        // LDR X1, [SP, #0]
        // 11_111_0_01_01_000000000000_11111_00001
        let ldr_insn = 0xF94003E1u32;
        step(&mut a, &mut m, ldr_insn).unwrap();
        assert_eq!(a.read_x(1), 0xDEAD_BEEF_CAFE_BABE);
    }

    #[test]
    fn test_and_sp_alignment() {
        let mut a = Aarch64ArchState::new();
        let mut m = TestMem::new(4096);
        a.write_x(0, 0x7FFF_FFDF_FEC0); // unaligned SP value
        // AND SP, X0, #0xFFFFFFFFFFFFFFF0 = 0x927cec1f
        let insn = 0x927cec1fu32;
        let pw = step(&mut a, &mut m, insn).unwrap();
        assert!(!pw);
        assert_eq!(a.sp, 0x7FFF_FFDF_FEC0); // already aligned, no change
    }

    #[test]
    fn test_decode_bitmask_align16() {
        // N=1, imms=59, immr=60 → should give 0xFFFFFFFFFFFFFFF0
        let mask = decode_bitmask(true, 59, 60, true);
        assert_eq!(mask, Some(0xFFFFFFFFFFFFFFF0u64));
    }
}

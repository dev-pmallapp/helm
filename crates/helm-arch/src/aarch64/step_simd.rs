//! AArch64 SIMD/FP and data-processing-register execution (raw instruction word).
//!
//! Three entry points called from the top-level raw-word executor:
//! - [`exec_dp_reg`]   — data-processing register group
//! - [`exec_simd_dp`]  — SIMD/FP data-processing group
//! - [`exec_ldst_simd`] — SIMD/FP loads and stores
//!
//! These operate on the raw 32-bit instruction word (not a decoded `Instruction`).
//! They are the faithful port of the reference interpreter's bit-pattern matching.

#![allow(clippy::unusual_byte_groupings)]
#![allow(clippy::unnecessary_cast, clippy::identity_op)]
#![allow(dead_code)]

use helm_core::{AccessType, HartException, MemInterface};

use super::arch_state::Aarch64ArchState;

// ═══════════════════════════════════════════════════════════════════════════════
// Helper: memory access wrappers
// ═══════════════════════════════════════════════════════════════════════════════

fn rd(mem: &mut impl MemInterface, addr: u64, sz: usize) -> Result<u64, HartException> {
    mem.read(addr, sz, AccessType::Load)
        .map_err(|_| HartException::LoadAccessFault { addr })
}

fn wr(mem: &mut impl MemInterface, addr: u64, val: u64, sz: usize) -> Result<(), HartException> {
    mem.write(addr, sz, val, AccessType::Store)
        .map_err(|_| HartException::StoreAccessFault { addr })
}

fn rd128(mem: &mut impl MemInterface, addr: u64) -> Result<u128, HartException> {
    let lo = mem.read(addr, 8, AccessType::Load)
        .map_err(|_| HartException::LoadAccessFault { addr })?;
    let hi = mem.read(addr + 8, 8, AccessType::Load)
        .map_err(|_| HartException::LoadAccessFault { addr: addr + 8 })?;
    Ok((hi as u128) << 64 | lo as u128)
}

fn wr128(mem: &mut impl MemInterface, addr: u64, val: u128) -> Result<(), HartException> {
    mem.write(addr, 8, val as u64, AccessType::Store)
        .map_err(|_| HartException::StoreAccessFault { addr })?;
    mem.write(addr + 8, 8, (val >> 64) as u64, AccessType::Store)
        .map_err(|_| HartException::StoreAccessFault { addr: addr + 8 })
}

// ═══════════════════════════════════════════════════════════════════════════════
// Helper: arithmetic
// ═══════════════════════════════════════════════════════════════════════════════

/// Add-with-carry: (a + b + cin) returning (result, carry, overflow).
fn awc(a: u64, b: u64, cin: bool, is64: bool) -> (u64, bool, bool) {
    if is64 {
        let (s1, c1) = a.overflowing_add(b);
        let (s2, c2) = s1.overflowing_add(cin as u64);
        let carry = c1 || c2;
        let ov = {
            let sa = (a >> 63) & 1;
            let sb = (b >> 63) & 1;
            let sr = (s2 >> 63) & 1;
            sa == sb && sa != sr
        };
        (s2, carry, ov)
    } else {
        let a = a as u32;
        let b = b as u32;
        let (s1, c1) = a.overflowing_add(b);
        let (s2, c2) = s1.overflowing_add(cin as u32);
        let carry = c1 || c2;
        let ov = {
            let sa = (a >> 31) & 1;
            let sb = (b >> 31) & 1;
            let sr = (s2 >> 31) & 1;
            sa == sb && sa != sr
        };
        (s2 as u64, carry, ov)
    }
}

/// Apply a shift (LSL=0, LSR=1, ASR=2, ROR=3) to a 32- or 64-bit value.
fn shft(val: u64, t: u32, amt: u32, is64: bool) -> u64 {
    if amt == 0 {
        return val;
    }
    match t {
        0 => val.wrapping_shl(amt),
        1 => {
            if is64 { val.wrapping_shr(amt) }
            else { ((val as u32).wrapping_shr(amt)) as u64 }
        }
        2 => {
            if is64 { ((val as i64).wrapping_shr(amt)) as u64 }
            else { ((val as i32).wrapping_shr(amt)) as u32 as u64 }
        }
        3 => val.rotate_right(amt),
        _ => val,
    }
}

/// Mask to 32-bit when sf==0.
#[inline]
fn mask(v: u64, sf: u32) -> u64 {
    if sf == 1 { v } else { v & 0xFFFF_FFFF }
}

/// Sign-extend a 32-bit value from `bits` width.
fn sext(val: u32, bits: u32) -> i64 {
    let s = 32 - bits;
    ((val << s) as i32 >> s) as i64
}

/// Sign-extend a 64-bit value from `bits` width.
fn sext64(val: u64, bits: u32) -> u64 {
    if bits >= 64 { val }
    else {
        let s = 64 - bits;
        ((val << s) as i64 >> s) as u64
    }
}

/// Set NZCV flags from result, carry, overflow and width.
#[inline]
fn flags(a: &mut Aarch64ArchState, r: u64, c: bool, v: bool, is64: bool) {
    let n = if is64 { r >> 63 != 0 } else { (r >> 31) & 1 != 0 };
    let z = if is64 { r == 0 } else { r & 0xFFFF_FFFF == 0 };
    a.set_nzcv(n, z, c, v);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Helper: CRC32
// ═══════════════════════════════════════════════════════════════════════════════

/// CRC32 (ISO 3309) one byte: polynomial 0x04C11DB7 (reflected: 0xEDB88320).
fn crc32_byte(crc: u32, byte: u8) -> u32 {
    let mut c = crc ^ (byte as u32);
    for _ in 0..8 {
        c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
    }
    c
}

/// CRC32C (Castagnoli) one byte: reflected polynomial 0x82F63B78.
fn crc32c_byte(crc: u32, byte: u8) -> u32 {
    let mut c = crc ^ (byte as u32);
    for _ in 0..8 {
        c = if c & 1 != 0 { (c >> 1) ^ 0x82F6_3B78 } else { c >> 1 };
    }
    c
}

/// Highest set bit position in `val` within `width` bits.
fn hsb(val: u32, width: u32) -> u32 {
    for i in (0..width).rev() {
        if (val >> i) & 1 != 0 { return i; }
    }
    0
}

/// Decode the AArch64 logical-immediate bitmask.
fn decode_bitmask(n: u32, imms: u32, immr: u32, is64: bool) -> u64 {
    let len = hsb((n << 6) | (!imms & 0x3F), 7);
    if len < 1 { return 0; }
    let levels = (1u32 << len) - 1;
    let s = imms & levels;
    let r = immr & levels;
    let esize = 1u64 << len;
    let welem = if s + 1 >= 64 { u64::MAX } else { (1u64 << (s + 1)) - 1 };
    let emask = if esize >= 64 { u64::MAX } else { (1u64 << esize) - 1 };
    let elem = if r == 0 {
        welem
    } else if esize >= 64 {
        welem.rotate_right(r)
    } else {
        ((welem >> r) | (welem << (esize as u32 - r))) & emask
    };
    let rsz = if is64 { 64u64 } else { 32 };
    let mut result = 0u64;
    let mut pos = 0u64;
    while pos < rsz {
        result |= elem << pos;
        pos += esize;
    }
    if !is64 { result &= 0xFFFF_FFFF; }
    result
}

// ═══════════════════════════════════════════════════════════════════════════════
// exec_dp_reg — Data Processing Register group
// ═══════════════════════════════════════════════════════════════════════════════

/// Execute a data-processing-register instruction from the raw 32-bit word.
pub fn exec_dp_reg(
    a: &mut Aarch64ArchState,
    insn: u32,
    _mem: &mut impl MemInterface,
) -> Result<(), HartException> {
    let sf = (insn >> 31) & 1;

    // ── Add/sub shifted register ─────────────────────────────────────────────
    if (insn >> 24) & 0x1F == 0b01011 && (insn >> 21) & 1 == 0 {
        let op = (insn >> 30) & 1;
        let s = (insn >> 29) & 1;
        let sht = (insn >> 22) & 0x3;
        let rm = ((insn >> 16) & 0x1F) as u32;
        let imm6 = ((insn >> 10) & 0x3F) as u32;
        let rn = ((insn >> 5) & 0x1F) as u32;
        let rd = (insn & 0x1F) as u32;
        let va = a.read_x(rn);
        let b = shft(a.read_x(rm), sht, imm6, sf == 1);
        let (r, c, v) = if op == 0 { awc(va, b, false, sf == 1) }
                         else       { awc(va, !b, true, sf == 1) };
        let r = mask(r, sf);
        if s == 1 { flags(a, r, c, v, sf == 1); }
        a.write_x(rd, r);
        return Ok(());
    }

    // ── Add/sub extended register ────────────────────────────────────────────
    if (insn >> 24) & 0x1F == 0b01011 && (insn >> 21) & 1 == 1 {
        let op = (insn >> 30) & 1;
        let s = (insn >> 29) & 1;
        let rm = ((insn >> 16) & 0x1F) as u32;
        let option = (insn >> 13) & 0x7;
        let imm3 = ((insn >> 10) & 0x7) as u32;
        let rn = ((insn >> 5) & 0x1F) as u32;
        let rd = (insn & 0x1F) as u32;
        let va = a.read_xsp(rn);
        let mut b = a.read_x(rm);
        b = match option {
            0 => b & 0xFF,
            1 => b & 0xFFFF,
            2 => b & 0xFFFF_FFFF,
            3 => b,
            4 => sext64(b & 0xFF, 8),
            5 => sext64(b & 0xFFFF, 16),
            6 => sext64(b & 0xFFFF_FFFF, 32),
            _ => b,
        };
        b = b.wrapping_shl(imm3);
        let (r, c, v) = if op == 0 { awc(va, b, false, sf == 1) }
                         else       { awc(va, !b, true, sf == 1) };
        let r = mask(r, sf);
        if s == 1 { flags(a, r, c, v, sf == 1); }
        if s == 0 { a.write_xsp(rd, r); } else { a.write_x(rd, r); }
        return Ok(());
    }

    // ── Logical shifted register ─────────────────────────────────────────────
    if (insn >> 24) & 0x1F == 0b01010 {
        let opc = (insn >> 29) & 0x3;
        let n = (insn >> 21) & 1;
        let sht = (insn >> 22) & 0x3;
        let rm = ((insn >> 16) & 0x1F) as u32;
        let imm6 = ((insn >> 10) & 0x3F) as u32;
        let rn = ((insn >> 5) & 0x1F) as u32;
        let rd = (insn & 0x1F) as u32;
        let va = a.read_x(rn);
        let mut b = shft(a.read_x(rm), sht, imm6, sf == 1);
        if n == 1 { b = !b; }
        let r = match opc {
            0 => va & b,
            1 => va | b,
            2 => va ^ b,
            3 => {
                let r = mask(va & b, sf);
                flags(a, r, a.flag_c(), a.flag_v(), sf == 1);
                r
            }
            _ => va,
        };
        a.write_x(rd, mask(r, sf));
        return Ok(());
    }

    // ── Multiply 3-source (MADD / MSUB / SMADDL / UMADDL / SMULH / UMULH) ──
    if (insn >> 24) & 0x1F == 0b11011 {
        let op31 = (insn >> 21) & 0x7;
        let rm = ((insn >> 16) & 0x1F) as u32;
        let o0 = (insn >> 15) & 1;
        let ra = ((insn >> 10) & 0x1F) as u32;
        let rn = ((insn >> 5) & 0x1F) as u32;
        let rd = (insn & 0x1F) as u32;
        match op31 {
            0 => {
                // MADD / MSUB (32-bit or 64-bit)
                let p = if sf == 1 {
                    a.read_x(rn).wrapping_mul(a.read_x(rm))
                } else {
                    (a.read_w(rn).wrapping_mul(a.read_w(rm))) as u64
                };
                let r = if o0 == 0 { a.read_x(ra).wrapping_add(p) }
                        else       { a.read_x(ra).wrapping_sub(p) };
                a.write_x(rd, mask(r, sf));
            }
            1 => {
                // SMADDL / SMSUBL
                let p = (a.read_w(rn) as i32 as i64).wrapping_mul(a.read_w(rm) as i32 as i64);
                let r = if o0 == 0 { (a.read_x(ra) as i64).wrapping_add(p) }
                        else       { (a.read_x(ra) as i64).wrapping_sub(p) };
                a.write_x(rd, r as u64);
            }
            2 => {
                // SMULH
                let r = ((a.read_x(rn) as i64 as i128) * (a.read_x(rm) as i64 as i128)) >> 64;
                a.write_x(rd, r as u64);
            }
            5 => {
                // UMADDL / UMSUBL
                let p = (a.read_w(rn) as u64).wrapping_mul(a.read_w(rm) as u64);
                let r = if o0 == 0 { a.read_x(ra).wrapping_add(p) }
                        else       { a.read_x(ra).wrapping_sub(p) };
                a.write_x(rd, r);
            }
            6 => {
                // UMULH
                let r = ((a.read_x(rn) as u128) * (a.read_x(rm) as u128)) >> 64;
                a.write_x(rd, r as u64);
            }
            _ => { /* unimplemented dp3_multiply — skip */ }
        }
        return Ok(());
    }

    // ── 2-source: UDIV/SDIV/LSLV/LSRV/ASRV/RORV/CRC32 ─────────────────────
    if (insn >> 21) & 0x3FF == 0xD6 {
        let rm = ((insn >> 16) & 0x1F) as u32;
        let op2 = (insn >> 10) & 0x3F;
        let rn = ((insn >> 5) & 0x1F) as u32;
        let rd = (insn & 0x1F) as u32;
        let va = a.read_x(rn);
        let b = a.read_x(rm);
        let bits = if sf == 1 { 64u32 } else { 32 };
        let r = match op2 {
            // UDIV
            2 => {
                if sf == 1 {
                    if b == 0 { 0 } else { va / b }
                } else {
                    (if a.read_w(rm) == 0 { 0 } else { a.read_w(rn) / a.read_w(rm) }) as u64
                }
            }
            // SDIV
            3 => {
                if sf == 1 {
                    if b == 0 { 0 } else { (va as i64).wrapping_div(b as i64) as u64 }
                } else {
                    (if a.read_w(rm) as i32 == 0 { 0 }
                     else { (a.read_w(rn) as i32).wrapping_div(a.read_w(rm) as i32) })
                        as u32 as u64
                }
            }
            // LSLV
            8 => va.wrapping_shl(b as u32 % bits),
            // LSRV
            9 => {
                if sf == 1 { va.wrapping_shr(b as u32 % bits) }
                else { (a.read_w(rn).wrapping_shr(b as u32 % bits)) as u64 }
            }
            // ASRV
            10 => {
                if sf == 1 { ((va as i64).wrapping_shr(b as u32 % bits)) as u64 }
                else { ((a.read_w(rn) as i32).wrapping_shr(b as u32 % bits)) as u32 as u64 }
            }
            // RORV
            11 => va.rotate_right(b as u32 % bits),
            // CRC32B/H/W/X (op2=16-19), CRC32CB/CH/CW/CX (op2=20-23)
            16..=23 => {
                let crc_c = op2 >= 20;
                let sz = (op2 & 3) as u32; // 0=B, 1=H, 2=W, 3=X
                let mut crc = a.read_w(rn);
                let data = if sz == 3 { a.read_x(rm) } else { a.read_w(rm) as u64 };
                let nbytes = 1usize << sz;
                for i in 0..nbytes {
                    let byte = ((data >> (i * 8)) & 0xFF) as u8;
                    crc = if crc_c { crc32c_byte(crc, byte) } else { crc32_byte(crc, byte) };
                }
                crc as u64
            }
            _ => va,
        };
        a.write_x(rd, mask(r, sf));
        return Ok(());
    }

    // ── 1-source: RBIT/REV16/REV32/REV/CLZ/CLS ──────────────────────────────
    if (insn >> 21) & 0x3FF == 0x2D6 {
        let op2 = (insn >> 10) & 0x3F;
        let rn = ((insn >> 5) & 0x1F) as u32;
        let rd = (insn & 0x1F) as u32;
        let va = a.read_x(rn);
        let r = match op2 {
            // RBIT
            0 => {
                if sf == 1 { va.reverse_bits() }
                else { (a.read_w(rn).reverse_bits()) as u64 }
            }
            // REV16
            1 => {
                let swap16 = |v: u16| -> u16 { v.swap_bytes() };
                if sf == 1 {
                    let b = va.to_le_bytes();
                    u64::from_le_bytes([b[1],b[0],b[3],b[2],b[5],b[4],b[7],b[6]])
                } else {
                    let w = a.read_w(rn);
                    let lo = swap16(w as u16) as u32;
                    let hi = swap16((w >> 16) as u16) as u32;
                    ((hi << 16) | lo) as u64
                }
            }
            // REV32 (64-bit) / REV (32-bit)
            2 => {
                if sf == 1 {
                    let b = va.to_le_bytes();
                    u64::from_le_bytes([b[3],b[2],b[1],b[0],b[7],b[6],b[5],b[4]])
                } else {
                    (a.read_w(rn).swap_bytes()) as u64
                }
            }
            // REV (64-bit)
            3 => {
                if sf == 1 { va.swap_bytes() }
                else { (a.read_w(rn).swap_bytes()) as u64 }
            }
            // CLZ
            4 => {
                if sf == 1 { va.leading_zeros() as u64 }
                else { a.read_w(rn).leading_zeros() as u64 }
            }
            // CLS
            5 => {
                if sf == 1 {
                    let s = if va >> 63 == 1 { (!va).leading_zeros() }
                            else { va.leading_zeros() };
                    s.saturating_sub(1) as u64
                } else {
                    let w = a.read_w(rn);
                    let s = if w >> 31 == 1 { (!w).leading_zeros() }
                            else { w.leading_zeros() };
                    s.saturating_sub(1) as u64
                }
            }
            _ => va,
        };
        a.write_x(rd, mask(r, sf));
        return Ok(());
    }

    // ── Conditional select (CSEL / CSINC / CSINV / CSNEG) ───────────────────
    if (insn >> 21) & 0x1FF == 0xD4 {
        let op = (insn >> 30) & 1;
        let rm = ((insn >> 16) & 0x1F) as u32;
        let cond = (insn >> 12) & 0xF;
        let op2 = (insn >> 10) & 0x3;
        let rn = ((insn >> 5) & 0x1F) as u32;
        let rd = (insn & 0x1F) as u32;
        let r = if a.eval_cond(cond) {
            a.read_x(rn)
        } else {
            let v = a.read_x(rm);
            match (op, op2 & 1) {
                (0, 0) => v,
                (0, 1) => v.wrapping_add(1),
                (1, 0) => !v,
                (1, 1) => (!v).wrapping_add(1),
                _ => v,
            }
        };
        a.write_x(rd, mask(r, sf));
        return Ok(());
    }

    // ── CCMP / CCMN ──────────────────────────────────────────────────────────
    if (insn >> 21) & 0x1FF == 0b111010010 {
        let op = (insn >> 30) & 1;
        let cond = (insn >> 12) & 0xF;
        let rn = ((insn >> 5) & 0x1F) as u32;
        let nzcv_imm = (insn & 0xF) as u32;
        let is_imm = (insn >> 11) & 1 == 1;
        if a.eval_cond(cond) {
            let va = a.read_x(rn);
            let b = if is_imm {
                ((insn >> 16) & 0x1F) as u64
            } else {
                a.read_x(((insn >> 16) & 0x1F) as u32)
            };
            let (r, c, v) = if op == 1 { awc(va, !b, true, sf == 1) }
                             else       { awc(va, b, false, sf == 1) };
            flags(a, mask(r, sf), c, v, sf == 1);
        } else {
            a.nzcv = nzcv_imm << 28;
        }
        return Ok(());
    }

    // ── ADC / SBC ────────────────────────────────────────────────────────────
    if (insn >> 21) & 0xFF == 0xD0 {
        let op = (insn >> 30) & 1;
        let s = (insn >> 29) & 1;
        let rm = ((insn >> 16) & 0x1F) as u32;
        let rn = ((insn >> 5) & 0x1F) as u32;
        let rd = (insn & 0x1F) as u32;
        let (r, c, v) = if op == 0 {
            awc(a.read_x(rn), a.read_x(rm), a.flag_c(), sf == 1)
        } else {
            awc(a.read_x(rn), !a.read_x(rm), a.flag_c(), sf == 1)
        };
        let r = mask(r, sf);
        if s == 1 { flags(a, r, c, v, sf == 1); }
        a.write_x(rd, r);
        return Ok(());
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// exec_simd_dp — SIMD/FP Data Processing group
// ═══════════════════════════════════════════════════════════════════════════════

/// Execute a SIMD/FP data-processing instruction from the raw 32-bit word.
pub fn exec_simd_dp(
    a: &mut Aarch64ArchState,
    insn: u32,
    _mem: &mut impl MemInterface,
) -> Result<(), HartException> {

    // ── DUP Vd.T, Wn/Xn ─────────────────────────────────────────────────────
    // 0 Q 00 1110 000 imm5 0 0001 1 Rn Rd
    if insn & 0xBFE0_FC00 == 0x0E00_0C00 {
        let q = (insn >> 30) & 1;
        let imm5 = (insn >> 16) & 0x1F;
        let rn = ((insn >> 5) & 0x1F) as u32;
        let rd = (insn & 0x1F) as usize;
        let (esize, val) = if imm5 & 1 != 0 {
            (8, a.read_x(rn) as u8 as u128)
        } else if imm5 & 2 != 0 {
            (16, a.read_x(rn) as u16 as u128)
        } else if imm5 & 4 != 0 {
            (32, a.read_x(rn) as u32 as u128)
        } else {
            (64, a.read_x(rn) as u128)
        };
        let total_bits = if q == 1 { 128 } else { 64 };
        let mut v: u128 = 0;
        for i in 0..(total_bits / esize) {
            v |= val << (i * esize);
        }
        a.v[rd] = v;
        return Ok(());
    }

    // ── INS Vd.Ts[idx], Wn/Xn ───────────────────────────────────────────────
    // 0 1 0 01110 000 imm5 0 00111 Rn Rd
    if insn & 0xFFE0_FC00 == 0x4E00_1C00 {
        let imm5 = (insn >> 16) & 0x1F;
        let rn = ((insn >> 5) & 0x1F) as u32;
        let rd = (insn & 0x1F) as usize;
        let val = a.read_x(rn);
        if imm5 & 1 != 0 {
            let idx = (imm5 >> 1) as usize;
            let shift = idx * 8;
            let m = !(0xFFu128 << shift);
            a.v[rd] = (a.v[rd] & m) | ((val as u128 & 0xFF) << shift);
        } else if imm5 & 2 != 0 {
            let idx = (imm5 >> 2) as usize;
            let shift = idx * 16;
            let m = !(0xFFFFu128 << shift);
            a.v[rd] = (a.v[rd] & m) | ((val as u128 & 0xFFFF) << shift);
        } else if imm5 & 4 != 0 {
            let idx = (imm5 >> 3) as usize;
            let shift = idx * 32;
            let m = !(0xFFFF_FFFFu128 << shift);
            a.v[rd] = (a.v[rd] & m) | ((val as u128 & 0xFFFF_FFFF) << shift);
        } else if imm5 & 8 != 0 {
            let idx = (imm5 >> 4) as usize;
            let shift = idx * 64;
            let m = !(0xFFFF_FFFF_FFFF_FFFFu128 << shift);
            a.v[rd] = (a.v[rd] & m) | ((val as u128 & 0xFFFF_FFFF_FFFF_FFFF) << shift);
        }
        return Ok(());
    }

    // ── ORR Vd.16B, Vn.16B, Vm.16B (MOV vector) ─────────────────────────────
    // 0 Q 00 1110 10 1 Rm 0 00011 1 Rn Rd
    if insn & 0xBFE0_FC00 == 0x0EA0_1C00 {
        let rm = ((insn >> 16) & 0x1F) as usize;
        let rn = ((insn >> 5) & 0x1F) as usize;
        let rd = (insn & 0x1F) as usize;
        a.v[rd] = a.v[rn] | a.v[rm];
        return Ok(());
    }

    // ── FP <-> integer conversions + FMOV ────────────────────────────────────
    // sf 00 11110 ftype 1 rmode opcode 000000 Rn Rd
    if insn & 0x5F20_FC00 == 0x1E20_0000 {
        let sf = (insn >> 31) & 1;
        let ftype = (insn >> 22) & 0x3;
        let rmode = (insn >> 19) & 0x3;
        let opcode = (insn >> 16) & 0x7;
        let rn = ((insn >> 5) & 0x1F) as usize;
        let rd = (insn & 0x1F) as usize;

        match (rmode, opcode) {
            // FMOV Sd, Wn
            (0, 7) if sf == 0 => {
                a.v[rd] = a.read_x(rn as u32) as u32 as u128;
            }
            // FMOV Wd, Sn
            (0, 6) if sf == 0 => {
                a.write_x(rd as u32, (a.v[rn] as u32) as u64);
            }
            // FMOV Dd, Xn
            (0, 7) if sf == 1 => {
                a.v[rd] = a.read_x(rn as u32) as u128;
            }
            // FMOV Xd, Dn
            (0, 6) if sf == 1 => {
                a.write_x(rd as u32, a.v[rn] as u64);
            }
            // SCVTF: signed int -> FP
            (0, 2) => {
                let ival = if sf == 1 { a.read_x(rn as u32) as i64 }
                           else       { a.read_x(rn as u32) as i32 as i64 };
                if ftype == 0 {
                    a.v[rd] = (ival as f32).to_bits() as u128;
                } else {
                    a.v[rd] = (ival as f64).to_bits() as u128;
                }
            }
            // UCVTF: unsigned int -> FP
            (0, 3) => {
                let uval = if sf == 1 { a.read_x(rn as u32) }
                           else       { a.read_x(rn as u32) as u32 as u64 };
                if ftype == 0 {
                    a.v[rd] = (uval as f32).to_bits() as u128;
                } else {
                    a.v[rd] = (uval as f64).to_bits() as u128;
                }
            }
            // FCVTZS: FP -> signed int (round toward zero)
            (3, 0) => {
                let val = if ftype == 0 {
                    let f = f32::from_bits(a.v[rn] as u32);
                    if sf == 1 { f as i64 as u64 } else { f as i32 as u32 as u64 }
                } else {
                    let f = f64::from_bits(a.v[rn] as u64);
                    if sf == 1 { f as i64 as u64 } else { f as i32 as u32 as u64 }
                };
                a.write_x(rd as u32, val);
            }
            // FCVTZU: FP -> unsigned int (round toward zero)
            (3, 1) => {
                let val = if ftype == 0 {
                    let f = f32::from_bits(a.v[rn] as u32);
                    if sf == 1 { f as u64 } else { f as u32 as u64 }
                } else {
                    let f = f64::from_bits(a.v[rn] as u64);
                    if sf == 1 { f as u64 } else { f as u32 as u64 }
                };
                a.write_x(rd as u32, val);
            }
            // FCVTNS: FP -> signed int (round nearest, ties to even)
            (0, 0) => {
                let val = if ftype == 0 {
                    let f = f32::from_bits(a.v[rn] as u32);
                    let r = f.round_ties_even();
                    if sf == 1 { r as i64 as u64 } else { r as i32 as u32 as u64 }
                } else {
                    let f = f64::from_bits(a.v[rn] as u64);
                    let r = f.round_ties_even();
                    if sf == 1 { r as i64 as u64 } else { r as i32 as u32 as u64 }
                };
                a.write_x(rd as u32, val);
            }
            // FCVTNU: FP -> unsigned int (round nearest, ties to even)
            (0, 1) => {
                let val = if ftype == 0 {
                    let f = f32::from_bits(a.v[rn] as u32);
                    let r = f.round_ties_even();
                    if sf == 1 { r as u64 } else { r as u32 as u64 }
                } else {
                    let f = f64::from_bits(a.v[rn] as u64);
                    let r = f.round_ties_even();
                    if sf == 1 { r as u64 } else { r as u32 as u64 }
                };
                a.write_x(rd as u32, val);
            }
            // FCVTMS: FP -> signed int (round toward -inf)
            (2, 0) => {
                let val = if ftype == 0 {
                    let f = f32::from_bits(a.v[rn] as u32).floor();
                    if sf == 1 { f as i64 as u64 } else { f as i32 as u32 as u64 }
                } else {
                    let f = f64::from_bits(a.v[rn] as u64).floor();
                    if sf == 1 { f as i64 as u64 } else { f as i32 as u32 as u64 }
                };
                a.write_x(rd as u32, val);
            }
            // FCVTPS: FP -> signed int (round toward +inf)
            (1, 0) => {
                let val = if ftype == 0 {
                    let f = f32::from_bits(a.v[rn] as u32).ceil();
                    if sf == 1 { f as i64 as u64 } else { f as i32 as u32 as u64 }
                } else {
                    let f = f64::from_bits(a.v[rn] as u64).ceil();
                    if sf == 1 { f as i64 as u64 } else { f as i32 as u32 as u64 }
                };
                a.write_x(rd as u32, val);
            }
            // FCVTAS: FP -> signed int (round to nearest, ties away)
            (0, 4) => {
                let val = if ftype == 0 {
                    let f = f32::from_bits(a.v[rn] as u32).round();
                    if sf == 1 { f as i64 as u64 } else { f as i32 as u32 as u64 }
                } else {
                    let f = f64::from_bits(a.v[rn] as u64).round();
                    if sf == 1 { f as i64 as u64 } else { f as i32 as u32 as u64 }
                };
                a.write_x(rd as u32, val);
            }
            _ => { /* unhandled FP<->int conversion — skip */ }
        }
        return Ok(());
    }

    // ── MOVI / MVNI (advanced SIMD modified immediate) ───────────────────────
    // 0 Q op 0111100000 a:b:c:d:e:f:g:h cmode 01 Rd
    if insn & 0x9FF8_0400 == 0x0F00_0400 {
        let q = (insn >> 30) & 1;
        let op = (insn >> 29) & 1;
        let rd = (insn & 0x1F) as usize;
        let cmode = (insn >> 12) & 0xF;
        let abc = ((insn >> 16) & 0x7) as u8;
        let defgh = ((insn >> 5) & 0x1F) as u8;
        let imm8 = (abc << 5) | defgh;
        let mut val: u128 = 0;
        if cmode == 0b1110 && op == 1 {
            // MOVI Dd/Vd.2D, #imm64
            let mut imm64: u64 = 0;
            for i in 0..8 {
                if (imm8 >> i) & 1 != 0 { imm64 |= 0xFFu64 << (i * 8); }
            }
            val = if q == 1 { ((imm64 as u128) << 64) | imm64 as u128 }
                  else      { imm64 as u128 };
        } else if cmode == 0b1110 && op == 0 {
            // MOVI Vd.xB, #imm8
            let byte_val = imm8 as u128;
            let bytes = if q == 1 { 16 } else { 8 };
            for i in 0..bytes { val |= byte_val << (i * 8); }
        } else {
            // Shifted byte/halfword immediate
            let shift = ((cmode >> 1) & 3) * 8;
            let base = (imm8 as u64) << shift;
            let elem_size = if cmode < 8 { 4usize } else { 2 };
            let elem_mask = if elem_size == 4 { 0xFFFF_FFFFu64 } else { 0xFFFFu64 };
            let elem = if op == 1 { !base & elem_mask } else { base & elem_mask };
            let total = if q == 1 { 16 } else { 8 };
            for i in 0..(total / elem_size) {
                val |= (elem as u128) << (i * elem_size * 8);
            }
        }
        a.v[rd] = val;
        return Ok(());
    }

    // ── SIMD across lanes ────────────────────────────────────────────────────
    // 0 Q U 01110 size 11000 opcode 10 Rn Rd
    if insn & 0x9F3E_0C00 == 0x0E30_0800 && (insn >> 17) & 0x1F == 0b11000 {
        let q = (insn >> 30) & 1;
        let u = (insn >> 29) & 1;
        let size = (insn >> 22) & 0x3;
        let opcode = (insn >> 12) & 0x1F;
        let rn = ((insn >> 5) & 0x1F) as usize;
        let rd = (insn & 0x1F) as usize;
        let bytes = if q == 1 { 16usize } else { 8 };
        let esize = 1usize << size;
        let ebits = esize * 8;
        let emask: u128 = if esize >= 16 { u128::MAX } else { (1u128 << ebits) - 1 };
        let count = bytes / esize;
        let src = a.v[rn];
        let mut acc = src & emask;
        for i in 1..count {
            let ea = (src >> (i * ebits)) & emask;
            acc = match (u, opcode) {
                // ADDV / SADDLV / UADDLV
                (_, 0b11011) | (0, 0b00011) | (1, 0b00011) => (acc + ea) & emask,
                // SMAXV
                (0, 0b01010) => {
                    let sa = acc as i128 - if acc >> (ebits - 1) != 0 { 1i128 << ebits } else { 0 };
                    let sb = ea as i128 - if ea >> (ebits - 1) != 0 { 1i128 << ebits } else { 0 };
                    if sa >= sb { acc } else { ea }
                }
                // UMAXV
                (1, 0b01010) => if acc >= ea { acc } else { ea },
                // SMINV
                (0, 0b11010) => {
                    let sa = acc as i128 - if acc >> (ebits - 1) != 0 { 1i128 << ebits } else { 0 };
                    let sb = ea as i128 - if ea >> (ebits - 1) != 0 { 1i128 << ebits } else { 0 };
                    if sa <= sb { acc } else { ea }
                }
                // UMINV
                (1, 0b11010) => if acc <= ea { acc } else { ea },
                _ => acc,
            };
        }
        a.v[rd] = acc;
        return Ok(());
    }

    // ── UMOV Wd/Xd, Vn.T[idx] ───────────────────────────────────────────────
    // 0 Q 00 1110 000 imm5 0 01111 Rn Rd
    if insn & 0xBFE0_FC00 == 0x0E00_3C00 {
        let imm5 = (insn >> 16) & 0x1F;
        let rn = ((insn >> 5) & 0x1F) as usize;
        let rd = (insn & 0x1F) as u32;
        let v = a.v[rn];
        let val = if imm5 & 1 != 0 {
            let idx = (imm5 >> 1) as usize;
            (v >> (idx * 8)) as u64 & 0xFF
        } else if imm5 & 2 != 0 {
            let idx = (imm5 >> 2) as usize;
            (v >> (idx * 16)) as u64 & 0xFFFF
        } else if imm5 & 4 != 0 {
            let idx = (imm5 >> 3) as usize;
            (v >> (idx * 32)) as u64 & 0xFFFF_FFFF
        } else {
            let idx = (imm5 >> 4) as usize;
            (v >> (idx * 64)) as u64
        };
        a.write_x(rd, val);
        return Ok(());
    }

    // ── Advanced SIMD three-same ─────────────────────────────────────────────
    // 0 Q U 01110 size 1 Rm opcode 1 Rn Rd
    if (insn >> 24) & 0x1F == 0b01110 && (insn >> 21) & 1 == 1 && (insn >> 10) & 1 == 1 {
        let q = (insn >> 30) & 1;
        let u = (insn >> 29) & 1;
        let size = (insn >> 22) & 0x3;
        let rm = ((insn >> 16) & 0x1F) as usize;
        let opcode = (insn >> 11) & 0x1F;
        let rn = ((insn >> 5) & 0x1F) as usize;
        let rd = (insn & 0x1F) as usize;
        let va = a.v[rn];
        let vb = a.v[rm];
        let bytes = if q == 1 { 16usize } else { 8 };

        // Logical operations (size encodes the operation, not element width)
        if opcode == 0b00011 {
            let r = match (u, size) {
                (0, 0) => va & vb,                            // AND
                (0, 1) => va & !vb,                           // BIC
                (0, 2) => va | vb,                            // ORR
                (0, 3) => va | !vb,                           // ORN
                (1, 0) => va ^ vb,                            // EOR
                (1, 1) => (va & !vb) | (a.v[rd] & vb),       // BSL
                (1, 2) => (va & vb) | (a.v[rd] & !vb),       // BIT
                (1, 3) => (!va & vb) | (a.v[rd] & !vb),      // BIF
                _ => va,
            };
            a.v[rd] = if q == 0 { r & ((1u128 << 64) - 1) } else { r };
            return Ok(());
        }

        let esize = 1usize << size;
        let ebits = esize * 8;
        let emask: u128 = if esize >= 16 { u128::MAX } else { (1u128 << ebits) - 1 };
        let mut result: u128 = 0;
        for i in 0..(bytes / esize) {
            let shift = i * ebits;
            let ea = (va >> shift) & emask;
            let eb = (vb >> shift) & emask;
            let sa = ea as i128 - if ea >> (ebits - 1) != 0 { 1i128 << ebits } else { 0 };
            let sb = eb as i128 - if eb >> (ebits - 1) != 0 { 1i128 << ebits } else { 0 };
            let er = match (u, opcode) {
                // ADD
                (0, 0b10000) => ea.wrapping_add(eb) & emask,
                // SUB
                (1, 0b10000) => ea.wrapping_sub(eb) & emask,
                // CMGT (signed)
                (0, 0b00110) => if sa > sb { emask } else { 0 },
                // CMHI (unsigned)
                (1, 0b00110) => if ea > eb { emask } else { 0 },
                // CMGE (signed)
                (0, 0b00111) => if sa >= sb { emask } else { 0 },
                // CMHS (unsigned)
                (1, 0b00111) => if ea >= eb { emask } else { 0 },
                // CMEQ
                (1, 0b10001) => if ea == eb { emask } else { 0 },
                // CMTST
                (0, 0b10001) => if ea & eb != 0 { emask } else { 0 },
                // SMAX
                (0, 0b01100) => if sa >= sb { ea } else { eb },
                // UMAX
                (1, 0b01100) => if ea >= eb { ea } else { eb },
                // SMIN
                (0, 0b01101) => if sa <= sb { ea } else { eb },
                // UMIN
                (1, 0b01101) => if ea <= eb { ea } else { eb },
                // MUL
                (0, 0b10011) => ea.wrapping_mul(eb) & emask,
                // MLA (multiply-accumulate)
                (0, 0b10010) => {
                    let d = (a.v[rd] >> shift) & emask;
                    d.wrapping_add(ea.wrapping_mul(eb)) & emask
                }
                // MLS (multiply-subtract)
                (1, 0b10010) => {
                    let d = (a.v[rd] >> shift) & emask;
                    d.wrapping_sub(ea.wrapping_mul(eb) & emask) & emask
                }
                // SHADD / UHADD (halving add)
                (0, 0b00000) => (sa.wrapping_add(sb) >> 1) as u128 & emask,
                (1, 0b00000) => ea.wrapping_add(eb) >> 1 & emask,
                // SHSUB / UHSUB (halving sub)
                (0, 0b00100) => (sa.wrapping_sub(sb) >> 1) as u128 & emask,
                (1, 0b00100) => ea.wrapping_sub(eb) >> 1 & emask,
                // SQADD / UQADD (saturating add — simplified)
                (0, 0b00001) => ea.wrapping_add(eb) & emask,
                (1, 0b00001) => { let s = ea + eb; if s > emask { emask } else { s } }
                // SQSUB / UQSUB (saturating sub — simplified)
                (0, 0b00101) => ea.wrapping_sub(eb) & emask,
                (1, 0b00101) => if ea >= eb { ea - eb } else { 0 },
                // SABD / UABD (absolute difference)
                (0, 0b01110) => (sa.wrapping_sub(sb)).unsigned_abs() & emask,
                (1, 0b01110) => if ea >= eb { ea - eb } else { eb - ea },
                // SABA / UABA (absolute difference accumulate)
                (0, 0b10111) => {
                    let d = (a.v[rd] >> shift) & emask;
                    d.wrapping_add((sa.wrapping_sub(sb)).unsigned_abs() & emask) & emask
                }
                (1, 0b10111) => {
                    let d = (a.v[rd] >> shift) & emask;
                    let diff = if ea >= eb { ea - eb } else { eb - ea };
                    d.wrapping_add(diff) & emask
                }
                // ADDP (pairwise add)
                (0, 0b10101) => {
                    let pair_idx = i / 2;
                    let src = if i % 2 == 0 { va } else { vb };
                    let lo = (src >> (pair_idx * 2 * ebits)) & emask;
                    let hi = (src >> ((pair_idx * 2 + 1) * ebits)) & emask;
                    lo.wrapping_add(hi) & emask
                }
                // SMAXP / UMAXP (pairwise max)
                (0, 0b10100) => {
                    let pair_idx = i / 2;
                    let src = if i % 2 == 0 { va } else { vb };
                    let lo = (src >> (pair_idx * 2 * ebits)) & emask;
                    let hi = (src >> ((pair_idx * 2 + 1) * ebits)) & emask;
                    let slo = lo as i128 - if lo >> (ebits - 1) != 0 { 1i128 << ebits } else { 0 };
                    let shi = hi as i128 - if hi >> (ebits - 1) != 0 { 1i128 << ebits } else { 0 };
                    if slo >= shi { lo } else { hi }
                }
                (1, 0b10100) => {
                    let pair_idx = i / 2;
                    let src = if i % 2 == 0 { va } else { vb };
                    let lo = (src >> (pair_idx * 2 * ebits)) & emask;
                    let hi = (src >> ((pair_idx * 2 + 1) * ebits)) & emask;
                    if lo >= hi { lo } else { hi }
                }
                // SMINP / UMINP (pairwise min)
                (0, 0b10110) => {
                    let pair_idx = i / 2;
                    let src = if i % 2 == 0 { va } else { vb };
                    let lo = (src >> (pair_idx * 2 * ebits)) & emask;
                    let hi = (src >> ((pair_idx * 2 + 1) * ebits)) & emask;
                    let slo = lo as i128 - if lo >> (ebits - 1) != 0 { 1i128 << ebits } else { 0 };
                    let shi = hi as i128 - if hi >> (ebits - 1) != 0 { 1i128 << ebits } else { 0 };
                    if slo <= shi { lo } else { hi }
                }
                (1, 0b10110) => {
                    let pair_idx = i / 2;
                    let src = if i % 2 == 0 { va } else { vb };
                    let lo = (src >> (pair_idx * 2 * ebits)) & emask;
                    let hi = (src >> ((pair_idx * 2 + 1) * ebits)) & emask;
                    if lo <= hi { lo } else { hi }
                }
                // SSHL / USHL (register shift)
                (0, 0b01000) | (1, 0b01000) => {
                    let shift_amt = sb as i8;
                    if shift_amt >= 0 { (ea << (shift_amt as u32 % ebits as u32)) & emask }
                    else { (ea >> ((-shift_amt) as u32 % ebits as u32)) & emask }
                }
                // SRSHL / URSHL (rounding register shift — simplified to non-rounding)
                (0, 0b01010) | (1, 0b01010) => {
                    let shift_amt = sb as i8;
                    if shift_amt >= 0 { (ea << (shift_amt as u32 % ebits as u32)) & emask }
                    else { (ea >> ((-shift_amt) as u32 % ebits as u32)) & emask }
                }
                _ => { return Ok(()); }
            };
            result |= er << shift;
        }
        a.v[rd] = result;
        return Ok(());
    }

    // ── SIMD two-reg misc ────────────────────────────────────────────────────
    // 0 Q U 01110 size 10000 opcode 10 Rn Rd
    if insn & 0x9F3E_0C00 == 0x0E20_0800 {
        let q = (insn >> 30) & 1;
        let u = (insn >> 29) & 1;
        let size = (insn >> 22) & 0x3;
        let opcode = (insn >> 12) & 0x1F;
        let rn = ((insn >> 5) & 0x1F) as usize;
        let rd = (insn & 0x1F) as usize;
        let bytes = if q == 1 { 16usize } else { 8 };
        let esize = 1usize << size;
        let ebits = esize * 8;
        let emask: u128 = if esize >= 16 { u128::MAX } else { (1u128 << ebits) - 1 };
        let src = a.v[rn];
        let mut result: u128 = 0;
        for i in 0..(bytes / esize) {
            let shift = i * ebits;
            let ea = (src >> shift) & emask;
            let sign = ea >> (ebits - 1);
            let er = match (u, opcode) {
                // CMGT #0
                (0, 8) => if sign == 0 && ea != 0 { emask } else { 0 },
                // CMEQ #0
                (0, 9) => if ea == 0 { emask } else { 0 },
                // CMLT #0
                (0, 10) => if sign != 0 { emask } else { 0 },
                // CMGE #0
                (1, 8) => if sign == 0 { emask } else { 0 },
                // CMLE #0
                (1, 9) => if sign != 0 || ea == 0 { emask } else { 0 },
                // ABS
                (0, 11) => {
                    let sa = ea as i128 - if sign != 0 { 1i128 << ebits } else { 0 };
                    (sa.unsigned_abs() as u128) & emask
                }
                // NEG
                (1, 11) => {
                    let sa = ea as i128 - if sign != 0 { 1i128 << ebits } else { 0 };
                    ((-sa) as u128) & emask
                }
                // CNT (popcount per byte, size=0 only)
                (0, 5) if size == 0 => ea.reverse_bits() >> (128 - ebits) & emask,
                // CNT (other sizes — count ones)
                (0, 5) => (ea.count_ones() as u128) & emask,
                // NOT (bitwise NOT, size=0)
                (1, 5) if size == 0 => (!ea) & emask,
                // FABS (vector)
                (0, 15) if size >= 2 => {
                    if size == 2 {
                        let f = f32::from_bits(ea as u32).abs();
                        (f.to_bits() as u128) & emask
                    } else {
                        let f = f64::from_bits(ea as u64).abs();
                        (f.to_bits() as u128) & emask
                    }
                }
                // FNEG (vector)
                (1, 15) if size >= 2 => {
                    if size == 2 {
                        let f = f32::from_bits(ea as u32);
                        ((-f).to_bits() as u128) & emask
                    } else {
                        let f = f64::from_bits(ea as u64);
                        ((-f).to_bits() as u128) & emask
                    }
                }
                // FSQRT (vector)
                (1, 31) if size >= 2 => {
                    if size == 2 {
                        let f = f32::from_bits(ea as u32).sqrt();
                        (f.to_bits() as u128) & emask
                    } else {
                        let f = f64::from_bits(ea as u64).sqrt();
                        (f.to_bits() as u128) & emask
                    }
                }
                // REV (byte reverse per element)
                (0, 0) if size < 3 => {
                    let mut rev = 0u128;
                    for b in 0..esize {
                        let byte = (ea >> (b * 8)) & 0xFF;
                        rev |= byte << ((esize - 1 - b) * 8);
                    }
                    rev & emask
                }
                // CLS (count leading sign bits)
                (0, 4) => {
                    let leading = if sign != 0 {
                        ((!ea & emask) << (128 - ebits)).leading_zeros() as u128
                    } else {
                        (ea << (128 - ebits)).leading_zeros() as u128
                    };
                    (leading.saturating_sub(1)) & emask
                }
                // CLZ (count leading zeros)
                (1, 4) => {
                    let lz = if ea == 0 { ebits as u128 }
                             else { (ea << (128 - ebits)).leading_zeros() as u128 };
                    lz & emask
                }
                // XTN / SQXTN (narrow — simplified to truncation)
                (0, 18) | (1, 18) => ea & emask,
                // SHLL (shift left long)
                (1, 19) => (ea << ebits) & emask,
                // SCVTF / UCVTF integer to FP vector
                (0, 29) | (1, 29) if size >= 2 => {
                    if size == 2 {
                        let ival = if u == 0 { (ea as i32) as f32 } else { ea as u32 as f32 };
                        (ival.to_bits() as u128) & emask
                    } else {
                        let ival = if u == 0 { (ea as i64) as f64 } else { ea as u64 as f64 };
                        (ival.to_bits() as u128) & emask
                    }
                }
                // FCVTZS / FCVTZU FP to integer vector (round toward zero)
                (0, 27) | (1, 27) if size >= 2 => {
                    if size == 2 {
                        let f = f32::from_bits(ea as u32);
                        let ival = if u == 0 { f as i32 as u32 } else { f as u32 };
                        (ival as u128) & emask
                    } else {
                        let f = f64::from_bits(ea as u64);
                        let ival = if u == 0 { f as i64 as u64 } else { f as u64 };
                        (ival as u128) & emask
                    }
                }
                _ => { return Ok(()); }
            };
            result |= er << shift;
        }
        a.v[rd] = result;
        return Ok(());
    }

    // ── SIMD shift by immediate ──────────────────────────────────────────────
    // 0 Q U 011110 immh:immb opcode 1 Rn Rd
    if (insn >> 24) & 0x1F == 0b01111 && (insn >> 10) & 1 == 1 && (insn >> 19) & 0xF != 0 {
        let q = (insn >> 30) & 1;
        let u = (insn >> 29) & 1;
        let immh = (insn >> 19) & 0xF;
        let immb = (insn >> 16) & 0x7;
        let opcode = (insn >> 11) & 0x1F;
        let rn = ((insn >> 5) & 0x1F) as usize;
        let rd = (insn & 0x1F) as usize;
        let shift_val = ((immh << 3) | immb) as usize;
        let src_esize = if immh & 8 != 0 { 64usize }
                        else if immh & 4 != 0 { 32 }
                        else if immh & 2 != 0 { 16 }
                        else { 8 };

        // SSHLL / USHLL (widening shift left)
        if opcode == 0b10100 {
            let amt = shift_val - src_esize;
            let dst_esize = src_esize * 2;
            let src_mask: u128 = (1u128 << src_esize) - 1;
            let dst_mask: u128 = (1u128 << dst_esize) - 1;
            let src_start = if q == 1 { 64 } else { 0 };
            let count = 64 / src_esize;
            let mut result: u128 = 0;
            for i in 0..count {
                let src_val = (a.v[rn] >> (src_start + i * src_esize)) & src_mask;
                let widened = if u == 0 {
                    let sign = src_val >> (src_esize - 1);
                    if sign != 0 { (src_val | (dst_mask & !src_mask)) << amt }
                    else { src_val << amt }
                } else {
                    src_val << amt
                };
                result |= (widened & dst_mask) << (i * dst_esize);
            }
            a.v[rd] = result;
            return Ok(());
        }

        let esize = src_esize;
        let emask: u128 = if esize >= 128 { u128::MAX } else { (1u128 << esize) - 1 };
        let bytes = if q == 1 { 16usize } else { 8 };
        let src = a.v[rn];
        let mut result: u128 = 0;
        for i in 0..(bytes * 8 / esize) {
            let bit_shift = i * esize;
            let ea = (src >> bit_shift) & emask;
            let er = match (u, opcode) {
                // USHR
                (1, 0b00000) => {
                    let amt = esize * 2 - shift_val;
                    if amt >= esize { 0 } else { (ea >> amt) & emask }
                }
                // SSHR
                (0, 0b00000) => {
                    let amt = esize * 2 - shift_val;
                    if amt >= esize {
                        if ea >> (esize - 1) != 0 { emask } else { 0 }
                    } else {
                        let sign_bit = ea >> (esize - 1);
                        let shifted = ea >> amt;
                        if sign_bit != 0 { (shifted | (emask << (esize - amt))) & emask }
                        else { shifted & emask }
                    }
                }
                // SHL
                (0, 0b01010) | (1, 0b01010) => {
                    let amt = shift_val - esize;
                    (ea << amt) & emask
                }
                // SSRA / USRA (shift right and accumulate)
                (0, 0b00010) | (1, 0b00010) => {
                    let amt = esize * 2 - shift_val;
                    let shifted = if u == 0 {
                        let sign_bit = ea >> (esize - 1);
                        let s = ea >> amt.min(esize - 1);
                        if sign_bit != 0 && amt < esize { (s | (emask << (esize - amt))) & emask }
                        else { s & emask }
                    } else {
                        if amt >= esize { 0 } else { (ea >> amt) & emask }
                    };
                    let d = (a.v[rd] >> bit_shift) & emask;
                    d.wrapping_add(shifted) & emask
                }
                // SRSHR / URSHR (rounding shift right — simplified to non-rounding)
                (0, 0b00100) | (1, 0b00100) => {
                    let amt = esize * 2 - shift_val;
                    if amt >= esize { 0 } else { (ea >> amt) & emask }
                }
                // SRSRA / URSRA (rounding shift right + accumulate — simplified)
                (0, 0b00110) | (1, 0b00110) => {
                    let amt = esize * 2 - shift_val;
                    let shifted = if amt >= esize { 0 } else { (ea >> amt) & emask };
                    let d = (a.v[rd] >> bit_shift) & emask;
                    d.wrapping_add(shifted) & emask
                }
                // SRI (shift right and insert)
                (1, 0b01000) => {
                    let amt = esize * 2 - shift_val;
                    let d = (a.v[rd] >> bit_shift) & emask;
                    if amt >= esize { d }
                    else {
                        let mask_hi = emask << (esize - amt) & emask;
                        (d & mask_hi) | ((ea >> amt) & !mask_hi & emask)
                    }
                }
                // SLI (shift left and insert)
                (1, 0b01011) => {
                    let amt = shift_val - esize;
                    let d = (a.v[rd] >> bit_shift) & emask;
                    let mask_lo = if amt == 0 { 0 } else { (1u128 << amt) - 1 };
                    (d & mask_lo) | ((ea << amt) & emask)
                }
                // SQSHL / UQSHL (saturating shift left imm — simplified)
                (0, 0b01110) | (1, 0b01110) => {
                    let amt = shift_val - esize;
                    (ea << amt) & emask
                }
                // SQSHRN / UQSHRN / SQSHRUN (narrowing shift — simplified)
                (0, 0b10010) | (1, 0b10010) | (0, 0b10000) => {
                    let amt = esize * 2 - shift_val;
                    if amt >= esize { 0 } else { (ea >> amt) & emask }
                }
                _ => { return Ok(()); }
            };
            result |= er << bit_shift;
        }
        a.v[rd] = result;
        return Ok(());
    }

    // ── INS Vd.Ts[idx1], Vn.Ts[idx2] (element to element) ──────────────────
    // 0 1 1 01110 000 imm5 0 imm4 1 Rn Rd
    if insn & 0xFFE0_8400 == 0x6E00_0400 {
        let imm5 = ((insn >> 16) & 0x1F) as usize;
        let imm4 = ((insn >> 11) & 0xF) as usize;
        let rn = ((insn >> 5) & 0x1F) as usize;
        let rd = (insn & 0x1F) as usize;
        let (esize, dst_idx, src_idx) = if imm5 & 1 != 0 {
            (8, imm5 >> 1, imm4)
        } else if imm5 & 2 != 0 {
            (16, imm5 >> 2, imm4 >> 1)
        } else if imm5 & 4 != 0 {
            (32, imm5 >> 3, imm4 >> 2)
        } else {
            (64, imm5 >> 4, imm4 >> 3)
        };
        let emask: u128 = if esize >= 128 { u128::MAX } else { (1u128 << esize) - 1 };
        let src_val = (a.v[rn] >> (src_idx * esize)) & emask;
        let dst_shift = dst_idx * esize;
        a.v[rd] = (a.v[rd] & !(emask << dst_shift)) | (src_val << dst_shift);
        return Ok(());
    }

    // ── EXT Vd.T, Vn.T, Vm.T, #imm ─────────────────────────────────────────
    // 0 Q 10 1110 00 0 Rm 0 imm4 0 Rn Rd
    if insn & 0xBFE0_8400 == 0x2E00_0000 && (insn >> 24) & 0x1F == 0b01110 {
        let q = (insn >> 30) & 1;
        let rm = ((insn >> 16) & 0x1F) as usize;
        let imm4 = ((insn >> 11) & 0xF) as usize;
        let rn = ((insn >> 5) & 0x1F) as usize;
        let rd = (insn & 0x1F) as usize;
        let total = if q == 1 { 16usize } else { 8 };
        let va = a.v[rn];
        let vb = a.v[rm];
        let mut result: u128 = 0;
        for i in 0..total {
            let idx = imm4 + i;
            let byte = if idx < total {
                ((va >> (idx * 8)) & 0xFF) as u8
            } else {
                ((vb >> ((idx - total) * 8)) & 0xFF) as u8
            };
            result |= (byte as u128) << (i * 8);
        }
        a.v[rd] = result;
        return Ok(());
    }

    // ── SIMD permute: ZIP / UZP / TRN ────────────────────────────────────────
    // 0 Q 0 01110 size 0 Rm 0 opcode 10 Rn Rd
    if (insn >> 24) & 0x1F == 0b01110 && (insn >> 21) & 1 == 0 && (insn >> 10) & 3 == 2 {
        let q = (insn >> 30) & 1;
        let size = (insn >> 22) & 0x3;
        let rm = ((insn >> 16) & 0x1F) as usize;
        let opcode = (insn >> 12) & 0x7;
        let rn = ((insn >> 5) & 0x1F) as usize;
        let rd = (insn & 0x1F) as usize;
        let esize = 1usize << size;
        let ebits = esize * 8;
        let emask: u128 = if esize >= 16 { u128::MAX } else { (1u128 << ebits) - 1 };
        let elems = if q == 1 { 128 / ebits } else { 64 / ebits };
        let va = a.v[rn];
        let vb = a.v[rm];
        let mut result: u128 = 0;
        match opcode {
            // UZP1 (opcode=1) / UZP2 (opcode=5)
            1 | 5 => {
                let step = if opcode == 1 { 0 } else { 1 };
                let mut ri = 0;
                for i in (step..elems).step_by(2) {
                    result |= ((va >> (i * ebits)) & emask) << (ri * ebits);
                    ri += 1;
                }
                for i in (step..elems).step_by(2) {
                    result |= ((vb >> (i * ebits)) & emask) << (ri * ebits);
                    ri += 1;
                }
            }
            // ZIP1 (opcode=3) / ZIP2 (opcode=7)
            3 | 7 => {
                let half = elems / 2;
                let base = if opcode == 3 { 0 } else { half };
                for i in 0..half {
                    let ai = (va >> ((base + i) * ebits)) & emask;
                    let bi = (vb >> ((base + i) * ebits)) & emask;
                    result |= ai << (i * 2 * ebits);
                    result |= bi << ((i * 2 + 1) * ebits);
                }
            }
            // TRN1 (opcode=2) / TRN2 (opcode=6)
            2 | 6 => {
                let step = if opcode == 2 { 0 } else { 1 };
                for i in 0..(elems / 2) {
                    let ai = (va >> ((i * 2 + step) * ebits)) & emask;
                    let bi = (vb >> ((i * 2 + step) * ebits)) & emask;
                    result |= ai << (i * 2 * ebits);
                    result |= bi << ((i * 2 + 1) * ebits);
                }
            }
            _ => { result = va; }
        }
        a.v[rd] = result;
        return Ok(());
    }

    // ── Advanced SIMD three-different (widening/narrowing) ────────────────────
    // 0 Q U 01110 size 1 Rm opcode 00 Rn Rd
    if (insn >> 24) & 0x1F == 0b01110 && (insn >> 21) & 1 == 1 && (insn >> 10) & 3 == 0 {
        let q = (insn >> 30) & 1;
        let u = (insn >> 29) & 1;
        let size = (insn >> 22) & 0x3;
        let rm = ((insn >> 16) & 0x1F) as usize;
        let opcode = (insn >> 12) & 0xF;
        let rn = ((insn >> 5) & 0x1F) as usize;
        let rd = (insn & 0x1F) as usize;
        let src_esize = 1usize << size;
        let dst_esize = src_esize * 2;
        let src_ebits = src_esize * 8;
        let dst_ebits = dst_esize * 8;
        let src_mask: u128 = (1u128 << src_ebits) - 1;
        let dst_mask: u128 = (1u128 << dst_ebits) - 1;
        let src_start = if q == 1 { 64 } else { 0 };
        let count = 64 / src_esize;
        let va = a.v[rn];
        let vb = a.v[rm];
        let mut result: u128 = 0;
        let is_wide = opcode & 1 != 0 && opcode < 8;
        for i in 0..count {
            let ea = if is_wide {
                va.wrapping_shr((i * dst_ebits) as u32) & dst_mask
            } else {
                let shift = (src_start + i * src_ebits) as u32;
                let raw = va.wrapping_shr(shift) & src_mask;
                if u == 0 && src_ebits > 0 && raw.wrapping_shr((src_ebits - 1) as u32) != 0 {
                    raw | (dst_mask & !src_mask)
                } else { raw }
            };
            let shift = (src_start + i * src_ebits) as u32;
            let eb_raw = vb.wrapping_shr(shift) & src_mask;
            let eb = if u == 0 && src_ebits > 0
                && eb_raw.wrapping_shr((src_ebits - 1) as u32) != 0
            {
                eb_raw | (dst_mask & !src_mask)
            } else { eb_raw };
            let er = match opcode >> 1 {
                0 => ea.wrapping_add(eb) & dst_mask,
                1 => ea.wrapping_sub(eb) & dst_mask,
                2 => ea.wrapping_add(eb) & dst_mask,
                3 => ea.wrapping_sub(eb) & dst_mask,
                4 => ea.wrapping_add(eb) & dst_mask,
                5 => ea.wrapping_mul(eb) & dst_mask,
                6 => ea.wrapping_sub(eb) & dst_mask,
                7 => ea.wrapping_add(ea.wrapping_mul(eb) & dst_mask) & dst_mask,
                _ => { return Ok(()); }
            };
            result |= er.wrapping_shl((i * dst_ebits) as u32);
        }
        a.v[rd] = result;
        return Ok(());
    }

    // ── Scalar ADDP Dd, Vn.2D ────────────────────────────────────────────────
    // 01 01 1110 11 11000 11011 10 Rn Rd
    if insn & 0xFFFF_FC00 == 0x5EF1_B800 {
        let rn = ((insn >> 5) & 0x1F) as usize;
        let rd = (insn & 0x1F) as usize;
        let lo = a.v[rn] as u64;
        let hi = (a.v[rn] >> 64) as u64;
        a.v[rd] = lo.wrapping_add(hi) as u128;
        return Ok(());
    }

    // ── FMOV (immediate to scalar) ──────────────────────────────────────────
    // sf 00 11110 type 1 imm8 100 00000 Rd
    if insn & 0x5F20_FC00 == 0x1E20_1000 {
        let ftype = (insn >> 22) & 0x3;
        let imm8 = ((insn >> 13) & 0xFF) as u8;
        let rd = (insn & 0x1F) as usize;
        if ftype == 0 {
            let sign = (imm8 >> 7) & 1;
            let exp = ((!(imm8 >> 6) & 1) << 7)
                | (if (imm8 >> 6) & 1 != 0 { 0x7C } else { 0 })
                | ((imm8 >> 4) & 0x3);
            let frac = ((imm8 & 0xF) as u32) << 19;
            let bits = ((sign as u32) << 31) | ((exp as u32) << 23) | frac as u32;
            a.v[rd] = bits as u128;
        } else if ftype == 1 {
            let sign = ((imm8 >> 7) & 1) as u64;
            let exp6 = (imm8 >> 6) & 1;
            let exp = ((((!exp6) & 1) as u64) << 10)
                | (if exp6 != 0 { 0x3FCu64 } else { 0u64 })
                | (((imm8 >> 4) & 0x3) as u64);
            let frac = ((imm8 & 0xF) as u64) << 48;
            let bits = (sign << 63) | (exp << 52) | frac;
            a.v[rd] = bits as u128;
        }
        return Ok(());
    }

    // ── CNT Vd.T, Vn.T (popcount bytes) ─────────────────────────────────────
    // 0 Q 00 1110 size 10000 00101 10 Rn Rd
    if insn & 0xBF3F_FC00 == 0x0E20_5800 {
        let q = (insn >> 30) & 1;
        let rn = ((insn >> 5) & 0x1F) as usize;
        let rd = (insn & 0x1F) as usize;
        let bytes = if q == 1 { 16usize } else { 8 };
        let src = a.v[rn];
        let mut result: u128 = 0;
        for i in 0..bytes {
            let byte = ((src >> (i * 8)) & 0xFF) as u8;
            result |= (byte.count_ones() as u128) << (i * 8);
        }
        a.v[rd] = result;
        return Ok(());
    }

    // ── Scalar FP 2-source ──────────────────────────────────────────────────
    // 0 0 0 11110 ftype 1 Rm opcode 10 Rn Rd
    if insn & 0xFF20_0C00 == 0x1E20_0800 {
        let ftype = (insn >> 22) & 0x3;
        let rm = ((insn >> 16) & 0x1F) as usize;
        let opcode = (insn >> 12) & 0xF;
        let rn = ((insn >> 5) & 0x1F) as usize;
        let rd = (insn & 0x1F) as usize;
        if ftype == 0 {
            let fa = f32::from_bits(a.v[rn] as u32);
            let fb = f32::from_bits(a.v[rm] as u32);
            let r = match opcode {
                0 => fa * fb,    // FMUL
                1 => fa / fb,    // FDIV
                2 => fa + fb,    // FADD
                3 => fa - fb,    // FSUB
                4 => fa.max(fb), // FMAX
                5 => fa.min(fb), // FMIN
                6 => fa.max(fb), // FMAXNM
                7 => fa.min(fb), // FMINNM
                8 => -(fa * fb), // FNMUL
                _ => return Ok(()),
            };
            a.v[rd] = r.to_bits() as u128;
        } else {
            let fa = f64::from_bits(a.v[rn] as u64);
            let fb = f64::from_bits(a.v[rm] as u64);
            let r = match opcode {
                0 => fa * fb,
                1 => fa / fb,
                2 => fa + fb,
                3 => fa - fb,
                4 => fa.max(fb),
                5 => fa.min(fb),
                6 => fa.max(fb),
                7 => fa.min(fb),
                8 => -(fa * fb),
                _ => return Ok(()),
            };
            a.v[rd] = r.to_bits() as u128;
        }
        return Ok(());
    }

    // ── Scalar FP 1-source ──────────────────────────────────────────────────
    // 0 0 0 11110 ftype 1 0000 opcode 10000 Rn Rd
    if insn & 0xFF3E_0C00 == 0x1E20_0000 {
        let ftype = (insn >> 22) & 0x3;
        let opcode = (insn >> 15) & 0x3F;
        let rn = ((insn >> 5) & 0x1F) as usize;
        let rd = (insn & 0x1F) as usize;
        match opcode {
            // FMOV same type
            0 => { a.v[rd] = a.v[rn]; }
            // FABS
            1 => {
                if ftype == 0 {
                    a.v[rd] = f32::from_bits(a.v[rn] as u32).abs().to_bits() as u128;
                } else {
                    a.v[rd] = f64::from_bits(a.v[rn] as u64).abs().to_bits() as u128;
                }
            }
            // FNEG
            2 => {
                if ftype == 0 {
                    a.v[rd] = (-f32::from_bits(a.v[rn] as u32)).to_bits() as u128;
                } else {
                    a.v[rd] = (-f64::from_bits(a.v[rn] as u64)).to_bits() as u128;
                }
            }
            // FSQRT
            3 => {
                if ftype == 0 {
                    a.v[rd] = f32::from_bits(a.v[rn] as u32).sqrt().to_bits() as u128;
                } else {
                    a.v[rd] = f64::from_bits(a.v[rn] as u64).sqrt().to_bits() as u128;
                }
            }
            // FCVT single->double
            4 => {
                let f = f32::from_bits(a.v[rn] as u32) as f64;
                a.v[rd] = f.to_bits() as u128;
            }
            // FCVT double->single
            5 => {
                let f = f64::from_bits(a.v[rn] as u64) as f32;
                a.v[rd] = f.to_bits() as u128;
            }
            // FRINTN (round to nearest, ties to even)
            6 => {
                if ftype == 0 {
                    a.v[rd] = f32::from_bits(a.v[rn] as u32).round_ties_even().to_bits() as u128;
                } else {
                    a.v[rd] = f64::from_bits(a.v[rn] as u64).round_ties_even().to_bits() as u128;
                }
            }
            // FRINTP (round toward +inf)
            7 => {
                if ftype == 0 {
                    a.v[rd] = f32::from_bits(a.v[rn] as u32).ceil().to_bits() as u128;
                } else {
                    a.v[rd] = f64::from_bits(a.v[rn] as u64).ceil().to_bits() as u128;
                }
            }
            // FRINTM (round toward -inf)
            8 => {
                if ftype == 0 {
                    a.v[rd] = f32::from_bits(a.v[rn] as u32).floor().to_bits() as u128;
                } else {
                    a.v[rd] = f64::from_bits(a.v[rn] as u64).floor().to_bits() as u128;
                }
            }
            // FRINTZ (round toward zero)
            9 => {
                if ftype == 0 {
                    a.v[rd] = f32::from_bits(a.v[rn] as u32).trunc().to_bits() as u128;
                } else {
                    a.v[rd] = f64::from_bits(a.v[rn] as u64).trunc().to_bits() as u128;
                }
            }
            // FRINTA (round to nearest, ties away)
            10 => {
                if ftype == 0 {
                    a.v[rd] = f32::from_bits(a.v[rn] as u32).round().to_bits() as u128;
                } else {
                    a.v[rd] = f64::from_bits(a.v[rn] as u64).round().to_bits() as u128;
                }
            }
            // FRINTX (round to current mode, signal inexact)
            14 => {
                if ftype == 0 {
                    a.v[rd] = f32::from_bits(a.v[rn] as u32).round_ties_even().to_bits() as u128;
                } else {
                    a.v[rd] = f64::from_bits(a.v[rn] as u64).round_ties_even().to_bits() as u128;
                }
            }
            // FRINTI (round to current mode)
            15 => {
                if ftype == 0 {
                    a.v[rd] = f32::from_bits(a.v[rn] as u32).round_ties_even().to_bits() as u128;
                } else {
                    a.v[rd] = f64::from_bits(a.v[rn] as u64).round_ties_even().to_bits() as u128;
                }
            }
            _ => { /* unimplemented fp_1source opcode — skip */ }
        }
        return Ok(());
    }

    // ── Scalar FP compare: FCMP / FCMPE ──────────────────────────────────────
    if insn & 0xFF20_FC07 == 0x1E20_2000 {
        let ftype = (insn >> 22) & 0x3;
        let rm = ((insn >> 16) & 0x1F) as usize;
        let rn = ((insn >> 5) & 0x1F) as usize;
        let opc = (insn >> 3) & 0x3;
        let (n_val, z_val, c_val, v_val) = if ftype == 0 {
            let fa = f32::from_bits(a.v[rn] as u32);
            let fb = if opc & 1 == 1 { 0.0f32 } else { f32::from_bits(a.v[rm] as u32) };
            if fa.is_nan() || fb.is_nan() { (false, false, true, true) }
            else if fa == fb { (false, true, true, false) }
            else if fa < fb  { (true, false, false, false) }
            else             { (false, false, true, false) }
        } else {
            let fa = f64::from_bits(a.v[rn] as u64);
            let fb = if opc & 1 == 1 { 0.0f64 } else { f64::from_bits(a.v[rm] as u64) };
            if fa.is_nan() || fb.is_nan() { (false, false, true, true) }
            else if fa == fb { (false, true, true, false) }
            else if fa < fb  { (true, false, false, false) }
            else             { (false, false, true, false) }
        };
        let nzcv = ((n_val as u32) << 3) | ((z_val as u32) << 2)
            | ((c_val as u32) << 1) | (v_val as u32);
        a.nzcv = nzcv << 28;
        return Ok(());
    }

    // ── FCSEL ────────────────────────────────────────────────────────────────
    // 0 0 0 11110 ftype 1 Rm cond 11 Rn Rd
    if insn & 0xFF20_0C00 == 0x1E20_0C00 {
        let ftype = (insn >> 22) & 0x3;
        let rm = ((insn >> 16) & 0x1F) as usize;
        let cond = (insn >> 12) & 0xF;
        let rn = ((insn >> 5) & 0x1F) as usize;
        let rd = (insn & 0x1F) as usize;
        let src = if a.eval_cond(cond) { rn } else { rm };
        if ftype == 0 {
            a.v[rd] = (a.v[src] as u32) as u128;
        } else {
            a.v[rd] = (a.v[src] as u64) as u128;
        }
        return Ok(());
    }

    // ── Unmatched SIMD/FP — silently skip ────────────────────────────────────
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// exec_ldst_simd — SIMD/FP Loads and Stores
// ═══════════════════════════════════════════════════════════════════════════════

/// Execute a SIMD/FP load or store from the raw 32-bit word.
pub fn exec_ldst_simd(
    a: &mut Aarch64ArchState,
    insn: u32,
    mem: &mut impl MemInterface,
) -> Result<(), HartException> {
    let size = (insn >> 30) & 0x3;
    let top5 = (insn >> 27) & 0x1F;

    // ── STP/LDP SIMD pair (S/D/Q) ───────────────────────────────────────────
    if top5 & 0b00111 == 0b00101 {
        let opc = (insn >> 30) & 0x3;
        let l = (insn >> 22) & 1;
        let idx = (insn >> 23) & 0x3;
        let imm7 = sext((insn >> 15) & 0x7F, 7);
        let rt2 = ((insn >> 10) & 0x1F) as usize;
        let rn = ((insn >> 5) & 0x1F) as u32;
        let rt = (insn & 0x1F) as usize;
        let scale: u64 = if opc == 0b10 { 16 } else { 4 << opc };
        let offset = (imm7 * scale as i64) as u64;
        let base = a.read_xsp(rn);
        let (addr, wb) = match idx {
            0b01 => (base, Some(base.wrapping_add(offset))),
            0b10 => (base.wrapping_add(offset), None),
            0b11 => { let x = base.wrapping_add(offset); (x, Some(x)) }
            _ => (base, None),
        };
        if l == 1 {
            // Load
            if opc == 0b10 {
                a.v[rt] = rd128(mem, addr)?;
                a.v[rt2] = rd128(mem, addr.wrapping_add(scale))?;
            } else {
                let sz = scale as usize;
                let lo = rd(mem, addr, sz)?;
                let hi = rd(mem, addr.wrapping_add(scale), sz)?;
                a.v[rt] = lo as u128;
                a.v[rt2] = hi as u128;
            }
        } else {
            // Store
            if opc == 0b10 {
                wr128(mem, addr, a.v[rt])?;
                wr128(mem, addr.wrapping_add(scale), a.v[rt2])?;
            } else {
                let sz = scale as usize;
                wr(mem, addr, a.v[rt] as u64, sz)?;
                wr(mem, addr.wrapping_add(scale), a.v[rt2] as u64, sz)?;
            }
        }
        if let Some(w) = wb { a.write_xsp(rn, w); }
        return Ok(());
    }

    // ── STR/LDR SIMD unsigned offset ─────────────────────────────────────────
    // size 111101 opc imm12 Rn Rt
    if (insn >> 24) & 0x3F == 0b111101 {
        let opc = (insn >> 22) & 0x3;
        let imm12 = ((insn >> 10) & 0xFFF) as u64;
        let rn = ((insn >> 5) & 0x1F) as u32;
        let rt = (insn & 0x1F) as usize;
        let is_q = (opc >> 1) & 1 == 1 && size == 0;
        let scale: u64 = if is_q { 16 } else { 1u64 << size };
        let offset = imm12 * scale;
        let base = a.read_xsp(rn);
        let addr = base.wrapping_add(offset);
        let is_load = opc & 1 == 1;
        if is_q {
            if is_load { a.v[rt] = rd128(mem, addr)?; }
            else       { wr128(mem, addr, a.v[rt])?; }
        } else {
            let sz = scale as usize;
            if is_load {
                let val = rd(mem, addr, sz.max(1))?;
                a.v[rt] = val as u128;
            } else {
                wr(mem, addr, a.v[rt] as u64, sz.max(1))?;
            }
        }
        return Ok(());
    }

    // ── STR/LDR SIMD pre/post/unscaled/register ─────────────────────────────
    // size 111100 opc ...
    if (insn >> 24) & 0x3F == 0b111100 {
        let opc = (insn >> 22) & 0x3;
        let rn = ((insn >> 5) & 0x1F) as u32;
        let rt = (insn & 0x1F) as usize;
        let idx_type = (insn >> 10) & 0x3;
        let base = a.read_xsp(rn);
        let (addr, wb) = if (insn >> 21) & 1 == 1 {
            // Register offset
            let rm = ((insn >> 16) & 0x1F) as u32;
            let option = (insn >> 13) & 0x7;
            let s_bit = (insn >> 12) & 1;
            let is_q = size == 0 && opc >= 2;
            let shift = if s_bit == 1 { if is_q { 4 } else { size } } else { 0 };
            let rm_val = a.read_x(rm);
            let offset = match option {
                0b010 => (rm_val as u32 as u64) << shift,
                0b011 => rm_val << shift,
                0b110 => (rm_val as i32 as i64 as u64) << shift,
                0b111 => rm_val << shift,
                _ => rm_val,
            };
            (base.wrapping_add(offset), None)
        } else {
            let imm9 = sext((insn >> 12) & 0x1FF, 9) as u64;
            match idx_type {
                0b00 => (base.wrapping_add(imm9), None),
                0b01 => (base, Some(base.wrapping_add(imm9))),
                0b11 => { let x = base.wrapping_add(imm9); (x, Some(x)) }
                _ => (base, None),
            }
        };
        let is_q = size == 0 && opc >= 2;
        let is_store = opc & 1 == 0;
        if is_store {
            if is_q { wr128(mem, addr, a.v[rt])?; }
            else {
                let sz = (1usize << size).max(1);
                wr(mem, addr, a.v[rt] as u64, sz)?;
            }
        } else {
            if is_q { a.v[rt] = rd128(mem, addr)?; }
            else {
                let sz = (1usize << size).max(1);
                let val = rd(mem, addr, sz)?;
                a.v[rt] = val as u128;
            }
        }
        if let Some(w) = wb { a.write_xsp(rn, w); }
        return Ok(());
    }

    // ── LD1/ST1 multiple structures ──────────────────────────────────────────
    // 0 Q 001100 L 0 00000 opcode size Rn Rt     (no post-index)
    // 0 Q 001100 L 1 Rm    opcode size Rn Rt     (post-index)
    if (insn >> 24) & 0x3E == 0b001100 {
        let q = (insn >> 30) & 1;
        let l = (insn >> 22) & 1;
        let rm = ((insn >> 16) & 0x1F) as u32;
        let post_index = (insn >> 23) & 1 == 1;
        let opcode = (insn >> 12) & 0xF;
        let rn = ((insn >> 5) & 0x1F) as u32;
        let rt = (insn & 0x1F) as usize;
        let reg_bytes: usize = if q == 1 { 16 } else { 8 };
        let nregs: usize = match opcode {
            0b0111 => 1, // LD1/ST1 x1
            0b1010 => 2, // LD1/ST1 x2
            0b0110 => 3, // LD1/ST1 x3
            0b0010 => 4, // LD1/ST1 x4
            0b1000 => 2, // LD2/ST2
            0b0100 => 3, // LD3/ST3
            0b0000 => 4, // LD4/ST4
            _ => return Ok(()),
        };
        let base = a.read_xsp(rn);
        let mut addr = base;
        if l == 1 {
            for i in 0..nregs {
                let vr = (rt + i) % 32;
                if reg_bytes == 16 {
                    a.v[vr] = rd128(mem, addr)?;
                } else {
                    let lo = rd(mem, addr, 8)?;
                    a.v[vr] = lo as u128;
                }
                addr = addr.wrapping_add(reg_bytes as u64);
            }
        } else {
            for i in 0..nregs {
                let vr = (rt + i) % 32;
                if reg_bytes == 16 {
                    wr128(mem, addr, a.v[vr])?;
                } else {
                    wr(mem, addr, a.v[vr] as u64, 8)?;
                }
                addr = addr.wrapping_add(reg_bytes as u64);
            }
        }
        if post_index {
            let offset = if rm == 31 { (nregs * reg_bytes) as u64 }
                         else        { a.read_x(rm) };
            a.write_xsp(rn, base.wrapping_add(offset));
        }
        return Ok(());
    }

    // ── Unmatched SIMD load/store — skip ─────────────────────────────────────
    Ok(())
}

//! AArch64 32-bit fixed-width instruction decoder.
//!
//! Top-level dispatch on bits [28:25] (op0), then per-group decoders.
//! Returns an [`Instruction`] struct with all relevant fields populated.
//!
//! Reference: ARM DDI 0487 (AArch64 Architecture Reference Manual), C4.

use super::insn::{Instruction, Opcode};
use crate::DecodeError;

// ── Bit helpers ───────────────────────────────────────────────────────────────

#[inline(always)]
fn bit(v: u32, pos: u32) -> u32 {
    (v >> pos) & 1
}
#[inline(always)]
fn bits(v: u32, hi: u32, lo: u32) -> u32 {
    (v >> lo) & ((1 << (hi - lo + 1)) - 1)
}
#[inline(always)]
fn sext(v: u64, bits_wide: u32) -> i64 {
    let shift = 64 - bits_wide;
    ((v as i64) << shift) >> shift
}

pub fn decode(raw: u32, pc: u64) -> Result<Instruction, DecodeError> {
    let op0 = bits(raw, 28, 25);
    let mut insn = Instruction::zeroed();
    insn.raw = raw;
    insn.pc = pc;

    match op0 {
        0b1000 | 0b1001 => decode_dp_imm(raw, &mut insn),
        0b1010 | 0b1011 => decode_branch_sys(raw, &mut insn),
        0b0100 | 0b0110 | 0b1100 | 0b1110 => decode_ldst(raw, &mut insn),
        0b0101 | 0b1101 => decode_dp_reg(raw, &mut insn),
        0b0111 | 0b1111 => decode_simd_fp(raw, &mut insn),
        _ => {
            insn.opcode = Opcode::Undefined;
        }
    }

    if insn.opcode == Opcode::Undefined {
        return Err(DecodeError::Unknown { raw, pc });
    }
    Ok(insn)
}

// ── Data-processing immediate (op0 = 100x) ───────────────────────────────────

fn decode_dp_imm(raw: u32, i: &mut Instruction) {
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

// ── Branches and system instructions (op0 = 101x) ────────────────────────────

fn decode_branch_sys(raw: u32, i: &mut Instruction) {
    let sf = bit(raw, 31) != 0;
    i.sf = sf;

    // Top discriminant: bits[31:29]
    let top3 = bits(raw, 31, 29);

    match top3 {
        0b000 => {
            // B.cond (bit31=0, bit30=1 → taken care of below) or B
            // Actually: bit29=0 → B / BL based on bit31; bit29=1 → conditionals
            // B: op=0b000_0 (bits31:29=000, bit24=0)
            if bit(raw, 24) == 0 {
                let imm26 = bits(raw, 25, 0);
                i.imm = sext((imm26 << 2) as u64, 28);
                i.opcode = Opcode::B;
            } else {
                i.opcode = Opcode::Undefined;
            }
        }
        0b001 => {
            // BL
            let imm26 = bits(raw, 25, 0);
            i.imm = sext((imm26 << 2) as u64, 28);
            i.opcode = Opcode::Bl;
        }
        0b010 => {
            // BR / BLR / RET (bits[31:29]=010, bit[24:21]=0000)
            // Conditional branches with cond (bits[31:29]=010, bit24=1)
            if bit(raw, 31) == 0 && bit(raw, 29) == 0 {
                // Actually let's look at bits[30:25] more carefully
                // The AArch64 encoding for B.cond has bit30=1
                let b30 = bit(raw, 30);
                if b30 == 1 {
                    // B.cond: bits[31:29]=010, bit24=0
                    let imm19 = bits(raw, 23, 5);
                    i.cond = bits(raw, 3, 0);
                    i.imm = sext((imm19 << 2) as u64, 21);
                    i.opcode = Opcode::BCond;
                } else {
                    i.opcode = Opcode::Undefined;
                }
            } else if bits(raw, 30, 25) == 0b111010 {
                // Unconditional branch register: bits[30:25]=111010
                let opc = bits(raw, 24, 21);
                i.rn = bits(raw, 9, 5);
                i.opcode = match opc {
                    0b0000 => Opcode::Br,
                    0b0001 => Opcode::Blr,
                    0b0010 => Opcode::Ret,
                    _ => Opcode::Undefined,
                };
            } else {
                i.opcode = Opcode::Undefined;
            }
        }
        _ => {
            i.opcode = Opcode::Undefined;
        }
    }

    // Override with better top-level checks for common patterns
    // B.cond: bits[31:24] = 0101_0100
    if bits(raw, 31, 24) == 0b0101_0100 {
        let imm19 = bits(raw, 23, 5);
        i.cond = bits(raw, 3, 0);
        i.imm = sext((imm19 << 2) as u64, 21);
        i.opcode = Opcode::BCond;
        return;
    }

    // ERET
    if raw == 0xD69F_03E0 {
        i.opcode = Opcode::Eret;
        return;
    }

    // BR/BLR/RET: bits[31:21] == 0b1101_0110_0_xxx
    if bits(raw, 31, 25) == 0b110101_1 {
        let opc = bits(raw, 24, 21);
        i.rn = bits(raw, 9, 5);
        i.opcode = match opc {
            0b0000 => Opcode::Br,
            0b0001 => Opcode::Blr,
            0b0010 => Opcode::Ret,
            _ => Opcode::Undefined,
        };
        return;
    }

    // CBZ / CBNZ: bits[30:25] = 0b011010 or 0b011011
    if bits(raw, 30, 24) == 0b011010_0 || bits(raw, 30, 24) == 0b011010_1 {
        let imm19 = bits(raw, 23, 5);
        i.rd = bits(raw, 4, 0); // Rt
        i.imm = sext((imm19 << 2) as u64, 21);
        i.opcode = if bit(raw, 24) == 0 {
            Opcode::Cbz
        } else {
            Opcode::Cbnz
        };
        return;
    }

    // TBZ / TBNZ
    if bits(raw, 30, 25) == 0b011011 {
        let imm14 = bits(raw, 18, 5);
        i.rn = bits(raw, 4, 0); // Rt
        i.imm = sext((imm14 << 2) as u64, 16);
        i.imm2 = (bit(raw, 31) << 5 | bits(raw, 23, 19)) as u64; // bit position
        i.opcode = if bit(raw, 24) == 0 {
            Opcode::Tbz
        } else {
            Opcode::Tbnz
        };
        return;
    }

    // B / BL: bits[30:26] = 00101 or 10101
    if bits(raw, 30, 26) == 0b00101 {
        let imm26 = bits(raw, 25, 0);
        i.imm = sext((imm26 << 2) as u64, 28);
        i.opcode = if bit(raw, 31) == 0 {
            Opcode::B
        } else {
            Opcode::Bl
        };
        return;
    }

    // Exception-generating instructions: bits[31:24] = 0b1101_0100 (SVC/HVC/SMC/BRK)
    if bits(raw, 31, 24) == 0b1101_0100 {
        decode_system(raw, i);
        return;
    }

    // System instructions: bits[31:22] = 0b1101_0101_00
    if bits(raw, 31, 22) == 0b1101_0101_00 {
        decode_system(raw, i);
    }
}

fn decode_system(raw: u32, i: &mut Instruction) {
    let l = bit(raw, 21); // 0=MSR/SYS, 1=MRS/SYSL
    let op0 = bits(raw, 20, 19);
    let op1 = bits(raw, 18, 16);
    let crn = bits(raw, 15, 12);
    let crm = bits(raw, 11, 8);
    let op2 = bits(raw, 7, 5);
    let rt = bits(raw, 4, 0);

    // SVC / HVC / SMC: bits[31:24]=11010100
    if bits(raw, 31, 24) == 0b1101_0100 {
        let opc = bits(raw, 23, 21);
        let ll = bits(raw, 1, 0);
        match (opc, ll) {
            (0b000, 0b01) => {
                i.imm = bits(raw, 20, 5) as i64;
                i.opcode = Opcode::Svc;
            }
            (0b000, 0b10) => {
                i.imm = bits(raw, 20, 5) as i64;
                i.opcode = Opcode::Hvc;
            }
            (0b000, 0b11) => {
                i.imm = bits(raw, 20, 5) as i64;
                i.opcode = Opcode::Smc;
            }
            (0b001, 0b00) => {
                i.imm = bits(raw, 20, 5) as i64;
                i.opcode = Opcode::Brk;
            }
            _ => {
                i.opcode = Opcode::Undefined;
            }
        }
        return;
    }

    // NOP
    if raw == 0xD503_201F {
        i.opcode = Opcode::Nop;
        return;
    }
    // WFI
    if raw == 0xD503_207F {
        i.opcode = Opcode::Wfi;
        return;
    }
    // ESB / SB (v8.4/v8.5 system hints)
    if raw == 0xD503_221F {
        i.opcode = Opcode::Esb;
        return;
    }
    if raw == 0xD503_30FF {
        i.opcode = Opcode::Sb;
        return;
    }
    // CFINV (v8.4 FlagM): D500401F
    if raw == 0xD500_401F {
        i.opcode = Opcode::Cfinv;
        return;
    }
    // XAFLAG / AXFLAG (v8.4 FlagM)
    if raw == 0xD500_403F {
        i.opcode = Opcode::Xaflag;
        return;
    }
    if raw == 0xD500_405F {
        i.opcode = Opcode::Axflag;
        return;
    }
    // BTI (v8.5): HINT with CRm=4 and op2 in {2,4,6} — imm value encodes target type
    // HINT encoding: bits[31:8]=0b1101_0101_0000_0011_0010, bits[7:5]=op2, bits[4:0]=0b11111
    // BTI: CRm=4 → bits[11:8]=0100, op2 in {2,4,6} → bits[7:5]={010,100,110}
    if bits(raw, 31, 12) == 0b1101_0101_0000_0011_0010
        && bits(raw, 11, 8) == 0b0100
        && bits(raw, 4, 0) == 0b11111
        && (bits(raw, 7, 5) == 2 || bits(raw, 7, 5) == 4 || bits(raw, 7, 5) == 6)
    {
        i.opcode = Opcode::Bti;
        return;
    }
    // ISB
    if bits(raw, 31, 8) == 0b1101_0101_0000_0011_0010 {
        let barrier_op = bits(raw, 7, 5);
        i.opcode = match barrier_op {
            0b110 => Opcode::Isb,
            0b100 | 0b101 => Opcode::Dsb,
            0b010 | 0b011 => Opcode::Dmb,
            _ => Opcode::Nop,
        };
        return;
    }

    // MSR (immediate) — PSTATE field access (DAIFSet, DAIFClr, SPSel)
    // Encoding: op0=00, L=0, CRn=0100
    // Pack op1:CRm:op2 into imm so executor can extract:
    //   op1 = (imm >> 16) & 7, crm = (imm >> 8) & 0xF, op2 = (imm >> 5) & 7
    if op0 == 0b00 && l == 0 && crn == 0b0100 {
        i.opcode = Opcode::MsrImm;
        i.imm = ((op1 as i64) << 16) | ((crm as i64) << 8) | ((op2 as i64) << 5);
        return;
    }

    // MRS / MSR
    // System register encoding: bits[31:20]=0b110101010001 (MSR) or 0b110101010011 (MRS)
    if bits(raw, 31, 20) == 0b1101_0101_0001 || bits(raw, 31, 20) == 0b1101_0101_0011 {
        i.opcode = if l == 1 { Opcode::Mrs } else { Opcode::Msr };
        i.rd = rt;
        // Encode sysreg as imm: op0:op1:CRn:CRm:op2
        i.imm = ((op0 << 14) | (op1 << 11) | (crn << 7) | (crm << 3) | op2) as i64;
        return;
    }

    // DC ZVA: op0=01, op1=011, CRn=0111, CRm=0100, op2=001
    if op0 == 0b01 && op1 == 0b011 && crn == 0b0111 && crm == 0b0100 && op2 == 0b001 {
        i.rd = rt; // Xt holds the VA
        i.opcode = Opcode::DcZva;
        return;
    }

    // Other SYS instructions (TLBI, DC, IC, AT) — treat as NOP in SE mode
    i.rd = rt;
    i.opcode = Opcode::Sys;
}

// ── Load/Store (op0 = 0x00, 0x10, 0x11, 0x01) ────────────────────────────────

fn decode_ldst(raw: u32, i: &mut Instruction) {
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
        // For load literal, opc bits[31:30] encode width, not size like reg loads:
        //   00 = LDR Wt (32-bit zero-extend)
        //   01 = LDR Xt (64-bit)
        //   10 = LDRSW  (sign-extend 32-bit to 64)
        //   11 = PRFM   (prefetch — not a real load)
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
        decode_ldst_simd(raw, i);
        return;
    }

    // ── PRFM (prefetch memory): pre/post/unscaled/register-offset forms ────
    // These share the generic "size 111000 opc ..." space with scalar loads
    // and stores. PRFM is size=11, V=0, opc=10 and must decode to a non-faulting
    // prefetch hint rather than a real memory access.
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
    // LDAPUR: bits[29:24]=011001, bit21=0, bits[11:10]=00
    // STLUR:  bits[29:24]=011000, bit21=0, bits[11:10]=00
    // Discriminated by bit24=1 only for scaled forms — these share bit24=0
    // but have bits[29:25]=0b01100 (STLUR) or 0b01100 + opc=01 (LDAPUR).
    // Actual encoding: bits[31:30]=size, bits[29:24]=011001 (load) / 011000 (store).
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
/// For signed loads: opc=10 → X target (sf=true), opc=11 → W target (sf=false).
/// For unsigned loads/stores: sf = (size == 3) for doubleword.
fn set_ldst_sf(i: &mut Instruction, size: u32, opc: u32, signed: bool) {
    if signed {
        // opc=10 (bit0=0): sign-extend to 64-bit (X register)
        // opc=11 (bit0=1): sign-extend to 32-bit (W register)
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
        (0, true, true, _) => Opcode::Ldrsb, // opc=10 with size=0 is LDRSB (sign-extend to 32)
        // Halfword
        (1, true, false, false) => Opcode::Strh,
        (1, true, false, true) => Opcode::Sturh,
        (1, false, false, false) => Opcode::Ldrh,
        (1, false, false, true) => Opcode::Ldurh,
        (1, false, true, false) => Opcode::Ldrsh,
        (1, false, true, true) => Opcode::Ldursh,
        (1, true, true, _) => Opcode::Ldrsh, // opc=10 with size=1 is LDRSH (sign-extend to 32)
        // Word
        (2, true, false, false) => Opcode::Str,
        (2, true, false, true) => Opcode::Stur,
        (2, false, false, false) => Opcode::Ldr,
        (2, false, false, true) => Opcode::Ldur,
        (2, false, true, false) => Opcode::Ldrsw,
        (2, false, true, true) => Opcode::Ldursw,
        (2, true, true, _) => Opcode::Ldrsw, // opc=10 with size=2 is LDRSW
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
    let s = bit(raw, 12); // shift
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
        // SIMD/FP pair: scale depends on opc (00=S/32, 01=D/64, 10=Q/128)
        let scale = match opc {
            0b00 => 2u32,
            0b01 => 3,
            0b10 => 4,
            _ => 2,
        };
        i.imm = sext(imm7 as u64, 7) << scale;
        i.ftype = opc; // 0=S, 1=D, 2=Q
        i.sf = opc >= 1;
    } else {
        let scale = if opc == 0b10 { 3u32 } else { 2u32 };
        i.imm = sext(imm7 as u64, 7) << scale;
        i.sf = opc == 0b10;
        i.signed_load = opc == 0b01; // LDPSW
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

/// Decode SIMD/FP scalar load/store (V=1, not pair, not literal).
fn decode_ldst_simd(raw: u32, i: &mut Instruction) {
    let size = bits(raw, 31, 30);
    let opc = bits(raw, 23, 22);
    i.rd = bits(raw, 4, 0);
    i.rn = bits(raw, 9, 5);
    // ftype: size selects B(0)/H(1)/S(2)/D(3); opc bit distinguishes Q(128)
    // opc=0b00 → STR, opc=0b01 → LDR, opc=0b10 → STR Q, opc=0b11 → LDR Q
    let is_128 = size == 0b00 && (opc & 0b10) != 0;
    i.ftype = if is_128 { 4 } else { size }; // 0=B,1=H,2=S,3=D,4=Q

    let is_load = (opc & 1) != 0;

    // Unsigned offset: bit24=1
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

    // Register offset: bit24=0, bit21=1, bits[11:10]=10
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
        // Signal register-offset via imm=i64::MIN (neither pre nor post index)
        i.imm = i64::MIN;
        return;
    }

    // Unscaled offset: bit24=0, bits[11:10]=00
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

    // Pre/post-index: bit24=0, bits[11:10]=01 or 11
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

/// Decode LSE atomic memory operations.
///
/// Encoding: `size 111000 A R Rs o3 opc Rn Rt`
/// - `bit[15]` = o3: when 1, SWP family; when 0, arithmetic ops
/// - `bits[14:12]` = opc: selects LDADD/LDCLR/LDEOR/LDSET
fn decode_ldst_atomic(raw: u32, i: &mut Instruction) {
    let size = bits(raw, 31, 30);
    let a = bit(raw, 23); // acquire
    let r = bit(raw, 22); // release
    let rs = bits(raw, 20, 16);
    let o3 = bit(raw, 15); // 1 = SWP, 0 = arithmetic
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

    // LDAPR (v8.3 LRCPC): o3=0, opc=100, Rs=11111 (no writeback, acquire-only RCpc)
    // Encoding: size:V:opc=?:0:01 with bit[23]=1 (a), bit[22]=0, Rs=11111, o3=0, opc=100
    if o3 == 0 && opc == 0b100 && rs == 31 && r == 0 {
        i.opcode = match size {
            0 => Opcode::Ldaprb,
            1 => Opcode::Ldaprh,
            _ => Opcode::Ldapr,
        };
        return;
    }

    // bit[15]=o3=1 selects SWP (atomic swap); opc field is ignored
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

    // LDAR/STLR (load-acquire/store-release, no exclusivity):
    //   LDAR*: bits[23:21]=110, rt2=11111
    //   STLR*: bits[23:21]=100, rt2=11111
    if o2 == 1 && o1 == 0 && o0 == 1 && rs == 31 && rt2 == 31 {
        i.acquire = l == 1;
        i.release = l == 0;
        i.opcode = if l == 1 { Opcode::Ldar } else { Opcode::Stlr };
        return;
    }

    // o2 = 1: LSE atomics (CAS/CASA/CASL/CASAL / CASP)
    if o2 == 1 {
        // CAS: o1=1, o0=1, rt2=11111 → single CAS
        // CASP: o1=1, o0=1, rt2 != 11111 → pair CAS
        // Acquire: o1 bit; Release: o0 bit
        i.acquire = o1 != 0;
        i.release = o0 != 0;
        if rt2 == 31 {
            i.opcode = Opcode::Cas;
        } else {
            i.opcode = Opcode::Casp;
        }
        return;
    }

    // CLREX: special encoding
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

// ── Data-processing register (op0 = 0101 / 1101) ─────────────────────────────

fn decode_dp_reg(raw: u32, i: &mut Instruction) {
    let sf = bit(raw, 31) != 0;
    let _op54 = bits(raw, 30, 29);
    let _s = bit(raw, 29) != 0;
    i.sf = sf;
    i.rd = bits(raw, 4, 0);
    i.rn = bits(raw, 9, 5);
    i.rm = bits(raw, 20, 16);

    // Logical shifted register: bits[28:24]=01010
    // Add/sub shifted/extended register: bits[28:24]=01011
    // Distinguish by bit24 (not bit28 — both have bit28=0):
    if bit(raw, 28) == 0 && bit(raw, 24) == 0 {
        decode_dp_logical_shift(raw, i);
        return;
    }

    // Add/Sub shifted: bits[28:24]=01011
    // Add/Sub extended: bits[28:24]=01011 + bit21=1
    // Mul/Div: bits[28:24]=11011
    // Conditional select: bit28=1, bits[23:21]=100
    // Data proc 1src: bit28=1, bits[23:21]=000, bit30=1

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

    // Conditional compare: bits[29:21] = 0b111010010 or similar — check bit28=1, bit27=1, bits24:21=0010
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

    // ADC / SBC: bits[28:21] = 11010000 (bit24=0, bits[23:21]=000, bit21=0)
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
    // SETF8/SETF16: bits[31:21]=0_0_111010_00_0, bits[15:10]=00100_0, bit4=0, bit3=1, bits[2:0]=101
    // Pattern: raw & 0xFFFF_F81F == 0x3A00_080D (SETF8) or 0x3A00_480D (SETF16)
    if raw & 0xFFFF_F01F == 0x3A00_000D {
        i.rn = bits(raw, 9, 5);
        i.opcode = if bit(raw, 14) == 0 {
            Opcode::Setf8
        } else {
            Opcode::Setf16
        };
        return;
    }
    // RMIF: bits[31:21]=10111010_000, bits[15:10]=000100, bit0=0
    // Encoding: sf=1, op=0, S=1, bits[28:21]=11010000... check with mask
    if bits(raw, 31, 21) == 0b1011_1010_000 && bits(raw, 15, 10) == 0b000100 {
        // imm6 = bits[21:15] → rotation amount (6 bits)
        // mask = bits[3:0] (which NZCV bits to update)
        i.rn = bits(raw, 9, 5);
        i.imm = bits(raw, 20, 15) as i64; // rotation (6-bit)
        i.imm2 = bits(raw, 3, 0) as u64; // mask (4-bit: NZCV)
        i.opcode = Opcode::Rmif;
        return;
    }

    // Add/sub register (shifted or extended)
    let extend_mode = bit(raw, 21) != 0; // bit21=1 → extended reg
    let shift_type = bits(raw, 23, 22);
    let shift_amt = bits(raw, 15, 10);
    let _imm6 = shift_amt;
    i.shift_type = shift_type;
    i.shift_amt = shift_amt;

    if extend_mode {
        i.extend_type = bits(raw, 15, 13);
        i.extend_amt = bits(raw, 12, 10);
        // Don't use shift_amt for extended register — it was wrongly
        // extracted from bits[15:10] which overlap with option:imm3.
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
    // Bits [23:21] distinguish mul from div
    let _op31 = bits(raw, 31, 29);
    let op1 = bits(raw, 23, 21);
    let ra = bits(raw, 14, 10);
    let o0 = bit(raw, 15);
    i.ra = ra;

    match op1 {
        0b000 => {
            // MADD / MSUB
            i.opcode = if o0 == 0 { Opcode::Madd } else { Opcode::Msub };
        }
        0b001 => {
            // SMADDL / SMSUBL (sf must be 1 for 64-bit result)
            i.opcode = if o0 == 0 {
                Opcode::Smaddl
            } else {
                Opcode::Smsubl
            };
        }
        0b010 => {
            // SMULH: op1=010 is always signed multiply-high (64x64→high64)
            i.opcode = Opcode::Smulh;
        }
        0b101 => {
            // UMADDL / UMSUBL
            i.opcode = if o0 == 0 {
                Opcode::Umaddl
            } else {
                Opcode::Umsubl
            };
        }
        0b110 => {
            // UMULH: op1=110 is always unsigned multiply-high
            i.opcode = Opcode::Umulh;
        }
        _ => {
            // UDIV / SDIV: within data-proc-2src group
            if bit(raw, 10) == 1 {
                i.opcode = if bit(raw, 29) == 0 {
                    Opcode::Udiv
                } else {
                    Opcode::Sdiv
                };
            } else {
                // Shift variable (LSLV/LSRV/ASRV/RORV)
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
        0b001000 => Opcode::Lsl, // LSLV
        0b001001 => Opcode::Lsr, // LSRV
        0b001010 => Opcode::Asr, // ASRV
        0b001011 => Opcode::Ror, // RORV
        // CRC32B/H/W (sf=0) and CRC32X (sf=1)
        0b010000 => {
            i.size = 0;
            Opcode::Crc32
        } // CRC32B
        0b010001 => {
            i.size = 1;
            Opcode::Crc32
        } // CRC32H
        0b010010 => {
            i.size = 2;
            Opcode::Crc32
        } // CRC32W
        0b010011 => {
            i.size = 3;
            Opcode::Crc32
        } // CRC32X
        // CRC32CB/CH/CW (sf=0) and CRC32CX (sf=1)
        0b010100 => {
            i.size = 0;
            Opcode::Crc32c
        } // CRC32CB
        0b010101 => {
            i.size = 1;
            Opcode::Crc32c
        } // CRC32CH
        0b010110 => {
            i.size = 2;
            Opcode::Crc32c
        } // CRC32CW
        0b010111 => {
            i.size = 3;
            Opcode::Crc32c
        } // CRC32CX
        _ => Opcode::Undefined,
    };
}

fn decode_dp_1src(raw: u32, i: &mut Instruction) {
    let _opcode2 = bits(raw, 25, 16);
    let op2 = bits(raw, 15, 10);

    i.rn = bits(raw, 9, 5);
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

// ── SIMD / FP (op0 = 0111 / 1111) ────────────────────────────────────────────

fn decode_simd_fp(raw: u32, i: &mut Instruction) {
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
    let q = bit(raw, 30); // 0=64-bit (8B/4H/2S), 1=128-bit (16B/8H/4S/2D)
    let u = bit(raw, 29);
    i.sf = q != 0; // re-use sf for Q bit
    i.size = bits(raw, 23, 22); // element size: 00=8b, 01=16b, 10=32b, 11=64b

    // SIMD three-same: bits[28:24]=01110, bit21=1, bit10=1
    if bits(raw, 28, 24) == 0b01110 && bit(raw, 21) == 1 && bit(raw, 10) == 1 {
        let opcode5 = bits(raw, 15, 11);
        i.opcode = match (u, opcode5) {
            (0, 0b10000) => Opcode::SimdAdd,
            (1, 0b10000) => Opcode::SimdSub,
            (0, 0b10011) => Opcode::SimdMul,
            (0, 0b00011) => Opcode::SimdAnd, // AND is u=0,opcode=00011
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
            _ => Opcode::SimdOther,
        };
        return;
    }

    // SIMD two-reg misc + across-lanes: bits[28:24]=0x1110, bit21=1, bits[11:10]=10
    if bits(raw, 28, 24) == 0b01110 && bit(raw, 21) == 1 && bits(raw, 11, 10) == 0b10 {
        let opcode5 = bits(raw, 16, 12);
        i.opcode = match (u, opcode5) {
            (0, 0b01000) => Opcode::SimdCmgt0, // CMGT #0
            (0, 0b01001) => Opcode::SimdCmeq0, // CMEQ #0
            (0, 0b01010) => Opcode::SimdCmlt0, // CMLT #0
            (1, 0b01000) => Opcode::SimdCmge0, // CMGE #0
            (1, 0b01001) => Opcode::SimdCmle0, // CMLE #0
            (1, 0b00101) => Opcode::SimdNot,   // NOT = u=1, opcode=00101
            (1, 0b01011) => Opcode::SimdNeg,
            (0, 0b01011) => Opcode::SimdAbs,
            (0, 0b00100) => Opcode::SimdClz,
            (0, 0b00101) => Opcode::SimdCnt,
            (1, 0b00000) => Opcode::SimdRev64,
            // Across-lanes reductions
            (1, 0b01010) => Opcode::SimdUmaxv, // UMAXV
            (1, 0b11010) => Opcode::SimdUminv, // UMINV
            (0, 0b11011) => Opcode::SimdAddv,  // ADDV
            _ => Opcode::SimdOther,
        };
        return;
    }

    // SIMD copy (DUP, INS, UMOV, SMOV): bits[28:24]=0x1110, bit21=0, bits[14:11]=vary
    if bits(raw, 28, 24) == 0b01110 && bit(raw, 21) == 0 {
        let imm4 = bits(raw, 14, 11);
        i.imm = bits(raw, 20, 16) as i64; // imm5 encodes element index + size
        i.opcode = match (u, imm4) {
            (0, 0b0000) => Opcode::SimdDup, // DUP (element)
            (0, 0b0001) => Opcode::SimdDup, // DUP (general)
            (0, 0b0011) => Opcode::SimdIns,
            (0, 0b0101) => Opcode::SimdSmov,
            (0, 0b0111) => Opcode::SimdUmov,
            _ => Opcode::SimdOther,
        };
        return;
    }

    // SIMD modified immediate (MOVI/MVNI/FMOV): bits[28:24]=0x1111
    if bits(raw, 28, 24) == 0b01111 {
        i.opcode = Opcode::SimdMovi;
        let abc = bits(raw, 18, 16);
        let defgh = bits(raw, 9, 5);
        i.imm = ((abc << 5) | defgh) as i64;
        return;
    }

    // SIMD shift by immediate: bits[28:23]=0x11110
    if bits(raw, 28, 23) == 0b011110 {
        let opcode5 = bits(raw, 15, 11);
        i.imm = bits(raw, 22, 16) as i64; // immh:immb
        i.opcode = match (u, opcode5) {
            (0, 0b00000) => Opcode::SimdSshr,
            (1, 0b00000) => Opcode::SimdUshr,
            (0, 0b01010) => Opcode::SimdShl,
            _ => Opcode::SimdOther,
        };
        return;
    }

    // SIMD across-lanes: bits[28:24]=0x1110, bit21=1, bits[16:12]=varies, bits[11:10]=10
    // EXT: bits[28:24]=0x1110, bit21=0, bits[15:11]=00000
    // ZIP/UZP/TRN: bits[28:24]=0x1110, bit21=0, bits[15:14]=00, bits[12:11]=vary

    // ── v8.4 Dot Product (SDOT/UDOT): bits[28:24]=01110, bit21=0, bits[15:12]=1001
    // SDOT: u=0, UDOT: u=1; size field encodes element arrangement
    if bits(raw, 28, 24) == 0b01110 && bit(raw, 21) == 0 && bits(raw, 15, 12) == 0b1001 {
        i.ra = bits(raw, 14, 10); // Ra (accumulate register)
        i.opcode = if u == 0 { Opcode::Sdot } else { Opcode::Udot };
        return;
    }

    // ── v8.3 FCMA: FCADD/FCMLA
    // FCADD: bits[28:24]=01110, bit21=0, bits[15:11]=11001, bit10=0
    if bits(raw, 28, 24) == 0b01110 && bit(raw, 21) == 0 && bits(raw, 15, 10) == 0b110010 {
        i.imm = bits(raw, 12, 12) as i64; // rotation (0=90°, 1=270°)
        i.opcode = Opcode::Fcadd;
        return;
    }
    // FCMLA: bits[28:24]=01110, bit21=0, bits[15:14]=11, bits[11:10]=01
    if bits(raw, 28, 24) == 0b01110
        && bit(raw, 21) == 0
        && bits(raw, 15, 14) == 0b11
        && bits(raw, 11, 10) == 0b01
    {
        i.ra = bits(raw, 14, 10);
        i.imm = bits(raw, 13, 12) as i64; // rotation field
        i.opcode = Opcode::Fcmla;
        return;
    }

    // ── Crypto stubs (SHA3/SHA512/SM3/SM4) — decode and raise #UD on execute ──
    // SHA3 family (EOR3, RAX1, XAR, BCAX): bits[28:23]=0b110010
    if bits(raw, 28, 23) == 0b110010 {
        i.opcode = Opcode::Sha3;
        return;
    }
    // SHA512: bits[28:24]=11001, bit21=1 (FP-like encoding)
    if bits(raw, 28, 24) == 0b11001 && bit(raw, 21) == 1 {
        i.opcode = Opcode::Sha512;
        return;
    }
    // SM3: bits[28:24]=11001, bit21=0, bits[23:22]=00
    if bits(raw, 28, 24) == 0b11001 && bit(raw, 21) == 0 && bits(raw, 23, 22) == 0b00 {
        i.opcode = Opcode::Sm3;
        return;
    }
    // SM4: bits[28:24]=11001, bit21=0, bits[23:22]=10
    if bits(raw, 28, 24) == 0b11001 && bit(raw, 21) == 0 && bits(raw, 23, 22) == 0b10 {
        i.opcode = Opcode::Sm4;
        return;
    }

    // Catch-all for unhandled SIMD
    i.opcode = Opcode::SimdOther;
}

fn decode_fp_data(raw: u32, i: &mut Instruction) {
    let ptype = bits(raw, 23, 22);
    let op = bits(raw, 21, 16);
    let _op2 = bits(raw, 15, 10);
    i.sf = bit(raw, 31) != 0; // FP uses real sf (bit31), not Q
    i.ftype = ptype;
    i.rd = bits(raw, 4, 0);
    i.rn = bits(raw, 9, 5);
    i.rm = bits(raw, 20, 16);

    // FJCVTZS (v8.3 JSCVT): ptype=01 (double), op=0b111110, bit21=0
    // Encoding: sf=0, ptype=01, bit21=0, op=111110, bits[15:10]=000000
    if ptype == 0b01 && bits(raw, 21, 16) == 0b011110 && bits(raw, 15, 10) == 0b000000 {
        i.opcode = Opcode::Fjcvtzs;
        return;
    }

    // FMOV (immediate): bits[21:16]=0b000001, bit11=0
    if op == 0b000001 && bit(raw, 11) == 0 {
        let imm8 = bits(raw, 20, 13);
        i.imm = imm8 as i64;
        i.opcode = Opcode::FmovImm;
        return;
    }

    // FMOV (register): op=0b000000
    if op == 0b000000 {
        i.opcode = Opcode::FmovReg;
        return;
    }

    // FMOV to/from GPR: bits[18:16]=110 (FP→GPR) or 111 (GPR→FP), bits[20:19]=00
    // bit21=0 for W↔S, bit21=1 for X↔D
    if bits(raw, 20, 19) == 0 && (bits(raw, 18, 16) == 0b110 || bits(raw, 18, 16) == 0b111) {
        i.opcode = Opcode::FmovGpr;
        return;
    }

    // ADDP (scalar): ADDP Dd, Vn.2D — bit[30]=1, ptype=11, bits[15:10]=101110 (ADDP two-reg-misc)
    // Encoding 0x5E/0x7E F1_B800 pattern: bit30=1, size=11, opcode=11011 in bits[16:12]
    if bit(raw, 30) == 1 && ptype == 3 && bits(raw, 15, 10) == 0b101110 {
        i.opcode = Opcode::ScalarAddp;
        return;
    }

    // FP arithmetic: remaining bit21=1 cases (and bit21=0 non-FMOV)
    let _op3 = bits(raw, 14, 10);
    i.fp_rounding = bits(raw, 23, 22); // reuse as rounding mode for convert ops
    i.opcode = match bits(raw, 15, 10) {
        0b001000 => Opcode::Fcmp,
        0b001001 => Opcode::Fcmpe,
        _ => match op {
            0b000010 => Opcode::Fadd,
            0b000011 => Opcode::Fsub,
            0b000100 => Opcode::Fmul,
            0b000110 => Opcode::Fdiv,
            0b000001 => Opcode::Fsqrt,
            0b000101 => Opcode::Fabs,
            0b000111 => Opcode::Fneg,
            0b001000 => Opcode::Fmax,
            0b001001 => Opcode::Fmin,
            0b001010 => Opcode::Fmaxnm,
            0b001011 => Opcode::Fminnm,
            // FCVT: op=0b000101, but varies by ptype — handled separately
            _ => Opcode::Fcvt,
        },
    };
}

// ── Bit-mask decode helper (N:immr:imms → mask) ───────────────────────────────

fn decode_bit_mask(n: bool, imms: u32, immr: u32, sf: bool) -> Option<u64> {
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

//! Branches and system instructions (op0 = 101x).

use super::{bit, bits, sext};
use crate::aarch64::insn::{Instruction, Opcode};

pub(super) fn decode_branch_sys(raw: u32, i: &mut Instruction) {
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

    // BR/BLR/RET and PAC-authenticated variants: bits[31:25] == 0b1101011
    if bits(raw, 31, 25) == 0b110101_1 {
        let opc = bits(raw, 24, 21);
        let m = bit(raw, 10);
        let a = bit(raw, 11);
        i.rn = bits(raw, 9, 5);
        i.rm = bits(raw, 4, 0);
        i.opcode = match opc {
            0b0000 if m == 0 => Opcode::Br,
            0b0001 if m == 0 => Opcode::Blr,
            0b0010 if m == 0 => Opcode::Ret,
            // ERET/ERETAA/ERETAB
            0b0100 if m == 0 => Opcode::Eret,
            0b0100 if m == 1 && a == 0 => Opcode::EretAut, // ERETAA
            0b0100 if m == 1 && a == 1 => Opcode::EretAut, // ERETAB
            // BRAAZ/BRABZ (opc=0b1000, M=0, Rm=11111)
            0b1000 if m == 0 && a == 0 => Opcode::BrAutZ,  // BRAAZ
            0b1000 if m == 0 && a == 1 => Opcode::BrAutZ,  // BRABZ
            // BLRAAZ/BLRABZ (opc=0b1001, M=0, Rm=11111)
            0b1001 if m == 0 && a == 0 => Opcode::BlrAutZ, // BLRAAZ
            0b1001 if m == 0 && a == 1 => Opcode::BlrAutZ, // BLRABZ
            // RETAA/RETAB (opc=0b0010, M=1)
            0b0010 if m == 1 && a == 0 => Opcode::RetAut,  // RETAA
            0b0010 if m == 1 && a == 1 => Opcode::RetAut,  // RETAB
            // BRAA/BRAB (opc=0b1000, M=1)
            0b1000 if m == 1 && a == 0 => Opcode::BrAut,   // BRAA
            0b1000 if m == 1 && a == 1 => Opcode::BrAut,   // BRAB
            // BLRAA/BLRAB (opc=0b1001, M=1)
            0b1001 if m == 1 && a == 0 => Opcode::BlrAut,  // BLRAA
            0b1001 if m == 1 && a == 1 => Opcode::BlrAut,  // BLRAB
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

    // B / BL: bits[30:26] = 00101
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
    // BTI (v8.5): HINT with CRm=4 and op2 in {2,4,6}
    if bits(raw, 31, 12) == 0b1101_0101_0000_0011_0010
        && bits(raw, 11, 8) == 0b0100
        && bits(raw, 4, 0) == 0b11111
        && (bits(raw, 7, 5) == 2 || bits(raw, 7, 5) == 4 || bits(raw, 7, 5) == 6)
    {
        i.opcode = Opcode::Bti;
        return;
    }

    // ── Pointer Authentication HINT-space instructions (ARMv8.3-PAuth) ──────
    // These are all in the HINT encoding: 1101_0101_0000_0011_0010_xxxx_xxx_11111
    // All PAC hint instructions: identity implementation (NOP).
    match raw {
        0xD503_211F => { i.opcode = Opcode::PacHint; return; } // PACIA1716
        0xD503_215F => { i.opcode = Opcode::PacHint; return; } // PACIB1716
        0xD503_219F => { i.opcode = Opcode::PacHint; return; } // AUTIA1716
        0xD503_21DF => { i.opcode = Opcode::PacHint; return; } // AUTIB1716
        0xD503_231F => { i.opcode = Opcode::PacHint; return; } // PACIAZ
        0xD503_233F => { i.opcode = Opcode::PacHint; return; } // PACIASP
        0xD503_235F => { i.opcode = Opcode::PacHint; return; } // PACIBZ
        0xD503_237F => { i.opcode = Opcode::PacHint; return; } // PACIBSP
        0xD503_239F => { i.opcode = Opcode::PacHint; return; } // AUTIAZ
        0xD503_23BF => { i.opcode = Opcode::PacHint; return; } // AUTIASP
        0xD503_23DF => { i.opcode = Opcode::PacHint; return; } // AUTIBZ
        0xD503_23FF => { i.opcode = Opcode::PacHint; return; } // AUTIBSP
        // XPACLRI: strip PAC from LR
        0xD503_20FF => { i.opcode = Opcode::PacHint; return; } // XPACLRI
        _ => {}
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
    if op0 == 0b00 && l == 0 && crn == 0b0100 {
        i.opcode = Opcode::MsrImm;
        i.imm = ((op1 as i64) << 16) | ((crm as i64) << 8) | ((op2 as i64) << 5);
        return;
    }

    // MRS / MSR
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

//! AArch64 execute — dp group.
#![allow(unused_imports, unused_variables)]
use crate::aarch64::arch_state::Aarch64ArchState;
use crate::aarch64::insn::{Instruction, Opcode};
use helm_core::{AccessType, HartException, MemFault, MemInterface};
#[allow(unused_imports)]
use helm_diag::{sim_stub, sim_warn};
use super::helpers::*;
use crate::aarch64::exception;

#[allow(clippy::too_many_lines)]
pub(super) fn exec_dp(
    insn: &Instruction,
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
) -> Result<bool, HartException> {
    use Opcode::*;
    let mut pc_written = false;
    match insn.opcode {
        // ── ADR / ADRP ──────────────────────────────────────────────────────
        Adr => {
            let val = a.pc.wrapping_add(insn.imm as u64);
            a.write_x(insn.rd, val);
        }
        Adrp => {
            let base = a.pc & !0xFFF;
            let val = base.wrapping_add((insn.imm as u64) << 12);
            a.write_x(insn.rd, val);
        }


        // ── ADD / SUB immediate ─────────────────────────────────────────────
        AddImm => {
            let src = if insn.sf {
                a.read_xsp(insn.rn)
            } else {
                a.read_xsp(insn.rn) & 0xFFFF_FFFF
            };
            let res = src.wrapping_add(insn.imm as u64);
            if insn.sf {
                a.write_xsp(insn.rd, res);
            } else {
                a.write_xsp(insn.rd, res & 0xFFFF_FFFF);
            }
        }
        SubImm => {
            let src = if insn.sf {
                a.read_xsp(insn.rn)
            } else {
                a.read_xsp(insn.rn) & 0xFFFF_FFFF
            };
            let res = src.wrapping_sub(insn.imm as u64);
            if insn.sf {
                a.write_xsp(insn.rd, res);
            } else {
                a.write_xsp(insn.rd, res & 0xFFFF_FFFF);
            }
        }
        AddsImm => {
            let rn = a.read_x(insn.rn);
            let imm = insn.imm as u64;
            let (res, c, v) = awc(rn, imm, false, insn.sf);
            set_flags(a, res, c, v, insn.sf);
            a.write_x(insn.rd, res);
        }
        SubsImm => {
            let rn = a.read_x(insn.rn);
            let imm = insn.imm as u64;
            let (res, c, v) = awc(rn, !imm, true, insn.sf);
            set_flags(a, res, c, v, insn.sf);
            a.write_x(insn.rd, res);
        }


        // ── Logical immediate ───────────────────────────────────────────────
        AndImm => {
            binop_imm(a, insn, |x, y| x & y);
        }
        OrrImm => {
            binop_imm(a, insn, |x, y| x | y);
        }
        EorImm => {
            binop_imm(a, insn, |x, y| x ^ y);
        }
        AndsImm => {
            let res = binop_imm_ret(a, insn, |x, y| x & y);
            set_flags(a, res, false, false, insn.sf);
        }


        // ── MOV wide ────────────────────────────────────────────────────────
        Movz => {
            if insn.sf {
                a.write_x(insn.rd, insn.imm as u64);
            } else {
                a.write_w(insn.rd, insn.imm as u32);
            }
        }
        Movn => {
            if insn.sf {
                a.write_x(insn.rd, insn.imm as u64);
            } else {
                a.write_w(insn.rd, insn.imm as u32);
            }
        }
        Movk => {
            let shift = insn.imm2 * 16;
            let mask = !(0xFFFFu64 << shift);
            let old = a.read_x(insn.rd);
            let val = (old & mask) | ((insn.imm as u64 & 0xFFFF) << shift);
            if insn.sf {
                a.write_x(insn.rd, val);
            } else {
                a.write_w(insn.rd, val as u32);
            }
        }


        // ── Bitfield ────────────────────────────────────────────────────────
        Sbfm => {
            exec_sbfm(a, insn);
        }
        Ubfm => {
            exec_ubfm(a, insn);
        }
        Bfm => {
            exec_bfm(a, insn);
        }
        Extr => {
            let immr = insn.imm as u32;
            let rs1 = a.read_x(insn.rn);
            let rs2 = a.read_x(insn.rm);
            let val = if insn.sf {
                if immr == 0 {
                    rs1
                } else {
                    (rs1 << (64 - immr)) | (rs2 >> immr)
                }
            } else {
                let r = ((rs1 as u32) << (32 - immr)) | ((rs2 as u32) >> immr);
                r as u64
            };
            a.write_x(insn.rd, val);
        }


        // ── ADD/SUB shifted register ───────────────────────────────────────
        // Shifted register: Rn=31 means XZR (not SP). Only the immediate
        // and extended-register variants use SP when Rn=31.
        AddReg | SubReg | AddsReg | SubsReg => {
            let src = a.read_x(insn.rn);
            let rm = apply_shift(a.read_x(insn.rm), insn.shift_type, insn.shift_amt, insn.sf);
            exec_addsub_reg(a, insn, src, rm)?;
        }


        // ── Logical register ────────────────────────────────────────────────
        AndReg => {
            let v = log_reg(a, insn, |x, y| x & y, false);
            a.write_x(insn.rd, v);
        }
        BicReg => {
            let v = log_reg(a, insn, |x, y| x & !y, false);
            a.write_x(insn.rd, v);
        }
        OrrReg => {
            let v = log_reg(a, insn, |x, y| x | y, false);
            a.write_x(insn.rd, v);
        }
        OrnReg => {
            let v = log_reg(a, insn, |x, y| x | !y, false);
            a.write_x(insn.rd, v);
        }
        EorReg => {
            let v = log_reg(a, insn, |x, y| x ^ y, false);
            a.write_x(insn.rd, v);
        }
        EonReg => {
            let v = log_reg(a, insn, |x, y| x ^ !y, false);
            a.write_x(insn.rd, v);
        }
        AndsReg => {
            let v = log_reg(a, insn, |x, y| x & y, true);
            a.write_x(insn.rd, v);
        }
        BicsReg => {
            let v = log_reg(a, insn, |x, y| x & !y, true);
            a.write_x(insn.rd, v);
        }


        // ── Shift ───────────────────────────────────────────────────────────
        Lsl | Lsr | Asr | Ror => {
            let src = a.read_x(insn.rn);
            let sh = (a.read_x(insn.rm) % if insn.sf { 64 } else { 32 }) as u32;
            let res = match insn.opcode {
                Lsl => {
                    if insn.sf {
                        src << sh
                    } else {
                        ((src as u32) << sh) as u64
                    }
                }
                Lsr => {
                    if insn.sf {
                        src >> sh
                    } else {
                        ((src as u32) >> sh) as u64
                    }
                }
                Asr => {
                    if insn.sf {
                        ((src as i64) >> sh) as u64
                    } else {
                        // 32-bit: sign-extend from 32-bit then zero-extend to 64
                    (((src as u32 as i32) >> sh) as u32) as u64
                    }
                }
                Ror => {
                    if insn.sf {
                        src.rotate_right(sh)
                    } else {
                        (src as u32).rotate_right(sh) as u64
                    }
                }
                _ => unreachable!(),
            };
            a.write_x(insn.rd, res);
        }


        // ── 1-source ────────────────────────────────────────────────────────
        Clz => {
            let src = a.read_x(insn.rn);
            let v = if insn.sf {
                src.leading_zeros() as u64
            } else {
                (src as u32).leading_zeros() as u64
            };
            a.write_x(insn.rd, v);
        }
        Cls => {
            let src = a.read_x(insn.rn);
            let v = if insn.sf {
                (src ^ (src << 1)).leading_zeros() as u64
            } else {
                ((src as u32) ^ ((src as u32) << 1)).leading_zeros() as u64 - 1
            };
            a.write_x(insn.rd, v);
        }
        Rev => {
            let src = a.read_x(insn.rn);
            let v = if insn.sf {
                src.swap_bytes()
            } else {
                (src as u32).swap_bytes() as u64
            };
            a.write_x(insn.rd, v);
        }
        Rev16 => {
            let src = a.read_x(insn.rn);
            let v = ((src & 0xFF00_FF00_FF00_FF00) >> 8) | ((src & 0x00FF_00FF_00FF_00FF) << 8);
            a.write_x(insn.rd, v);
        }
        Rev32 => {
            let src = a.read_x(insn.rn);
            let hi = (src >> 32) as u32;
            let lo = src as u32;
            a.write_x(
                insn.rd,
                ((lo.swap_bytes() as u64) << 32) | hi.swap_bytes() as u64,
            );
        }
        Rbit => {
            let src = a.read_x(insn.rn);
            let v = if insn.sf {
                src.reverse_bits()
            } else {
                (src as u32).reverse_bits() as u64
            };
            a.write_x(insn.rd, v);
        }


        // ── ADC / SBC ────────────────────────────────────────────────────────
        Adc | Adcs => {
            let rn = a.read_x(insn.rn);
            let rm = a.read_x(insn.rm);
            let (res, c, v) = awc(rn, rm, a.flag_c(), insn.sf);
            let res = if insn.sf { res } else { (res as u32) as u64 };
            a.write_x(insn.rd, res);
            if insn.opcode == Adcs {
                set_flags(a, res, c, v, insn.sf);
            }
        }
        Sbc | Sbcs => {
            let rn = a.read_x(insn.rn);
            let rm = a.read_x(insn.rm);
            let (res, c, v) = awc(rn, !rm, a.flag_c(), insn.sf);
            let res = if insn.sf { res } else { (res as u32) as u64 };
            a.write_x(insn.rd, res);
            if insn.opcode == Sbcs {
                set_flags(a, res, c, v, insn.sf);
            }
        }


        // ── Conditional select ───────────────────────────────────────────────
        Csel => {
            let val = if a.eval_cond(insn.cond) {
                a.read_x(insn.rn)
            } else {
                a.read_x(insn.rm)
            };
            // Zero-extend 32-bit result
            a.write_x(insn.rd, if insn.sf { val } else { val & 0xFFFF_FFFF });
        }
        Csinc => {
            let val = if a.eval_cond(insn.cond) {
                a.read_x(insn.rn)
            } else {
                a.read_x(insn.rm).wrapping_add(1)
            };
            a.write_x(insn.rd, if insn.sf { val } else { val & 0xFFFF_FFFF });
        }
        Csinv => {
            let val = if a.eval_cond(insn.cond) {
                a.read_x(insn.rn)
            } else {
                !a.read_x(insn.rm)
            };
            a.write_x(insn.rd, if insn.sf { val } else { val & 0xFFFF_FFFF });
        }
        Csneg => {
            let val = if a.eval_cond(insn.cond) {
                a.read_x(insn.rn)
            } else {
                a.read_x(insn.rm).wrapping_neg()
            };
            a.write_x(insn.rd, if insn.sf { val } else { val & 0xFFFF_FFFF });
        }


        // ── Conditional compare ──────────────────────────────────────────────
        Ccmp | Ccmn => {
            if a.eval_cond(insn.cond) {
                let rn_val = a.read_x(insn.rn);
                // Detect immediate vs register via raw instruction bit 11
                let op2 = if (insn.raw >> 11) & 1 == 1 {
                    ((insn.raw >> 16) & 0x1F) as u64
                } else {
                    a.read_x(insn.rm)
                };
                let (res, c, v) = if insn.opcode == Ccmp {
                    awc(rn_val, !op2, true, insn.sf) // CMP = a + NOT(b) + 1
                } else {
                    awc(rn_val, op2, false, insn.sf) // CMN = a + b
                };
                let res = if insn.sf { res } else { (res as u32) as u64 };
                set_flags(a, res, c, v, insn.sf);
            } else {
                a.nzcv = insn.nzcv_imm << 28;
            }
        }


        // ── FP ───────────────────────────────────────────────────────────────
        FmovReg => {
            // FMOV between FP registers
            let src = a.v[insn.rn as usize];
            a.v[insn.rd as usize] = src;
        }
        FmovGpr => {
            // FMOV between GPR and FP register.
            // Direction: opcode bits[18:16] = 110 → FP to GPR, 111 → GPR to FP.
            let to_gpr = (insn.raw >> 16) & 7 == 0b110;
            if insn.sf {
                if to_gpr {
                    a.write_x(insn.rd, a.v[insn.rn as usize] as u64);
                } else {
                    a.v[insn.rd as usize] = a.read_x(insn.rn) as u128;
                }
            } else {
                if to_gpr {
                    a.write_w(insn.rd, a.v[insn.rn as usize] as u32);
                } else {
                    a.v[insn.rd as usize] = a.read_w(insn.rn) as u128;
                }
            }
        }
        FmovImm => {
            // 8-bit immediate to FP register
            let imm8 = insn.imm as u32;
            let f32_val = fp_imm8_to_f32(imm8);
            if insn.ftype == 1 {
                a.v[insn.rd as usize] = (f64::from(f32_val)).to_bits() as u128;
            } else {
                a.v[insn.rd as usize] = f32_val.to_bits() as u128;
            }
        }
        Fadd | Fsub | Fmul | Fdiv | Fmax | Fmin | Fmaxnm | Fminnm => {
            exec_fp_binary(a, insn);
        }
        Fsqrt | Fabs | Fneg => {
            exec_fp_unary(a, insn);
        }
        Fcmp | Fcmpe => {
            exec_fcmp(a, insn);
        }
        Fcvt => {
            exec_fcvt(a, insn);
        }
        FcvtzsGpr | FcvtzuGpr | ScvtfGpr | UcvtfGpr => {
            exec_fp_gpr_convert(a, insn);
        }
        Fmadd | Fmsub | Fnmadd | Fnmsub => {
            exec_fp_fused(a, insn);
        }
        Fsel => {
            let val = if a.eval_cond(insn.cond) {
                a.v[insn.rn as usize]
            } else {
                a.v[insn.rm as usize]
            };
            a.v[insn.rd as usize] = val;
        }


        // ── Extended register add/sub ────────────────────────────────────
        AddExt | SubExt | AddsExt | SubsExt => {
            let src = a.read_xsp(insn.rn);
            let ext_val = apply_extend(a.read_x(insn.rm), insn.extend_type, insn.extend_amt);
            let is_sub = matches!(insn.opcode, SubExt | SubsExt);
            let setf = matches!(insn.opcode, AddsExt | SubsExt);
            let (res, c, v) = if is_sub {
                awc(src, !ext_val, true, insn.sf)
            } else {
                awc(src, ext_val, false, insn.sf)
            };
            let res = if insn.sf { res } else { (res as u32) as u64 };
            if setf {
                set_flags(a, res, c, v, insn.sf);
                a.write_x(insn.rd, res);
            } else {
                a.write_xsp(insn.rd, res);
            }
        }


        // ── FlagM (v8.4) ────────────────────────────────────────────────────
        Setf8 => {
            let v = a.read_x(insn.rn) as u8;
            let n = (v >> 7) as u32;
            let z = (v == 0) as u32;
            // C and V are UNPREDICTABLE per spec — we leave them unchanged
            a.nzcv = (n << 31) | (z << 30) | (a.nzcv & 0x3000_0000);
        }
        Setf16 => {
            let v = a.read_x(insn.rn) as u16;
            let n = (v >> 15) as u32;
            let z = (v == 0) as u32;
            a.nzcv = (n << 31) | (z << 30) | (a.nzcv & 0x3000_0000);
        }
        Cfinv => {
            // Invert the C (carry) flag in NZCV
            a.nzcv ^= 0x2000_0000;
        }
        Rmif => {
            // Rotate Xn right by imm6 bits, then insert selected flag bits
            let rotation = (insn.imm as u32) & 63;
            let mask = insn.imm2 as u32; // 4-bit: {N,Z,C,V}
            let val = a.read_x(insn.rn).rotate_right(rotation);
            // bits [63:60] of rotated val → NZCV[3:0] = {N,Z,C,V}
            let new_nzcv = ((val >> 60) as u32) << 28;
            // Apply only bits where mask bit is set
            let keep = a.nzcv & !(mask << 28);
            let insert = new_nzcv & (mask << 28);
            a.nzcv = keep | insert;
        }

        _ => unreachable!("wrong dispatch to dp"),
    }
    Ok(pc_written)
}

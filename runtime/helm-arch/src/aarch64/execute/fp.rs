//! AArch64 execute — fp group.
#![allow(unused_imports, unused_variables)]
use super::helpers::*;
use crate::aarch64::arch_state::Aarch64ArchState;
use crate::aarch64::exception;
use crate::aarch64::insn::{Instruction, Opcode};
use helm_core::{AccessType, HartException, MemFault, MemInterface};
#[allow(unused_imports)]
use helm_diag::{sim_stub, sim_warn};

#[allow(clippy::too_many_lines)]
pub fn exec_fp(
    insn: &Instruction,
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
) -> Result<bool, HartException> {
    use Opcode::*;
    let pc_written = false;
    match insn.opcode {
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

        // ── CRC32 / CRC32C ──────────────────────────────────────────────
        Crc32 | Crc32c => {
            let crc_c = matches!(insn.opcode, Crc32c);
            let sz = insn.size; // 0=B, 1=H, 2=W, 3=X
            let mut crc = a.read_w(insn.rn);
            let data = if sz == 3 {
                a.read_x(insn.rm)
            } else {
                a.read_w(insn.rm) as u64
            };
            let nbytes = 1usize << sz;
            for i in 0..nbytes {
                let byte = ((data >> (i * 8)) & 0xFF) as u8;
                crc = if crc_c {
                    crc32c_byte(crc, byte)
                } else {
                    crc32_byte(crc, byte)
                };
            }
            a.write_w(insn.rd, crc);
        }

        // ── FP conditional compare ───────────────────────────────────────
        Fccmp | Fccmpe => {
            if a.eval_cond(insn.cond) {
                // Use same logic as Fcmp
                if insn.ftype == 1 {
                    let rn = f64::from_bits(a.v[insn.rn as usize] as u64);
                    let rm = f64::from_bits(a.v[insn.rm as usize] as u64);
                    if rn.is_nan() || rm.is_nan() {
                        a.set_nzcv(false, false, true, true);
                    } else if rn == rm {
                        a.set_nzcv(false, true, true, false);
                    } else if rn < rm {
                        a.set_nzcv(true, false, false, false);
                    } else {
                        a.set_nzcv(false, false, true, false);
                    }
                }
            } else {
                let nz = insn.nzcv_imm;
                a.set_nzcv(nz & 8 != 0, nz & 4 != 0, nz & 2 != 0, nz & 1 != 0);
            }
        }
        Fnmul => {
            if insn.ftype == 1 {
                let rn = f64::from_bits(a.v[insn.rn as usize] as u64);
                let rm = f64::from_bits(a.v[insn.rm as usize] as u64);
                a.v[insn.rd as usize] = (-(rn * rm)).to_bits() as u128;
            } else {
                let rn = f32::from_bits(a.v[insn.rn as usize] as u32);
                let rm = f32::from_bits(a.v[insn.rm as usize] as u32);
                a.v[insn.rd as usize] = (-(rn * rm)).to_bits() as u128;
            }
        }

        // ── FJCVTZS (v8.3 JSCVT): float64 → JS ToInt32 ──────────────────
        Fjcvtzs => {
            let d = f64::from_bits(a.v[insn.rn as usize] as u64);
            let result: i32 = if d.is_nan() || d.is_infinite() || d == 0.0 {
                0
            } else {
                // JavaScript ToInt32: truncate toward zero, mod 2^32, then sign-extend
                let trunc = d as i64;
                trunc as i32
            };
            a.write_x(insn.rd, result as i64 as u64);
            // FJCVTZS sets Z flag only (Z=1 if result==0 and conversion was exact)
            // Simplified: set Z if result == 0
            let z = result == 0;
            a.nzcv = if z { 0x4000_0000 } else { 0 };
        }

        // ── FP rounding-mode converts ────────────────────────────────────
        FcvtnsGpr | FcvtnuGpr | FcvtmsGpr | FcvtmuGpr | FcvtpsGpr | FcvtpuGpr | FcvtasGpr
        | FcvtauGpr => {
            // Helper macro for correct rounding per opcode
            let signed = matches!(insn.opcode, FcvtnsGpr | FcvtmsGpr | FcvtpsGpr | FcvtasGpr);
            if insn.ftype == 1 {
                let rn = f64::from_bits(a.v[insn.rn as usize] as u64);
                let rounded = match insn.opcode {
                    FcvtnsGpr | FcvtnuGpr => rn.round_ties_even(), // round to nearest, ties to even
                    FcvtmsGpr | FcvtmuGpr => rn.floor(),           // round toward -inf
                    FcvtpsGpr | FcvtpuGpr => rn.ceil(),            // round toward +inf
                    FcvtasGpr | FcvtauGpr => rn.round(),           // round ties-away from zero
                    _ => rn.trunc(),
                };
                if signed {
                    if insn.sf {
                        a.write_x(insn.rd, rounded as i64 as u64);
                    } else {
                        a.write_x(insn.rd, rounded as i32 as i64 as u64);
                    }
                } else {
                    if insn.sf {
                        a.write_x(insn.rd, rounded as u64);
                    } else {
                        a.write_x(insn.rd, rounded as u32 as u64);
                    }
                }
            } else {
                let rn = f32::from_bits(a.v[insn.rn as usize] as u32);
                let rounded = match insn.opcode {
                    FcvtnsGpr | FcvtnuGpr => rn.round_ties_even(),
                    FcvtmsGpr | FcvtmuGpr => rn.floor(),
                    FcvtpsGpr | FcvtpuGpr => rn.ceil(),
                    FcvtasGpr | FcvtauGpr => rn.round(),
                    _ => rn.trunc(),
                };
                if signed {
                    if insn.sf {
                        a.write_x(insn.rd, rounded as i64 as u64);
                    } else {
                        a.write_x(insn.rd, rounded as i32 as i64 as u64);
                    }
                } else {
                    if insn.sf {
                        a.write_x(insn.rd, rounded as u64);
                    } else {
                        a.write_x(insn.rd, rounded as u32 as u64);
                    }
                }
            }
        }

        _ => return Err(illegal_instruction(insn)),
    }
    Ok(pc_written)
}

/// CRC32 (ISO 3309) one byte: reflected polynomial 0xEDB88320
fn crc32_byte(crc: u32, byte: u8) -> u32 {
    let mut c = crc ^ (byte as u32);
    for _ in 0..8 {
        c = if c & 1 != 0 {
            (c >> 1) ^ 0xEDB8_8320
        } else {
            c >> 1
        };
    }
    c
}

/// CRC32C (Castagnoli) one byte: reflected polynomial 0x82F63B78
fn crc32c_byte(crc: u32, byte: u8) -> u32 {
    let mut c = crc ^ (byte as u32);
    for _ in 0..8 {
        c = if c & 1 != 0 {
            (c >> 1) ^ 0x82F6_3B78
        } else {
            c >> 1
        };
    }
    c
}

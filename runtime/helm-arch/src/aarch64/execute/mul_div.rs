//! AArch64 execute — mul_div group.
#![allow(unused_imports, unused_variables)]
use crate::aarch64::arch_state::Aarch64ArchState;
use crate::aarch64::insn::{Instruction, Opcode};
use helm_core::{AccessType, HartException, MemFault, MemInterface};
#[allow(unused_imports)]
use helm_diag::{sim_stub, sim_warn};
use super::helpers::*;
use crate::aarch64::exception;

#[allow(clippy::too_many_lines)]
pub(super) fn exec_mul_div(
    insn: &Instruction,
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
) -> Result<bool, HartException> {
    use Opcode::*;
    let mut pc_written = false;
    match insn.opcode {
        // ── Multiply ────────────────────────────────────────────────────────
        Mul | Madd => {
            let rn = a.read_x(insn.rn);
            let rm = a.read_x(insn.rm);
            let ra = if insn.opcode == Madd {
                a.read_x(insn.ra)
            } else {
                0
            };
            let res = if insn.sf {
                rn.wrapping_mul(rm).wrapping_add(ra)
            } else {
                ((rn as u32).wrapping_mul(rm as u32) as u64).wrapping_add(ra) & 0xFFFF_FFFF
            };
            a.write_x(insn.rd, res);
        }
        Msub | Mneg => {
            let rn = a.read_x(insn.rn);
            let rm = a.read_x(insn.rm);
            let ra = if insn.opcode == Msub {
                a.read_x(insn.ra)
            } else {
                0
            };
            let res = ra.wrapping_sub(rn.wrapping_mul(rm));
            a.write_x(insn.rd, res);
        }
        Smulh => {
            let rn = a.read_x(insn.rn) as i64 as i128;
            let rm = a.read_x(insn.rm) as i64 as i128;
            a.write_x(insn.rd, ((rn * rm) >> 64) as u64);
        }
        Umulh => {
            let rn = a.read_x(insn.rn) as u128;
            let rm = a.read_x(insn.rm) as u128;
            a.write_x(insn.rd, ((rn * rm) >> 64) as u64);
        }


        // ── Divide ──────────────────────────────────────────────────────────
        Udiv => {
            let (rn, rm) = if insn.sf {
                (a.read_x(insn.rn), a.read_x(insn.rm))
            } else {
                // 32-bit: use only low 32 bits of each operand
                (a.read_x(insn.rn) as u32 as u64, a.read_x(insn.rm) as u32 as u64)
            };
            let quot = if rm == 0 { 0 } else { rn / rm };
            a.write_x(insn.rd, if insn.sf { quot } else { quot & 0xFFFF_FFFF });
        }
        Sdiv => {
            let (rn, rm) = if insn.sf {
                (a.read_x(insn.rn) as i64, a.read_x(insn.rm) as i64)
            } else {
                // 32-bit: sign-extend inputs from 32 bits
                (a.read_x(insn.rn) as u32 as i32 as i64, a.read_x(insn.rm) as u32 as i32 as i64)
            };
            let quot = if rm == 0 { 0i64 } else { rn.wrapping_div(rm) };
            // 32-bit result: zero-extend the low 32 bits
            a.write_x(insn.rd, if insn.sf { quot as u64 } else { quot as i32 as u32 as u64 });
        }


        // ── Widening multiply ────────────────────────────────────────────
        Smaddl => {
            let rn = a.read_x(insn.rn) as i32 as i64;
            let rm = a.read_x(insn.rm) as i32 as i64;
            let ra = a.read_x(insn.ra) as i64;
            a.write_x(insn.rd, rn.wrapping_mul(rm).wrapping_add(ra) as u64);
        }
        Smsubl => {
            let rn = a.read_x(insn.rn) as i32 as i64;
            let rm = a.read_x(insn.rm) as i32 as i64;
            let ra = a.read_x(insn.ra) as i64;
            a.write_x(insn.rd, ra.wrapping_sub(rn.wrapping_mul(rm)) as u64);
        }
        Umaddl => {
            let rn = a.read_x(insn.rn) as u32 as u64;
            let rm = a.read_x(insn.rm) as u32 as u64;
            let ra = a.read_x(insn.ra);
            a.write_x(insn.rd, rn.wrapping_mul(rm).wrapping_add(ra));
        }
        Umsubl => {
            let rn = a.read_x(insn.rn) as u32 as u64;
            let rm = a.read_x(insn.rm) as u32 as u64;
            let ra = a.read_x(insn.ra);
            a.write_x(insn.rd, ra.wrapping_sub(rn.wrapping_mul(rm)));
        }


        _ => unreachable!("wrong dispatch to mul_div"),
    }
    Ok(pc_written)
}

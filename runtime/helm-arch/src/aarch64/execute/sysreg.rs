//! AArch64 execute — sysreg group.
#![allow(unused_imports, unused_variables)]
use crate::aarch64::arch_state::Aarch64ArchState;
use crate::aarch64::insn::{Instruction, Opcode};
use helm_core::{AccessType, HartException, MemFault, MemInterface};
#[allow(unused_imports)]
use helm_diag::{sim_stub, sim_warn};
use super::helpers::*;
use crate::aarch64::exception;

fn sysreg_trap_iss(raw: u32) -> u32 {
    let l = (raw >> 21) & 1;
    let op0 = (raw >> 19) & 0x3;
    let op1 = (raw >> 16) & 0x7;
    let crn = (raw >> 12) & 0xF;
    let crm = (raw >> 8) & 0xF;
    let op2 = (raw >> 5) & 0x7;
    let rt = raw & 0x1F;
    (l << 24) | (op0 << 20) | (op2 << 17) | (op1 << 14) | (crn << 10) | (rt << 5) | (crm << 1)
}

#[allow(clippy::too_many_lines)]
pub(super) fn exec_sysreg(
    insn: &Instruction,
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
) -> Result<bool, HartException> {
    use Opcode::*;
    let pc_written = false;
    match insn.opcode {
        // ── MRS / MSR ────────────────────────────────────────────────────────
        Mrs => {
            let encoded = insn.imm as u32;
            if should_tvm_trap(a, encoded) {
                let syndrome = exception::EC_SYSREG_TRAP | sysreg_trap_iss(insn.raw);
                exception::exception_entry(a, 2, syndrome, 0);
                return Ok(true);
            }
            let val = read_sysreg(a, redirect_sysreg(a, encoded));
            a.write_x(insn.rd, val);
        }
        Msr => {
            // Immediate MSR (PSTATE fields): check if Rn encodes a field
            let val = a.read_x(insn.rd); // Rt is actually in rd field for MSR
            let encoded = insn.imm as u32;
            if should_tvm_trap(a, encoded) {
                let syndrome = exception::EC_SYSREG_TRAP | sysreg_trap_iss(insn.raw);
                exception::exception_entry(a, 2, syndrome, 0);
                return Ok(true);
            }
            write_sysreg(a, redirect_sysreg(a, encoded), val);
        }
        Sys => {
            // Decode from the original raw instruction:
            //   op0 = bits[20:19], op1 = bits[18:16], CRn = bits[15:12],
            //   CRm = bits[11:8], op2 = bits[7:5], Rt = bits[4:0].
            let raw = insn.raw;
            let op0 = (raw >> 19) & 0x3;
            let op1 = (raw >> 16) & 0x7;
            let crn = (raw >> 12) & 0xF;
            let crm = (raw >> 8) & 0xF;
            let op2 = (raw >> 5) & 0x7;
            let rt = raw & 0x1F;

            // TLBI: flush the software TLB on the next step-loop boundary.
            if op0 == 0b01 && crn == 0b1000 {
                a.tlb_flush_pending = true;
                return Ok(pc_written);
            }

            // AT: approximate the architectural PAR_EL1 side effect.
            // Full VA->PA translation through physical table walks needs a
            // lower-level memory interface than `MemInterface` exposes here.
            // Preserve useful behavior for common early-boot cases:
            //   MMU off -> identity translation succeeds
            //   MMU on  -> report translation failure in PAR_EL1
            if op0 == 0b01 && crn == 0b0111 && crm == 0b1000 {
                let va = a.read_x(rt);
                if a.mmu_enabled() {
                    a.par_el1 = 1; // F=1 -> translation fault / unsupported walk
                } else {
                    a.par_el1 = va & 0x0000_FFFF_FFFF_F000;
                }
                return Ok(pc_written);
            }

            // IC IVAU / IALLU and related maintenance ops are no-ops in the
            // current functional executor.  FS-mode decodes each instruction
            // afresh, so there is no instruction cache state to invalidate.
        }


        _ => unreachable!("wrong dispatch to sysreg"),
    }
    Ok(pc_written)
}

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

// Encoding: op0<<14 | op1<<11 | CRn<<7 | CRm<<3 | op2

// ZCR_ELx — SVE vector-length control (requires SVE)
const ZCR_EL1: u32 = 0b11_000_0001_0010_000; // (3,0,1,2,0)
const ZCR_EL2: u32 = 0b11_100_0001_0010_000; // (3,4,1,2,0)
const ZCR_EL3: u32 = 0b11_110_0001_0010_000; // (3,6,1,2,0)

// SMCR_ELx — SME vector-length control (requires SME)
const SMCR_EL1: u32 = 0b11_000_0001_0010_110; // (3,0,1,2,6)
const SMCR_EL2: u32 = 0b11_100_0001_0010_110; // (3,4,1,2,6)
const SMCR_EL3: u32 = 0b11_110_0001_0010_110; // (3,6,1,2,6)

/// Check whether accessing this sysreg should trap because its gating
/// feature is not implemented. On real hardware, the MRS/MSR would take
/// an Undefined Instruction exception.
fn should_trap_missing_feature(a: &Aarch64ArchState, encoded: u32) -> bool {
    match encoded {
        // SVE registers: require ID_AA64PFR0_EL1.SVE (bits[35:32]) != 0
        ZCR_EL1 | ZCR_EL2 | ZCR_EL3 => (a.id_aa64pfr0_el1 >> 32) & 0xF == 0,
        // SME registers: require ID_AA64PFR1_EL1.SME (bits[27:24]) != 0
        SMCR_EL1 | SMCR_EL2 | SMCR_EL3 => (a.id_aa64pfr1_el1 >> 24) & 0xF == 0,
        _ => false,
    }
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
            // Feature-gated registers: trap as undefined when feature absent
            if should_trap_missing_feature(a, encoded) {
                let target_el = a.current_el.max(1);
                // EC=0 (Unknown reason), IL=1 (32-bit instruction)
                let syndrome = exception::EC_UNKNOWN | (1 << 25);
                exception::exception_entry(a, target_el, syndrome, 0);
                return Ok(true);
            }
            let val = read_sysreg(a, redirect_sysreg(a, encoded));
            a.write_x(insn.rd, val);
        }
        Msr => {
            let val = a.read_x(insn.rd);
            let encoded = insn.imm as u32;
            if should_tvm_trap(a, encoded) {
                let syndrome = exception::EC_SYSREG_TRAP | sysreg_trap_iss(insn.raw);
                exception::exception_entry(a, 2, syndrome, 0);
                return Ok(true);
            }
            // Feature-gated registers: trap as undefined when feature absent
            if should_trap_missing_feature(a, encoded) {
                let target_el = a.current_el.max(1);
                let syndrome = exception::EC_UNKNOWN | (1 << 25);
                exception::exception_entry(a, target_el, syndrome, 0);
                return Ok(true);
            }
            write_sysreg(a, redirect_sysreg(a, encoded), val);
        }
        Sys => {
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
            if op0 == 0b01 && crn == 0b0111 && crm == 0b1000 {
                let va = a.read_x(rt);
                if a.mmu_enabled() {
                    a.par_el1 = 1;
                } else {
                    a.par_el1 = va & 0x0000_FFFF_FFFF_F000;
                }
                return Ok(pc_written);
            }

            // IC IVAU / IALLU and related maintenance ops are no-ops.
        }

        _ => unreachable!("wrong dispatch to sysreg"),
    }
    Ok(pc_written)
}

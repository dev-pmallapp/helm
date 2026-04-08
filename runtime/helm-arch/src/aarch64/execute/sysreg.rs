//! AArch64 execute — sysreg group.
#![allow(unused_imports, unused_variables)]
use super::helpers::*;
use crate::aarch64::arch_state::Aarch64ArchState;
use crate::aarch64::exception;
use crate::aarch64::insn::{Instruction, Opcode};
use helm_core::{AccessType, HartException, MemFault, MemInterface};
#[allow(unused_imports)]
use helm_diag::{sim_stub, sim_warn};

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

fn tlbi_va(xt: u64) -> u64 {
    let raw = xt << 12;
    if raw & (1u64 << 55) != 0 {
        raw | 0xFF00_0000_0000_0000
    } else {
        raw
    }
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
pub fn exec_sysreg(
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

            // TLBI: invalidate TLB entries on the next step-loop boundary.
            if op0 == 0b01 && crn == 0b1000 {
                a.tlb_flush_pending = true;
                a.tlb_flush_broadcast = true;
                a.tlb_flush_asid = None;
                a.tlb_flush_va = match (op1, crm, op2) {
                    // VA-targeted TLBI forms encode the page number in Xt[55:12].
                    // Keep the top bits sign-extended so higher-half kernel VAs
                    // invalidate the right TLB slot.
                    (0, 3, 1)
                    | (0, 7, 1)
                    | (0, 3, 5)
                    | (0, 7, 5)
                    | (0, 3, 3)
                    | (0, 7, 3)
                    | (0, 3, 7)
                    | (0, 7, 7)
                    | (4, 3, 1)
                    | (4, 7, 1)
                    | (4, 3, 5)
                    | (4, 7, 5)
                    | (6, 3, 1)
                    | (6, 7, 1)
                    | (6, 3, 5)
                    | (6, 7, 5) => Some(tlbi_va(a.read_x(rt))),
                    (0, 3, 2) | (0, 7, 2) => {
                        let asid_mask = if (a.tcr_el1 >> 36) & 1 != 0 {
                            0xFFFF
                        } else {
                            0x00FF
                        };
                        a.tlb_flush_asid = Some(((a.read_x(rt) >> 48) as u16) & asid_mask);
                        None
                    }
                    _ => None,
                };
                return Ok(pc_written);
            }

            // AT S1E1R/W, S1E0R/W: handled in the FS step loop where we
            // have access to physical memory. In SE mode, fall through to
            // the identity stub below.
            // AT encoding: op0=0b00 (S1E1R/W) or op0=0b01 (S1E0R/W),
            // CRn=0b0111, CRm=0b1000.
            // Note: prior code checked op0==0b01 which never matched S1E1R;
            // PAR_EL1 stayed at 0 (success) by accident.
            if crn == 0b0111 && crm == 0b1000 && op0 <= 0b01 {
                // SE mode: identity (MMU off). FS mode intercepts in
                // try_exec_at_instruction before we get here.
                let va = a.read_x(rt);
                a.par_el1 = va & 0x0000_FFFF_FFFF_F000;
                return Ok(pc_written);
            }

            // IC IVAU / IALLU and related maintenance ops are no-ops.
        }

        _ => unreachable!("wrong dispatch to sysreg"),
    }
    Ok(pc_written)
}

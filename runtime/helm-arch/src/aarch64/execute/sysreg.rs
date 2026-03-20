//! AArch64 execute — sysreg group.
#![allow(unused_imports, unused_variables)]
use crate::aarch64::arch_state::Aarch64ArchState;
use crate::aarch64::insn::{Instruction, Opcode};
use helm_core::{AccessType, HartException, MemFault, MemInterface};
#[allow(unused_imports)]
use helm_diag::{sim_stub, sim_warn};
use super::helpers::*;
use crate::aarch64::exception;

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
            let val = read_sysreg(a, insn.imm as u32);
            a.write_x(insn.rd, val);
        }
        Msr => {
            // Immediate MSR (PSTATE fields): check if Rn encodes a field
            let val = a.read_x(insn.rd); // Rt is actually in rd field for MSR
            write_sysreg(a, insn.imm as u32, val);
        }
        Sys => {
            // TLBI/DC/IC: barrier/cache ops — no cache state in functional mode.
            // Signal the step loop to flush the software TLB: TLBI instructions
            // invalidate TLB entries and must be honoured even in a functional sim
            // to avoid returning stale VA→PA mappings after kernel page table updates.
            a.tlb_flush_pending = true;
        }


        _ => unreachable!("wrong dispatch to sysreg"),
    }
    Ok(pc_written)
}

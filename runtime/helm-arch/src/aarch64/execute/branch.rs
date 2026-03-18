//! AArch64 execute — branch group.
#![allow(unused_imports, unused_variables)]
use crate::aarch64::arch_state::Aarch64ArchState;
use crate::aarch64::insn::{Instruction, Opcode};
use helm_core::{AccessType, HartException, MemFault, MemInterface};
#[allow(unused_imports)]
use helm_debug::{sim_stub, sim_warn};
use super::helpers::*;
use crate::aarch64::exception;

#[allow(clippy::too_many_lines)]
pub(super) fn exec_branch(
    insn: &Instruction,
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
) -> Result<bool, HartException> {
    use Opcode::*;
    let mut pc_written = false;
    match insn.opcode {
        // ── Branches ────────────────────────────────────────────────────────
        B => {
            a.pc = a.pc.wrapping_add(insn.imm as u64);
            pc_written = true;
        }
        Bl => {
            a.write_x(30, a.pc.wrapping_add(4)); // LR = PC+4
            a.pc = a.pc.wrapping_add(insn.imm as u64);
            pc_written = true;
        }
        Br => {
            a.pc = a.read_x(insn.rn);
            pc_written = true;
        }
        Blr => {
            a.write_x(30, a.pc.wrapping_add(4));
            a.pc = a.read_x(insn.rn);
            pc_written = true;
        }
        Ret => {
            a.pc = a.read_x(insn.rn); // default rn=30 (LR)
            pc_written = true;
        }
        BCond => {
            if a.eval_cond(insn.cond) {
                a.pc = a.pc.wrapping_add(insn.imm as u64);
                pc_written = true;
            }
        }
        Cbz => {
            let val = if insn.sf {
                a.read_x(insn.rd)
            } else {
                a.read_x(insn.rd) & 0xFFFF_FFFF
            };
            if val == 0 {
                a.pc = a.pc.wrapping_add(insn.imm as u64);
                pc_written = true;
            }
        }
        Cbnz => {
            let val = if insn.sf {
                a.read_x(insn.rd)
            } else {
                a.read_x(insn.rd) & 0xFFFF_FFFF
            };
            if val != 0 {
                a.pc = a.pc.wrapping_add(insn.imm as u64);
                pc_written = true;
            }
        }
        Tbz => {
            if a.read_x(insn.rn) & (1 << insn.imm2) == 0 {
                a.pc = a.pc.wrapping_add(insn.imm as u64);
                pc_written = true;
            }
        }
        Tbnz => {
            if a.read_x(insn.rn) & (1 << insn.imm2) != 0 {
                a.pc = a.pc.wrapping_add(insn.imm as u64);
                pc_written = true;
            }
        }


        // ── System / SVC ─────────────────────────────────────────────────────
        Svc => {
            if a.current_el >= 1 {
                // FS mode: SVC at EL1 -> synchronous exception
                use crate::aarch64::exception::*;
                let vector_offset = if a.spsel {
                    SYNC_EL1_SP1
                } else {
                    SYNC_EL1_SP0
                };
                let syndrome = EC_SVC_A64 | (insn.imm as u32 & 0xFFFF);
                exception_entry(a, vector_offset, syndrome, 0);
                pc_written = true;
            } else {
                // SE mode: raise EnvironmentCall for syscall handler.
                return Err(HartException::EnvironmentCall {
                    pc: a.pc,
                    nr: a.x[8], // AArch64 Linux: syscall nr in X8
                });
            }
        }
        Brk => {
            return Err(HartException::Breakpoint { pc: a.pc });
        }
        Nop | Wfe | Sev | Sevl | Bti => { /* no-op — BTI is a landing pad hint, NOP in functional mode */ }
        Wfi => {
            if a.current_el >= 1 {
                return Err(HartException::WaitForInterrupt);
            }
            // SE mode: no-op
        }
        Dmb | Dsb | Isb => { /* memory barriers -- no-op in single-threaded mode */ }
        Eret => {
            use crate::aarch64::exception::exception_return;
            exception_return(a);
            pc_written = true;
        }
        Hvc | Smc => {
            // Inline PSCI firmware stub (SMCCC convention: function ID in x0, result in x0).
            // No EL2 or EL3 in this model — we handle PSCI calls directly.
            let func_id = a.x[0] as u32;
            let result: i64 = match func_id {
                0x8400_0000 => 0x0000_0002,   // PSCI_VERSION → v0.2
                0x8400_000a => 0x0000_0000,   // PSCI_FEATURES → not supported
                0xc400_0003 => -2,            // CPU_ON → INVALID_PARAMS (single core)
                0xc400_0004 => 1,             // AFFINITY_INFO → CPU_OFF
                0x8400_0008 | 0x8400_0009 => {
                    // SYSTEM_OFF / SYSTEM_RESET → request simulator exit
                    return Err(HartException::Exit { code: 0 });
                }
                _ => -1,                      // PSCI_RET_NOT_SUPPORTED
            };
            a.x[0] = result as u64;
            // PC will be advanced by 4 by the normal return path (pc_written=false)
        }


        // ── Yield (hint) ────────────────────────────────────────────────
        Yield => {}


        // ── MSR immediate ───────────────────────────────────────────────
        MsrImm => {
            // The op1:CRm:op2 fields encode which PSTATE field:
            //   op1=3, op2=6 -> DAIFSet (set DAIF bits where imm=1)
            //   op1=3, op2=7 -> DAIFClr (clear DAIF bits where imm=1)
            //   op1=0, op2=5 -> SPSel
            let op1 = (insn.imm >> 16) & 7;
            let op2 = (insn.imm >> 5) & 7;
            let crm = (insn.imm >> 8) & 0xF; // imm4 value
            match (op1, op2) {
                (3, 6) => {
                    // DAIFSet
                    a.daif |= crm as u32;
                }
                (3, 7) => {
                    // DAIFClr
                    a.daif &= !(crm as u32);
                }
                (0, 5) => {
                    // SPSel
                    a.spsel = (crm & 1) != 0;
                }
                _ => { /* unknown PSTATE field -- ignore */ }
            }
        }


        _ => unreachable!("wrong dispatch to branch"),
    }
    Ok(pc_written)
}

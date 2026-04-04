//! AArch64 execute — branch group.
#![allow(unused_imports, unused_variables)]
use super::helpers::*;
use crate::aarch64::arch_state::Aarch64ArchState;
use crate::aarch64::exception;
use crate::aarch64::insn::{Instruction, Opcode};
use helm_core::{AccessType, HartException, MemFault, MemInterface};
#[allow(unused_imports)]
use helm_diag::{sim_stub, sim_warn};

const HCR_HCD: u64 = 1 << 29;
const HCR_TSC: u64 = 1 << 19;
const HCR_TGE: u64 = 1 << 27;
const SCR_SMD: u64 = 1 << 7;

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
                let syndrome = exception::EC_SVC_A64 | (insn.imm as u32 & 0xFFFF);
                let target_el = exception::route_sync_exception(a, exception::EC_SVC_A64);
                exception::exception_entry(a, target_el, syndrome, 0);
                pc_written = true;
            } else if (a.hcr_el2 & HCR_TGE) != 0
                || a.vbar_el1 != 0
                || a.vbar_el2 != 0
                || a.vbar_el3 != 0
            {
                let syndrome = exception::EC_SVC_A64 | (insn.imm as u32 & 0xFFFF);
                let target_el = exception::route_sync_exception(a, exception::EC_SVC_A64);
                exception::exception_entry(a, target_el, syndrome, 0);
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
            if a.current_el >= 1 {
                let syndrome = exception::EC_BRK_A64 | (insn.imm as u32 & 0xFFFF);
                exception::exception_entry(
                    a,
                    exception::route_sync_exception(a, exception::EC_BRK_A64),
                    syndrome,
                    0,
                );
                pc_written = true;
            } else {
                // SE mode: stop simulation
                return Err(HartException::Breakpoint { pc: a.pc });
            }
        }
        Nop | Wfe | Sev | Sevl | Bti | Esb | Sb => { /* no-op system hints */ }
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
            if a.psci_via_engine {
                return Err(HartException::PsciCall {
                    conduit: if insn.opcode == Hvc { "hvc" } else { "smc" },
                    function: a.x[0] as u32,
                    arg1: a.x[1],
                    arg2: a.x[2],
                    arg3: a.x[3],
                });
            }
            if insn.opcode == Hvc && a.current_el == 1 && (a.hcr_el2 & HCR_HCD) != 0 {
                exception::exception_entry(a, 1, exception::EC_UNKNOWN, 0);
                return Ok(true);
            }
            if insn.opcode == Smc && (a.scr_el3 & SCR_SMD) != 0 {
                exception::exception_entry(a, a.current_el.max(1), exception::EC_UNKNOWN, 0);
                return Ok(true);
            }
            if insn.opcode == Hvc && a.current_el == 1 && a.vbar_el2 != 0 {
                let syndrome = exception::EC_HVC_A64 | (insn.imm as u32 & 0xFFFF);
                exception::exception_entry(a, 2, syndrome, 0);
                return Ok(true);
            }
            if insn.opcode == Smc {
                if a.current_el == 1 && (a.hcr_el2 & HCR_TSC) != 0 && a.vbar_el2 != 0 {
                    let syndrome = exception::EC_SMC_A64 | (insn.imm as u32 & 0xFFFF);
                    exception::exception_entry(a, 2, syndrome, 0);
                    return Ok(true);
                }
                if matches!(a.current_el, 1 | 2) && a.vbar_el3 != 0 {
                    let syndrome = exception::EC_SMC_A64 | (insn.imm as u32 & 0xFFFF);
                    exception::exception_entry(a, 3, syndrome, 0);
                    return Ok(true);
                }
            }

            // Inline PSCI firmware stub (SMCCC convention: function ID in x0, result in x0).
            // When no higher-EL vector is configured, keep the direct firmware
            // path used by FS boot.
            let func_id = a.x[0] as u32;
            let result: i64 = match func_id {
                0x8400_0000 => 0x0001_0001, // PSCI_VERSION → v1.1
                0x8400_0001 => 0x0000_0000, // CPU_SUSPEND → SUCCESS
                0x8400_0002 => 0x0000_0000, // CPU_OFF → SUCCESS (single-core stub)
                0x8400_0006 => 0x0000_0002, // MIGRATE_INFO_TYPE → TOS not present
                0x8400_000a => match a.x[1] as u32 {
                    0x8400_0000 | 0x8400_0001 | 0x8400_0002 | 0x8400_0003 | 0x8400_0006
                    | 0x8400_0008 | 0x8400_0009 | 0x8400_000a => 0x0000_0000,
                    _ => -1,
                },
                0x8400_0003 | 0xc400_0003 => -4, // CPU_ON → ALREADY_ON (single core)
                0xc400_0004 => 1,                // AFFINITY_INFO → CPU_OFF
                0x8400_0008 | 0x8400_0009 => {
                    // SYSTEM_OFF / SYSTEM_RESET → request simulator exit
                    return Err(HartException::Exit { code: 0 });
                }
                _ => -1, // PSCI_RET_NOT_SUPPORTED
            };
            a.x[0] = result as u64;
            // PC will be advanced by 4 by the normal return path (pc_written=false)
        }

        // ── Yield (hint) ────────────────────────────────────────────────
        Yield => {}

        // ── Pointer Authentication (identity implementation) ─────────
        // PAC hint instructions: NOP (pointer unchanged).
        PacHint => {}
        // Register-form PAC/AUT: identity — pointer unchanged.
        PacReg | PacRegZ | AutReg | AutRegZ | Xpac => {}
        // RETAA/RETAB: authenticate LR then RET (identity = plain RET).
        RetAut => {
            a.pc = a.x[30];
            pc_written = true;
        }
        // BRAAZ/BRABZ: authenticate target then BR (identity = plain BR).
        BrAutZ => {
            a.pc = a.read_x(insn.rn);
            pc_written = true;
        }
        // BLRAAZ/BLRABZ: authenticate target then BLR (identity = plain BLR).
        BlrAutZ => {
            a.x[30] = a.pc.wrapping_add(4);
            a.pc = a.read_x(insn.rn);
            pc_written = true;
        }
        // BRAA/BRAB: authenticate target with context then BR.
        BrAut => {
            a.pc = a.read_x(insn.rn);
            pc_written = true;
        }
        // BLRAA/BLRAB: authenticate target with context then BLR.
        BlrAut => {
            a.x[30] = a.pc.wrapping_add(4);
            a.pc = a.read_x(insn.rn);
            pc_written = true;
        }
        // ERETAA/ERETAB: authenticate ELR then ERET.
        EretAut => {
            use crate::aarch64::exception::exception_return;
            exception_return(a);
            pc_written = true;
        }

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

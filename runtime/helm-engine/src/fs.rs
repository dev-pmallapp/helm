//! Full-system (FS) mode step loop for AArch64.
//!
//! Provides `step_aarch64_fs()` — a single instruction step that:
//! 1. Checks for pending IRQ (via GIC) and delivers exception if unmasked
//! 2. Translates PC via MMU → fetch → decode → execute
//! 3. Translates data accesses via MMU
//! 4. Checks generic timer against tick counter

use helm_arch::aarch64::exception::{self, *};
use helm_arch::aarch64::mmu::{self, MmuAccess};
use helm_arch::aarch64::arch_state::Aarch64ArchState;
use helm_arch::{aarch64_decode, aarch64_execute};
use helm_core::{AccessType, HartException, MemFault, MemInterface};
use helm_probe::{probe, BranchEvent, BranchKind, CpuProbes, CpuStepEvent, CpuFaultEvent};

use crate::system_mem::SystemMem;

/// FS-mode CPU state (per-core).
pub struct FsState {
    /// Whether an external IRQ is pending (set by GIC callback).
    pub irq_pending: bool,
    /// Monotonic tick counter (incremented each instruction).
    pub tick: u64,
}

impl FsState {
    /// Create a new FS state.
    pub fn new() -> Self {
        Self {
            irq_pending: false,
            tick: 0,
        }
    }
}

/// Snapshot of MMU configuration for address translation.
///
/// Captured before execute() so we don't hold a reference to ArchState
/// while execute() borrows it mutably.
#[derive(Clone, Copy)]
struct MmuConfig {
    sctlr_el1: u64,
    tcr_el1: u64,
    ttbr0_el1: u64,
    ttbr1_el1: u64,
}

impl MmuConfig {
    fn from_arch(a: &Aarch64ArchState) -> Self {
        Self {
            sctlr_el1: a.sctlr_el1,
            tcr_el1: a.tcr_el1,
            ttbr0_el1: a.ttbr0_el1,
            ttbr1_el1: a.ttbr1_el1,
        }
    }

    fn mmu_enabled(&self) -> bool {
        self.sctlr_el1 & 1 != 0
    }
}

/// Memory wrapper that translates VA→PA using a snapshotted MMU config.
pub struct TranslatingMem<'a> {
    pub sys_mem: &'a mut SystemMem,
    mmu_cfg: MmuConfig,
}

impl<'a> TranslatingMem<'a> {
    fn translate_va(&mut self, va: u64, access: MmuAccess) -> Result<u64, MemFault> {
        if !self.mmu_cfg.mmu_enabled() {
            return Ok(va);
        }
        // Build a temporary ArchState-like view for the MMU walker
        let mut tmp = Aarch64ArchState::new();
        tmp.sctlr_el1 = self.mmu_cfg.sctlr_el1;
        tmp.tcr_el1 = self.mmu_cfg.tcr_el1;
        tmp.ttbr0_el1 = self.mmu_cfg.ttbr0_el1;
        tmp.ttbr1_el1 = self.mmu_cfg.ttbr1_el1;
        tmp.current_el = 1;

        mmu::translate(&tmp, va, access, self.sys_mem)
            .map(|r| r.pa)
            .map_err(|fault| MemFault::PageFault { addr: fault.va })
    }
}

impl<'a> MemInterface for TranslatingMem<'a> {
    fn read(&mut self, addr: u64, size: usize, ty: AccessType) -> Result<u64, MemFault> {
        let mmu_access = match ty {
            AccessType::Fetch => MmuAccess::Execute,
            _ => MmuAccess::Read,
        };
        let pa = self.translate_va(addr, mmu_access)?;
        self.sys_mem.read(pa, size, ty)
    }

    fn write(&mut self, addr: u64, size: usize, val: u64, ty: AccessType) -> Result<(), MemFault> {
        let pa = self.translate_va(addr, MmuAccess::Write)?;
        self.sys_mem.write(pa, size, val, ty)
    }
}

/// Execute one FS-mode AArch64 instruction.
///
/// Returns `Ok(())` on success, `Err` on exception (WFI, abort, etc.).
/// The caller should handle `WaitForInterrupt` by advancing the event queue.
pub fn step_aarch64_fs(
    a64: &mut Aarch64ArchState,
    sys_mem: &mut SystemMem,
    fs: &mut FsState,
    probes: &CpuProbes,
) -> Result<(), HartException> {
    // 1. Check for pending IRQ: deliver if unmasked
    if fs.irq_pending && (a64.daif & 0x2) == 0 {
        // DAIF bit 1 = I (IRQ mask). 0 = unmasked.
        let vector_offset = if a64.current_el == 0 {
            IRQ_EL0_64
        } else if a64.spsel {
            IRQ_EL1_SP1
        } else {
            IRQ_EL1_SP0
        };
        exception::exception_entry(a64, vector_offset, 0, 0);
        fs.irq_pending = false;
        return Ok(());
    }

    let pc = a64.pc;

    probe!(probes.pre_step, CpuStepEvent {
        pc,
        raw: 0,
        insn_class: helm_probe::InsnClass::Unknown,
        is_stub: false,
    });

    // 2. Fetch: translate PC via MMU, then read instruction
    let fetch_result = mmu::translate(a64, pc, MmuAccess::Execute, sys_mem);
    let fetch_pa = match fetch_result {
        Ok(r) => r.pa,
        Err(fault) => {
            let ec = if a64.current_el == 0 {
                EC_INSN_ABORT_EL0
            } else {
                EC_INSN_ABORT_EL1
            };
            let iss = fault.iss_insn();
            let vector_offset = if a64.current_el == 0 {
                SYNC_EL0_64
            } else if a64.spsel {
                SYNC_EL1_SP1
            } else {
                SYNC_EL1_SP0
            };
            exception::exception_entry(a64, vector_offset, ec | (1 << 25) | iss, pc);
            // Don't return error — exception was delivered internally
            probe!(probes.fault, CpuFaultEvent { pc, raw: 0, kind: "insn-abort" });
            return Ok(());
        }
    };

    let raw = sys_mem
        .read(fetch_pa, 4, AccessType::Fetch)
        .map_err(|_| HartException::InstructionAccessFault { addr: pc })?
        as u32;

    // 3. Decode
    let insn = match aarch64_decode(raw, pc) {
        Ok(insn) => insn,
        Err(_) => {
            return Err(HartException::IllegalInstruction { pc, raw });
        }
    };

    // 4. Snapshot MMU config before execute (avoids borrow conflict)
    let mmu_cfg = MmuConfig::from_arch(a64);

    // 5. Execute with translating memory
    let mut tmem = TranslatingMem { sys_mem, mmu_cfg };
    let exec_result = aarch64_execute(&insn, a64, &mut tmem);

    match exec_result {
        Ok(pc_written) => {
            if !pc_written {
                a64.pc = a64.pc.wrapping_add(4);
            }
            let (fs_class, _, fs_is_stub) = crate::classify_aarch64_opcode(insn.opcode);
            probe!(probes.post_step, CpuStepEvent {
                pc,
                raw,
                insn_class: crate::to_probe_class(fs_class),
                is_stub: fs_is_stub,
            });
            if insn.is_branch() {
                probe!(probes.branch, BranchEvent {
                    pc,
                    target: a64.pc,
                    taken: pc_written,
                    kind: BranchKind::DirectUncond,  // simplified in FS mode
                });
            }
        }
        Err(HartException::WaitForInterrupt) => {
            a64.pc = a64.pc.wrapping_add(4);
            return Err(HartException::WaitForInterrupt);
        }
        Err(HartException::EnvironmentCall { pc: _, nr: _ }) => {
            probe!(probes.fault, CpuFaultEvent { pc, raw, kind: "svc" });
            // SVC from EL0 in FS mode
            let vector_offset = SYNC_EL0_64;
            let syndrome = EC_SVC_A64 | (1 << 25) | (insn.imm as u32 & 0xFFFF);
            exception::exception_entry(a64, vector_offset, syndrome, 0);
        }
        Err(HartException::LoadAccessFault { addr }) => {
            probe!(probes.fault, CpuFaultEvent { pc, raw, kind: "data-abort" });
            let ec = if a64.current_el == 0 { EC_DATA_ABORT_EL0 } else { EC_DATA_ABORT_EL1 };
            let iss = 0b000101; // Translation fault L1
            let vector_offset = if a64.current_el == 0 {
                SYNC_EL0_64
            } else if a64.spsel {
                SYNC_EL1_SP1
            } else {
                SYNC_EL1_SP0
            };
            exception::exception_entry(a64, vector_offset, ec | (1 << 25) | iss, addr);
        }
        Err(HartException::StoreAccessFault { addr }) => {
            probe!(probes.fault, CpuFaultEvent { pc, raw, kind: "store-abort" });
            let ec = if a64.current_el == 0 { EC_DATA_ABORT_EL0 } else { EC_DATA_ABORT_EL1 };
            let iss = (1 << 6) | 0b000101; // WnR=1, Translation fault L1
            let vector_offset = if a64.current_el == 0 {
                SYNC_EL0_64
            } else if a64.spsel {
                SYNC_EL1_SP1
            } else {
                SYNC_EL1_SP0
            };
            exception::exception_entry(a64, vector_offset, ec | (1 << 25) | iss, addr);
        }
        Err(e) => return Err(e),
    }

    // 6. Advance tick counter
    fs.tick += 1;

    // 7. Update virtual counter (used by MRS CNTVCT_EL0)
    a64.cntvct_el0 = fs.tick;

    Ok(())
}

/// Check and fire the generic timer if conditions are met.
///
/// Call this periodically (e.g. every 1024 instructions) to evaluate both
/// the physical (INTID 30) and virtual (INTID 27) generic timers.
///
/// Maintains `CNTP_CTL_EL0.ISTATUS` (bit 2) and `CNTV_CTL_EL0.ISTATUS` (bit 2)
/// so the Linux ISR can confirm the timer fired by reading CTL.ISTATUS.
///
/// Returns `(p_fire, v_fire)`: whether physical / virtual timer should assert.
/// Caller must assert/deassert INTID 30 / INTID 27 in the GIC accordingly.
pub fn check_timers(a64: &mut Aarch64ArchState, fs: &mut FsState) -> (bool, bool) {
    // Physical timer (CNTP): INTID 30
    let p_ctl = a64.cntp_ctl_el0;
    let p_cond = (p_ctl & 1 != 0) && a64.cntp_cval_el0 <= fs.tick; // ENABLE && deadline met
    // Maintain ISTATUS (bit 2) — read-only to SW, set by hardware
    if p_cond { a64.cntp_ctl_el0 |= 4; } else { a64.cntp_ctl_el0 &= !4; }
    let p_fire = p_cond && (p_ctl & 2 == 0); // ENABLE && deadline && !IMASK

    // Virtual timer (CNTV): INTID 27
    let v_ctl = a64.cntv_ctl_el0;
    let v_cond = (v_ctl & 1 != 0) && a64.cntv_cval_el0 <= fs.tick;
    if v_cond { a64.cntv_ctl_el0 |= 4; } else { a64.cntv_ctl_el0 &= !4; }
    let v_fire = v_cond && (v_ctl & 2 == 0);

    (p_fire, v_fire)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_arch::aarch64::insn::Opcode;
    use crate::FlatMem;
    use crate::system_mem::SystemMem;

    fn make_fs_env() -> (Aarch64ArchState, SystemMem, FsState, CpuProbes) {
        let mut a64 = Aarch64ArchState::new();
        a64.current_el = 1;
        a64.spsel = true;
        a64.sctlr_el1 = 0; // MMU disabled

        let ram = FlatMem::new(0, 0);
        let sys_mem = SystemMem::new(ram);
        let fs = FsState::new();
        let probes = CpuProbes::default();

        (a64, sys_mem, fs, probes)
    }

    #[test]
    fn irq_delivered_when_unmasked() {
        let (mut a64, mut sys_mem, mut fs, probes) = make_fs_env();
        a64.vbar_el1 = 0x1000;
        a64.pc = 0x2000;
        a64.daif = 0;

        fs.irq_pending = true;

        let result = step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes);
        assert!(result.is_ok());
        assert_eq!(a64.pc, 0x1000 + 0x280);
        assert!(!fs.irq_pending);
    }

    #[test]
    fn irq_suppressed_when_masked() {
        let (mut a64, mut sys_mem, mut fs, probes) = make_fs_env();
        a64.vbar_el1 = 0x1000;
        a64.pc = 0x2000;
        a64.daif = 0x2; // IRQ masked

        fs.irq_pending = true;

        // Write a NOP instruction at PC
        let nop: u32 = 0xD503201F;
        sys_mem.ram.load_bytes(0x2000, &nop.to_le_bytes());

        let result = step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes);
        assert!(result.is_ok());
        assert_eq!(a64.pc, 0x2004);
        assert!(fs.irq_pending);
    }

    #[test]
    fn fs_step_executes_nop() {
        let (mut a64, mut sys_mem, mut fs, probes) = make_fs_env();
        a64.pc = 0x1000;

        let nop: u32 = 0xD503201F;
        sys_mem.ram.load_bytes(0x1000, &nop.to_le_bytes());

        let result = step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes);
        assert!(result.is_ok());
        assert_eq!(a64.pc, 0x1004);
        assert_eq!(fs.tick, 1);
    }

    #[test]
    fn fs_step_executes_eret_sequence() {
        let (mut a64, mut sys_mem, mut fs, probes) = make_fs_env();
        a64.pc = 0x1000;
        a64.x[0] = 0xE11;
        a64.x[30] = 0x2000;

        let program = [
            0xD518_4000u32, // msr spsr_el1, x0
            0xD518_403Eu32, // msr elr_el1, x30
            0xD69F_03E0u32, // eret
        ];
        for (idx, insn) in program.iter().enumerate() {
            sys_mem.ram.load_bytes(0x1000 + (idx as u64 * 4), &insn.to_le_bytes());
        }

        assert!(step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes).is_ok());
        assert_eq!(a64.spsr_el1, 0xE11);

        assert!(step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes).is_ok());
        assert_eq!(a64.elr_el1, 0x2000);
        let result = step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes);
        assert!(result.is_ok());
        assert_eq!(a64.pc, 0x2000);
        assert_eq!(a64.current_el, 1);
        assert!(a64.spsel);
    }

    #[test]
    fn eret_decodes_and_executes_directly() {
        let insn = helm_arch::aarch64_decode(0xD69F_03E0, 0x1008).unwrap();
        assert_eq!(insn.opcode, Opcode::Eret);

        let mut a64 = Aarch64ArchState::new();
        a64.pc = 0x1008;
        a64.current_el = 1;
        a64.spsel = true;
        a64.elr_el1 = 0x2000;
        a64.spsr_el1 = 0xE11;

        let mut mem = FlatMem::new(0, 0);
        let result = helm_arch::aarch64_execute(&insn, &mut a64, &mut mem);
        assert!(result.is_ok());
        assert_eq!(a64.pc, 0x2000);
    }

    #[test]
   fn timer_fires_when_conditions_met() {
       let mut a64 = Aarch64ArchState::new();
       let mut fs = FsState::new();
       fs.tick = 1000;

       a64.cntp_ctl_el0 = 1; // ENABLE=1, IMASK=0
       a64.cntp_cval_el0 = 500;

        let (p_fire, v_fire) = check_timers(&mut a64, &mut fs);
        assert!(p_fire, "physical timer must fire");
        assert!(!v_fire, "virtual timer must not fire (disabled)");
        assert_eq!(a64.cntp_ctl_el0 & 4, 4, "ISTATUS must be set");
   }

   #[test]
   fn timer_suppressed_when_masked() {
       let mut a64 = Aarch64ArchState::new();
       let mut fs = FsState::new();
       fs.tick = 1000;

       a64.cntp_ctl_el0 = 3; // ENABLE=1, IMASK=1
       a64.cntp_cval_el0 = 500;

        let (p_fire, _) = check_timers(&mut a64, &mut fs);
        assert!(!p_fire, "masked timer must not fire");
        assert_eq!(a64.cntp_ctl_el0 & 4, 4, "ISTATUS still set even when masked");
   }
}

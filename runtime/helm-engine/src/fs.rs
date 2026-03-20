//! Full-system (FS) mode step loop for AArch64.
//!
//! Provides `step_aarch64_fs()` — a single instruction step that:
//! 1. Checks for pending IRQ (via GIC) and delivers exception if unmasked
//! 2. Translates PC via MMU → fetch → decode → execute
//! 3. Translates data accesses via MMU
//! 4. Checks generic timer against tick counter

use helm_arch::aarch64::exception::{self, *};
use helm_arch::aarch64::mmu::{self, MmuAccess, MmuConfig, Tlb};
use helm_arch::aarch64::arch_state::Aarch64ArchState;
use helm_arch::aarch64::insn::Opcode;
use helm_arch::{aarch64_decode, aarch64_execute};
use helm_core::{AccessType, HartException, MemFault, MemInterface};
use helm_probe::{probe, BranchEvent, BranchKind, CpuProbes, CpuStepEvent, CpuFaultEvent, MemAccessEvent};
use helm_arch::aarch64::mmu::MmuFault;
use helm_plugin::PluginRegistry;

use crate::system_mem::SystemMem;

/// FS-mode CPU state (per-core).
pub struct FsState {
    /// Whether an external IRQ is pending (set by GIC callback).
    pub irq_pending: bool,
    /// Monotonic tick counter (incremented each instruction).
    pub tick: u64,
    /// Software TLB — direct-mapped 256-entry VA→PA cache.
    pub tlb: Tlb,
}

impl FsState {
    /// Create a new FS state.
    pub fn new() -> Self {
        Self {
            irq_pending: false,
            tick: 0,
            tlb: Tlb::new(),
        }
    }
}

/// Memory wrapper that translates VA→PA using a snapshotted MMU config.
pub struct TranslatingMem<'a> {
    pub sys_mem: &'a mut SystemMem,
    mmu_cfg: MmuConfig,
    tlb: &'a mut Tlb,
}

#[derive(Clone, Copy)]
struct MemAccessRecord {
    vaddr: u64,
    size: u8,
    is_store: bool,
    is_atomic: bool,
}

impl Default for MemAccessRecord {
    fn default() -> Self {
        Self { vaddr: 0, size: 0, is_store: false, is_atomic: false }
    }
}

impl<'a> TranslatingMem<'a> {
    fn new(sys_mem: &'a mut SystemMem, mmu_cfg: MmuConfig, tlb: &'a mut Tlb) -> Self {
        Self { sys_mem, mmu_cfg, tlb }
    }

    #[inline]
    fn translate_va(&mut self, va: u64, access: MmuAccess) -> Result<u64, MemFault> {
        if !self.mmu_cfg.mmu_enabled() {
            return Ok(va);
        }
        mmu::translate_cfg(&self.mmu_cfg, va, access, self.sys_mem, Some(self.tlb))
            .map_err(|fault| mmu_fault_to_mem_fault(&fault, access))
    }
}

/// Convert an MMU fault to a `MemFault`, preserving the ISS for correct ESR injection.
#[inline]
fn mmu_fault_to_mem_fault(fault: &MmuFault, access: MmuAccess) -> MemFault {
    let is_write = access == MmuAccess::Write;
    let iss = fault.iss_data(is_write);
    MemFault::PageFault { addr: fault.va, iss }
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

struct InstrumentedTranslatingMem<'a> {
    inner: TranslatingMem<'a>,
    records: [MemAccessRecord; 8],
    count: usize,
}

impl<'a> InstrumentedTranslatingMem<'a> {
    fn new(sys_mem: &'a mut SystemMem, mmu_cfg: MmuConfig, tlb: &'a mut Tlb) -> Self {
        Self {
            inner: TranslatingMem::new(sys_mem, mmu_cfg, tlb),
            records: [MemAccessRecord::default(); 8],
            count: 0,
        }
    }

    fn push(&mut self, vaddr: u64, size: u8, is_store: bool, is_atomic: bool) {
        if self.count < self.records.len() {
            self.records[self.count] = MemAccessRecord { vaddr, size, is_store, is_atomic };
            self.count += 1;
        }
    }

    fn recorded(&self) -> &[MemAccessRecord] {
        &self.records[..self.count]
    }
}

impl<'a> MemInterface for InstrumentedTranslatingMem<'a> {
    fn read(&mut self, addr: u64, size: usize, ty: AccessType) -> Result<u64, MemFault> {
        self.push(addr, size as u8, false, ty == AccessType::Atomic);
        self.inner.read(addr, size, ty)
    }

    fn write(&mut self, addr: u64, size: usize, val: u64, ty: AccessType) -> Result<(), MemFault> {
        self.push(addr, size as u8, true, ty == AccessType::Atomic);
        self.inner.write(addr, size, val, ty)
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
    plugins: &PluginRegistry,
    vcpu_idx: usize,
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
        exception::exception_entry_el1(a64, vector_offset, 0, 0);
        fs.irq_pending = false;
        return Ok(());
    }

    let pc = a64.pc;

    // 2. Fetch: translate PC via MMU (with TLB), then read instruction
    let fetch_result = mmu::translate(a64, pc, MmuAccess::Execute, sys_mem, Some(&mut fs.tlb));
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
            exception::exception_entry_el1(a64, vector_offset, ec | (1 << 25) | iss, pc);
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

    // 4. Snapshot MMU config before execute (avoids borrow conflict on a64)
    let mmu_cfg = MmuConfig::from_arch(a64);

    let record_mem = plugins.has_mem_callbacks() || probes.mem.has_listeners();
    // 5. Execute with translating memory (TLB shared between fetch and data accesses)
    let exec_result = if record_mem {
        let mut tmem = InstrumentedTranslatingMem::new(sys_mem, mmu_cfg, &mut fs.tlb);
        let exec_result = aarch64_execute(&insn, a64, &mut tmem);
        for rec in tmem.recorded() {
            plugins.fire_mem_access(vcpu_idx, &helm_plugin::runtime::MemInfo {
                vaddr: rec.vaddr,
                size: rec.size,
                is_store: rec.is_store,
                is_atomic: rec.is_atomic,
            });
            probe!(probes.mem, MemAccessEvent {
                addr: rec.vaddr,
                size: rec.size,
                is_store: rec.is_store,
                pc,
            });
        }
        exec_result
    } else {
        let mut tmem = TranslatingMem::new(sys_mem, mmu_cfg, &mut fs.tlb);
        aarch64_execute(&insn, a64, &mut tmem)
    };

    match exec_result {
        Ok(pc_written) => {
            if !pc_written {
                a64.pc = a64.pc.wrapping_add(4);
            }
            let (fs_class, opcode_name, fs_is_stub) = crate::classify_aarch64_opcode(insn.opcode);
            probe!(probes.post_step, CpuStepEvent {
                pc,
                raw,
                insn_class: crate::to_probe_class(fs_class),
                is_stub: fs_is_stub,
            });
            if plugins.has_insn_callbacks() {
                plugins.fire_insn_exec(vcpu_idx, &helm_plugin::runtime::InsnInfo {
                    pc,
                    raw,
                    size: 4,
                    class: fs_class,
                    opcode_name,
                    is_stub: fs_is_stub,
                    context: helm_plugin::runtime::ArchContext::Aarch64 {
                        x: a64.x,
                        sp: a64.sp,
                        pc: a64.pc,
                        nzcv: a64.nzcv,
                    },
                });
            }
            if insn.is_branch() {
                let target = a64.pc;
                probe!(probes.branch, BranchEvent {
                    pc,
                    target,
                    taken: pc_written,
                    kind: BranchKind::DirectUncond,  // simplified in FS mode
                });
                if plugins.has_branch_callbacks() {
                    plugins.fire_branch(vcpu_idx, &helm_plugin::runtime::BranchInfo {
                        pc,
                        target,
                        taken: pc_written,
                        kind: crate::classify_branch_kind(insn.opcode),
                    });
                }
            }
            if matches!(insn.opcode, Opcode::Brk) {
                if plugins.has_fault_callbacks() {
                    plugins.fire_fault(&helm_plugin::runtime::FaultInfo {
                        vcpu_idx,
                        pc,
                        raw,
                        kind: helm_plugin::runtime::FaultKind::Breakpoint,
                        message: format!("guest BRK at {pc:#x}"),
                        insn_count: fs.tick,
                        context: helm_plugin::runtime::ArchContext::Aarch64 {
                            x: a64.x,
                            sp: a64.sp,
                            pc: a64.pc,
                            nzcv: a64.nzcv,
                        },
                    });
                }
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
            exception::exception_entry_el1(a64, vector_offset, syndrome, 0);
        }
        Err(HartException::LoadAccessFault { addr }) => {
            probe!(probes.fault, CpuFaultEvent { pc, raw, kind: "data-abort" });
            if plugins.has_fault_callbacks() {
                plugins.fire_fault(&helm_plugin::runtime::FaultInfo {
                    vcpu_idx,
                    pc,
                    raw,
                    kind: helm_plugin::runtime::FaultKind::MemoryFault,
                    message: format!("load access fault at {addr:#x}"),
                    insn_count: fs.tick,
                    context: helm_plugin::runtime::ArchContext::Aarch64 {
                        x: a64.x,
                        sp: a64.sp,
                        pc: a64.pc,
                        nzcv: a64.nzcv,
                    },
                });
            }
            let ec = if a64.current_el == 0 { EC_DATA_ABORT_EL0 } else { EC_DATA_ABORT_EL1 };
            let iss = 0b000101; // Translation fault L1
            let vector_offset = if a64.current_el == 0 {
                SYNC_EL0_64
            } else if a64.spsel {
                SYNC_EL1_SP1
            } else {
                SYNC_EL1_SP0
            };
            exception::exception_entry_el1(a64, vector_offset, ec | (1 << 25) | iss, addr);
        }
        Err(HartException::StoreAccessFault { addr }) => {
            probe!(probes.fault, CpuFaultEvent { pc, raw, kind: "store-abort" });
            if plugins.has_fault_callbacks() {
                plugins.fire_fault(&helm_plugin::runtime::FaultInfo {
                    vcpu_idx,
                    pc,
                    raw,
                    kind: helm_plugin::runtime::FaultKind::MemoryFault,
                    message: format!("store/AMO access fault at {addr:#x}"),
                    insn_count: fs.tick,
                    context: helm_plugin::runtime::ArchContext::Aarch64 {
                        x: a64.x,
                        sp: a64.sp,
                        pc: a64.pc,
                        nzcv: a64.nzcv,
                    },
                });
            }
            let ec = if a64.current_el == 0 { EC_DATA_ABORT_EL0 } else { EC_DATA_ABORT_EL1 };
            let iss = (1 << 6) | 0b000101; // WnR=1, Translation fault L1
            let vector_offset = if a64.current_el == 0 {
                SYNC_EL0_64
            } else if a64.spsel {
                SYNC_EL1_SP1
            } else {
                SYNC_EL1_SP0
            };
            exception::exception_entry_el1(a64, vector_offset, ec | (1 << 25) | iss, addr);
        }
        Err(HartException::DataAbort { addr, iss }) => {
            probe!(probes.fault, CpuFaultEvent { pc, raw, kind: "data-abort" });
            if plugins.has_fault_callbacks() {
                plugins.fire_fault(&helm_plugin::runtime::FaultInfo {
                    vcpu_idx,
                    pc,
                    raw,
                    kind: helm_plugin::runtime::FaultKind::MemoryFault,
                    message: format!("data abort at {addr:#x} iss={iss:#x}"),
                    insn_count: fs.tick,
                    context: helm_plugin::runtime::ArchContext::Aarch64 {
                        x: a64.x,
                        sp: a64.sp,
                        pc: a64.pc,
                        nzcv: a64.nzcv,
                    },
                });
            }
            let ec = if a64.current_el == 0 { EC_DATA_ABORT_EL0 } else { EC_DATA_ABORT_EL1 };
            let vector_offset = if a64.current_el == 0 {
                SYNC_EL0_64
            } else if a64.spsel {
                SYNC_EL1_SP1
            } else {
                SYNC_EL1_SP0
            };
            exception::exception_entry_el1(a64, vector_offset, ec | (1 << 25) | iss, addr);
        }
        Err(HartException::InstructionAbort { addr, iss }) => {
            probe!(probes.fault, CpuFaultEvent { pc, raw, kind: "insn-abort" });
            if plugins.has_fault_callbacks() {
                plugins.fire_fault(&helm_plugin::runtime::FaultInfo {
                    vcpu_idx,
                    pc,
                    raw,
                    kind: helm_plugin::runtime::FaultKind::MemoryFault,
                    message: format!("instruction abort at {addr:#x} iss={iss:#x}"),
                    insn_count: fs.tick,
                    context: helm_plugin::runtime::ArchContext::Aarch64 {
                        x: a64.x,
                        sp: a64.sp,
                        pc: a64.pc,
                        nzcv: a64.nzcv,
                    },
                });
            }
            let ec = if a64.current_el == 0 { EC_INSN_ABORT_EL0 } else { EC_INSN_ABORT_EL1 };
            let vector_offset = if a64.current_el == 0 {
                SYNC_EL0_64
            } else if a64.spsel {
                SYNC_EL1_SP1
            } else {
                SYNC_EL1_SP0
            };
            exception::exception_entry_el1(a64, vector_offset, ec | (1 << 25) | iss, addr);
        }
        Err(e) => return Err(e),
    }

    // 6. Flush software TLB if any TLBI/DC/IC instruction was executed.
    // Linux always issues TLBI after modifying page tables; honouring it is
    // required for correctness even in a functional (no-cache) simulation.
    if a64.tlb_flush_pending {
        fs.tlb.flush();
        a64.tlb_flush_pending = false;
    }

    // 7. Advance tick counter
    fs.tick += 1;

    // 8. Update virtual counter (used by MRS CNTVCT_EL0)
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
    use helm_plugin::PluginRegistry;
    use crate::FlatMem;
    use crate::system_mem::SystemMem;
    use std::sync::{Arc, Mutex};

    fn make_fs_env() -> (Aarch64ArchState, SystemMem, FsState, CpuProbes, PluginRegistry) {
        let mut a64 = Aarch64ArchState::new();
        a64.current_el = 1;
        a64.spsel = true;
        a64.sctlr_el1 = 0; // MMU disabled

        let ram = FlatMem::new(0, 0);
        let sys_mem = SystemMem::new(ram);
        let fs = FsState::new();
        let probes = CpuProbes::default();
        let plugins = PluginRegistry::new();

        (a64, sys_mem, fs, probes, plugins)
    }

    #[test]
    fn irq_delivered_when_unmasked() {
        let (mut a64, mut sys_mem, mut fs, probes, plugins) = make_fs_env();
        a64.vbar_el1 = 0x1000;
        a64.pc = 0x2000;
        a64.daif = 0;

        fs.irq_pending = true;

        let result = step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0);
        assert!(result.is_ok());
        assert_eq!(a64.pc, 0x1000 + 0x280);
        assert!(!fs.irq_pending);
    }

    #[test]
    fn irq_suppressed_when_masked() {
        let (mut a64, mut sys_mem, mut fs, probes, plugins) = make_fs_env();
        a64.vbar_el1 = 0x1000;
        a64.pc = 0x2000;
        a64.daif = 0x2; // IRQ masked

        fs.irq_pending = true;

        // Write a NOP instruction at PC
        let nop: u32 = 0xD503201F;
        sys_mem.ram.load_bytes(0x2000, &nop.to_le_bytes());

        let result = step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0);
        assert!(result.is_ok());
        assert_eq!(a64.pc, 0x2004);
        assert!(fs.irq_pending);
    }

    #[test]
    fn fs_step_executes_nop() {
        let (mut a64, mut sys_mem, mut fs, probes, plugins) = make_fs_env();
        a64.pc = 0x1000;

        let nop: u32 = 0xD503201F;
        sys_mem.ram.load_bytes(0x1000, &nop.to_le_bytes());

        let result = step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0);
        assert!(result.is_ok());
        assert_eq!(a64.pc, 0x1004);
        assert_eq!(fs.tick, 1);
    }

    #[test]
    fn fs_step_executes_eret_sequence() {
        let (mut a64, mut sys_mem, mut fs, probes, plugins) = make_fs_env();
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

        assert!(step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0).is_ok());
        assert_eq!(a64.spsr_el1, 0xE11);

        assert!(step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0).is_ok());
        assert_eq!(a64.elr_el1, 0x2000);
        let result = step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0);
        assert!(result.is_ok());
        assert_eq!(a64.pc, 0x2000);
        // SPSR_EL1 = 0xE11: bits[3:2]=0b00 → EL0, bit[0]=1 → SPSel=1
        assert_eq!(a64.current_el, 0);
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
    fn fs_step_fires_plugin_fault_for_guest_brk() {
        let (mut a64, mut sys_mem, mut fs, probes, mut plugins) = make_fs_env();
        a64.pc = 0x1000;
        a64.vbar_el1 = 0x8000;

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_fault = Arc::clone(&seen);
        plugins.on_fault(Box::new(move |info| {
            seen_fault.lock().unwrap().push((info.pc, info.raw, info.kind));
        }));

        let brk: u32 = 0xD421_0000;
        sys_mem.ram.load_bytes(0x1000, &brk.to_le_bytes());

        let result = step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0);
        assert!(result.is_ok());

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, 0x1000);
        assert_eq!(seen[0].1, brk);
        assert_eq!(seen[0].2, helm_plugin::runtime::FaultKind::Breakpoint);
    }

    #[test]
    fn fs_step_fires_mem_callbacks_for_load_store() {
        let (mut a64, mut sys_mem, mut fs, probes, mut plugins) = make_fs_env();
        a64.pc = 0x1000;
        a64.sp = 0x4000;
        a64.x[0] = 0x1122_3344_5566_7788;

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_mem = Arc::clone(&seen);
        plugins.on_mem_access(
            helm_plugin::runtime::MemFilter::All,
            Box::new(move |_vcpu, info| {
                seen_mem
                    .lock()
                    .unwrap()
                    .push((info.vaddr, info.size, info.is_store, info.is_atomic));
            }),
        );

        let program = [
            0xF90003E0u32, // STR X0, [SP]
            0xF94003E1u32, // LDR X1, [SP]
        ];
        for (idx, insn) in program.iter().enumerate() {
            sys_mem.ram.load_bytes(0x1000 + (idx as u64 * 4), &insn.to_le_bytes());
        }

        assert!(step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0).is_ok());
        assert!(step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0).is_ok());

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0], (0x4000, 8, true, false));
        assert_eq!(seen[1], (0x4000, 8, false, false));
        assert_eq!(a64.x[1], a64.x[0]);
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

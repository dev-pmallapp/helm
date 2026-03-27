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
use helm_arch::aarch64::insn::{Instruction, Opcode};
use helm_arch::{aarch64_decode, aarch64_execute};
use helm_core::{AccessType, HartException, MemFault, MemInterface};
use helm_diag::sim_warn;
use helm_probe::{probe, BranchEvent, BranchKind, CpuProbes, CpuStepEvent, CpuFaultEvent, MemAccessEvent};
use helm_arch::aarch64::mmu::MmuFault;
use helm_plugin::HelmPluginRegistry;

use crate::address_space::HelmAddressSpace;
use helm_hw_intc::GicV3SharedState;
use std::sync::{Arc, Mutex};

/// FS-mode CPU state (per-core).
pub struct FsState {
    /// Whether an external IRQ is pending (set by GIC irq_line AtomicBool).
    pub irq_pending: bool,
    /// CPU executed WFI and is waiting for an interrupt before resuming.
    /// Cleared when `irq_pending` becomes true or a timer fires.
    pub wfi_idle: bool,
    /// Monotonic tick counter (incremented each instruction).
    pub tick: u64,
    /// Virtual-time scale factor: tick advances by this many per instruction.
    /// Default 1 gives cycle-accurate timing (62.5 MHz at 1 tick/insn).
    /// Higher values make delay loops and timer waits complete faster.
    pub tick_scale: u64,
    /// Software TLB — direct-mapped 256-entry VA→PA cache.
    pub tlb: Tlb,
    /// Small direct-mapped decode cache keyed by physical address + raw word.
    decode_cache: DecodeCache,
}

impl FsState {
    /// Create a new FS state.
    pub fn new() -> Self {
        Self {
            irq_pending: false,
            wfi_idle: false,
            tick: 0,
            tick_scale: 1,
            tlb: Tlb::new(),
            decode_cache: DecodeCache::new(),
        }
    }
}

const DECODE_CACHE_ENTRIES: usize = 4096;
const DECODE_CACHE_MASK: u64 = (DECODE_CACHE_ENTRIES as u64) - 1;

#[derive(Clone, Copy)]
struct DecodeCacheEntry {
    pa: u64,
    raw: u32,
    insn: Instruction,
    valid: bool,
}

impl Default for DecodeCacheEntry {
    fn default() -> Self {
        Self {
            pa: 0,
            raw: 0,
            insn: Instruction::zeroed(),
            valid: false,
        }
    }
}

struct DecodeCache {
    entries: Box<[DecodeCacheEntry; DECODE_CACHE_ENTRIES]>,
}

impl DecodeCache {
    fn new() -> Self {
        Self {
            entries: Box::new([DecodeCacheEntry::default(); DECODE_CACHE_ENTRIES]),
        }
    }

    #[inline]
    fn idx(pa: u64) -> usize {
        ((pa >> 2) & DECODE_CACHE_MASK) as usize
    }

    #[inline]
    fn lookup(&self, pa: u64, pc: u64) -> Option<(u32, Instruction)> {
        let entry = self.entries[Self::idx(pa)];
        if entry.valid && entry.pa == pa {
            let mut insn = entry.insn;
            insn.pc = pc;
            Some((entry.raw, insn))
        } else {
            None
        }
    }

    #[inline]
    fn insert(&mut self, pa: u64, raw: u32, insn: Instruction) {
        self.entries[Self::idx(pa)] = DecodeCacheEntry {
            pa,
            raw,
            insn,
            valid: true,
        };
    }

    #[inline]
    fn invalidate_range(&mut self, pa: u64, size: usize) {
        let start = pa & !0x3;
        let end = pa.saturating_add(size.saturating_sub(1) as u64);
        let mut cur = start;
        while cur <= end {
            self.entries[Self::idx(cur)].valid = false;
            cur = cur.saturating_add(4);
        }
    }
}

/// Memory wrapper that translates VA→PA using a snapshotted MMU config.
pub struct TranslatingMem<'a> {
    pub sys_mem: &'a mut HelmAddressSpace,
    mmu_cfg: MmuConfig,
    tlb: &'a mut Tlb,
    decode_cache: &'a mut DecodeCache,
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
    fn new(
        sys_mem: &'a mut HelmAddressSpace,
        mmu_cfg: MmuConfig,
        tlb: &'a mut Tlb,
        decode_cache: &'a mut DecodeCache,
    ) -> Self {
        Self { sys_mem, mmu_cfg, tlb, decode_cache }
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
        self.decode_cache.invalidate_range(pa, size);
        self.sys_mem.write(pa, size, val, ty)
    }
}

struct InstrumentedTranslatingMem<'a> {
    inner: TranslatingMem<'a>,
    records: [MemAccessRecord; 8],
    count: usize,
}

impl<'a> InstrumentedTranslatingMem<'a> {
    fn new(
        sys_mem: &'a mut HelmAddressSpace,
        mmu_cfg: MmuConfig,
        tlb: &'a mut Tlb,
        decode_cache: &'a mut DecodeCache,
    ) -> Self {
        Self {
            inner: TranslatingMem::new(sys_mem, mmu_cfg, tlb, decode_cache),
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
    sys_mem: &mut HelmAddressSpace,
    fs: &mut FsState,
    probes: &CpuProbes,
    plugins: &HelmPluginRegistry,
    vcpu_idx: usize,
    gicv3: Option<&Arc<Mutex<GicV3SharedState>>>,
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

    let (raw, insn) = if let Some((raw, insn)) = fs.decode_cache.lookup(fetch_pa, pc) {
        (raw, insn)
    } else {
        let raw = sys_mem
            .read(fetch_pa, 4, AccessType::Fetch)
            .map_err(|_| HartException::InstructionAccessFault { addr: pc })?
            as u32;
        let insn = match aarch64_decode(raw, pc) {
            Ok(insn) => insn,
            Err(_) => {
                return Err(HartException::IllegalInstruction { pc, raw });
            }
        };
        fs.decode_cache.insert(fetch_pa, raw, insn);
        (raw, insn)
    };

    // 4. Snapshot MMU config before execute (avoids borrow conflict on a64)
    let mmu_cfg = MmuConfig::from_arch(a64);

    let has_mem_probe = probes.mem.has_listeners();
    let has_post_step_probe = probes.post_step.has_listeners();
    let has_branch_probe = probes.branch.has_listeners();
    let has_fault_probe = probes.fault.has_listeners();
    let has_mem_callbacks = plugins.has_mem_callbacks();
    let has_insn_callbacks = plugins.has_insn_callbacks();
    let has_branch_callbacks = plugins.has_branch_callbacks();
    let has_fault_callbacks = plugins.has_fault_callbacks();

    let record_mem = has_mem_callbacks || has_mem_probe;
    // 5. Execute with translating memory (TLB shared between fetch and data accesses)
    let exec_result = if let Some(pc_written) = try_exec_gicv3_sysreg(&insn, a64, vcpu_idx, gicv3) {
        Ok(pc_written)
    } else if record_mem {
        let mut tmem = InstrumentedTranslatingMem::new(
            sys_mem,
            mmu_cfg,
            &mut fs.tlb,
            &mut fs.decode_cache,
        );
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
        let mut tmem = TranslatingMem::new(sys_mem, mmu_cfg, &mut fs.tlb, &mut fs.decode_cache);
        aarch64_execute(&insn, a64, &mut tmem)
    };

    match exec_result {
        Ok(pc_written) => {
            if !pc_written {
                a64.pc = a64.pc.wrapping_add(4);
            }
            if has_post_step_probe || has_insn_callbacks {
                let (fs_class, opcode_name, fs_is_stub) = crate::classify_aarch64_opcode(insn.opcode);
                probe!(probes.post_step, CpuStepEvent {
                    pc,
                    raw,
                    insn_class: crate::to_probe_class(fs_class),
                    is_stub: fs_is_stub,
                });
                if has_insn_callbacks {
                    plugins.fire_insn_exec(vcpu_idx, &helm_plugin::runtime::PluginInsnInfo {
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
            }
            if (has_branch_probe || has_branch_callbacks) && insn.is_branch() {
                let target = a64.pc;
                probe!(probes.branch, BranchEvent {
                    pc,
                    target,
                    taken: pc_written,
                    kind: BranchKind::DirectUncond,  // simplified in FS mode
                });
                if has_branch_callbacks {
                    plugins.fire_branch(vcpu_idx, &helm_plugin::runtime::BranchInfo {
                        pc,
                        target,
                        taken: pc_written,
                        kind: crate::classify_branch_kind(insn.opcode),
                    });
                }
            }
            if matches!(insn.opcode, Opcode::Brk) {
                sim_warn!(component="aarch64-brk", pc=pc,
                    "BRK #{} x0={:#x} lr={:#x}",
                    insn.imm, a64.x[0], a64.x[30]);
                if has_fault_callbacks {
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
            if has_fault_probe {
                probes.fault.notify(&CpuFaultEvent { pc, raw, kind: "data-abort" });
            }
            if has_fault_callbacks {
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
            if has_fault_probe {
                probes.fault.notify(&CpuFaultEvent { pc, raw, kind: "store-abort" });
            }
            if has_fault_callbacks {
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
            if has_fault_probe {
                probes.fault.notify(&CpuFaultEvent { pc, raw, kind: "data-abort" });
            }
            if has_fault_callbacks {
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
            if has_fault_probe {
                probes.fault.notify(&CpuFaultEvent { pc, raw, kind: "insn-abort" });
            }
            if has_fault_callbacks {
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
        Err(HartException::IllegalInstruction { pc, raw }) => {
            // Route undefined instructions through the kernel's exception
            // vector (architecturally correct: UNDEF -> synchronous exception).
            // EC=0 (Unknown reason), IL=1 (32-bit instruction length)
            if has_fault_probe {
                probes.fault.notify(&CpuFaultEvent { pc, raw, kind: "undef" });
            }
            if has_fault_callbacks {
                plugins.fire_fault(&helm_plugin::runtime::FaultInfo {
                    vcpu_idx,
                    pc,
                    raw,
                    kind: helm_plugin::runtime::FaultKind::IllegalInstruction,
                    message: format!("undefined instruction at {pc:#x} (raw={raw:#010x})"),
                    insn_count: fs.tick,
                    context: helm_plugin::runtime::ArchContext::Aarch64 {
                        x: a64.x,
                        sp: a64.sp,
                        pc: a64.pc,
                        nzcv: a64.nzcv,
                    },
                });
            }
            let syndrome = EC_UNKNOWN | (1 << 25);
            let vector_offset = if a64.current_el == 0 {
                SYNC_EL0_64
            } else if a64.spsel {
                SYNC_EL1_SP1
            } else {
                SYNC_EL1_SP0
            };
            exception::exception_entry_el1(a64, vector_offset, syndrome, 0);
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
    fs.tick += fs.tick_scale;

    // 8. Update virtual counter (used by MRS CNTVCT_EL0)
    a64.cntvct_el0 = fs.tick;

    Ok(())
}

fn try_exec_gicv3_sysreg(
    insn: &helm_arch::aarch64::insn::Instruction,
    a64: &mut Aarch64ArchState,
    vcpu_idx: usize,
    gicv3: Option<&Arc<Mutex<GicV3SharedState>>>,
) -> Option<bool> {
    match insn.opcode {
        Opcode::Mrs => {
            let shared = gicv3?;
            let encoded = insn.imm as u32;
            if !helm_hw_intc::gicv3::sysregs::is_icc_reg(encoded) {
                return None;
            }
            let mut shared = shared.lock().unwrap();
            let val = helm_hw_intc::gicv3::sysregs::icc_read(&mut shared, vcpu_idx, encoded)?;
            a64.write_x(insn.rd, val);
            Some(false)
        }
        Opcode::Msr => {
            let shared = gicv3?;
            let encoded = insn.imm as u32;
            if !helm_hw_intc::gicv3::sysregs::is_icc_reg(encoded) {
                return None;
            }
            let mut shared = shared.lock().unwrap();
            let val = a64.read_x(insn.rd);
            helm_hw_intc::gicv3::sysregs::icc_write(&mut shared, vcpu_idx, encoded, val)
                .then_some(false)
        }
        _ => None,
    }
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
    use helm_plugin::HelmPluginRegistry;
    use crate::FlatMem;
    use crate::address_space::HelmAddressSpace;
    use std::sync::{Arc, Mutex};

    fn make_fs_env() -> (Aarch64ArchState, HelmAddressSpace, FsState, CpuProbes, HelmPluginRegistry) {
        let mut a64 = Aarch64ArchState::new();
        a64.current_el = 1;
        a64.spsel = true;
        a64.sctlr_el1 = 0; // MMU disabled

        let ram = FlatMem::new(0, 0);
        let sys_mem = HelmAddressSpace::new(ram);
        let fs = FsState::new();
        let probes = CpuProbes::default();
        let plugins = HelmPluginRegistry::new();

        (a64, sys_mem, fs, probes, plugins)
    }

    #[test]
    fn irq_delivered_when_unmasked() {
        let (mut a64, mut sys_mem, mut fs, probes, plugins) = make_fs_env();
        a64.vbar_el1 = 0x1000;
        a64.pc = 0x2000;
        a64.daif = 0;

        fs.irq_pending = true;

        let result = step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0, None);
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

        let result = step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0, None);
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

        let result = step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0, None);
        assert!(result.is_ok());
        assert_eq!(a64.pc, 0x1004);
        assert_eq!(fs.tick, 1);
    }

    #[test]
    fn decode_cache_rechecks_raw_word_after_code_change() {
        let (mut a64, mut sys_mem, mut fs, probes, plugins) = make_fs_env();
        a64.pc = 0x1000;

        let nop: u32 = 0xD503201F;
        sys_mem.ram.load_bytes(0x1000, &nop.to_le_bytes());
        let result = step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0, None);
        assert!(result.is_ok());

        a64.pc = 0x1000;
        let undef: u32 = 0;
        sys_mem.ram.load_bytes(0x1000, &undef.to_le_bytes());
        let result = step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0, None);
        assert!(matches!(result, Err(HartException::IllegalInstruction { pc: 0x1000, raw: 0 })));
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

        assert!(step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0, None).is_ok());
        assert_eq!(a64.spsr_el1, 0xE11);

        assert!(step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0, None).is_ok());
        assert_eq!(a64.elr_el1, 0x2000);
        let result = step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0, None);
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

        let result = step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0, None);
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
        a64.x[2] = 0x4000;
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
            0xF9000040u32, // STR X0, [X2]
            0xF9400041u32, // LDR X1, [X2]
        ];
        for (idx, insn) in program.iter().enumerate() {
            sys_mem.ram.load_bytes(0x1000 + (idx as u64 * 4), &insn.to_le_bytes());
        }

        assert!(step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0, None).is_ok());
        assert!(step_aarch64_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins, 0, None).is_ok());

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

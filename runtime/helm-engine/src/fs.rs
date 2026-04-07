//! Full-system (FS) mode step loop for AArch64.
//!
//! Provides `step_aarch64_fs()` — a single instruction step that:
//! 1. Checks for pending IRQ (via GIC) and delivers exception if unmasked
//! 2. Translates PC via MMU → fetch → decode → execute
//! 3. Translates data accesses via MMU
//! 4. Checks generic timer against tick counter

use helm_arch::aarch64::arch_state::Aarch64ArchState;
use helm_arch::aarch64::exception::{self, *};
use helm_arch::aarch64::insn::Opcode;
use helm_arch::aarch64::mmu::MmuFault;
use helm_arch::aarch64::mmu::{self, MmuAccess, MmuConfig, Tlb};
use helm_arch::aarch64_execute;
use helm_core::{AccessType, HartException, MemFault, MemInterface};
use helm_diag::sim_warn;
use helm_plugin::HelmPluginRegistry;
use helm_probe::{probe, BranchEvent, CpuFaultEvent, CpuProbes, CpuStepEvent, MemAccessEvent};
use helm_timing::TimingModel;

use crate::aarch64_decode_cache::{Aarch64DecodeCache, DecodedAarch64Insn};
use crate::address_space::{drain_all_pci_bus_remaps, process_all_virtio_pci_pending, HelmAddressSpace};
use helm_hw_intc::{GicSharedState, GicV3SharedState};
use std::sync::atomic::{AtomicU32, Ordering};
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
    pub(crate) decode_cache: Aarch64DecodeCache,
    pub(crate) timing_mem_model: crate::TimingMemModel,
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
            decode_cache: Aarch64DecodeCache::new(),
            timing_mem_model: crate::TimingMemModel::new(crate::TimingMemModelConfig::default()),
        }
    }
}

// Low-address faults are almost always a real bug in the simulator rather than
// a recoverable guest condition. Log only a few to keep boot output usable.
static LOW_ADDR_ABORT_LOG_BUDGET: AtomicU32 = AtomicU32::new(8);

fn maybe_log_low_addr_abort(kind: &str, pc: u64, raw: u32, addr: u64, a64: &Aarch64ArchState) {
    if addr >= 0x1000 {
        return;
    }
    let remaining =
        LOW_ADDR_ABORT_LOG_BUDGET
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
    if remaining.is_err() {
        return;
    }
    sim_warn!(
        component = "aarch64-low-addr-abort",
        pc = pc,
        "{} addr={:#x} raw={:#010x} x0={:#x} x1={:#x} x20={:#x} x24={:#x} x26={:#x}",
        kind,
        addr,
        raw,
        a64.x[0],
        a64.x[1],
        a64.x[20],
        a64.x[24],
        a64.x[26]
    );
}

/// Memory wrapper that translates VA→PA using a snapshotted MMU config.
pub struct TranslatingMem<'a> {
    pub sys_mem: &'a mut HelmAddressSpace,
    mmu_cfg: MmuConfig,
    tlb: &'a mut Tlb,
    decode_cache: &'a mut Aarch64DecodeCache,
    gicv2: Option<Arc<Mutex<GicSharedState>>>,
    gicv3: Option<Arc<Mutex<GicV3SharedState>>>,
}

#[derive(Clone, Copy)]
struct MemAccessRecord {
    pc: u64,
    vaddr: u64,
    paddr: u64,
    size: u8,
    is_store: bool,
    is_atomic: bool,
    value_before: Option<u64>,
    value_after: Option<u64>,
}

impl Default for MemAccessRecord {
    fn default() -> Self {
        Self {
            pc: 0,
            vaddr: 0,
            paddr: 0,
            size: 0,
            is_store: false,
            is_atomic: false,
            value_before: None,
            value_after: None,
        }
    }
}

impl<'a> TranslatingMem<'a> {
    fn new(
        sys_mem: &'a mut HelmAddressSpace,
        mmu_cfg: MmuConfig,
        tlb: &'a mut Tlb,
        decode_cache: &'a mut Aarch64DecodeCache,
        gicv2: Option<Arc<Mutex<GicSharedState>>>,
        gicv3: Option<Arc<Mutex<GicV3SharedState>>>,
    ) -> Self {
        Self {
            sys_mem,
            mmu_cfg,
            tlb,
            decode_cache,
            gicv2,
            gicv3,
        }
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
    MemFault::PageFault {
        addr: fault.va,
        iss,
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
        self.decode_cache.invalidate_range(pa, size);
        let mmio = self.sys_mem.address_map.lookup(pa).is_some();
        self.sys_mem.write(pa, size, val, ty)?;
        if mmio {
            let _ = drain_all_pci_bus_remaps(self.sys_mem);
            let messages = process_all_virtio_pci_pending(self.sys_mem);
            deliver_pci_msi_messages(&messages, self.gicv2.as_ref(), self.gicv3.as_ref());
        }
        Ok(())
    }
}

struct InstrumentedTranslatingMem<'a> {
    inner: TranslatingMem<'a>,
    records: [MemAccessRecord; 8],
    count: usize,
    pc: u64,
}

impl<'a> InstrumentedTranslatingMem<'a> {
    fn new(
        sys_mem: &'a mut HelmAddressSpace,
        mmu_cfg: MmuConfig,
        tlb: &'a mut Tlb,
        decode_cache: &'a mut Aarch64DecodeCache,
        pc: u64,
        gicv2: Option<Arc<Mutex<GicSharedState>>>,
        gicv3: Option<Arc<Mutex<GicV3SharedState>>>,
    ) -> Self {
        Self {
            inner: TranslatingMem::new(sys_mem, mmu_cfg, tlb, decode_cache, gicv2, gicv3),
            records: [MemAccessRecord::default(); 8],
            count: 0,
            pc,
        }
    }

    fn push(&mut self, rec: MemAccessRecord) {
        if self.count < self.records.len() {
            self.records[self.count] = rec;
            self.count += 1;
        }
    }

    fn recorded(&self) -> &[MemAccessRecord] {
        &self.records[..self.count]
    }
}

impl<'a> MemInterface for InstrumentedTranslatingMem<'a> {
    fn read(&mut self, addr: u64, size: usize, ty: AccessType) -> Result<u64, MemFault> {
        let mmu_access = match ty {
            AccessType::Fetch => MmuAccess::Execute,
            _ => MmuAccess::Read,
        };
        let pa = self.inner.translate_va(addr, mmu_access)?;
        let val = self.inner.sys_mem.read(pa, size, ty)?;
        self.push(MemAccessRecord {
            pc: self.pc,
            vaddr: addr,
            paddr: pa,
            size: size as u8,
            is_store: false,
            is_atomic: ty == AccessType::Atomic,
            value_before: Some(val),
            value_after: None,
        });
        Ok(val)
    }

    fn write(&mut self, addr: u64, size: usize, val: u64, ty: AccessType) -> Result<(), MemFault> {
        let pa = self.inner.translate_va(addr, MmuAccess::Write)?;
        let old = self
            .inner
            .sys_mem
            .read(pa, size, AccessType::Load)
            .unwrap_or(0);
        self.inner.decode_cache.invalidate_range(pa, size);
        let mmio = self.inner.sys_mem.address_map.lookup(pa).is_some();
        self.inner.sys_mem.write(pa, size, val, ty)?;
        if mmio {
            let _ = drain_all_pci_bus_remaps(self.inner.sys_mem);
            let messages = process_all_virtio_pci_pending(self.inner.sys_mem);
            deliver_pci_msi_messages(&messages, self.inner.gicv2.as_ref(), self.inner.gicv3.as_ref());
        }
        self.push(MemAccessRecord {
            pc: self.pc,
            vaddr: addr,
            paddr: pa,
            size: size as u8,
            is_store: true,
            is_atomic: ty == AccessType::Atomic,
            value_before: Some(old),
            value_after: Some(val),
        });
        Ok(())
    }
}

fn deliver_pci_msi_messages(
    messages: &[(u64, u32)],
    gicv2: Option<&Arc<Mutex<GicSharedState>>>,
    gicv3: Option<&Arc<Mutex<GicV3SharedState>>>,
) {
    for &(_addr, data) in messages {
        let intid = data;
        if let Some(shared) = gicv3 {
            shared.lock().unwrap().pend_spi_edge(intid);
        } else if let Some(shared) = gicv2 {
            shared.lock().unwrap().pend_irq_edge(intid);
        }
    }
}

/// Execute one FS-mode AArch64 instruction.
///
/// Returns `Ok(())` on success, `Err` on exception (WFI, abort, etc.).
/// The caller should handle `WaitForInterrupt` by advancing the event queue.
pub fn step_aarch64_fs<T: TimingModel>(
    a64: &mut Aarch64ArchState,
    sys_mem: &mut HelmAddressSpace,
    fs: &mut FsState,
    timing: &mut T,
    probes: &CpuProbes,
    plugins: &HelmPluginRegistry,
    vcpu_idx: usize,
    gicv2: Option<&Arc<Mutex<GicSharedState>>>,
    gicv3: Option<&Arc<Mutex<GicV3SharedState>>>,
) -> Result<(), HartException> {
    // 1. Check for pending IRQ: deliver if unmasked
    if fs.irq_pending && (a64.daif & 0x2) == 0 {
        // DAIF bit 1 = I (IRQ mask). 0 = unmasked.
        let target_el = exception::route_physical_irq(a64);
        let vector_offset = exception::irq_vector_offset(a64, target_el);
        exception::exception_entry_with_offset(a64, target_el, vector_offset, 0, 0);
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
            let syndrome = ec | (1 << 25) | iss;
            let target_el = exception::route_sync_exception(a64, ec);
            exception::exception_entry(a64, target_el, syndrome, pc);
            // Don't return error — exception was delivered internally
            probe!(
                probes.fault,
                CpuFaultEvent {
                    pc,
                    raw: 0,
                    kind: "insn-abort"
                }
            );
            if plugins.has_fault_callbacks() {
                plugins.fire_fault(&helm_plugin::runtime::FaultInfo {
                    vcpu_idx,
                    pc,
                    raw: 0,
                    kind: helm_plugin::runtime::FaultKind::MemoryFault,
                    message: format!("instruction abort at {pc:#x}: {fault:?}"),
                    insn_count: fs.tick,
                    context: helm_plugin::runtime::ArchContext::Aarch64 {
                        x: a64.x,
                        sp: a64.current_sp(),
                        pc: a64.pc,
                        nzcv: a64.nzcv,
                    },
                });
            }
            return Ok(());
        }
    };

    let raw = sys_mem
        .read(fetch_pa, 4, AccessType::Fetch)
        .map_err(|_| HartException::InstructionAccessFault { addr: pc })? as u32;
    let decoded = if let Some(decoded) = fs.decode_cache.lookup(fetch_pa, raw) {
        decoded
    } else {
        let decoded = match DecodedAarch64Insn::decode(raw, pc) {
            Ok(decoded) => decoded,
            Err(_) => {
                return Err(HartException::IllegalInstruction { pc, raw });
            }
        };
        fs.decode_cache.insert(fetch_pa, decoded);
        decoded
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

    let record_mem = has_mem_callbacks
        || has_mem_probe
        || (T::model_caps().needs_mem_access_timing && decoded.records_mem_access);
    // 5. Execute with translating memory (TLB shared between fetch and data accesses)
    let exec_result =
        if let Some(pc_written) = try_exec_gicv3_sysreg(&decoded.insn, a64, vcpu_idx, gicv3) {
            Ok(pc_written)
        } else if let Some(pc_written) = try_exec_at_instruction(&decoded.insn, a64, sys_mem) {
            Ok(pc_written)
        } else if record_mem {
            let mut tmem = InstrumentedTranslatingMem::new(
                sys_mem,
                mmu_cfg,
                &mut fs.tlb,
                &mut fs.decode_cache,
                pc,
                gicv2.cloned(),
                gicv3.cloned(),
            );
            let exec_result = aarch64_execute(&decoded.insn, a64, &mut tmem);
            for rec in tmem.recorded() {
                timing.on_mem_access(&crate::estimate_timing_mem_access(
                    &mut fs.timing_mem_model,
                    rec.vaddr,
                    rec.size as usize,
                    rec.is_store,
                    rec.is_atomic,
                ));
                plugins.fire_mem_access(
                    vcpu_idx,
                    &helm_plugin::runtime::MemInfo {
                        pc: rec.pc,
                        raw,
                        opcode_name: decoded.opcode_name,
                        class: decoded.class,
                        vaddr: rec.vaddr,
                        paddr: rec.paddr,
                        size: rec.size,
                        is_store: rec.is_store,
                        is_atomic: rec.is_atomic,
                        value_before: rec.value_before,
                        value_after: rec.value_after,
                    },
                );
                probe!(
                    probes.mem,
                    MemAccessEvent {
                        addr: rec.vaddr,
                        size: rec.size,
                        is_store: rec.is_store,
                        pc,
                    }
                );
            }
            exec_result
        } else {
            let mut tmem = TranslatingMem::new(
                sys_mem,
                mmu_cfg,
                &mut fs.tlb,
                &mut fs.decode_cache,
                gicv2.cloned(),
                gicv3.cloned(),
            );
            aarch64_execute(&decoded.insn, a64, &mut tmem)
        };

    match exec_result {
        Ok(pc_written) => {
            if !pc_written {
                a64.pc = a64.pc.wrapping_add(4);
            }
            if decoded.is_branch {
                let target = a64.pc;
                timing.on_branch(pc_written, decoded.predict_branch(pc, target));
            }
            timing.on_insn(&crate::aarch64_timing_info_for::<T>(&decoded, pc));
            if has_post_step_probe || has_insn_callbacks {
                probe!(
                    probes.post_step,
                    CpuStepEvent {
                        pc,
                        raw,
                        insn_class: decoded.probe_class,
                        is_stub: decoded.is_stub,
                    }
                );
                if has_insn_callbacks {
                    plugins.fire_insn_exec(
                        vcpu_idx,
                        &helm_plugin::runtime::PluginInsnInfo {
                            pc,
                            raw,
                            size: 4,
                            class: decoded.class,
                            opcode_name: decoded.opcode_name,
                            is_stub: decoded.is_stub,
                            context: helm_plugin::runtime::ArchContext::Aarch64 {
                                x: a64.x,
                                sp: a64.current_sp(),
                                pc: a64.pc,
                                nzcv: a64.nzcv,
                            },
                        },
                    );
                }
            }
            if (has_branch_probe || has_branch_callbacks) && decoded.is_branch {
                let target = a64.pc;
                probe!(
                    probes.branch,
                    BranchEvent {
                        pc,
                        target,
                        taken: pc_written,
                        kind: decoded.probe_branch_kind,
                    }
                );
                if has_branch_callbacks {
                    plugins.fire_branch(
                        vcpu_idx,
                        &helm_plugin::runtime::BranchInfo {
                            pc,
                            target,
                            taken: pc_written,
                            kind: decoded.plugin_branch_kind,
                        },
                    );
                }
            }
            if matches!(decoded.insn.opcode, Opcode::Brk) {
                sim_warn!(
                    component = "aarch64-brk",
                    pc = pc,
                    "BRK #{} x0={:#x} lr={:#x}",
                    decoded.insn.imm,
                    a64.x[0],
                    a64.x[30]
                );
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
                            sp: a64.current_sp(),
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
            probe!(
                probes.fault,
                CpuFaultEvent {
                    pc,
                    raw,
                    kind: "svc"
                }
            );
            // SVC from EL0 in FS mode
            let syndrome = EC_SVC_A64 | (1 << 25) | (decoded.insn.imm as u32 & 0xFFFF);
            let target_el = exception::route_sync_exception(a64, EC_SVC_A64);
            exception::exception_entry(a64, target_el, syndrome, 0);
        }
        Err(HartException::LoadAccessFault { addr }) => {
            maybe_log_low_addr_abort("load-abort", pc, raw, addr, a64);
            if has_fault_probe {
                probes.fault.notify(&CpuFaultEvent {
                    pc,
                    raw,
                    kind: "data-abort",
                });
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
                        sp: a64.current_sp(),
                        pc: a64.pc,
                        nzcv: a64.nzcv,
                    },
                });
            }
            let ec = if a64.current_el == 0 {
                EC_DATA_ABORT_EL0
            } else {
                EC_DATA_ABORT_EL1
            };
            let iss = 0b000101; // Translation fault L1
            let syndrome = ec | (1 << 25) | iss;
            let target_el = exception::route_sync_exception(a64, ec);
            exception::exception_entry(a64, target_el, syndrome, addr);
        }
        Err(HartException::StoreAccessFault { addr }) => {
            maybe_log_low_addr_abort("store-abort", pc, raw, addr, a64);
            if has_fault_probe {
                probes.fault.notify(&CpuFaultEvent {
                    pc,
                    raw,
                    kind: "store-abort",
                });
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
                        sp: a64.current_sp(),
                        pc: a64.pc,
                        nzcv: a64.nzcv,
                    },
                });
            }
            let ec = if a64.current_el == 0 {
                EC_DATA_ABORT_EL0
            } else {
                EC_DATA_ABORT_EL1
            };
            let iss = (1 << 6) | 0b000101; // WnR=1, Translation fault L1
            let syndrome = ec | (1 << 25) | iss;
            let target_el = exception::route_sync_exception(a64, ec);
            exception::exception_entry(a64, target_el, syndrome, addr);
        }
        Err(HartException::DataAbort { addr, iss }) => {
            maybe_log_low_addr_abort("data-abort", pc, raw, addr, a64);
            if has_fault_probe {
                probes.fault.notify(&CpuFaultEvent {
                    pc,
                    raw,
                    kind: "data-abort",
                });
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
                        sp: a64.current_sp(),
                        pc: a64.pc,
                        nzcv: a64.nzcv,
                    },
                });
            }
            let ec = if a64.current_el == 0 {
                EC_DATA_ABORT_EL0
            } else {
                EC_DATA_ABORT_EL1
            };
            let syndrome = ec | (1 << 25) | iss;
            let target_el = exception::route_sync_exception(a64, ec);
            exception::exception_entry(a64, target_el, syndrome, addr);
        }
        Err(HartException::InstructionAbort { addr, iss }) => {
            if has_fault_probe {
                probes.fault.notify(&CpuFaultEvent {
                    pc,
                    raw,
                    kind: "insn-abort",
                });
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
                        sp: a64.current_sp(),
                        pc: a64.pc,
                        nzcv: a64.nzcv,
                    },
                });
            }
            let ec = if a64.current_el == 0 {
                EC_INSN_ABORT_EL0
            } else {
                EC_INSN_ABORT_EL1
            };
            let syndrome = ec | (1 << 25) | iss;
            let target_el = exception::route_sync_exception(a64, ec);
            exception::exception_entry(a64, target_el, syndrome, addr);
        }
        Err(HartException::IllegalInstruction { pc, raw }) => {
            // Route undefined instructions through the kernel's exception
            // vector (architecturally correct: UNDEF -> synchronous exception).
            // EC=0 (Unknown reason), IL=1 (32-bit instruction length)
            if has_fault_probe {
                probes.fault.notify(&CpuFaultEvent {
                    pc,
                    raw,
                    kind: "undef",
                });
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
                        sp: a64.current_sp(),
                        pc: a64.pc,
                        nzcv: a64.nzcv,
                    },
                });
            }
            let syndrome = EC_UNKNOWN | (1 << 25);
            let target_el = exception::route_sync_exception(a64, EC_UNKNOWN);
            exception::exception_entry(a64, target_el, syndrome, 0);
        }
        Err(e) => return Err(e),
    }

    // 6. Invalidate software TLB if any TLBI/DC/IC instruction was executed.
    // Linux always issues TLBI after modifying page tables; honouring it is
    // required for correctness even in a functional (no-cache) simulation.
    if a64.tlb_flush_pending {
        if let Some(va) = a64.tlb_flush_va.take() {
            fs.tlb.invalidate_va(va);
        } else {
            fs.tlb.flush();
        }
        a64.tlb_flush_pending = false;
    }

    // 7. Advance tick counter.
    fs.tick += effective_tick_step(a64, fs);

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

/// Handle AT S1E1R/W instructions using physical memory (bypasses MMU).
///
/// The AT instruction performs a page table walk using physical addresses
/// (the same as `mmu::translate`). It must use `sys_mem` (raw physical memory)
/// NOT `TranslatingMem` — using TranslatingMem would cause infinite recursion
/// (AT → translate → mem.read → translate → ...).
///
/// Returns `Some(false)` if the instruction was an AT, `None` otherwise.
fn try_exec_at_instruction(
    insn: &helm_arch::aarch64::insn::Instruction,
    a64: &mut Aarch64ArchState,
    sys_mem: &mut HelmAddressSpace,
) -> Option<bool> {
    if insn.opcode != Opcode::Sys {
        return None;
    }
    let raw = insn.raw;
    let op0 = (raw >> 19) & 0x3;
    let crn = (raw >> 12) & 0xF;
    let crm = (raw >> 8) & 0xF;
    let op2 = (raw >> 5) & 0x7;
    let rt = raw & 0x1F;

    // AT: op0 in {0b00, 0b01}, CRn=0b0111, CRm=0b1000
    // S1E1R/W: op0=0b00. S1E0R/W: op0=0b00 (op1 differs). S12E1/S12E0: op0=0b01.
    if !(crn == 0b0111 && crm == 0b1000 && op0 <= 0b01) {
        return None;
    }

    let va = a64.read_x(rt);
    if !a64.mmu_enabled() {
        a64.par_el1 = va & 0x0000_FFFF_FFFF_F000;
    } else {
        let access = if op2 & 1 != 0 {
            mmu::MmuAccess::Write
        } else {
            mmu::MmuAccess::Read
        };
        // Walk page tables using physical memory — no TLB, no MMU wrapping.
        match mmu::translate(a64, va, access, sys_mem, None) {
            Ok(result) => {
                a64.par_el1 = result.pa & 0x0000_FFFF_FFFF_F000;
            }
            Err(_fault) => {
                // The page table walk failed. On real hardware AT always
                // performs a fresh walk (never uses the TLB) so it would
                // succeed for any VA the kernel has mapped in its page
                // tables. Our walker can fail during early boot when the
                // kernel's `init_pg_dir` covers only a subset of RAM.
                //
                // The kernel uses AT inside `is_spurious_el1_translation_fault()`
                // to decide whether a data/instruction abort is real or stale-TLB.
                // If AT reports failure (PAR_EL1.F=1), the kernel treats the
                // fault as genuine and enters the abort handler — which during
                // early init causes a hang or panic.
                //
                // Strategy: derive PAGE_OFFSET from TCR_EL1.T1SZ to convert
                // the kernel VA to a candidate PA, then accept it if it lands
                // in the RAM region. This works for any VA_BITS configuration
                // (39, 48, 52, ...) because T1SZ fully encodes the split.
                // If the VA doesn't map to RAM, report the real fault status
                // so the kernel can handle truly invalid accesses.
                let ram_base: u64 = 0x4000_0000;
                let ram_end: u64 = ram_base + sys_mem.ram.size_bytes;
                let t1sz = ((a64.tcr_el1 >> 16) & 0x3F) as u32;
                let va_bits = 64u32.saturating_sub(t1sz).max(25);
                let page_offset: u64 = (!0u64) << va_bits;
                let pa_est = va.wrapping_sub(page_offset).wrapping_add(ram_base);
                if pa_est >= ram_base && pa_est < ram_end {
                    a64.par_el1 = pa_est & 0x0000_FFFF_FFFF_F000;
                } else {
                    let fst = _fault.fault_status_code_pub();
                    a64.par_el1 = 1 | ((fst as u64) << 1);
                }
            }
        }
    }
    Some(false)
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
    if p_cond {
        a64.cntp_ctl_el0 |= 4;
    } else {
        a64.cntp_ctl_el0 &= !4;
    }
    let p_fire = p_cond && (p_ctl & 2 == 0); // ENABLE && deadline && !IMASK

    // Virtual timer (CNTV): INTID 27
    let v_ctl = a64.cntv_ctl_el0;
    let v_cond = (v_ctl & 1 != 0) && a64.cntv_cval_el0 <= fs.tick;
    if v_cond {
        a64.cntv_ctl_el0 |= 4;
    } else {
        a64.cntv_ctl_el0 &= !4;
    }
    let v_fire = v_cond && (v_ctl & 2 == 0);

    (p_fire, v_fire)
}

#[inline]
fn effective_tick_step(a64: &Aarch64ArchState, fs: &FsState) -> u64 {
    let phys_timer_live = (a64.cntp_ctl_el0 & 1 != 0) && (a64.cntp_ctl_el0 & 2 == 0);
    let virt_timer_live = (a64.cntv_ctl_el0 & 1 != 0) && (a64.cntv_ctl_el0 & 2 == 0);

    // Global tick scaling is safe for early boot delay loops, but once the
    // guest has a live generic timer it can outrun the timer IRQ handler and
    // corrupt the kernel's interrupt return path. Fall back to the base rate
    // while timer IRQ delivery is active.
    if phys_timer_live || virt_timer_live {
        1
    } else {
        fs.tick_scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address_space::HelmAddressSpace;
    use crate::FlatMem;
    use helm_arch::aarch64::insn::Opcode;
    use helm_core::MemInterface;
    use helm_hw_intc::build_gicv2_mp;
    use helm_hw_pci::{config::PciConfigSpace, Bdf, PciBus, PciEndpoint};
    use helm_hw_virtio::pci::build_virtio_pci_rng_pair;
    use helm_plugin::HelmPluginRegistry;
    use helm_timing::VirtualTiming;
    use std::sync::{Arc, Mutex};

    fn make_fs_env() -> (
        Aarch64ArchState,
        HelmAddressSpace,
        FsState,
        CpuProbes,
        HelmPluginRegistry,
    ) {
        let mut a64 = Aarch64ArchState::new();
        a64.current_el = 1;
        a64.spsel = true;
        a64.sctlr_el1 = 0; // MMU disabled

        let ram = FlatMem::new(0, 0x40_0000);
        let sys_mem = HelmAddressSpace::new(ram);
        let fs = FsState::new();
        let probes = CpuProbes::default();
        let plugins = HelmPluginRegistry::new();

        (a64, sys_mem, fs, probes, plugins)
    }

    fn step_fs(
        a64: &mut Aarch64ArchState,
        sys_mem: &mut HelmAddressSpace,
        fs: &mut FsState,
        probes: &CpuProbes,
        plugins: &HelmPluginRegistry,
    ) -> Result<(), HartException> {
        let mut timing = VirtualTiming::default();
        step_aarch64_fs(a64, sys_mem, fs, &mut timing, probes, plugins, 0, None, None)
    }

    fn store_u64(sys_mem: &mut HelmAddressSpace, addr: u64, value: u64) {
        sys_mem.ram.load_bytes(addr, &value.to_le_bytes());
    }

    fn map_l3_page(
        sys_mem: &mut HelmAddressSpace,
        root: u64,
        l2_table: u64,
        l3_table: u64,
        va: u64,
        pa: u64,
        leaf_extra: u64,
    ) {
        let l1_index = ((va >> 30) & 0x1FF) * 8;
        let l2_index = ((va >> 21) & 0x1FF) * 8;
        let l3_index = ((va >> 12) & 0x1FF) * 8;
        store_u64(sys_mem, root + l1_index, l2_table | 0x3);
        store_u64(sys_mem, l2_table + l2_index, l3_table | 0x3);
        store_u64(
            sys_mem,
            l3_table + l3_index,
            (pa & !0xFFF) | leaf_extra | 0x3,
        );
    }

    struct TestPciEndpoint {
        config: PciConfigSpace,
        vendor: u16,
        device: u16,
        class: u32,
    }

    impl TestPciEndpoint {
        fn new(vendor_id: u16, device_id: u16, class_code: u32) -> Self {
            Self {
                config: PciConfigSpace::new(vendor_id, device_id, class_code, 0x00),
                vendor: vendor_id,
                device: device_id,
                class: class_code,
            }
        }

        fn with_bar0(mut self, base: u32, size: u32) -> Self {
            self.config.set_bar_size(0, size);
            self.config.write(0x10, 4, base);
            self
        }
    }

    impl PciEndpoint for TestPciEndpoint {
        fn config_read(&self, offset: u16, size: usize) -> u32 {
            let off = offset as usize;
            match size {
                1 => self.config.data_ref().get(off).copied().unwrap_or(0) as u32,
                2 => self
                    .config
                    .data_ref()
                    .get(off..off + 2)
                    .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]) as u32)
                    .unwrap_or(0),
                4 => self
                    .config
                    .data_ref()
                    .get(off..off + 4)
                    .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                    .unwrap_or(0),
                _ => 0,
            }
        }

        fn config_write(&mut self, offset: u16, size: usize, val: u32) {
            self.config.write(offset, size, val);
        }

        fn vendor_id(&self) -> u16 {
            self.vendor
        }

        fn device_id(&self) -> u16 {
            self.device
        }

        fn class_code(&self) -> u32 {
            self.class
        }

        fn bar_base(&self, bar_index: u8) -> Option<u64> {
            self.config.bar_address(bar_index as usize)
        }

        fn bar_size(&self, bar_index: u8) -> Option<u64> {
            self.config.bar_size(bar_index as usize)
        }
    }

    struct MockBarDevice {
        last_write_offset: u64,
        last_write_val: u64,
    }

    impl MockBarDevice {
        fn new() -> Self {
            Self {
                last_write_offset: u64::MAX,
                last_write_val: 0,
            }
        }
    }

    impl helm_devices::Device for MockBarDevice {
        fn read(&mut self, _offset: u64, _size: usize) -> u64 {
            0
        }

        fn write(&mut self, offset: u64, _size: usize, val: u64) {
            self.last_write_offset = offset;
            self.last_write_val = val;
        }

        fn region_size(&self) -> u64 {
            0x1000
        }
    }

    #[test]
    fn irq_delivered_when_unmasked() {
        let (mut a64, mut sys_mem, mut fs, probes, plugins) = make_fs_env();
        a64.vbar_el1 = 0x1000;
        a64.pc = 0x2000;
        a64.daif = 0;

        fs.irq_pending = true;

        let result = step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins);
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

        let result = step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins);
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

        let result = step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins);
        assert!(result.is_ok());
        assert_eq!(a64.pc, 0x1004);
        assert_eq!(fs.tick, 1);
    }

    #[test]
    fn translating_mem_write_auto_drains_pci_bar_remaps() {
        const ECAM_BASE: u64 = 0x3000_0000;
        const BAR0_BASE: u64 = 0x0A00_0000;
        const NEW_BAR0_BASE: u64 = 0x0B00_0000;

        let mut sys_mem = HelmAddressSpace::new(FlatMem::new(0, 0));
        let mut pci_bus = PciBus::new("pci0");
        let endpoint =
            TestPciEndpoint::new(0x1AF4, 0x1001, 0x010000).with_bar0(BAR0_BASE as u32, 0x1000);
        pci_bus
            .attach_endpoint(Bdf::new(0, 1, 0), Box::new(endpoint))
            .unwrap();
        sys_mem.add_device(ECAM_BASE, Box::new(pci_bus));

        let bar_dev_idx = sys_mem.add_device(BAR0_BASE, Box::new(MockBarDevice::new()));
        assert!(sys_mem.register_pci_bar_region(0, 1, 0, 0, bar_dev_idx, BAR0_BASE, 0x1000, 0));

        let mut tlb = Tlb::new();
        let mut decode_cache = Aarch64DecodeCache::new();
        let a64 = Aarch64ArchState::new();
        let mut mem = TranslatingMem::new(
            &mut sys_mem,
            MmuConfig::from_arch(&a64),
            &mut tlb,
            &mut decode_cache,
            None,
            None,
        );

        let ecam_bar0 = ECAM_BASE + (1u64 << 15) + 0x10;
        mem.write(ecam_bar0, 4, NEW_BAR0_BASE, AccessType::Store)
            .unwrap();

        assert!(mem.sys_mem.address_map.lookup(BAR0_BASE).is_none());
        assert!(mem.sys_mem.address_map.lookup(NEW_BAR0_BASE).is_some());

        mem.write(NEW_BAR0_BASE + 0x20, 4, 0x44, AccessType::Store)
            .unwrap();
        let bar_dev = mem
            .sys_mem
            .device_as_mut::<MockBarDevice>(bar_dev_idx)
            .unwrap();
        assert_eq!(bar_dev.last_write_offset, 0x20);
        assert_eq!(bar_dev.last_write_val, 0x44);
    }

    #[test]
    fn translating_mem_write_processes_standard_virtio_pci_queue_notify() {
        const BAR0_BASE: u64 = 0x0A00_0000;
        const BAR4_BASE: u64 = BAR0_BASE + 0x1000;
        const DESC_BASE: u64 = 0x2000;
        const AVAIL_BASE: u64 = 0x2200;
        const USED_BASE: u64 = 0x2400;
        const DATA_BASE: u64 = 0x2800;
        const QUEUE_SIZE: u16 = 8;

        const REG_QUEUE_SIZE_AND_VECTOR: u64 = 0x18;
        const REG_QUEUE_ENABLE_AND_NOTIFY_OFF: u64 = 0x1C;
        const REG_QUEUE_DESC_LOW: u64 = 0x20;
        const REG_QUEUE_DRIVER_LOW: u64 = 0x28;
        const REG_QUEUE_DEVICE_LOW: u64 = 0x30;
        const REG_MSIX_CONFIG_AND_QUEUE_COUNT: u64 = 0x10;
        const NOTIFY_OFFSET: u64 = 0x40;
        const ISR_OFFSET: u64 = 0x38;
        const MSIX_CAP_OFFSET: u64 = 0x90;
        const MSIX_CONTROL_OFFSET: u64 = 0x92;
        const MSIX_ENABLE_BIT: u64 = 1 << 15;
        const MSI_INTID: u32 = 65;

        let mut sys_mem = HelmAddressSpace::new(FlatMem::new(0, 0x40_0000));
        let mut pci_bus = PciBus::new("pci0");
        let (endpoint, bar0, bar4) = build_virtio_pci_rng_pair(BAR0_BASE, 0x1234_5678).unwrap();
        pci_bus
            .attach_endpoint(Bdf::new(0, 1, 0), Box::new(endpoint))
            .unwrap();
        sys_mem.add_device(0x3000_0000, Box::new(pci_bus));
        sys_mem.add_device(BAR0_BASE, Box::new(bar0));
        sys_mem.add_device(BAR4_BASE, Box::new(bar4));

        let (_gicd, _giccs, irq_lines, gic_state) = build_gicv2_mp(128, 1);
        {
            let mut gic = gic_state.lock().unwrap();
            gic.dist.dist_ctlr = 1;
            gic.cpus[0].cpu_ctlr = 1;
            gic.dist.enabled[(MSI_INTID / 32) as usize] |= 1u32 << (MSI_INTID & 31);
            gic.dist.targets[MSI_INTID as usize] = 1;
            gic.dist.priority[MSI_INTID as usize] = 0x40;
        }

        // Single write-only descriptor for 32 bytes of entropy.
        sys_mem.write(DESC_BASE, 8, DATA_BASE, AccessType::Store).unwrap();
        sys_mem.write(DESC_BASE + 8, 4, 32, AccessType::Store).unwrap();
        sys_mem.write(DESC_BASE + 12, 2, 0x2, AccessType::Store).unwrap();
        sys_mem.write(DESC_BASE + 14, 2, 0, AccessType::Store).unwrap();
        sys_mem.write(AVAIL_BASE + 4, 2, 0, AccessType::Store).unwrap();
        sys_mem.write(AVAIL_BASE + 2, 2, 1, AccessType::Store).unwrap();

        let mut tlb = Tlb::new();
        let mut decode_cache = Aarch64DecodeCache::new();
        let a64 = Aarch64ArchState::new();
        {
            let mut mem = TranslatingMem::new(
                &mut sys_mem,
                MmuConfig::from_arch(&a64),
                &mut tlb,
                &mut decode_cache,
                None,
                None,
            );

            // Program MSI-X: enable, config vector=0, queue0 vector=1, vector1 masked,
            // vector1 message data = MSI_INTID.
            mem.write(
                0x3000_0000 + (1u64 << 15) + MSIX_CONTROL_OFFSET,
                2,
                MSIX_ENABLE_BIT,
                AccessType::Store,
            )
            .unwrap();
            let msix_cap = mem
                .read(
                    0x3000_0000 + (1u64 << 15) + MSIX_CAP_OFFSET,
                    1,
                    AccessType::Load,
                )
                .unwrap();
            assert_eq!(msix_cap, 0x11);
            mem.write(BAR4_BASE + 0x18, 4, MSI_INTID as u64, AccessType::Store)
                .unwrap();
            mem.write(BAR4_BASE + 0x1C, 4, 0, AccessType::Store).unwrap();
            mem.write(
                BAR0_BASE + REG_MSIX_CONFIG_AND_QUEUE_COUNT,
                4,
                0,
                AccessType::Store,
            )
            .unwrap();
            mem.write(BAR0_BASE + REG_QUEUE_SIZE_AND_VECTOR, 4, QUEUE_SIZE as u64, AccessType::Store)
                .unwrap();
            mem.write(
                BAR0_BASE + REG_QUEUE_SIZE_AND_VECTOR,
                4,
                QUEUE_SIZE as u64 | (1u64 << 16),
                AccessType::Store,
            )
            .unwrap();
            mem.write(BAR0_BASE + REG_QUEUE_DESC_LOW, 4, DESC_BASE, AccessType::Store)
                .unwrap();
            mem.write(BAR0_BASE + REG_QUEUE_DRIVER_LOW, 4, AVAIL_BASE, AccessType::Store)
                .unwrap();
            mem.write(BAR0_BASE + REG_QUEUE_DEVICE_LOW, 4, USED_BASE, AccessType::Store)
                .unwrap();
            mem.gicv2 = Some(Arc::clone(&gic_state));
            mem.write(BAR0_BASE + REG_QUEUE_ENABLE_AND_NOTIFY_OFF, 4, 1, AccessType::Store)
                .unwrap();
            mem.write(BAR0_BASE + NOTIFY_OFFSET, 4, 0, AccessType::Store)
                .unwrap();
            let isr = mem.read(BAR0_BASE + ISR_OFFSET, 4, AccessType::Load).unwrap();
            assert_eq!(isr, 1);
            let isr_cleared = mem.read(BAR0_BASE + ISR_OFFSET, 4, AccessType::Load).unwrap();
            assert_eq!(isr_cleared, 0);
        }

        let used_idx = sys_mem.read(USED_BASE + 2, 2, AccessType::Load).unwrap();
        assert_eq!(used_idx, 1);
        let used_len = sys_mem.read(USED_BASE + 8, 4, AccessType::Load).unwrap();
        assert_eq!(used_len, 32);
        let any_nonzero = (0..32).any(|i| {
            sys_mem
                .read(DATA_BASE + i, 1, AccessType::Load)
                .unwrap()
                != 0
        });
        assert!(any_nonzero, "guest entropy buffer should be populated");
        assert!(
            irq_lines[0].load(Ordering::Relaxed),
            "MSI-X queue notification should raise a real GIC irq line"
        );
    }

    #[test]
    fn decode_cache_rechecks_raw_word_after_code_change() {
        let (mut a64, mut sys_mem, mut fs, probes, plugins) = make_fs_env();
        a64.pc = 0x1000;

        let nop: u32 = 0xD503201F;
        sys_mem.ram.load_bytes(0x1000, &nop.to_le_bytes());
        let result = step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins);
        assert!(result.is_ok());

        a64.pc = 0x1000;
        let undef: u32 = 0;
        sys_mem.ram.load_bytes(0x1000, &undef.to_le_bytes());
        let result = step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins);
        assert!(matches!(
            result,
            Err(HartException::IllegalInstruction { pc: 0x1000, raw: 0 })
        ));
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
            sys_mem
                .ram
                .load_bytes(0x1000 + (idx as u64 * 4), &insn.to_le_bytes());
        }

        assert!(step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins).is_ok());
        assert_eq!(a64.spsr_el1, 0xE11);

        assert!(step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins).is_ok());
        assert_eq!(a64.elr_el1, 0x2000);
        let result = step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins);
        assert!(result.is_ok());
        assert_eq!(a64.pc, 0x2000);
        // SPSR_EL1 = 0xE11: bits[3:2]=0b00 → EL0, bit[0]=1 → SPSel=1
        assert_eq!(a64.current_el, 0);
        assert!(a64.spsel);
    }

    #[test]
    fn fs_step_after_eret_fetches_from_user_ttbr0_mapping() {
        let (mut a64, mut sys_mem, mut fs, probes, plugins) = make_fs_env();
        let kernel_va = 0xFFFF_FF80_0000_1000u64;
        let kernel_pa = 0x100_000u64;
        let user_va = 0x0000_0000_0000_4000u64;
        let user_pa = 0x120_000u64;

        a64.pc = kernel_va;
        a64.x[0] = 0xE11;
        a64.x[30] = user_va;
        a64.sctlr_el1 = 1;
        a64.tcr_el1 = 25 | (25 << 16);
        a64.ttbr1_el1 = 0x10_000;
        a64.ttbr0_el1 = 0x40_000;

        map_l3_page(
            &mut sys_mem,
            a64.ttbr1_el1,
            0x11_000,
            0x12_000,
            kernel_va,
            kernel_pa,
            1 << 10,
        );
        map_l3_page(
            &mut sys_mem,
            a64.ttbr0_el1,
            0x41_000,
            0x42_000,
            user_va,
            user_pa,
            (1 << 10) | (0b01 << 6),
        );

        let program = [
            0xD518_4000u32, // msr spsr_el1, x0
            0xD518_403Eu32, // msr elr_el1, x30
            0xD69F_03E0u32, // eret
        ];
        for (idx, insn) in program.iter().enumerate() {
            sys_mem
                .ram
                .load_bytes(kernel_pa + (idx as u64 * 4), &insn.to_le_bytes());
        }
        let nop: u32 = 0xD503_201F;
        sys_mem.ram.load_bytes(user_pa, &nop.to_le_bytes());

        assert!(step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins).is_ok());
        assert!(step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins).is_ok());
        assert!(step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins).is_ok());
        assert_eq!(a64.current_el, 0);
        assert_eq!(a64.pc, user_va);

        assert!(step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins).is_ok());
        assert_eq!(a64.pc, user_va + 4);
    }

    #[test]
    fn fs_step_after_eret_traps_on_user_uxn_fetch() {
        let (mut a64, mut sys_mem, mut fs, probes, plugins) = make_fs_env();
        let kernel_va = 0xFFFF_FF80_0000_1000u64;
        let kernel_pa = 0x100_000u64;
        let user_va = 0x0000_0000_0000_4000u64;
        let user_pa = 0x120_000u64;

        a64.pc = kernel_va;
        a64.x[0] = 0xE11;
        a64.x[30] = user_va;
        a64.vbar_el1 = 0x80_000;
        a64.sctlr_el1 = 1;
        a64.tcr_el1 = 25 | (25 << 16);
        a64.ttbr1_el1 = 0x10_000;
        a64.ttbr0_el1 = 0x40_000;

        map_l3_page(
            &mut sys_mem,
            a64.ttbr1_el1,
            0x11_000,
            0x12_000,
            kernel_va,
            kernel_pa,
            1 << 10,
        );
        map_l3_page(
            &mut sys_mem,
            a64.ttbr0_el1,
            0x41_000,
            0x42_000,
            user_va,
            user_pa,
            (1 << 10) | (0b01 << 6) | (1u64 << 54),
        );

        let program = [
            0xD518_4000u32, // msr spsr_el1, x0
            0xD518_403Eu32, // msr elr_el1, x30
            0xD69F_03E0u32, // eret
        ];
        for (idx, insn) in program.iter().enumerate() {
            sys_mem
                .ram
                .load_bytes(kernel_pa + (idx as u64 * 4), &insn.to_le_bytes());
        }
        let nop: u32 = 0xD503_201F;
        sys_mem.ram.load_bytes(user_pa, &nop.to_le_bytes());

        assert!(step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins).is_ok());
        assert!(step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins).is_ok());
        assert!(step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins).is_ok());
        assert_eq!(a64.current_el, 0);
        assert_eq!(a64.pc, user_va);

        assert!(step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins).is_ok());
        assert_eq!(a64.current_el, 1);
        assert_eq!(a64.pc, a64.vbar_el1 + SYNC_EL0_64);
        assert_eq!(a64.elr_el1, user_va);
        assert_eq!(a64.esr_el1 & 0xFC00_0000, EC_INSN_ABORT_EL0);
        assert_ne!(a64.esr_el1 & (1 << 25), 0);
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
            seen_fault
                .lock()
                .unwrap()
                .push((info.pc, info.raw, info.kind));
        }));

        let brk: u32 = 0xD421_0000;
        sys_mem.ram.load_bytes(0x1000, &brk.to_le_bytes());

        let result = step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins);
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
                seen_mem.lock().unwrap().push((
                    info.pc,
                    info.raw,
                    info.class,
                    info.vaddr,
                    info.size,
                    info.is_store,
                    info.is_atomic,
                ));
            }),
        );

        let program = [
            0xF9000040u32, // STR X0, [X2]
            0xF9400041u32, // LDR X1, [X2]
        ];
        for (idx, insn) in program.iter().enumerate() {
            sys_mem
                .ram
                .load_bytes(0x1000 + (idx as u64 * 4), &insn.to_le_bytes());
        }

        assert!(step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins).is_ok());
        assert!(step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins).is_ok());

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(
            seen[0],
            (
                0x1000,
                0xF900_0040,
                helm_plugin::runtime::InsnClass::Store,
                0x4000,
                8,
                true,
                false
            )
        );
        assert_eq!(
            seen[1],
            (
                0x1004,
                0xF940_0041,
                helm_plugin::runtime::InsnClass::Load,
                0x4000,
                8,
                false,
                false
            )
        );
        assert_eq!(a64.x[1], a64.x[0]);
    }

    #[test]
    fn fs_step_fires_insn_callbacks() {
        let (mut a64, mut sys_mem, mut fs, probes, mut plugins) = make_fs_env();
        a64.pc = 0x1000;
        a64.x[0] = 1;
        a64.x[1] = 2;

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_insn = Arc::clone(&seen);
        plugins.on_insn_exec(Box::new(move |_vcpu, info| {
            seen_insn.lock().unwrap().push((
                info.pc,
                info.raw,
                info.class,
                info.opcode_name,
                info.context.clone(),
            ));
        }));

        let add_x2_x0_x1 = 0x8B01_0002u32; // ADD X2, X0, X1
        sys_mem.ram.load_bytes(0x1000, &add_x2_x0_x1.to_le_bytes());

        assert!(step_fs(&mut a64, &mut sys_mem, &mut fs, &probes, &plugins).is_ok());

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, 0x1000);
        assert_eq!(seen[0].1, add_x2_x0_x1);
        assert_eq!(seen[0].2, helm_plugin::runtime::InsnClass::IntAlu);
        assert!(!seen[0].3.is_empty());
        assert!(matches!(
            seen[0].4,
            helm_plugin::runtime::ArchContext::Aarch64 { pc: 0x1004, .. }
        ));
        assert_eq!(a64.x[2], 3);
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
        assert_eq!(
            a64.cntp_ctl_el0 & 4,
            4,
            "ISTATUS still set even when masked"
        );
    }

    #[test]
    fn tick_scale_applies_before_timer_irq_delivery_is_live() {
        let mut a64 = Aarch64ArchState::new();
        let mut fs = FsState::new();
        fs.tick_scale = 100;

        assert_eq!(effective_tick_step(&a64, &fs), 100);

        a64.cntp_ctl_el0 = 0b11; // enabled but masked
        assert_eq!(effective_tick_step(&a64, &fs), 100);
    }

    #[test]
    fn tick_scale_is_disabled_while_generic_timer_irq_is_live() {
        let mut a64 = Aarch64ArchState::new();
        let mut fs = FsState::new();
        fs.tick_scale = 100;

        a64.cntp_ctl_el0 = 0b01; // enabled, unmasked
        assert_eq!(effective_tick_step(&a64, &fs), 1);

        a64.cntp_ctl_el0 = 0;
        a64.cntv_ctl_el0 = 0b01; // enabled, unmasked
        assert_eq!(effective_tick_step(&a64, &fs), 1);
    }
}

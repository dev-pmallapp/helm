//! `helm-engine` — simulation kernel.
//!
//! # Key types
//! - [`HelmEngine<T>`] — generic simulation kernel; `T` is the `TimingModel`
//! - [`HelmSim`]       — enum wrapping all timing variants; the PyO3 boundary
//! - [`Isa`]           — which ISA is active (dispatch once per `run()` call)
//! - [`ExecMode`]      — functional / syscall-emulation / full-system
//! - [`FlatMem`]       — sparse RAM backend re-exported from `helm-memory`
//! - [`StopReason`]    — why `run()` returned
//!
//! # Inner loop contract
//! The inner loop (`step_*`) is hot. No allocations, no trait objects, no
//! dynamic dispatch. All cross-component refs are stored during `elaborate()`.

#![allow(missing_docs)]

pub mod address_space;
pub mod fs;
pub mod loader;
mod machine;
pub mod platform;
pub mod se;
pub mod session;

pub use helm_arch;
use helm_arch::{
    aarch64_decode, aarch64_execute, riscv_decode, riscv_execute, Aarch64ArchState, DecodeError,
};
pub use helm_core::{AccessType, MemFault, MemInterface};
use helm_core::{ExecContext, HartException};
use helm_event::{EventId, EventQueue, Tick};
pub use helm_memory::FlatMem;
use helm_timing::{
    AccurateTiming, IntervalTiming, MemAccess, TimingInsnClass, TimingInsnInfo, TimingModel,
    VirtualTiming,
};

pub use helm_plugin;
use helm_plugin::HelmPluginRegistry;

use helm_probe::{
    probe, BranchEvent, BranchKind as ProbeBranchKind, CpuProbes, CpuStepEvent, MemAccessEvent,
};

use crate::address_space::HelmAddressSpace;
use crate::fs::FsState;
use crate::platform::arm_virt::{self};
use crate::session::{
    Aarch64Core, HelmBoard, HelmCore, HelmCoreSet, HelmGic, HelmMachine, HelmVcpu, RiscvCore,
    RunStep,
};
use helm_devices::{CharBackend, Device, TickableDevice};
use helm_diag;
use helm_diag::sim_info;
use helm_hw_intc::GicSharedState;
use helm_hw_rtc::Pl031;
use helm_platform::{BoardQuirk, PlatformQuirk, QuirkKey, QuirkSet};
use se::{LinuxAarch64SyscallHandler, LinuxRiscv64SyscallHandler, SyscallArgs, SyscallHandler};
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct UnimplementedInstructionSite {
    pc: u64,
    raw: u32,
    opcode_name: &'static str,
}

const TIMER_CHECK_INTERVAL: u32 = 1024;
const IRQ_POLL_INTERVAL: u8 = 16;
const ENGINE_EVENT_CALLBACK_CLASS: u32 = u32::MAX;

struct EngineCallbackEvent<T: TimingModel> {
    callback: Box<dyn FnOnce(&mut HelmEngine<T>) + Send>,
}

type EngineEventHandler<T> = Box<dyn FnMut(&mut HelmEngine<T>, u64, Box<dyn Any + Send>) + Send>;

#[derive(Debug, Clone, Copy)]
pub struct TickableDeviceEvent {
    pub device_idx: usize,
    pub cycles: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct TickableDeviceHandle {
    pub class_id: u32,
    pub device_idx: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct ArmVirtTickableDevices {
    pub rtc: TickableDeviceHandle,
}

struct TimingCacheLevel {
    sets: Vec<Vec<u64>>,
    assoc: usize,
    num_sets: usize,
    line_bits: u32,
    set_bits: u32,
}

impl TimingCacheLevel {
    fn new(total_size: usize, assoc: usize, line_size: usize) -> Self {
        let assoc = assoc.max(1);
        let line_size = line_size.next_power_of_two().max(1);
        let num_sets = (total_size / (assoc * line_size))
            .next_power_of_two()
            .max(1);
        Self {
            sets: vec![Vec::with_capacity(assoc); num_sets],
            assoc,
            num_sets,
            line_bits: line_size.trailing_zeros(),
            set_bits: num_sets.trailing_zeros(),
        }
    }

    fn access(&mut self, addr: u64) -> bool {
        let set_idx = ((addr >> self.line_bits) as usize) & (self.num_sets - 1);
        let tag = addr >> (self.line_bits + self.set_bits);
        let set = &mut self.sets[set_idx];

        if let Some(pos) = set.iter().position(|&entry| entry == tag) {
            set.remove(pos);
            set.insert(0, tag);
            return true;
        }

        if set.len() >= self.assoc {
            set.pop();
        }
        set.insert(0, tag);
        false
    }
}

pub(crate) struct TimingMemModel {
    l1d: TimingCacheLevel,
    l2: TimingCacheLevel,
}

impl TimingMemModel {
    pub(crate) fn new() -> Self {
        Self {
            l1d: TimingCacheLevel::new(32 * 1024, 8, 64),
            l2: TimingCacheLevel::new(256 * 1024, 8, 64),
        }
    }

    pub(crate) fn access(
        &mut self,
        addr: u64,
        size: usize,
        is_store: bool,
        is_atomic: bool,
    ) -> MemAccess {
        if is_atomic {
            return MemAccess {
                addr,
                size,
                is_store,
                hit_l1: false,
                hit_l2: false,
            };
        }

        let hit_l1 = self.l1d.access(addr);
        let hit_l2 = !hit_l1 && self.l2.access(addr);
        MemAccess {
            addr,
            size,
            is_store,
            hit_l1,
            hit_l2,
        }
    }
}

#[inline(always)]
pub(crate) fn estimate_timing_mem_access(
    timing_mem_model: &mut TimingMemModel,
    addr: u64,
    size: usize,
    is_store: bool,
    is_atomic: bool,
) -> MemAccess {
    timing_mem_model.access(addr, size, is_store, is_atomic)
}

// ── Isa ───────────────────────────────────────────────────────────────────────

/// Which ISA the engine is running.
///
/// Dispatched once per `run()` call via `match self.isa { ... }`.
/// Zero dispatch inside the per-instruction step functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isa {
    RiscV,
    AArch64,
    AArch32,
}

// ── ExecMode ──────────────────────────────────────────────────────────────────

/// Simulation execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    /// Pure instruction execution — no syscall interception, no interrupts.
    Functional,
    /// Intercept `ecall` / `svc` and forward to the Linux ABI emulator.
    Syscall,
    /// Full-system: boot a real kernel, deliver interrupts, emulate MMU.
    System,
}

// ── StopReason ────────────────────────────────────────────────────────────────

/// Why `HelmEngine::run()` returned.
#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    /// Ran the requested number of instructions.
    Quantum,
    /// Guest called `exit()`.
    Exit { code: i32 },
    /// Unhandled exception — simulation cannot continue.
    Exception(HartException),
    /// ISA not yet implemented.
    Unsupported,
}

// ── InstrumentedMem ──────────────────────────────────────────────────────────

/// Stack-allocated memory access recorder for the plugin system.
///
/// Wraps `&mut FlatMem`, delegates all accesses, and records up to 8 entries
/// for post-execute callback dispatch.
struct InstrumentedMem<'a> {
    inner: &'a mut FlatMem,
    records: [MemAccessRecord; 8],
    count: usize,
}

#[derive(Clone, Copy)]
struct MemAccessRecord {
    vaddr: u64,
    size: u8,
    is_store: bool,
    is_atomic: bool,
    value_before: Option<u64>,
    value_after: Option<u64>,
}

impl Default for MemAccessRecord {
    fn default() -> Self {
        Self {
            vaddr: 0,
            size: 0,
            is_store: false,
            is_atomic: false,
            value_before: None,
            value_after: None,
        }
    }
}

impl<'a> InstrumentedMem<'a> {
    fn new(inner: &'a mut FlatMem) -> Self {
        Self {
            inner,
            records: [MemAccessRecord::default(); 8],
            count: 0,
        }
    }

    fn push(&mut self, rec: MemAccessRecord) {
        if self.count < 8 {
            self.records[self.count] = rec;
            self.count += 1;
        }
    }

    fn recorded(&self) -> &[MemAccessRecord] {
        &self.records[..self.count]
    }
}

impl<'a> MemInterface for InstrumentedMem<'a> {
    fn read(&mut self, addr: u64, size: usize, ty: AccessType) -> Result<u64, MemFault> {
        let is_atomic = ty == AccessType::Atomic;
        let val = self.inner.read(addr, size, ty)?;
        self.push(MemAccessRecord {
            vaddr: addr,
            size: size as u8,
            is_store: false,
            is_atomic,
            value_before: Some(val),
            value_after: None,
        });
        Ok(val)
    }

    fn write(&mut self, addr: u64, size: usize, val: u64, ty: AccessType) -> Result<(), MemFault> {
        let is_atomic = ty == AccessType::Atomic;
        let old = self.inner.read(addr, size, AccessType::Load).unwrap_or(0);
        self.inner.write(addr, size, val, ty)?;
        self.push(MemAccessRecord {
            vaddr: addr,
            size: size as u8,
            is_store: true,
            is_atomic,
            value_before: Some(old),
            value_after: Some(val),
        });
        Ok(())
    }
}

// ── HelmEngine<T> ─────────────────────────────────────────────────────────────

/// The simulation kernel, generic over timing model `T`.
///
/// Monomorphized at compile time — one binary specialization per timing variant.
/// The `HelmSim` enum selects which specialization to construct.
pub struct HelmEngine<T: TimingModel> {
    pub isa: Isa,
    pub mode: ExecMode,
    pub timing: T,
    timing_mem_model: TimingMemModel,

    /// HelmCore session state. Today this wraps a homogeneous runtime
    /// collection, but the shape is intended to evolve toward heterogeneous
    /// systems.
    session: HelmMachine,

    mem_size: usize,
    pub memory: FlatMem,
    pub events: EventQueue,

    /// Total instructions retired.
    pub insns_retired: u64,

    /// Countdown for the FS-mode periodic timer check (fires at 0, resets to TIMER_CHECK_INTERVAL).
    timer_countdown: u32,
    /// Countdown for IRQ line polling (poll every 16 instructions instead of every instruction).
    irq_poll_countdown: u8,
    /// Countdown for throttled SMP progress logging in FS mode.
    fs_status_countdown: u32,

    /// Plugin callback registry.
    pub plugins: HelmPluginRegistry,

    /// Typed probe bundle — zero-cost in release builds.
    pub probes: CpuProbes,

    next_event_class_id: u32,
    event_handlers: HashMap<u32, EngineEventHandler<T>>,

    /// ELF symbol table (populated after load_aarch64_elf).
    pub symbols: Vec<loader::ElfSymbol>,

    /// Unique stubbed instruction sites encountered during execution.
    unimplemented_instruction_sites: std::collections::HashSet<UnimplementedInstructionSite>,

    /// JIT translation block cache (only present when `jit` feature is enabled
    /// and `set_jit(true)` has been called).
    #[cfg(feature = "jit")]
    jit_cache: Option<helm_jit::cache::JitCache>,
    /// Primary JIT compilation backend (stencil in tiered mode, or the sole backend).
    #[cfg(feature = "jit")]
    jit_backend: Option<Box<dyn helm_jit::backend::JitBackend>>,
    /// Hot-tier backend for recompiling promoted blocks (dynasm in tiered mode).
    #[cfg(feature = "jit")]
    jit_hot_backend: Option<Box<dyn helm_jit::backend::JitBackend>>,
    /// Whether JIT execution is enabled (set via `set_jit(true)`).
    #[cfg(feature = "jit")]
    jit_enabled: bool,
}

impl<T: TimingModel> HelmEngine<T> {
    fn riscv(&self) -> &RiscvCore {
        self.session.riscv().expect("riscv runtime missing")
    }

    fn riscv_mut(&mut self) -> &mut RiscvCore {
        self.session.riscv_mut().expect("riscv runtime missing")
    }

    fn active_mode(&self) -> ExecMode {
        self.session.active_mode().unwrap_or(self.mode)
    }

    fn online_fs_cpus(machine: &HelmBoard) -> Vec<usize> {
        machine
            .vcpus
            .iter()
            .enumerate()
            .filter_map(|(idx, vcpu)| vcpu.powered_on.then_some(idx))
            .collect()
    }

    fn pick_next_fs_vcpu(machine: &mut HelmBoard) -> Option<usize> {
        if machine.vcpus.is_empty() {
            return None;
        }
        // Sync IRQ lines for WFI-idle vCPUs so they can wake up.
        for i in 0..machine.vcpus.len() {
            if machine.vcpus[i].fs.wfi_idle {
                machine.vcpus[i].fs.irq_pending = machine
                    .irq_lines
                    .get(i)
                    .map_or(false, |l| l.load(std::sync::atomic::Ordering::Relaxed));
                // Auto-wake: if an IRQ arrived, clear wfi_idle so the CPU
                // runs normally once selected (no extra WFI re-check).
                if machine.vcpus[i].fs.irq_pending {
                    machine.vcpus[i].fs.wfi_idle = false;
                }
            }
        }
        let start = machine.next_vcpu % machine.vcpus.len();
        // First pass: skip CPUs that are powered off or idle in WFI
        // (no interrupt pending). This prevents idle secondaries from
        // starving the boot CPU of instruction quanta.
        for off in 0..machine.vcpus.len() {
            let idx = (start + off) % machine.vcpus.len();
            let vcpu = &machine.vcpus[idx];
            if vcpu.powered_on && !(vcpu.fs.wfi_idle && !vcpu.fs.irq_pending) {
                machine.next_vcpu = (idx + 1) % machine.vcpus.len();
                return Some(idx);
            }
        }
        // All powered-on CPUs are WFI-idle. Return None so the engine
        // yields the quantum; the outer loop will re-poll IRQ lines.
        None
    }

    fn handle_fs_psci_call(
        machine: &mut HelmBoard,
        vcpu_idx: usize,
        conduit: &str,
        function: u32,
        arg1: u64,
        arg2: u64,
        arg3: u64,
    ) -> Result<(), HartException> {
        let current_sp_el1 = machine.vcpus[vcpu_idx].arch.sp_el1;
        let current_mpidr = machine.vcpus[vcpu_idx].arch.mpidr_el1;
        let current_pc = machine.vcpus[vcpu_idx].arch.pc;
        let target_idx = machine
            .vcpus
            .iter()
            .position(|cpu| cpu.arch.mpidr_el1 == arg1)
            .or_else(|| {
                machine
                    .vcpus
                    .iter()
                    .position(|cpu| (cpu.arch.mpidr_el1 & 0xFF_FFFF) == (arg1 & 0xFF_FFFF))
            });
        let result: i64 = match function {
            0x8400_0000 => 0x0001_0001,
            0x8400_0001 => 0,
            0x8400_0002 => {
                machine.vcpus[vcpu_idx].powered_on = false;
                sim_info!(
                    component = "aarch64-fs-smp",
                    pc = current_pc,
                    "PSCI CPU_OFF: cpu{} mpidr={:#x}",
                    vcpu_idx,
                    current_mpidr
                );
                0
            }
            0x8400_0006 => 2,
            0x8400_000a => match arg1 as u32 {
                0x8400_0000 | 0x8400_0001 | 0x8400_0002 | 0x8400_0003 | 0x8400_0006
                | 0x8400_0008 | 0x8400_0009 | 0x8400_000a => 0,
                _ => -1,
            },
            0x8400_0003 | 0xc400_0003 => match target_idx {
                Some(target_idx) => {
                    if machine.vcpus[target_idx].powered_on {
                        sim_info!(
                            component = "aarch64-fs-smp",
                            pc = current_pc,
                            "PSCI CPU_ON rejected: src_cpu{} mpidr={:#x} target_cpu{} target_mpidr={:#x} already_on",
                            vcpu_idx,
                            current_mpidr,
                            target_idx,
                            machine.vcpus[target_idx].arch.mpidr_el1
                        );
                        -4
                    } else {
                        let target_mpidr;
                        let caller_tick = machine.vcpus[vcpu_idx].fs.tick;
                        let psci_via_engine =
                            machine.has_quirk(QuirkKey::Board(BoardQuirk::PsciViaEngine));
                        {
                            let target = &mut machine.vcpus[target_idx];
                            target.arch.pc = arg2;
                            target.arch.x = [0; 31];
                            target.arch.x[0] = arg3;
                            target.arch.sp = 0;
                            target.arch.sp_el1 =
                                current_sp_el1.wrapping_sub(((target_idx + 1) as u64) * 0x10000);
                            target.arch.current_el = 1;
                            target.arch.spsel = true;
                            target.arch.sctlr_el1 = 0x0000_0800;
                            target.arch.daif = 0xF;
                            target.arch.psci_via_engine = psci_via_engine;
                            target.powered_on = true;
                            target_mpidr = target.arch.mpidr_el1;
                            // Sync tick counter so all CPUs share a consistent
                            // time base. Without this, tick_scale > 1 causes
                            // massive CNTVCT divergence between CPUs.
                            target.fs.tick = caller_tick;
                            target.arch.cntvct_el0 = caller_tick;
                        }
                        let online = Self::online_fs_cpus(machine);
                        sim_info!(
                            component = "aarch64-fs-smp",
                            pc = current_pc,
                            "PSCI CPU_ON: src_cpu{} mpidr={:#x} -> target_cpu{} target_mpidr={:#x} entry={:#x} ctx={:#x} online={:?}",
                            vcpu_idx,
                            current_mpidr,
                            target_idx,
                            target_mpidr,
                            arg2,
                            arg3,
                            online
                        );
                        0
                    }
                }
                None => {
                    sim_info!(
                        component = "aarch64-fs-smp",
                        pc = current_pc,
                        "PSCI CPU_ON failed: src_cpu{} mpidr={:#x} target_mpidr={:#x} not_found",
                        vcpu_idx,
                        current_mpidr,
                        arg1
                    );
                    -2
                }
            },
            0xc400_0004 => match target_idx {
                Some(target_idx) if !machine.vcpus[target_idx].powered_on => 1,
                Some(_) => 0,
                None => -2,
            },
            0x8400_0008 | 0x8400_0009 => return Err(HartException::Exit { code: 0 }),
            _ => -1,
        };
        let current = &mut machine.vcpus[vcpu_idx];
        current.arch.x[0] = result as u64;
        current.arch.pc = current.arch.pc.wrapping_add(4);
        let _ = conduit;
        Ok(())
    }

    pub fn new(isa: Isa, mode: ExecMode, timing: T, mem_base: u64, mem_size: usize) -> Self {
        let runtimes = match (isa, mode) {
            (Isa::RiscV, _) => HelmCoreSet::new_primary(HelmCore::Riscv(RiscvCore::default())),
            (Isa::AArch64, ExecMode::Functional) => HelmCoreSet::new_primary(HelmCore::Aarch64(
                Aarch64Core::Functional(Aarch64ArchState::new()),
            )),
            (Isa::AArch64, _) => HelmCoreSet::new_primary(HelmCore::Aarch64(Aarch64Core::Disabled)),
            (Isa::AArch32, _) => HelmCoreSet::default(),
        };
        Self {
            isa,
            mode,
            timing,
            timing_mem_model: TimingMemModel::new(),
            session: HelmMachine::from_runtimes(runtimes),
            mem_size,
            memory: FlatMem::new(mem_base, mem_size),
            events: EventQueue::new(),
            insns_retired: 0,
            timer_countdown: TIMER_CHECK_INTERVAL,
            irq_poll_countdown: IRQ_POLL_INTERVAL,
            fs_status_countdown: 50_000_000,
            plugins: HelmPluginRegistry::new(),
            probes: CpuProbes::default(),
            next_event_class_id: 1,
            event_handlers: HashMap::new(),
            symbols: Vec::new(),
            unimplemented_instruction_sites: std::collections::HashSet::new(),
            #[cfg(feature = "jit")]
            jit_cache: None,
            #[cfg(feature = "jit")]
            jit_backend: None,
            #[cfg(feature = "jit")]
            jit_hot_backend: None,
            #[cfg(feature = "jit")]
            jit_enabled: false,
        }
        .with_initial_runtime_mode(mode)
    }

    fn with_initial_runtime_mode(mut self, mode: ExecMode) -> Self {
        let _ = self.session.set_riscv_mode(mode);
        self
    }

    fn maybe_log_fs_smp_progress(&mut self) {
        if self.active_mode() != ExecMode::System || !helm_diag::is_monitor_active() {
            return;
        }
        if self.fs_status_countdown > 1 {
            self.fs_status_countdown -= 1;
            return;
        }
        self.fs_status_countdown = 50_000_000;
        if let Some(machine) = self.session.aarch64().and_then(Aarch64Core::machine) {
            if machine.vcpus.len() <= 1 {
                return;
            }
            let online = Self::online_fs_cpus(machine);
            sim_info!(
                component = "aarch64-fs-smp",
                "progress insns={} online_cpus={:?} next_vcpu={}",
                self.insns_retired,
                online,
                machine.next_vcpu
            );
        }
    }

    /// Set the program counter (reset vector).
    pub fn set_pc(&mut self, pc: u64) {
        if let Some(rv) = self.session.riscv_mut() {
            rv.pc = pc;
        }
        if let Some(a64) = self.session.aarch64_mut().and_then(Aarch64Core::state_mut) {
            a64.pc = pc;
        }
        if let Some(machine) = self
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::machine_mut)
        {
            if let Some(vcpu0) = machine.vcpus.first_mut() {
                vcpu0.arch.pc = pc;
                vcpu0.powered_on = true;
            }
        }
    }

    /// Load bytes into memory (e.g. from ELF loader).
    pub fn load_bytes(&mut self, addr: u64, bytes: &[u8]) {
        self.memory.load_bytes(addr, bytes);
    }

    /// Attach a syscall handler (required for `ExecMode::Syscall`).
    pub fn set_syscall_handler(&mut self, h: Box<dyn SyscallHandler>) {
        let _ = self.session.set_riscv_syscall_handler(Some(h));
        let _ = self.session.set_riscv_mode(ExecMode::Syscall);
    }

    fn note_unimplemented_instruction(
        &mut self,
        pc: u64,
        raw: u32,
        opcode_name: &'static str,
    ) -> bool {
        let site = UnimplementedInstructionSite {
            pc,
            raw,
            opcode_name,
        };
        if self.unimplemented_instruction_sites.insert(site) {
            log::warn!(
                "unimplemented instruction executed at pc={pc:#x} raw={raw:#010x} kind={opcode_name}; future encounters of this site will be ignored"
            );
            true
        } else {
            false
        }
    }

    pub fn has_unimplemented_instructions(&self) -> bool {
        !self.unimplemented_instruction_sites.is_empty()
    }

    pub fn unimplemented_instruction_count(&self) -> usize {
        self.unimplemented_instruction_sites.len()
    }

    #[allow(dead_code)]
    pub(crate) fn machine_coordination_state(&self) -> crate::machine::MachineCoordinationState {
        self.session.machine_coordination_state()
    }

    #[allow(dead_code)]
    pub(crate) fn machine_policy_feedback(&self) -> crate::machine::MachinePolicyFeedback {
        self.session.machine_policy_feedback()
    }

    pub fn current_cycles(&self) -> Tick {
        self.timing.current_cycles()
    }

    pub fn system_board_has_quirk(&self, key: QuirkKey) -> Option<bool> {
        let machine = self.session.aarch64().and_then(Aarch64Core::machine)?;
        Some(machine.has_quirk(key))
    }

    pub fn with_system_memory_mut<R>(
        &mut self,
        f: impl FnOnce(&mut HelmAddressSpace) -> R,
    ) -> Option<R> {
        let machine = self
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::machine_mut)?;
        Some(f(&mut machine.sys_mem))
    }

    pub fn with_a64_state_mut<R>(
        &mut self,
        f: impl FnOnce(&mut Aarch64ArchState) -> R,
    ) -> Option<R> {
        let state = self
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::state_mut)?;
        Some(f(state))
    }

    pub fn install_test_aarch64_system_board(
        &mut self,
        sys_mem: HelmAddressSpace,
    ) -> Result<(), &'static str> {
        self.install_aarch64_system_board_internal(
            sys_mem,
            crate::platform::arm_virt::ArmVirtDevices {
                gicd_idx: 0,
                gicc_idx: 0,
                uart_idx: 0,
                rtc_idx: None,
            },
            Vec::new(),
            QuirkSet::default(),
            None,
        )
    }

    pub fn install_aarch64_system_board_v2(
        &mut self,
        sys_mem: HelmAddressSpace,
        devs: crate::platform::arm_virt::ArmVirtDevices,
        irq_lines: Vec<std::sync::Arc<std::sync::atomic::AtomicBool>>,
        gic_state: Arc<Mutex<GicSharedState>>,
    ) -> Result<(), &'static str> {
        self.install_aarch64_system_board_internal(
            sys_mem,
            devs,
            irq_lines,
            QuirkSet::default(),
            Some(HelmGic::V2(gic_state)),
        )
    }

    pub fn install_arm_virt_board(
        &mut self,
        mem_mib: usize,
        num_cpus: usize,
        gic_version: arm_virt::ArmVirtGicVersion,
        uart_backend: Box<dyn CharBackend>,
    ) -> Result<(), &'static str> {
        let quirks = arm_virt::default_arm_virt_quirks();
        match gic_version {
            arm_virt::ArmVirtGicVersion::V2 => {
                let (sys_mem, devs, irq_lines, gic_state) =
                    arm_virt::build_arm_virt_with_cpus(mem_mib, num_cpus, uart_backend);
                self.install_aarch64_system_board_internal(
                    sys_mem,
                    devs,
                    irq_lines,
                    quirks.clone(),
                    Some(HelmGic::V2(gic_state)),
                )
            }
            arm_virt::ArmVirtGicVersion::V3 => {
                let (sys_mem, devs, irq_lines, gic_state) =
                    arm_virt::build_arm_virt_gicv3(mem_mib, num_cpus, uart_backend);
                self.install_aarch64_system_board_internal(
                    sys_mem,
                    devs,
                    irq_lines,
                    quirks,
                    Some(HelmGic::V3(gic_state)),
                )
            }
        }
    }

    fn install_aarch64_system_board_internal(
        &mut self,
        sys_mem: HelmAddressSpace,
        devs: crate::platform::arm_virt::ArmVirtDevices,
        irq_lines: Vec<std::sync::Arc<std::sync::atomic::AtomicBool>>,
        quirks: QuirkSet,
        gic: Option<HelmGic>,
    ) -> Result<(), &'static str> {
        if self.isa != Isa::AArch64 {
            return Err("AArch64 system board helper requires AArch64 engine");
        }

        let mut cpu = Aarch64ArchState::new();
        cpu.current_el = 1;
        cpu.spsel = true;

        self.session
            .replace_primary(HelmCore::Aarch64(Aarch64Core::System(HelmBoard {
                sys_mem,
                vcpus: vec![HelmVcpu {
                    arch: cpu,
                    fs: FsState::new(),
                    powered_on: true,
                }],
                next_vcpu: 0,
                devs,
                quirks,
                irq_lines,
                gic,
            })));
        self.mode = ExecMode::System;
        Ok(())
    }

    pub fn post_callback_after<F>(&mut self, delay: Tick, callback: F) -> EventId
    where
        F: FnOnce(&mut HelmEngine<T>) + Send + 'static,
    {
        self.events.post_after(
            delay,
            ENGINE_EVENT_CALLBACK_CLASS,
            0,
            EngineCallbackEvent {
                callback: Box::new(callback),
            },
        )
    }

    pub fn post_callback_at<F>(&mut self, fire_at: Tick, callback: F) -> EventId
    where
        F: FnOnce(&mut HelmEngine<T>) + Send + 'static,
    {
        self.events.post_at(
            fire_at,
            ENGINE_EVENT_CALLBACK_CLASS,
            0,
            EngineCallbackEvent {
                callback: Box::new(callback),
            },
        )
    }

    pub fn post_event_after<D>(
        &mut self,
        delay: Tick,
        class_id: u32,
        owner_id: u64,
        data: D,
    ) -> EventId
    where
        D: Any + Send + 'static,
    {
        self.events.post_after(delay, class_id, owner_id, data)
    }

    pub fn post_event_at<D>(
        &mut self,
        fire_at: Tick,
        class_id: u32,
        owner_id: u64,
        data: D,
    ) -> EventId
    where
        D: Any + Send + 'static,
    {
        self.events.post_at(fire_at, class_id, owner_id, data)
    }

    pub fn register_event_handler<F>(&mut self, class_id: u32, handler: F)
    where
        F: FnMut(&mut HelmEngine<T>, u64, Box<dyn Any + Send>) + Send + 'static,
    {
        self.event_handlers.insert(class_id, Box::new(handler));
    }

    pub fn allocate_event_class_id(&mut self) -> u32 {
        let class_id = self.next_event_class_id;
        self.next_event_class_id = self.next_event_class_id.saturating_add(1);
        if self.next_event_class_id == ENGINE_EVENT_CALLBACK_CLASS {
            self.next_event_class_id = 1;
        }
        class_id
    }

    pub fn register_event_handler_auto<F>(&mut self, handler: F) -> u32
    where
        F: FnMut(&mut HelmEngine<T>, u64, Box<dyn Any + Send>) + Send + 'static,
    {
        let class_id = self.allocate_event_class_id();
        self.register_event_handler(class_id, handler);
        class_id
    }

    pub fn register_tickable_device_handler<D>(
        &mut self,
        sys_mem: Arc<Mutex<HelmAddressSpace>>,
    ) -> u32
    where
        D: Device + TickableDevice + 'static,
    {
        let class_id = self.allocate_event_class_id();
        self.register_event_handler(class_id, move |_engine, owner_id, data| {
            let event = *data
                .downcast::<TickableDeviceEvent>()
                .expect("tickable device event payload must match TickableDeviceEvent");
            let mut sys = sys_mem.lock().expect("device event address space mutex poisoned");
            if sys
                .with_device_mut::<D, _>(event.device_idx, |dev| dev.tick(event.cycles))
                .is_none()
            {
                log::warn!(
                    "dropping tickable device event class_id={class_id} owner_id={owner_id} device_idx={}",
                    event.device_idx
                );
            }
        });
        class_id
    }

    pub fn register_system_tickable_device_handler<D>(&mut self) -> u32
    where
        D: Device + TickableDevice + 'static,
    {
        let class_id = self.allocate_event_class_id();
        self.register_event_handler(class_id, move |engine, owner_id, data| {
            let event = *data
                .downcast::<TickableDeviceEvent>()
                .expect("tickable device event payload must match TickableDeviceEvent");
            let handled = engine.with_system_memory_mut(|sys| {
                sys.with_device_mut::<D, _>(event.device_idx, |dev| dev.tick(event.cycles))
            });

            match handled.flatten() {
                Some(()) => {}
                None => {
                    log::warn!(
                        "dropping system tickable device event class_id={class_id} owner_id={owner_id} device_idx={}",
                        event.device_idx
                    );
                }
            }
        });
        class_id
    }

    pub fn register_arm_virt_tickable_devices(&mut self) -> Option<ArmVirtTickableDevices> {
        let rtc_idx = {
            let machine = self.session.aarch64().and_then(Aarch64Core::machine)?;
            if !machine.has_quirk(QuirkKey::Platform(PlatformQuirk::ArmVirtPl031Rtc)) {
                return None;
            }
            machine.devs.rtc_idx?
        };
        let rtc_class_id = self.register_system_tickable_device_handler::<Pl031>();
        Some(ArmVirtTickableDevices {
            rtc: TickableDeviceHandle {
                class_id: rtc_class_id,
                device_idx: rtc_idx,
            },
        })
    }

    pub fn post_tickable_device_after(
        &mut self,
        delay: Tick,
        class_id: u32,
        device_idx: usize,
        cycles: u64,
    ) -> EventId {
        self.post_event_after(
            delay,
            class_id,
            0,
            TickableDeviceEvent { device_idx, cycles },
        )
    }

    fn drain_ready_events(&mut self) {
        let target_tick = self.timing.current_cycles();
        let mut queue = std::mem::take(&mut self.events);
        let mut callbacks: Vec<Box<dyn FnOnce(&mut HelmEngine<T>) + Send>> = Vec::new();
        let mut pending_events: Vec<(u32, u64, Box<dyn Any + Send>)> = Vec::new();

        queue.drain_until(target_tick, |class_id, owner_id, data| {
            if class_id == ENGINE_EVENT_CALLBACK_CLASS {
                let event = *data
                    .downcast::<EngineCallbackEvent<T>>()
                    .expect("engine callback event payload must match timing specialization");
                callbacks.push(event.callback);
            } else {
                pending_events.push((class_id, owner_id, data));
            }
        });

        self.events = queue;
        for callback in callbacks {
            callback(self);
        }
        for (class_id, owner_id, data) in pending_events {
            if let Some(mut handler) = self.event_handlers.remove(&class_id) {
                handler(self, owner_id, data);
                self.event_handlers.insert(class_id, handler);
            } else {
                log::warn!(
                    "dropping undeliverable engine event class_id={class_id} owner_id={owner_id}"
                );
            }
        }
    }

    #[inline(always)]
    fn advance_timing_boundary(&mut self) {
        self.timing.on_boundary(&mut self.events);
        self.drain_ready_events();
    }

    /// Run up to `max_insns` instructions. Returns the reason for stopping.
    pub fn run(&mut self, max_insns: u64) -> StopReason {
        for _ in 0..max_insns {
            if helm_diag::is_monitor_active() {
                helm_diag::update_sim_ctx(self.insns_retired, 1_000_000_000);
            }
            let result = match self.session.active_isa().unwrap_or(self.isa) {
                Isa::RiscV => self.step_riscv(),
                Isa::AArch64 => {
                    if self.active_mode() == ExecMode::System {
                        self.step_aarch64_system()
                    } else {
                        self.step_aarch64()
                    }
                }
                Isa::AArch32 => return StopReason::Unsupported,
            };
            match result {
                Ok(()) => {
                    self.insns_retired += 1;
                    self.session.on_progress(RunStep::RetiredInstruction);
                    self.maybe_log_fs_smp_progress();
                    // Only update probe insn count when probe subscribers exist.
                    // In release builds probe!() is zero-sized; this guard also
                    // eliminates the thread-local write on the common no-probe path.
                    if self.probes.any_active() {
                        helm_probe::update_probe_insn_count(self.insns_retired);
                    }
                    // Update sim-trace context only when a MonitorSink is active.
                    if helm_diag::is_monitor_active() {
                        helm_diag::update_sim_ctx(self.insns_retired, 1_000_000_000);
                    }
                    if self.plugins.has_any_callbacks() && self.plugins.has_timer_callbacks() {
                        self.plugins.fire_timer(0, self.insns_retired);
                    }
                    self.advance_timing_boundary();
                }
                Err(exc) => {
                    let stop = self.handle_exception(exc);
                    // Check if AArch64 handler requested exit
                    if let Some(h) = self.session.aarch64().and_then(Aarch64Core::handler) {
                        if h.should_exit {
                            return StopReason::Exit { code: h.exit_code };
                        }
                    }
                    match stop {
                        // Syscall handled OK — count it and keep running.
                        StopReason::Quantum => {
                            self.insns_retired += 1;
                            self.session.on_progress(RunStep::YieldedQuantum);
                            self.maybe_log_fs_smp_progress();
                            if helm_diag::is_monitor_active() {
                                helm_diag::update_sim_ctx(self.insns_retired, 1_000_000_000);
                            }
                            self.advance_timing_boundary();
                        }
                        ref s @ StopReason::Exit { .. } | ref s @ StopReason::Exception(_) => {
                            self.plugins.fire_vcpu_exit(0);
                            return s.clone();
                        }
                        other => return other,
                    }
                }
            }
        }
        StopReason::Quantum
    }

    /// Enable or disable the JIT backend.
    #[cfg(feature = "jit")]
    pub fn set_jit(&mut self, enabled: bool) {
        self.jit_enabled = enabled;
        if enabled {
            if self.jit_cache.is_none() {
                self.jit_cache = Some(helm_jit::cache::JitCache::new());
            }
            if self.jit_backend.is_none() {
                // Tiered mode: stencil as baseline, dynasm as hot-tier.
                // Non-tiered: use whichever backend is enabled.
                #[cfg(feature = "jit-tiered")]
                {
                    self.jit_backend = Some(Box::new(
                        helm_jit::stencil::StencilBackend::new_aarch64(),
                    ));
                    self.jit_hot_backend = Some(Box::new(
                        helm_jit::dynasm::DynasmBackend::new(),
                    ));
                    log::info!("jit: tiered mode (stencil baseline + dynasm hot-tier)");
                }
                #[cfg(all(feature = "jit-stencil", not(feature = "jit-tiered")))]
                {
                    self.jit_backend = Some(Box::new(
                        helm_jit::stencil::StencilBackend::new_aarch64(),
                    ));
                }
                #[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
                {
                    self.jit_backend = Some(Box::new(helm_jit::dynasm::DynasmBackend::new()));
                }
            }
        }
    }

    /// Run up to `max_insns` instructions using the JIT backend.
    ///
    /// Falls back to the interpreter for unsupported opcodes. Works for
    /// AArch64 SE and FS modes.
    #[cfg(feature = "jit")]
    #[allow(unsafe_code)]
    pub fn run_jit(&mut self, max_insns: u64) -> StopReason {
        use helm_jit::block::EXIT_END_OF_BLOCK;
        use helm_jit::regs;

        if self.isa != Isa::AArch64 {
            // RISC-V JIT: stencils exist but engine wiring is not yet
            // implemented. Fall back to interpreter.
            // TODO: add RV64 decode loop, regs_rv64 sync, and
            // JitBackendRv64 trait for RISC-V stencil compilation.
            return self.run(max_insns);
        }

        let cache = match self.jit_cache.as_mut() {
            Some(c) => c as *mut helm_jit::cache::JitCache,
            None => return self.run(max_insns),
        };

        let is_fs = self.active_mode() == ExecMode::System;

        // Sync arch state → flat register array
        let a64 = match self.session.aarch64().and_then(Aarch64Core::state) {
            Some(s) => s,
            None => return StopReason::Unsupported,
        };
        let mut flat_regs = regs::arch_to_flat(a64);

        // Set up memory pointer and helper function pointers based on mode.
        //
        // SE mode: mem_ptr = &mut FlatMem, helpers = jit_mem_read/write
        // FS mode: mem_ptr = &mut JitFsContext, helpers = jit_fs_mem_read/write
        //
        // The JitFsContext contains pointers to the address space + TLB +
        // snapshotted MMU config for VA→PA translation.
        let mut fs_ctx: Option<helm_jit::helpers::JitFsContext> = None;
        let mem_ptr: *mut u8;

        let (jit_mr, jit_mw) = if is_fs {
            // FS mode: create JitFsContext with MMU config snapshot
            let a64_ref = self.session.aarch64().and_then(Aarch64Core::state)
                .expect("aarch64 state");
            let mmu_cfg = helm_arch::aarch64::mmu::MmuConfig::from_arch(a64_ref);
            let board = self.session.aarch64_mut()
                .and_then(Aarch64Core::machine_mut)
                .expect("board");
            fs_ctx = Some(helm_jit::helpers::JitFsContext {
                sys_mem: &mut board.sys_mem as *mut _,
                tlb: &mut board.vcpus[board.next_vcpu].fs.tlb as *mut _,
                mmu_cfg,
            });
            mem_ptr = fs_ctx.as_mut().unwrap() as *mut helm_jit::helpers::JitFsContext as *mut u8;
            (
                helm_jit::helpers::jit_fs_mem_read as *const () as u64,
                helm_jit::helpers::jit_fs_mem_write as *const () as u64,
            )
        } else {
            // SE mode: direct FlatMem access
            mem_ptr = &mut self.memory as *mut FlatMem as *mut u8;
            (
                helm_jit::helpers::jit_mem_read as *const () as u64,
                helm_jit::helpers::jit_mem_write as *const () as u64,
            )
        };

        flat_regs[regs::REG_JIT_MEM_READ] = jit_mr;
        flat_regs[regs::REG_JIT_MEM_WRITE] = jit_mw;

        let mut retired: u64 = 0;

        while retired < max_insns {
            let pc = flat_regs[regs::REG_PC];

            // Try cache lookup with heat tracking
            let cache_ref = unsafe { &mut *cache };
            if let Some(hit) = cache_ref.lookup_hot(pc) {
                // Check for tiered promotion: if this is a stencil block that
                // has been executed enough times, recompile with dynasm.
                #[cfg(feature = "jit")]
                if hit.exec_count == helm_jit::cache::PROMOTE_THRESHOLD
                    && hit.tier == helm_jit::cache::JitTier::Stencil
                {
                    if let Some(hot) = self.jit_hot_backend.as_mut() {
                        // Decode the block again for dynasm recompilation
                        let mut insns = Vec::new();
                        let mut dpc = pc;
                        for _ in 0..64 {
                            let raw = match self.memory.fetch32(dpc) {
                                Ok(r) => r,
                                Err(_) => break,
                            };
                            match aarch64_decode(raw, dpc) {
                                Ok(insn) => {
                                    let is_branch = insn.is_branch();
                                    insns.push(insn);
                                    dpc += 4;
                                    if is_branch { break; }
                                }
                                Err(_) => break,
                            }
                        }
                        if !insns.is_empty() {
                            if let Some(promoted) = hot.compile_block(pc, &insns) {
                                log::trace!(
                                    "jit: promoting pc={pc:#x} to {} ({} insns)",
                                    hot.name(),
                                    promoted.insn_count
                                );
                                cache_ref.promote(
                                    pc,
                                    promoted,
                                    helm_jit::cache::JitTier::Dynasm,
                                );
                                // Re-lookup to get the promoted block
                                if let Some(new_hit) = cache_ref.lookup_hot(pc) {
                                    let exit_code = unsafe {
                                        (new_hit.block.entry)(flat_regs.as_mut_ptr(), mem_ptr)
                                    };
                                    retired += u64::from(new_hit.block.insn_count);
                                    match exit_code {
                                        EXIT_END_OF_BLOCK => continue,
                                        _ => break,
                                    }
                                }
                            }
                        }
                    }
                }

                // Execute the cached block (stencil or dynasm)
                let exit_code = unsafe {
                    (hit.block.entry)(flat_regs.as_mut_ptr(), mem_ptr)
                };
                retired += u64::from(hit.block.insn_count);

                match exit_code {
                    EXIT_END_OF_BLOCK => continue,
                    _ => break,
                }
            }

            // Cache miss — decode instructions and try to compile a block
            log::trace!("jit: cache miss pc={pc:#x}, decoding...");
            let mut insns = Vec::new();
            let mut decode_pc = pc;
            for _ in 0..64 {
                let raw = match self.memory.fetch32(decode_pc) {
                    Ok(r) => r,
                    Err(_) => break,
                };
                match aarch64_decode(raw, decode_pc) {
                    Ok(insn) => {
                        let is_branch = insn.is_branch();
                        insns.push(insn);
                        decode_pc += 4;
                        if is_branch {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }

            if insns.is_empty() {
                // Can't decode anything — fall back to interpreter for one step
                let a64_mut = self
                    .session
                    .aarch64_mut()
                    .and_then(Aarch64Core::state_mut)
                    .expect("aarch64 state");
                regs::flat_to_arch(&mut flat_regs, a64_mut);
                self.insns_retired += retired;
                return self.run(max_insns.saturating_sub(retired));
            }

            // Try to compile the block
            log::trace!("jit: decoded {} insns starting at pc={pc:#x}", insns.len());
            let cache_ref = unsafe { &mut *cache };
            let backend = match self.jit_backend.as_mut() {
                Some(b) => b,
                None => {
                    // No backend available — fall back to interpreter
                    let a64_mut = self
                        .session
                        .aarch64_mut()
                        .and_then(Aarch64Core::state_mut)
                        .expect("aarch64 state");
                    regs::flat_to_arch(&mut flat_regs, a64_mut);
                    self.insns_retired += retired;
                    return self.run(max_insns.saturating_sub(retired));
                }
            };
            match backend.compile_block(pc, &insns) {
                Some(block) => {
                    log::trace!("jit: compiled block pc={pc:#x} insns={}", block.insn_count);
                    cache_ref.insert(block);
                    // Loop back to execute the newly cached block
                }
                None => {
                    // First instruction unsupported — interpreter fallback.
                    // Commit JIT-retired insns, run interpreter for a batch,
                    // then return. The Python loop re-enters run_jit() where
                    // the JIT cache may hit for subsequent blocks.
                    let a64_mut = self
                        .session
                        .aarch64_mut()
                        .and_then(Aarch64Core::state_mut)
                        .expect("aarch64 state");
                    regs::flat_to_arch(&mut flat_regs, a64_mut);
                    self.insns_retired += retired;
                    // FS: small batch to re-enter JIT sooner for hot blocks.
                    // SE: full remaining quantum (Python loop re-enters).
                    let batch = if is_fs {
                        256u64.min(max_insns.saturating_sub(retired))
                    } else {
                        max_insns.saturating_sub(retired)
                    };
                    return self.run(batch);
                }
            }
        }

        // Sync flat regs → arch state
        let a64_mut = self
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::state_mut)
            .expect("aarch64 state");
        regs::flat_to_arch(&mut flat_regs, a64_mut);
        self.insns_retired += retired;

        StopReason::Quantum
    }

    /// Single-step one AArch64 instruction via decode() → execute().
    fn step_aarch64(&mut self) -> Result<(), HartException> {
        let pc = self
            .session
            .aarch64()
            .and_then(Aarch64Core::state)
            .ok_or(HartException::Unsupported)?
            .pc;

        probe!(
            self.probes.pre_step,
            CpuStepEvent {
                pc,
                raw: 0,
                insn_class: helm_probe::InsnClass::Unknown,
                is_stub: false,
            }
        );

        // 1. Fetch
        let raw = self
            .memory
            .fetch32(pc)
            .map_err(|_| HartException::InstructionAccessFault { addr: pc })?;

        // 2. Decode
        let insn = match aarch64_decode(raw, pc) {
            Ok(insn) => insn,
            Err(DecodeError::Unknown { raw, pc }) => {
                return Err(HartException::IllegalInstruction { pc, raw });
            }
            Err(DecodeError::Unimplemented) => {
                self.note_unimplemented_instruction(pc, raw, "DecodeUnimplemented");
                return Err(HartException::Unsupported);
            }
        };

        let (class, opcode_name, is_stub) = classify_aarch64_opcode(insn.opcode);
        let timing_class = to_timing_class(class);

        // 3. Execute — instrument memory when timing or observers need access records.
        let use_mem_instrumentation = self.plugins.has_mem_callbacks()
            || self.probes.mem.has_listeners()
            || matches!(
                timing_class,
                TimingInsnClass::Load | TimingInsnClass::Store | TimingInsnClass::Atomic
            );
        let pc_written;

        if use_mem_instrumentation {
            // Destructure to satisfy borrow checker: borrow AArch64 runtime and memory separately.
            let HelmEngine {
                ref mut session,
                ref mut memory,
                ref mut timing,
                ref mut timing_mem_model,
                ref plugins,
                ref probes,
                ..
            } = *self;
            let a64 = session
                .aarch64_mut()
                .and_then(Aarch64Core::state_mut)
                .ok_or(HartException::Unsupported)?;
            let mut imem = InstrumentedMem::new(memory);
            let (mem_class, mem_opcode_name, _) = crate::classify_aarch64_opcode(insn.opcode);
            let exec_result = aarch64_execute(&insn, a64, &mut imem);
            for rec in imem.recorded() {
                timing.on_mem_access(&estimate_timing_mem_access(
                    timing_mem_model,
                    rec.vaddr,
                    rec.size as usize,
                    rec.is_store,
                    rec.is_atomic,
                ));
                plugins.fire_mem_access(
                    0,
                    &helm_plugin::runtime::MemInfo {
                        pc,
                        raw,
                        opcode_name: mem_opcode_name,
                        class: mem_class,
                        vaddr: rec.vaddr,
                        paddr: rec.vaddr,
                        size: rec.size,
                        is_store: rec.is_store,
                        is_atomic: rec.is_atomic,
                        value_before: rec.value_before,
                        value_after: rec.value_after,
                    },
                );
            }
            for rec in imem.recorded() {
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
            pc_written = exec_result?;
            if !pc_written {
                a64.pc = a64.pc.wrapping_add(4);
            }
        } else {
            let a64 = self
                .session
                .aarch64_mut()
                .and_then(Aarch64Core::state_mut)
                .ok_or(HartException::Unsupported)?;
            pc_written = aarch64_execute(&insn, a64, &mut self.memory)?;
            if !pc_written {
                a64.pc = a64.pc.wrapping_add(4);
            }
        }

        // Probe: post-step
        probe!(
            self.probes.post_step,
            CpuStepEvent {
                pc,
                raw,
                insn_class: to_probe_class(class),
                is_stub,
            }
        );

        // Probe: branch
        if insn.is_branch() {
            let target = self
                .session
                .aarch64()
                .and_then(Aarch64Core::state)
                .map(|s| s.pc)
                .unwrap_or(pc.wrapping_add(4));
            probe!(
                self.probes.branch,
                BranchEvent {
                    pc,
                    target,
                    taken: pc_written,
                    kind: probe_branch_kind(insn.opcode),
                }
            );
            self.timing
                .on_branch(pc_written, predict_aarch64_branch(insn.opcode, pc, target));
        }

        // 4. Timing
        let tinfo = TimingInsnInfo {
            pc,
            class: timing_class,
            is_branch: insn.is_branch(),
            is_load: matches!(timing_class, TimingInsnClass::Load),
            is_store: matches!(timing_class, TimingInsnClass::Store),
            is_fp: matches!(
                timing_class,
                TimingInsnClass::FpAlu | TimingInsnClass::SimdAlu
            ),
        };
        self.timing.on_insn(&tinfo);

        // 5. Plugin callbacks
        if is_stub {
            self.note_unimplemented_instruction(pc, raw, opcode_name);
        }
        if self.plugins.has_insn_callbacks() {
            self.plugins.fire_insn_exec(
                0,
                &helm_plugin::runtime::PluginInsnInfo {
                    pc,
                    raw,
                    size: 4,
                    class,
                    opcode_name,
                    is_stub,
                    context: if let Some(a) = self.session.aarch64().and_then(Aarch64Core::state) {
                        helm_plugin::runtime::ArchContext::Aarch64 {
                            x: a.x,
                            sp: a.sp,
                            pc: a.pc,
                            nzcv: a.nzcv,
                        }
                    } else {
                        helm_plugin::runtime::ArchContext::None
                    },
                },
            );
        }

        // 6. Branch callback
        if self.plugins.has_branch_callbacks() && insn.is_branch() {
            let target = self
                .session
                .aarch64()
                .and_then(Aarch64Core::state)
                .ok_or(HartException::Unsupported)?
                .pc;
            self.plugins.fire_branch(
                0,
                &helm_plugin::runtime::BranchInfo {
                    pc,
                    target,
                    taken: pc_written,
                    kind: classify_branch_kind(insn.opcode),
                },
            );
        }

        Ok(())
    }

    fn step_aarch64_system(&mut self) -> Result<(), HartException> {
        let HelmEngine {
            ref mut session,
            ref mut timing,
            ref probes,
            ref plugins,
            ..
        } = *self;
        let machine = session
            .aarch64_mut()
            .and_then(Aarch64Core::machine_mut)
            .ok_or(HartException::Unsupported)?;
        let mut idle_fast_forward_to = None;
        let vcpu_idx = match Self::pick_next_fs_vcpu(machine) {
            Some(idx) => idx,
            None => {
                // All vCPUs are WFI-idle with no IRQs pending.
                // Fast-forward all timers to fire any pending deadlines,
                // then re-check if any CPU became schedulable.
                for i in 0..machine.vcpus.len() {
                    if !machine.vcpus[i].powered_on {
                        continue;
                    }
                    let vcpu = &mut machine.vcpus[i];
                    let mut nearest = u64::MAX;
                    if vcpu.arch.cntp_ctl_el0 & 1 != 0 {
                        nearest = nearest.min(vcpu.arch.cntp_cval_el0);
                    }
                    if vcpu.arch.cntv_ctl_el0 & 1 != 0 {
                        nearest = nearest.min(vcpu.arch.cntv_cval_el0);
                    }
                    if nearest != u64::MAX && nearest > vcpu.fs.tick {
                        vcpu.fs.tick = nearest;
                        vcpu.arch.cntvct_el0 = nearest;
                        idle_fast_forward_to = Some(
                            idle_fast_forward_to
                                .map_or(nearest, |current: u64| current.max(nearest)),
                        );
                    }
                    match machine.gic.as_ref() {
                        Some(HelmGic::V3(shared)) => {
                            arm_virt::inject_timers_gicv3(&mut vcpu.arch, &mut vcpu.fs, shared, i);
                        }
                        _ => {
                            arm_virt::inject_timers_gicv2(
                                &mut vcpu.arch,
                                &mut vcpu.fs,
                                &mut machine.sys_mem,
                            );
                        }
                    }
                }
                // Re-try after timer injection may have set IRQ lines.
                Self::pick_next_fs_vcpu(machine).ok_or(HartException::WaitForInterrupt)?
            }
        };
        if let Some(gic) = &machine.gic {
            if let HelmGic::V2(shared) = gic {
                shared.lock().unwrap().set_active_cpu(vcpu_idx);
            }
        }
        let (a64, fs_state) = {
            let vcpu = &mut machine.vcpus[vcpu_idx];
            (&mut vcpu.arch, &mut vcpu.fs)
        };
        // This CPU was selected to run — clear WFI idle state.
        fs_state.wfi_idle = false;

        // Record PC before step so we can detect and log branches afterwards.
        // pc_before is retained for future branch probe wiring (Phase 2).
        let _pc_before = a64.pc;

        // Physical timer (PPI 30, INTID 30) — level-triggered signal.
        // Countdown replaces the previous `% 1024` modulo (avoids integer division).
        self.timer_countdown -= 1;
        if self.timer_countdown == 0 {
            self.timer_countdown = TIMER_CHECK_INTERVAL;
            match machine.gic.as_ref() {
                Some(HelmGic::V3(_)) => {
                    let Some(HelmGic::V3(shared)) = machine.gic.as_ref() else {
                        unreachable!()
                    };
                    arm_virt::inject_timers_gicv3(a64, fs_state, shared, vcpu_idx);
                }
                _ => {
                    arm_virt::inject_timers_gicv2(a64, fs_state, &mut machine.sys_mem);
                }
            }
        }

        // Sync irq_pending from the GIC IRQ line (level-triggered, not edge).
        // Polled every IRQ_POLL_INTERVAL instructions instead of every instruction to avoid
        // the AtomicBool load overhead on the critical path. IRQ latency is at
        // most IRQ_POLL_INTERVAL instructions, which is acceptable for the
        // current functional FS boot model.
        //
        // Critical: ASSIGN (not OR) so irq_pending tracks the line exactly.
        // When the kernel reads GICC_IAR the line drops; on the next poll
        // irq_pending becomes false, preventing spurious re-interrupts after ERET.
        self.irq_poll_countdown -= 1;
        if self.irq_poll_countdown == 0 {
            self.irq_poll_countdown = IRQ_POLL_INTERVAL;
            fs_state.irq_pending = machine
                .irq_lines
                .get(vcpu_idx)
                .map_or(false, |l| l.load(std::sync::atomic::Ordering::Relaxed));
        }

        let result = fs::step_aarch64_fs(
            a64,
            &mut machine.sys_mem,
            fs_state,
            timing,
            probes,
            plugins,
            vcpu_idx,
            match machine.gic.as_ref() {
                Some(HelmGic::V3(shared)) => Some(shared),
                _ => None,
            },
        );

        // TLBI broadcast: if this vCPU issued a TLB invalidate, flush all
        // other vCPUs' TLBs. ARM TLBI instructions are broadcast (IS=Inner
        // Shareable) in SMP kernels. Without this, other CPUs use stale
        // translations after page table modifications, causing memory corruption.
        let needs_tlbi_broadcast = a64.tlb_flush_broadcast;
        a64.tlb_flush_broadcast = false;

        // WFI fast-forward: when the kernel executes WFI, fs.tick stops advancing
        // (step returned early before tick += 1). Jump tick forward to the nearest
        // armed timer deadline so the next timer-check interval fires immediately
        // rather than waiting for real instruction-count progress to catch up.
        // WFI fast-forward: advance the virtual tick to the nearest timer deadline
        // so the next inject_timers() call fires immediately.
        if matches!(result, Err(helm_core::HartException::WaitForInterrupt)) {
            // Mark this vCPU as idle so pick_next_fs_vcpu skips it
            // until an interrupt is pending.
            fs_state.wfi_idle = true;

            let mut nearest = u64::MAX;
            if a64.cntp_ctl_el0 & 1 != 0 {
                nearest = nearest.min(a64.cntp_cval_el0);
            }
            if a64.cntv_ctl_el0 & 1 != 0 {
                nearest = nearest.min(a64.cntv_cval_el0);
            }
            if nearest != u64::MAX && nearest > fs_state.tick {
                fs_state.tick = nearest;
                a64.cntvct_el0 = nearest;
                idle_fast_forward_to =
                    Some(idle_fast_forward_to.map_or(nearest, |current| current.max(nearest)));
            }
            match machine.gic.as_ref() {
                Some(HelmGic::V3(_)) => {
                    let Some(HelmGic::V3(shared)) = machine.gic.as_ref() else {
                        unreachable!()
                    };
                    arm_virt::inject_timers_gicv3(a64, fs_state, shared, vcpu_idx);
                }
                _ => {
                    arm_virt::inject_timers_gicv2(a64, fs_state, &mut machine.sys_mem);
                }
            }
        }
        if let Some(target_tick) = idle_fast_forward_to {
            timing.advance_to(target_tick);
        }
        // Broadcast TLBI after a64/fs_state borrows are no longer needed.
        if needs_tlbi_broadcast {
            for i in 0..machine.vcpus.len() {
                if i != vcpu_idx {
                    machine.vcpus[i].fs.tlb.flush();
                }
            }
        }
        match result {
            Err(HartException::PsciCall {
                conduit,
                function,
                arg1,
                arg2,
                arg3,
            }) => Self::handle_fs_psci_call(machine, vcpu_idx, conduit, function, arg1, arg2, arg3),
            other => other,
        }
    }

    /// Load a static AArch64 ELF binary and set up the engine for SE mode.
    ///
    /// Initialises AArch64 SE-mode state and configures the syscall handler.
    pub fn load_aarch64_elf(
        &mut self,
        path: &str,
        argv: &[&str],
        envp: &[&str],
    ) -> Result<(), String> {
        use loader::load_elf;

        let loaded = load_elf(path, argv, envp, &mut self.memory)?;

        // Pre-map the initial brk page so musl can access it before calling brk()
        // Linux always has at least one page mapped at the brk base
        {
            let zeros = vec![0u8; 0x1000];
            self.memory.load_bytes(loaded.brk_base, &zeros);
        }

        let mut state = Aarch64ArchState::new();
        state.pc = loaded.entry_point;
        state.sp = loaded.initial_sp;

        let mut handler = LinuxAarch64SyscallHandler::new(loaded.brk_base);
        handler.binary_path = path.to_string();

        self.session
            .replace_primary(HelmCore::Aarch64(Aarch64Core::Syscall { state, handler }));
        self.mode = ExecMode::Syscall;
        self.symbols = loaded.symbols;

        self.plugins.fire_vcpu_init(0);
        Ok(())
    }

    /// Load a static RISC-V 64 ELF binary and configure the engine for SE mode.
    pub fn load_riscv64_elf(
        &mut self,
        path: &str,
        argv: &[&str],
        envp: &[&str],
    ) -> Result<(), String> {
        use loader::{load_elf, setup_riscv_tp};

        let loaded = load_elf(path, argv, envp, &mut self.memory)?;

        // Pre-map one page at brk so musl can touch it before calling brk()
        {
            let zeros = vec![0u8; 0x1000];
            self.memory.load_bytes(loaded.brk_base, &zeros);
        }

        // Set up the thread-local storage block and thread pointer (tp = x4)
        let tp = setup_riscv_tp(&loaded, &mut self.memory);

        // RISC-V: PC = entry, sp = x2, tp = x4
        self.session
            .replace_primary(HelmCore::Riscv(RiscvCore::default()));
        self.riscv_mut().pc = loaded.entry_point;
        self.riscv_mut().iregs[2] = loaded.initial_sp; // sp
        self.riscv_mut().iregs[4] = tp; // tp
        let _ = self.session.set_riscv_mode(ExecMode::Syscall);
        self.isa = Isa::RiscV;
        self.mode = ExecMode::Syscall;
        self.symbols = loaded.symbols;

        let mut handler = LinuxRiscv64SyscallHandler::new(loaded.brk_base);
        handler.binary_path = path.to_string();
        let _ = self
            .session
            .set_riscv_syscall_handler(Some(Box::new(handler)));

        self.plugins.fire_vcpu_init(0);
        Ok(())
    }

    /// Load an ARM64 Linux Image and configure the engine for FS mode on arm-virt.
    ///
    /// `append` overrides the DTB `/chosen/bootargs` (highest precedence).
    pub fn load_aarch64_kernel(
        &mut self,
        kernel_path: &str,
        dtb_path: &str,
        initrd_path: Option<&str>,
        append: Option<&str>,
        num_cpus: usize,
        gic_version: arm_virt::ArmVirtGicVersion,
    ) -> Result<(), String> {
        let (boot_vcpus, sys_mem, devs, irq_lines, gic_state, quirks) =
            arm_virt::setup_arm_virt_boot_with_cpus(
                kernel_path,
                dtb_path,
                initrd_path,
                append,
                self.mem_size / (1024 * 1024),
                num_cpus,
                gic_version,
                Box::new(arm_virt::StdioCharBackend),
            )?;

        self.session
            .replace_primary(HelmCore::Aarch64(Aarch64Core::System(HelmBoard {
                sys_mem,
                vcpus: boot_vcpus
                    .into_iter()
                    .enumerate()
                    .map(|(idx, (arch, fs))| HelmVcpu {
                        arch,
                        fs,
                        powered_on: idx == 0,
                    })
                    .collect(),
                next_vcpu: 0,
                devs,
                quirks,
                irq_lines,
                gic: Some(gic_state),
            })));
        self.mode = ExecMode::System;
        self.symbols.clear();

        if let Some(machine) = self.session.aarch64().and_then(Aarch64Core::machine) {
            for idx in 0..machine.vcpus.len() {
                self.plugins.fire_vcpu_init(idx);
            }
        }
        Ok(())
    }

    /// Load an ARM64 Linux Image and configure the engine for FS mode on arm-virt,
    /// using an in-memory DTB blob instead of a filesystem path.
    pub fn load_aarch64_kernel_dtb_bytes(
        &mut self,
        kernel_path: &str,
        dtb_data: &[u8],
        initrd_path: Option<&str>,
        append: Option<&str>,
        num_cpus: usize,
        gic_version: arm_virt::ArmVirtGicVersion,
    ) -> Result<(), String> {
        let (boot_vcpus, sys_mem, devs, irq_lines, gic_state, quirks) =
            arm_virt::setup_arm_virt_boot_with_cpus_dtb_bytes(
                kernel_path,
                dtb_data,
                initrd_path,
                append,
                self.mem_size / (1024 * 1024),
                num_cpus,
                gic_version,
                Box::new(arm_virt::StdioCharBackend),
            )?;

        self.session
            .replace_primary(HelmCore::Aarch64(Aarch64Core::System(HelmBoard {
                sys_mem,
                vcpus: boot_vcpus
                    .into_iter()
                    .enumerate()
                    .map(|(idx, (arch, fs))| HelmVcpu {
                        arch,
                        fs,
                        powered_on: idx == 0,
                    })
                    .collect(),
                next_vcpu: 0,
                devs,
                quirks,
                irq_lines,
                gic: Some(gic_state),
            })));
        self.mode = ExecMode::System;
        self.symbols.clear();

        if let Some(machine) = self.session.aarch64().and_then(Aarch64Core::machine) {
            for idx in 0..machine.vcpus.len() {
                self.plugins.fire_vcpu_init(idx);
            }
        }
        Ok(())
    }

    fn step_riscv(&mut self) -> Result<(), HartException> {
        use helm_arch::riscv_expand_c;

        let pc = self.riscv().pc;

        // 1. Fetch: probe bits[1:0] to detect RVC (C extension) instructions.
        //    All 32-bit RISC-V instructions have bits[1:0] == 0b11.
        //    C-extension instructions have bits[1:0] != 0b11 (00, 01, or 10).
        let raw32 = self
            .memory
            .fetch32(pc)
            .map_err(|_| HartException::InstructionAccessFault { addr: pc })?;

        let (insn, insn_size) = if (raw32 & 0b11) != 0b11 {
            // 16-bit C-extension instruction — low 16 bits are the instruction
            let c = raw32 as u16;
            let i = riscv_expand_c(c, pc).map_err(|e| match e {
                DecodeError::Unknown { raw, pc } => HartException::IllegalInstruction {
                    pc,
                    raw: raw as u32,
                },
                DecodeError::Unimplemented => HartException::Unsupported,
            })?;
            (i, 2u64)
        } else {
            // 2. Decode 32-bit instruction
            let i = riscv_decode(raw32, pc).map_err(|e| match e {
                DecodeError::Unknown { raw, pc } => HartException::IllegalInstruction { pc, raw },
                DecodeError::Unimplemented => HartException::Unsupported,
            })?;
            (i, 4u64)
        };

        // 3. Execute
        // For C-ext: execute() writes PC += 4 for non-CF instructions, so we
        // must undo that and apply the correct insn_size advance instead.
        // Strategy: save PC before execute, then fix up if it was advanced by 4.
        let pc_before = self.riscv().pc;
        riscv_execute(insn, self)?;
        // If execute() advanced PC by exactly 4 (non-CF path), and this is a
        // C-ext instruction, correct the advance to 2.
        if insn_size == 2 && self.riscv().pc == pc_before.wrapping_add(4) {
            self.riscv_mut().pc = pc_before.wrapping_add(2);
        }

        let timing_class = classify_riscv_timing_class(&insn);
        if insn.is_control_flow() {
            let target = self.riscv().pc;
            self.timing.on_branch(
                riscv_branch_taken(&insn, target, pc.wrapping_add(insn_size)),
                predict_riscv_branch(&insn, pc, target),
            );
        }

        // 4. Timing
        let info = TimingInsnInfo {
            pc,
            class: timing_class,
            is_branch: insn.is_control_flow(),
            is_load: matches!(timing_class, TimingInsnClass::Load),
            is_store: matches!(timing_class, TimingInsnClass::Store),
            is_fp: matches!(
                timing_class,
                TimingInsnClass::FpAlu | TimingInsnClass::SimdAlu
            ),
        };
        self.timing.on_insn(&info);

        Ok(())
    }

    fn handle_exception(&mut self, exc: HartException) -> StopReason {
        match exc {
            HartException::EnvironmentCall { pc: _, nr } => {
                if self.active_mode() == ExecMode::Syscall {
                    // AArch64: syscall number from X8 (passed in `nr`), args from X0-X5
                    if self.session.active_isa().unwrap_or(self.isa) == Isa::AArch64 {
                        return self.dispatch_aarch64_syscall(nr);
                    }
                    // RISC-V: dispatch through LinuxRiscv64SyscallHandler with memory access
                    return self.dispatch_riscv_syscall(nr);
                }
                StopReason::Exception(HartException::EnvironmentCall { pc: 0, nr })
            }
            HartException::Exit { code } => StopReason::Exit { code },
            HartException::WaitForInterrupt => {
                // In FS mode, WFI is a hint; resume on next run() call.
                StopReason::Quantum
            }
            HartException::DataAbort { .. } | HartException::InstructionAbort { .. } => {
                // In FS mode, these are delivered as exceptions by the step function.
                // If we get them here, the FS handler didn't catch them.
                StopReason::Exception(exc)
            }
            HartException::PsciCall { .. } => StopReason::Exception(exc),
            HartException::Unsupported => StopReason::Unsupported,
            other => {
                // Fire plugin fault callback before returning.
                let (pc, raw_insn, kind, message) = match &other {
                    HartException::IllegalInstruction { pc, raw } => (
                        *pc,
                        *raw,
                        helm_plugin::runtime::FaultKind::IllegalInstruction,
                        format!("illegal instruction at {pc:#x} (raw={raw:#010x})"),
                    ),
                    HartException::Breakpoint { pc } => {
                        let raw = self.memory.fetch32(*pc).unwrap_or(0);
                        (
                            *pc,
                            raw,
                            helm_plugin::runtime::FaultKind::Breakpoint,
                            format!("breakpoint at {pc:#x}"),
                        )
                    }
                    HartException::InstructionAddressMisaligned { addr } => (
                        *addr,
                        0,
                        helm_plugin::runtime::FaultKind::WildJump,
                        format!("instruction address misaligned: {addr:#x}"),
                    ),
                    HartException::LoadAccessFault { addr } => (
                        0,
                        0,
                        helm_plugin::runtime::FaultKind::MemoryFault,
                        format!("load access fault at {addr:#x}"),
                    ),
                    HartException::StoreAccessFault { addr } => (
                        0,
                        0,
                        helm_plugin::runtime::FaultKind::MemoryFault,
                        format!("store/AMO access fault at {addr:#x}"),
                    ),
                    HartException::InstructionAccessFault { addr } => (
                        *addr,
                        0,
                        helm_plugin::runtime::FaultKind::MemoryFault,
                        format!("instruction access fault at {addr:#x}"),
                    ),
                    _ => (
                        0,
                        0,
                        helm_plugin::runtime::FaultKind::IllegalInstruction,
                        format!("{other}"),
                    ),
                };
                let context = if let Some(a) = self.session.aarch64().and_then(Aarch64Core::state) {
                    helm_plugin::runtime::ArchContext::Aarch64 {
                        x: a.x,
                        sp: a.sp,
                        pc: a.pc,
                        nzcv: a.nzcv,
                    }
                } else {
                    helm_plugin::runtime::ArchContext::RiscV {
                        x: self.riscv().iregs,
                        pc: self.riscv().pc,
                    }
                };
                self.plugins.fire_fault(&helm_plugin::runtime::FaultInfo {
                    vcpu_idx: 0,
                    pc,
                    raw: raw_insn,
                    kind,
                    message,
                    insn_count: self.insns_retired,
                    context,
                });
                StopReason::Exception(other)
            }
        }
    }

    /// Dispatch one AArch64 SVC syscall to `LinuxAarch64SyscallHandler`.
    fn dispatch_aarch64_syscall(&mut self, nr: u64) -> StopReason {
        // Borrow arch state and handler separately — can't borrow self twice.
        let (x0, x1, x2, x3, x4, x5) = {
            let a = self
                .session
                .aarch64()
                .and_then(Aarch64Core::state)
                .expect("a64_state missing");
            (a.x[0], a.x[1], a.x[2], a.x[3], a.x[4], a.x[5])
        };
        let args = SyscallArgs {
            a0: x0,
            a1: x1,
            a2: x2,
            a3: x3,
            a4: x4,
            a5: x5,
        };

        // Fire plugin pre-syscall event
        self.plugins
            .fire_syscall(&helm_plugin::runtime::SyscallInfo {
                vcpu_idx: 0,
                number: nr,
                args: [x0, x1, x2, x3, x4, x5],
            });

        let result = if let Some(h) = self
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::handler_mut)
        {
            h.handle(nr, args, &mut self.memory)
        } else {
            Ok(-38) // -ENOSYS if no handler
        };

        match result {
            Ok(ret) => {
                if let Some(a) = self.session.aarch64_mut().and_then(Aarch64Core::state_mut) {
                    a.x[0] = ret as u64;
                    // Advance PC past the SVC instruction
                    a.pc = a.pc.wrapping_add(4);
                }
                // Fire plugin post-syscall event
                self.plugins
                    .fire_syscall_ret(&helm_plugin::runtime::SyscallRetInfo {
                        vcpu_idx: 0,
                        number: nr,
                        ret_value: ret as u64,
                    });
                StopReason::Quantum
            }
            Err(HartException::Exit { code }) => StopReason::Exit { code },
            Err(e) => StopReason::Exception(e),
        }
    }

    /// Dispatch one RISC-V `ecall` syscall via `LinuxRiscv64SyscallHandler`.
    fn dispatch_riscv_syscall(&mut self, nr: u64) -> StopReason {
        // RISC-V Linux: a0–a5 = x10–x15, nr = x17
        let args = SyscallArgs {
            a0: self.riscv().iregs[10],
            a1: self.riscv().iregs[11],
            a2: self.riscv().iregs[12],
            a3: self.riscv().iregs[13],
            a4: self.riscv().iregs[14],
            a5: self.riscv().iregs[15],
        };

        // Use split-borrow: take handler out temporarily, call it with &mut memory,
        // then put it back. This avoids borrowing self mutably twice.
        let result = if let Some(mut handler) = self.riscv_mut().syscall_handler.take() {
            let r = handler.handle(nr, args, &mut self.memory);
            self.riscv_mut().syscall_handler = Some(handler);
            r
        } else {
            Ok(-38) // -ENOSYS
        };

        match result {
            Ok(ret) => {
                self.riscv_mut().iregs[10] = ret as u64;
                // ECALL does not advance PC automatically; execute.rs returns
                // Err(EnvironmentCall) before advancing, so we advance here.
                self.riscv_mut().pc = self.riscv().pc.wrapping_add(4);
                StopReason::Quantum
            }
            Err(HartException::Exit { code }) => StopReason::Exit { code },
            Err(e) => self.handle_exception(e),
        }
    }
}

// ── ExecContext impl for HelmEngine<T> ───────────────────────────────────────

impl<T: TimingModel> ExecContext for HelmEngine<T> {
    #[inline(always)]
    fn read_int_reg(&self, idx: usize) -> u64 {
        self.riscv().iregs[idx]
    }

    #[inline(always)]
    fn write_int_reg(&mut self, idx: usize, val: u64) {
        if idx != 0 {
            self.riscv_mut().iregs[idx] = val;
        }
    }

    #[inline(always)]
    fn read_float_reg_bits(&self, idx: usize) -> u64 {
        self.riscv().fregs[idx]
    }

    #[inline(always)]
    fn write_float_reg_bits(&mut self, idx: usize, val: u64) {
        self.riscv_mut().fregs[idx] = val;
    }

    #[inline(always)]
    fn read_csr(&self, addr: u16) -> u64 {
        self.riscv().csrs[addr as usize]
    }

    #[inline(always)]
    fn write_csr(&mut self, addr: u16, val: u64) {
        self.riscv_mut().csrs[addr as usize] = val;
    }

    #[inline(always)]
    fn read_pc(&self) -> u64 {
        self.riscv().pc
    }

    #[inline(always)]
    fn write_pc(&mut self, val: u64) {
        self.riscv_mut().pc = val;
    }

    #[inline(always)]
    fn read_mem(&mut self, addr: u64, size: usize, ty: AccessType) -> Result<u64, MemFault> {
        let val = self.memory.read(addr, size, ty)?;
        if matches!(ty, AccessType::Load | AccessType::Atomic) {
            let access = estimate_timing_mem_access(
                &mut self.timing_mem_model,
                addr,
                size,
                false,
                ty == AccessType::Atomic,
            );
            self.timing.on_mem_access(&access);
        }
        Ok(val)
    }

    #[inline(always)]
    fn write_mem(
        &mut self,
        addr: u64,
        size: usize,
        val: u64,
        ty: AccessType,
    ) -> Result<(), MemFault> {
        self.memory.write(addr, size, val, ty)?;
        if matches!(ty, AccessType::Store | AccessType::Atomic) {
            let access = estimate_timing_mem_access(
                &mut self.timing_mem_model,
                addr,
                size,
                true,
                ty == AccessType::Atomic,
            );
            self.timing.on_mem_access(&access);
        }
        Ok(())
    }
}

// ── HelmSim ───────────────────────────────────────────────────────────────────

/// The PyO3 boundary — one enum variant per timing model.
///
/// Python calls `build_simulator()` which returns a `HelmSim`.
/// All Python-facing methods dispatch into the appropriate arm.
pub enum HelmSim {
    VirtualTiming(HelmEngine<VirtualTiming>),
    IntervalTiming(HelmEngine<IntervalTiming>),
    AccurateTiming(HelmEngine<AccurateTiming>),
}

impl HelmSim {
    pub fn run(&mut self, max_insns: u64) -> StopReason {
        match self {
            Self::VirtualTiming(e) => e.run(max_insns),
            Self::IntervalTiming(e) => e.run(max_insns),
            Self::AccurateTiming(e) => e.run(max_insns),
        }
    }

    /// Enable or disable the JIT backend.
    #[cfg(feature = "jit")]
    pub fn set_jit(&mut self, enabled: bool) {
        match self {
            Self::VirtualTiming(e) => e.set_jit(enabled),
            Self::IntervalTiming(e) => e.set_jit(enabled),
            Self::AccurateTiming(e) => e.set_jit(enabled),
        }
    }

    /// Run with JIT if enabled, otherwise fall back to interpreter.
    #[cfg(feature = "jit")]
    pub fn run_jit(&mut self, max_insns: u64) -> StopReason {
        match self {
            Self::VirtualTiming(e) => e.run_jit(max_insns),
            Self::IntervalTiming(e) => e.run_jit(max_insns),
            Self::AccurateTiming(e) => e.run_jit(max_insns),
        }
    }

    pub fn insns_retired(&self) -> u64 {
        match self {
            Self::VirtualTiming(e) => e.insns_retired,
            Self::IntervalTiming(e) => e.insns_retired,
            Self::AccurateTiming(e) => e.insns_retired,
        }
    }

    pub fn current_cycles(&self) -> Tick {
        match self {
            Self::VirtualTiming(e) => e.current_cycles(),
            Self::IntervalTiming(e) => e.current_cycles(),
            Self::AccurateTiming(e) => e.current_cycles(),
        }
    }

    pub fn set_pc(&mut self, pc: u64) {
        match self {
            Self::VirtualTiming(e) => e.set_pc(pc),
            Self::IntervalTiming(e) => e.set_pc(pc),
            Self::AccurateTiming(e) => e.set_pc(pc),
        }
    }

    pub fn load_bytes(&mut self, addr: u64, bytes: &[u8]) {
        match self {
            Self::VirtualTiming(e) => e.load_bytes(addr, bytes),
            Self::IntervalTiming(e) => e.load_bytes(addr, bytes),
            Self::AccurateTiming(e) => e.load_bytes(addr, bytes),
        }
    }

    /// Load an AArch64 ELF binary and configure the engine for SE mode.
    pub fn load_aarch64_elf(
        &mut self,
        path: &str,
        argv: &[&str],
        envp: &[&str],
    ) -> Result<(), String> {
        match self {
            Self::VirtualTiming(e) => e.load_aarch64_elf(path, argv, envp),
            Self::IntervalTiming(e) => e.load_aarch64_elf(path, argv, envp),
            Self::AccurateTiming(e) => e.load_aarch64_elf(path, argv, envp),
        }
    }

    /// Load a static RISC-V 64 ELF binary and configure the simulator for SE mode.
    pub fn load_riscv64_elf(
        &mut self,
        path: &str,
        argv: &[&str],
        envp: &[&str],
    ) -> Result<(), String> {
        match self {
            Self::VirtualTiming(e) => e.load_riscv64_elf(path, argv, envp),
            Self::IntervalTiming(e) => e.load_riscv64_elf(path, argv, envp),
            Self::AccurateTiming(e) => e.load_riscv64_elf(path, argv, envp),
        }
    }

    /// Load an ARM64 Linux Image and configure the simulator for FS mode.
    ///
    /// `append` overrides the DTB `/chosen/bootargs` (highest precedence).
    pub fn load_aarch64_kernel(
        &mut self,
        kernel_path: &str,
        dtb_path: &str,
        initrd_path: Option<&str>,
        append: Option<&str>,
        num_cpus: usize,
        gic_version: arm_virt::ArmVirtGicVersion,
    ) -> Result<(), String> {
        match self {
            Self::VirtualTiming(e) => e.load_aarch64_kernel(
                kernel_path,
                dtb_path,
                initrd_path,
                append,
                num_cpus,
                gic_version,
            ),
            Self::IntervalTiming(e) => e.load_aarch64_kernel(
                kernel_path,
                dtb_path,
                initrd_path,
                append,
                num_cpus,
                gic_version,
            ),
            Self::AccurateTiming(e) => e.load_aarch64_kernel(
                kernel_path,
                dtb_path,
                initrd_path,
                append,
                num_cpus,
                gic_version,
            ),
        }
    }

    /// Load an ARM64 Linux Image using an in-memory DTB blob.
    pub fn load_aarch64_kernel_dtb_bytes(
        &mut self,
        kernel_path: &str,
        dtb_data: &[u8],
        initrd_path: Option<&str>,
        append: Option<&str>,
        num_cpus: usize,
        gic_version: arm_virt::ArmVirtGicVersion,
    ) -> Result<(), String> {
        match self {
            Self::VirtualTiming(e) => e.load_aarch64_kernel_dtb_bytes(
                kernel_path,
                dtb_data,
                initrd_path,
                append,
                num_cpus,
                gic_version,
            ),
            Self::IntervalTiming(e) => e.load_aarch64_kernel_dtb_bytes(
                kernel_path,
                dtb_data,
                initrd_path,
                append,
                num_cpus,
                gic_version,
            ),
            Self::AccurateTiming(e) => e.load_aarch64_kernel_dtb_bytes(
                kernel_path,
                dtb_data,
                initrd_path,
                append,
                num_cpus,
                gic_version,
            ),
        }
    }

    /// Get mutable reference to the plugin registry.
    pub fn plugins_mut(&mut self) -> &mut HelmPluginRegistry {
        match self {
            Self::VirtualTiming(e) => &mut e.plugins,
            Self::IntervalTiming(e) => &mut e.plugins,
            Self::AccurateTiming(e) => &mut e.plugins,
        }
    }

    /// Get the loaded ELF symbol table.
    pub fn symbols(&self) -> &[loader::ElfSymbol] {
        match self {
            Self::VirtualTiming(e) => &e.symbols,
            Self::IntervalTiming(e) => &e.symbols,
            Self::AccurateTiming(e) => &e.symbols,
        }
    }

    /// Resolve a symbol name to its address. Returns None if not found.
    pub fn resolve_symbol(&self, name: &str) -> Option<u64> {
        self.symbols()
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.addr)
    }

    pub fn has_unimplemented_instructions(&self) -> bool {
        match self {
            Self::VirtualTiming(e) => e.has_unimplemented_instructions(),
            Self::IntervalTiming(e) => e.has_unimplemented_instructions(),
            Self::AccurateTiming(e) => e.has_unimplemented_instructions(),
        }
    }

    pub fn unimplemented_instruction_count(&self) -> usize {
        match self {
            Self::VirtualTiming(e) => e.unimplemented_instruction_count(),
            Self::IntervalTiming(e) => e.unimplemented_instruction_count(),
            Self::AccurateTiming(e) => e.unimplemented_instruction_count(),
        }
    }

    /// Immutable reference to the AArch64 architectural state (if ISA == AArch64).
    pub fn a64_state(&self) -> Option<&Aarch64ArchState> {
        match self {
            Self::VirtualTiming(e) => e.session.aarch64().and_then(Aarch64Core::state),
            Self::IntervalTiming(e) => e.session.aarch64().and_then(Aarch64Core::state),
            Self::AccurateTiming(e) => e.session.aarch64().and_then(Aarch64Core::state),
        }
    }

    /// Current program counter.
    pub fn pc(&self) -> u64 {
        match self {
            Self::VirtualTiming(e) => e
                .session
                .aarch64()
                .and_then(Aarch64Core::state)
                .map_or_else(|| e.session.riscv().map_or(0, |r| r.pc), |s| s.pc),
            Self::IntervalTiming(e) => e
                .session
                .aarch64()
                .and_then(Aarch64Core::state)
                .map_or_else(|| e.session.riscv().map_or(0, |r| r.pc), |s| s.pc),
            Self::AccurateTiming(e) => e
                .session
                .aarch64()
                .and_then(Aarch64Core::state)
                .map_or_else(|| e.session.riscv().map_or(0, |r| r.pc), |s| s.pc),
        }
    }

    /// Mutable reference to the CPU probe bundle.
    pub fn probes_mut(&mut self) -> &mut CpuProbes {
        match self {
            Self::VirtualTiming(e) => &mut e.probes,
            Self::IntervalTiming(e) => &mut e.probes,
            Self::AccurateTiming(e) => &mut e.probes,
        }
    }

    /// Read `size` bytes from guest memory (for debugging).
    pub fn read_mem(&mut self, addr: u64, size: usize) -> u64 {
        use helm_core::{AccessType, MemInterface};
        match self {
            Self::VirtualTiming(e) => e
                .memory
                .read(addr, size, AccessType::Load)
                .unwrap_or(0xDEAD),
            Self::IntervalTiming(e) => e
                .memory
                .read(addr, size, AccessType::Load)
                .unwrap_or(0xDEAD),
            Self::AccurateTiming(e) => e
                .memory
                .read(addr, size, AccessType::Load)
                .unwrap_or(0xDEAD),
        }
    }

    /// Apply an ARM core model, setting MIDR_EL1 and ID registers.
    ///
    /// `model_name` is case-insensitive: `"cortex-a55"`, `"neoverse-n1"`, etc.
    /// Returns `Err(String)` for unknown names.
    pub fn set_cpu_model(&mut self, model_name: &str) -> Result<(), String> {
        let m = helm_arch::ArmCoreModel::from_name(model_name)
            .ok_or_else(|| format!("Unknown ARM core model '{model_name}'"))?;
        match self {
            Self::VirtualTiming(e) => {
                if let Some(s) = e.session.aarch64_mut().and_then(Aarch64Core::state_mut) {
                    m.apply(s);
                }
                if let Some(machine) = e.session.aarch64_mut().and_then(Aarch64Core::machine_mut) {
                    for vcpu in &mut machine.vcpus {
                        m.apply(&mut vcpu.arch);
                    }
                }
            }
            Self::IntervalTiming(e) => {
                if let Some(s) = e.session.aarch64_mut().and_then(Aarch64Core::state_mut) {
                    m.apply(s);
                }
                if let Some(machine) = e.session.aarch64_mut().and_then(Aarch64Core::machine_mut) {
                    for vcpu in &mut machine.vcpus {
                        m.apply(&mut vcpu.arch);
                    }
                }
            }
            Self::AccurateTiming(e) => {
                if let Some(s) = e.session.aarch64_mut().and_then(Aarch64Core::state_mut) {
                    m.apply(s);
                }
                if let Some(machine) = e.session.aarch64_mut().and_then(Aarch64Core::machine_mut) {
                    for vcpu in &mut machine.vcpus {
                        m.apply(&mut vcpu.arch);
                    }
                }
            }
        }
        Ok(())
    }

    /// Set the virtual-time scale factor for all vCPUs.
    /// Each instruction advances the tick counter by `scale` instead of 1.
    /// Higher values make delay loops and timer waits complete faster.
    pub fn set_tick_scale(&mut self, scale: u64) {
        let scale = scale.max(1);
        let apply = |machine: &mut crate::session::HelmBoard| {
            for vcpu in &mut machine.vcpus {
                vcpu.fs.tick_scale = scale;
            }
        };
        match self {
            Self::VirtualTiming(e) => {
                if let Some(machine) = e.session.aarch64_mut().and_then(Aarch64Core::machine_mut) {
                    apply(machine);
                }
            }
            Self::IntervalTiming(e) => {
                if let Some(machine) = e.session.aarch64_mut().and_then(Aarch64Core::machine_mut) {
                    apply(machine);
                }
            }
            Self::AccurateTiming(e) => {
                if let Some(machine) = e.session.aarch64_mut().and_then(Aarch64Core::machine_mut) {
                    apply(machine);
                }
            }
        }
    }
}

// ── build_simulator ───────────────────────────────────────────────────────────

/// Constructor called from Python config (or Rust tests).
///
/// `mem_base` and `mem_size` define the flat guest-physical memory window.
pub fn build_simulator(
    isa: Isa,
    mode: ExecMode,
    timing: TimingChoice,
    mem_base: u64,
    mem_size: usize,
) -> HelmSim {
    match timing {
        TimingChoice::VirtualTiming { ipc } => HelmSim::VirtualTiming(HelmEngine::new(
            isa,
            mode,
            VirtualTiming::new(ipc),
            mem_base,
            mem_size,
        )),
        TimingChoice::IntervalTiming { ipc, interval_len } => {
            HelmSim::IntervalTiming(HelmEngine::new(
                isa,
                mode,
                IntervalTiming::new(ipc, interval_len),
                mem_base,
                mem_size,
            ))
        }
        TimingChoice::AccurateTiming => HelmSim::AccurateTiming(HelmEngine::new(
            isa,
            mode,
            AccurateTiming::default(),
            mem_base,
            mem_size,
        )),
    }
}

/// Classify an AArch64 opcode for the plugin system.
/// Returns (InsnClass, opcode_name, is_stub).
pub(crate) fn classify_aarch64_opcode(
    op: helm_arch::aarch64::insn::Opcode,
) -> (helm_plugin::runtime::InsnClass, &'static str, bool) {
    use helm_arch::aarch64::insn::Opcode::*;
    use helm_plugin::runtime::InsnClass;

    match op {
        // Data processing
        Adr | Adrp | AddImm | SubImm | AddsImm | SubsImm | AndImm | OrrImm | EorImm | AndsImm
        | Movn | Movz | Movk | Sbfm | Bfm | Ubfm | Extr | AddReg | SubReg | AddsReg | SubsReg
        | AddExt | SubExt | AddsExt | SubsExt | AndReg | OrrReg | EorReg | AndsReg | BicReg
        | OrnReg | EonReg | BicsReg | Adc | Adcs | Sbc | Sbcs | Lsl | Lsr | Asr | Ror | Cls
        | Clz | Rev | Rev16 | Rev32 | Rbit | Csel | Csinc | Csinv | Csneg | Ccmn | Ccmp => {
            (InsnClass::IntAlu, "IntAlu", false)
        }

        Mul | Madd | Msub | Mneg | Smulh | Umulh | Sdiv | Udiv | Smaddl | Smsubl | Umaddl
        | Umsubl => (InsnClass::IntMul, "IntMul", false),

        Crc32 | Crc32c => (InsnClass::IntAlu, "Crc32", true), // stub

        // Branch
        B | Bl | Br | Blr | Ret | BCond | Cbz | Cbnz | Tbz | Tbnz | Svc | Hvc | Smc | Eret => {
            (InsnClass::Branch, "Branch", false)
        }

        // Load/Store
        Ldr | Ldrb | Ldrh | Ldrsb | Ldrsh | Ldrsw | LdrLit | LdrswLit | Ldp | Ldur | Ldurb
        | Ldurh | Ldursb | Ldursh | Ldursw | Ldxr | Ldaxr | Ldxp | Ldaxp | Ldar | LdrSimd
        | LdpSimd | LdurSimd => (InsnClass::Load, "Load", false),

        Str | Strb | Strh | Stp | Stur | Sturb | Sturh | Stxr | Stlxr | Stxp | Stlxp | Stlr
        | StrSimd | StpSimd | SturSimd => (InsnClass::Store, "Store", false),

        // Atomics
        Ldadd | Ldclr | Ldeor | Ldset | LdSmax | LdSmin | LdUmax | LdUmin | Swp | Cas | Casp => {
            (InsnClass::Atomic, "Atomic", false)
        }

        // FP
        FmovImm | FmovReg | FmovGpr | Fadd | Fsub | Fmul | Fdiv | Fsqrt | Fabs | Fneg | Fnmul
        | Fmax | Fmin | Fmaxnm | Fminnm | Fmadd | Fmsub | Fnmadd | Fnmsub | Fcmp | Fcmpe
        | Fccmp | Fccmpe | Fcvt | Fsel | FcvtzsGpr | FcvtzuGpr | ScvtfGpr | UcvtfGpr
        | FcvtnsGpr | FcvtnuGpr | FcvtmsGpr | FcvtmuGpr | FcvtpsGpr | FcvtpuGpr | FcvtasGpr
        | FcvtauGpr => (InsnClass::FpAlu, "FpAlu", false),

        // System
        Nop | Wfi | Wfe | Sev | Sevl | Yield | Dmb | Dsb | Isb | Esb | Sb | Brk | Mrs | Msr
        | MsrImm | Sys | Clrex | Prfm => (InsnClass::System, "System", false),

        DcZva => (InsnClass::System, "DcZva", false),

        // SIMD — implemented
        SimdDup | SimdIns | SimdUmov | SimdSmov | SimdMovi | SimdAdd | SimdSub | SimdMul
        | SimdAnd | SimdOrr | SimdEor | SimdBic | SimdNot | SimdNeg | SimdAbs | SimdCmeq
        | SimdCmgt | SimdCmge | SimdCmhi | SimdCmhs | SimdUmaxv | SimdUminv => {
            (InsnClass::SimdAlu, "SimdImpl", false)
        }

        // SIMD — stubs (silently skipped)
        SimdOther => (InsnClass::SimdAlu, "SimdOther", true),
        SimdBif | SimdBit | SimdBsl | SimdOrrImm => (InsnClass::SimdAlu, "SimdLogic", true),
        SimdCmgt0 | SimdCmeq0 | SimdCmlt0 | SimdCmge0 | SimdCmle0 => {
            (InsnClass::SimdAlu, "SimdCmpZero", false)
        }
        SimdCmtst => (InsnClass::SimdAlu, "SimdCmp", true),
        SimdAddp | SimdAddv => (InsnClass::SimdAlu, "SimdReduce", true),
        SimdSshl | SimdUshl | SimdSshr | SimdUshr | SimdShl => {
            (InsnClass::SimdAlu, "SimdShift", true)
        }
        SimdTbl | SimdTbx => (InsnClass::SimdAlu, "SimdTbl", true),
        SimdZip1 | SimdZip2 | SimdUzp1 | SimdUzp2 | SimdTrn1 | SimdTrn2 => {
            (InsnClass::SimdAlu, "SimdPermute", true)
        }
        SimdExt => (InsnClass::SimdAlu, "SimdExt", true),
        SimdRev64 | SimdRev32 | SimdRev16 => (InsnClass::SimdAlu, "SimdRev", true),
        SimdCnt | SimdClz => (InsnClass::SimdAlu, "SimdBitCount", true),
        SimdSxtl | SimdUxtl => (InsnClass::SimdAlu, "SimdExtend", true),
        SimdSmin | SimdUmin | SimdSmax | SimdUmax => (InsnClass::SimdAlu, "SimdMinMax", true),
        SimdFadd | SimdFsub | SimdFmul | SimdFdiv | SimdFabs | SimdFneg | SimdFsqrt | SimdFcmeq
        | SimdFcmgt | SimdFcmge | SimdFcvtzs | SimdFcvtzu | SimdScvtf | SimdUcvtf | SimdFrintm
        | SimdFrintn | SimdFrintp | SimdFrintz => (InsnClass::SimdAlu, "SimdFp", true),
        SimdMvni | SimdFmov => (InsnClass::SimdAlu, "SimdMov", true),
        SimdLd1 | SimdSt1 | SimdLd2 | SimdSt2 | SimdLd3 | SimdSt3 | SimdLd4 | SimdSt4
        | SimdLd1r => (InsnClass::SimdAlu, "SimdMultiStruct", true),

        FcvtzsVec | FcvtzuVec => (InsnClass::SimdAlu, "SimdVecCvt", true),

        ScalarAddp => (InsnClass::SimdAlu, "ScalarAddp", false),
        // v8.3/v8.4 new opcodes
        Ldapr | Ldaprh | Ldaprb => (InsnClass::Load, "LdaprRcpc", false),
        LdapurB | LdapurH | Ldapur => (InsnClass::Load, "LdapurRcpc2", false),
        StlurB | StlurH | Stlur => (InsnClass::Store, "StlurRcpc2", false),
        Fjcvtzs => (InsnClass::FpAlu, "Fjcvtzs", false),
        Fcadd | Fcmla => (InsnClass::SimdAlu, "FcmaComplex", false),
        Sdot | Udot => (InsnClass::SimdAlu, "DotProduct", false),
        Setf8 | Setf16 | Cfinv | Rmif | Xaflag | Axflag => (InsnClass::IntAlu, "FlagM", false),
        Bti => (InsnClass::System, "Bti", false),
        Sha3 | Sha512 | Sm3 | Sm4 => (InsnClass::SimdAlu, "CryptoStub", true),

        Undefined => (InsnClass::Unknown, "Undefined", false),
    }
}

/// Convert an AArch64 opcode to a `helm_probe::BranchKind`.
fn probe_branch_kind(op: helm_arch::aarch64::insn::Opcode) -> ProbeBranchKind {
    use helm_arch::aarch64::insn::Opcode::*;
    match op {
        B => ProbeBranchKind::DirectUncond,
        Bl => ProbeBranchKind::Call,
        Ret | Eret => ProbeBranchKind::Return,
        Br => ProbeBranchKind::IndirectJump,
        Blr => ProbeBranchKind::IndirectCall,
        BCond | Cbz | Cbnz | Tbz | Tbnz => ProbeBranchKind::DirectCond,
        _ => ProbeBranchKind::DirectUncond,
    }
}

/// Map a helm-plugin InsnClass to helm-probe InsnClass.
pub(crate) fn to_probe_class(c: helm_plugin::runtime::InsnClass) -> helm_probe::InsnClass {
    use helm_plugin::runtime::InsnClass as P;
    use helm_probe::InsnClass as H;
    match c {
        P::IntAlu => H::IntAlu,
        P::IntMul => H::IntMul,
        P::Branch => H::Branch,
        P::Load => H::Load,
        P::Store => H::Store,
        P::FpAlu => H::FpAlu,
        P::SimdAlu => H::SimdAlu,
        P::System => H::System,
        P::Nop => H::Nop,
        P::Atomic => H::Atomic,
        P::Unknown => H::Unknown,
    }
}

/// Map a helm-plugin InsnClass to helm-timing's TimingInsnClass.
pub(crate) fn to_timing_class(c: helm_plugin::runtime::InsnClass) -> TimingInsnClass {
    use helm_plugin::runtime::InsnClass as P;
    use TimingInsnClass as T;

    match c {
        P::IntAlu => T::IntAlu,
        P::IntMul => T::IntMul,
        P::Branch => T::Branch,
        P::Load => T::Load,
        P::Store => T::Store,
        P::FpAlu => T::FpAlu,
        P::SimdAlu => T::SimdAlu,
        P::System => T::System,
        P::Nop => T::Nop,
        P::Atomic => T::Atomic,
        P::Unknown => T::Unknown,
    }
}

pub(crate) fn predict_aarch64_branch(
    op: helm_arch::aarch64::insn::Opcode,
    pc: u64,
    target: u64,
) -> bool {
    use helm_arch::aarch64::insn::Opcode::*;

    match op {
        BCond | Cbz | Cbnz | Tbz | Tbnz => target <= pc,
        _ => true,
    }
}

fn predict_riscv_branch(insn: &helm_arch::riscv::Instruction, pc: u64, target: u64) -> bool {
    use helm_arch::riscv::Instruction::*;

    match insn {
        BEQ { .. } | BNE { .. } | BLT { .. } | BGE { .. } | BLTU { .. } | BGEU { .. } => {
            target <= pc
        }
        _ => true,
    }
}

fn riscv_branch_taken(
    insn: &helm_arch::riscv::Instruction,
    next_pc: u64,
    fallthrough: u64,
) -> bool {
    use helm_arch::riscv::Instruction::*;

    match insn {
        BEQ { .. } | BNE { .. } | BLT { .. } | BGE { .. } | BLTU { .. } | BGEU { .. } => {
            next_pc != fallthrough
        }
        _ => true,
    }
}

fn classify_riscv_timing_class(insn: &helm_arch::riscv::Instruction) -> TimingInsnClass {
    use helm_arch::riscv::Instruction::*;

    match insn {
        LB { .. }
        | LH { .. }
        | LW { .. }
        | LD { .. }
        | LBU { .. }
        | LHU { .. }
        | LWU { .. }
        | FLW { .. }
        | FLD { .. } => TimingInsnClass::Load,

        SB { .. } | SH { .. } | SW { .. } | SD { .. } | FSW { .. } | FSD { .. } => {
            TimingInsnClass::Store
        }

        JAL { .. }
        | JALR { .. }
        | BEQ { .. }
        | BNE { .. }
        | BLT { .. }
        | BGE { .. }
        | BLTU { .. }
        | BGEU { .. }
        | ECALL
        | EBREAK
        | MRET
        | SRET => TimingInsnClass::Branch,

        MUL { .. }
        | MULH { .. }
        | MULHSU { .. }
        | MULHU { .. }
        | DIV { .. }
        | DIVU { .. }
        | REM { .. }
        | REMU { .. }
        | MULW { .. }
        | DIVW { .. }
        | DIVUW { .. }
        | REMW { .. }
        | REMUW { .. }
        | CLMUL { .. }
        | CLMULH { .. }
        | CLMULR { .. } => TimingInsnClass::IntMul,

        LR_W { .. }
        | SC_W { .. }
        | AMOSWAP_W { .. }
        | AMOADD_W { .. }
        | AMOXOR_W { .. }
        | AMOAND_W { .. }
        | AMOOR_W { .. }
        | AMOMIN_W { .. }
        | AMOMAX_W { .. }
        | AMOMINU_W { .. }
        | AMOMAXU_W { .. }
        | LR_D { .. }
        | SC_D { .. }
        | AMOSWAP_D { .. }
        | AMOADD_D { .. }
        | AMOXOR_D { .. }
        | AMOAND_D { .. }
        | AMOOR_D { .. }
        | AMOMIN_D { .. }
        | AMOMAX_D { .. }
        | AMOMINU_D { .. }
        | AMOMAXU_D { .. } => TimingInsnClass::Atomic,

        FMADD_S { .. }
        | FMSUB_S { .. }
        | FNMSUB_S { .. }
        | FNMADD_S { .. }
        | FADD_S { .. }
        | FSUB_S { .. }
        | FMUL_S { .. }
        | FDIV_S { .. }
        | FSQRT_S { .. }
        | FSGNJ_S { .. }
        | FSGNJN_S { .. }
        | FSGNJX_S { .. }
        | FMIN_S { .. }
        | FMAX_S { .. }
        | FCVT_W_S { .. }
        | FCVT_WU_S { .. }
        | FCVT_L_S { .. }
        | FCVT_LU_S { .. }
        | FMV_X_W { .. }
        | FEQ_S { .. }
        | FLT_S { .. }
        | FLE_S { .. }
        | FCLASS_S { .. }
        | FCVT_S_W { .. }
        | FCVT_S_WU { .. }
        | FCVT_S_L { .. }
        | FCVT_S_LU { .. }
        | FMV_W_X { .. }
        | FMADD_D { .. }
        | FMSUB_D { .. }
        | FNMSUB_D { .. }
        | FNMADD_D { .. }
        | FADD_D { .. }
        | FSUB_D { .. }
        | FMUL_D { .. }
        | FDIV_D { .. }
        | FSQRT_D { .. }
        | FSGNJ_D { .. }
        | FSGNJN_D { .. }
        | FSGNJX_D { .. }
        | FMIN_D { .. }
        | FMAX_D { .. }
        | FCVT_S_D { .. }
        | FCVT_D_S { .. }
        | FEQ_D { .. }
        | FLT_D { .. }
        | FLE_D { .. }
        | FCLASS_D { .. }
        | FCVT_W_D { .. }
        | FCVT_WU_D { .. }
        | FCVT_L_D { .. }
        | FCVT_LU_D { .. }
        | FMV_X_D { .. }
        | FCVT_D_W { .. }
        | FCVT_D_WU { .. }
        | FCVT_D_L { .. }
        | FCVT_D_LU { .. }
        | FMV_D_X { .. } => TimingInsnClass::FpAlu,

        VSETVLI { .. }
        | VSETIVLI { .. }
        | VSETVL { .. }
        | VECTOR_OP { .. }
        | VECTOR_CRYPTO_OP { .. } => TimingInsnClass::SimdAlu,

        FENCE { .. }
        | FENCE_I
        | CSRRW { .. }
        | CSRRS { .. }
        | CSRRC { .. }
        | CSRRWI { .. }
        | CSRRSI { .. }
        | CSRRCI { .. }
        | WFI
        | SFENCE_VMA { .. }
        | XTHEAD_OP { .. } => TimingInsnClass::System,

        _ => TimingInsnClass::IntAlu,
    }
}

/// Classify an AArch64 branch opcode into a `BranchKind`.
fn classify_branch_kind(op: helm_arch::aarch64::insn::Opcode) -> helm_plugin::runtime::BranchKind {
    use helm_arch::aarch64::insn::Opcode::*;
    use helm_plugin::runtime::BranchKind;

    match op {
        B => BranchKind::DirectUncond,
        Bl => BranchKind::Call,
        Ret => BranchKind::Return,
        Br => BranchKind::IndirectJump,
        Blr => BranchKind::IndirectCall,
        BCond | Cbz | Cbnz | Tbz | Tbnz => BranchKind::DirectCond,
        _ => BranchKind::DirectUncond,
    }
}

/// Timing configuration passed to `build_simulator`.
pub enum TimingChoice {
    VirtualTiming { ipc: f64 },
    IntervalTiming { ipc: f64, interval_len: u64 },
    AccurateTiming,
}

#[cfg(test)]
mod tests;

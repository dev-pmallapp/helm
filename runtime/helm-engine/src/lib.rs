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

#![allow(
    missing_docs,
    clippy::pedantic,
    clippy::large_enum_variant,
    clippy::missing_const_for_thread_local,
    clippy::needless_range_loop,
    clippy::new_without_default,
    clippy::nonminimal_bool,
    clippy::ptr_arg,
    clippy::useless_vec
)]

mod aarch64_decode_cache;
pub mod address_space;
pub mod dispatch;
pub mod fs;
#[cfg(feature = "jit")]
mod jit;
#[cfg(feature = "jit")]
pub use helm_jit::debug::{DispatchDecision, JitDebugController, JitTraceWindow};
pub mod loader;
mod machine;
pub mod platform;
pub mod se;
pub mod session;
mod timing_operands;

pub use helm_arch;
use helm_arch::{aarch64_execute, riscv_decode, riscv_execute, Aarch64ArchState, DecodeError};
pub use helm_core::{AccessType, MemFault, MemInterface};
use helm_core::{ExecContext, HartException};
use helm_event::{EventData, EventId, EventQueue, Tick};
pub use helm_memory::FlatMem;
pub use helm_stats::JitPerfStats;
use helm_timing::{
    AccurateTiming, IntervalTiming, MemAccess, TimingInsnClass, TimingInsnInfo, TimingModel,
    VirtualTiming,
};

pub use helm_plugin;
use helm_plugin::HelmPluginRegistry;

use helm_probe::{
    probe, BranchEvent, BranchKind as ProbeBranchKind, CpuProbes, CpuStepEvent, MemAccessEvent,
};

use crate::aarch64_decode_cache::{Aarch64DecodeCache, DecodedAarch64Insn};
use crate::address_space::HelmAddressSpace;
use crate::fs::FsState;
use crate::platform::arm_virt::{self};
use crate::session::{
    Aarch64Core, BuiltAarch64System, BuiltSystem, HelmBoard, HelmCore, HelmCoreSet, HelmGic,
    HelmMachine, HelmVcpu, RiscvCore, RunStep,
};
use helm_devices::{CharBackend, Device, MessageInterruptEmitter, TickableDevice};
use helm_diag::{sim_info, sim_warn};
use helm_hw_char::Pl011;
use helm_hw_intc::GicSharedState;
use helm_hw_rtc::Pl031;
use helm_platform::{BoardQuirk, BuiltInPlatform, PlatformQuirk, QuirkKey, QuirkSet};
use se::{LinuxAarch64SyscallHandler, LinuxRiscv64SyscallHandler, SyscallArgs, SyscallHandler};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use timing_operands::{
    aarch64_timing_dst_regs, aarch64_timing_src_regs, riscv_timing_dst_regs, riscv_timing_src_regs,
};

#[derive(Debug, Error)]
pub enum EngineLoadError {
    #[error("{0}")]
    Elf(#[from] loader::ElfLoadError),
    #[error("{0}")]
    Arm64Kernel(#[from] loader::Arm64KernelLoadError),
    #[error("{0}")]
    BootPolicy(#[from] arm_virt::ArmVirtBootPolicyError),
    #[error("{0}")]
    BoardInstall(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct UnimplementedInstructionSite {
    pc: u64,
    raw: u32,
    opcode_name: &'static str,
}

const TIMER_CHECK_INTERVAL: u32 = 1024;
const TIMER_CHECK_MAX: u32 = 4096;
const IRQ_POLL_INTERVAL: u8 = 16;

fn build_single_vcpu_aarch64_system_board(
    sys_mem: HelmAddressSpace,
    devs: crate::platform::arm_virt::ArmVirtDevices,
    irq_lines: Vec<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    quirks: QuirkSet,
    gic: Option<HelmGic>,
    pci_msi: Option<MessageInterruptEmitter>,
) -> HelmBoard {
    let mut cpu = Aarch64ArchState::new();
    cpu.current_el = 1;
    cpu.spsel = true;

    HelmBoard {
        sys_mem: Box::new(sys_mem),
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
        pci_msi,
    }
}

/// Compute the next timer check countdown from the nearest enabled timer deadline.
///
/// If a timer is enabled and its compare value is in the future, the countdown is
/// the distance to that deadline (clamped to `TIMER_CHECK_MAX`). If no timer is
/// enabled or the deadline has already passed, falls back to `TIMER_CHECK_INTERVAL`.
fn next_timer_countdown(a64: &helm_arch::Aarch64ArchState, fs: &fs::FsState) -> u32 {
    let mut nearest = u64::MAX;

    // Physical timer (CNTP)
    if a64.cntp_ctl_el0 & 1 != 0 {
        // ENABLE=1
        if a64.cntp_cval_el0 > fs.tick {
            nearest = nearest.min(a64.cntp_cval_el0 - fs.tick);
        } else {
            // Already expired — check soon
            return TIMER_CHECK_INTERVAL;
        }
    }

    // Virtual timer (CNTV)
    if a64.cntv_ctl_el0 & 1 != 0 {
        if a64.cntv_cval_el0 > fs.tick {
            nearest = nearest.min(a64.cntv_cval_el0 - fs.tick);
        } else {
            return TIMER_CHECK_INTERVAL;
        }
    }

    // Hypervisor physical timer (CNTHP)
    if a64.cnthp_ctl_el2 & 1 != 0 {
        if a64.cnthp_cval_el2 > fs.tick {
            nearest = nearest.min(a64.cnthp_cval_el2 - fs.tick);
        } else {
            return TIMER_CHECK_INTERVAL;
        }
    }

    if nearest == u64::MAX {
        TIMER_CHECK_INTERVAL
    } else {
        nearest.min(u64::from(TIMER_CHECK_MAX)) as u32
    }
}
const ENGINE_EVENT_CALLBACK_CLASS: u32 = u32::MAX;

struct EngineCallbackEvent<T: TimingModel> {
    callback: Box<dyn FnOnce(&mut HelmEngine<T>) + Send>,
}

type EngineEventHandler<T> = Box<dyn FnMut(&mut HelmEngine<T>, u64, EventData) + Send>;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimingCacheConfig {
    pub size_bytes: usize,
    pub assoc: usize,
    pub line_size: usize,
}

impl TimingCacheConfig {
    pub const fn new(size_bytes: usize, assoc: usize, line_size: usize) -> Self {
        Self {
            size_bytes,
            assoc,
            line_size,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimingMemModelConfig {
    pub l1d: TimingCacheConfig,
    pub l2: TimingCacheConfig,
}

impl Default for TimingMemModelConfig {
    fn default() -> Self {
        Self {
            l1d: TimingCacheConfig::new(32 * 1024, 8, 64),
            l2: TimingCacheConfig::new(256 * 1024, 8, 64),
        }
    }
}

struct TimingCacheLevel {
    tags: Box<[u64]>,
    counts: Box<[u16]>,
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
            tags: vec![0; num_sets * assoc].into_boxed_slice(),
            counts: vec![0; num_sets].into_boxed_slice(),
            assoc,
            num_sets,
            line_bits: line_size.trailing_zeros(),
            set_bits: num_sets.trailing_zeros(),
        }
    }

    fn access(&mut self, addr: u64) -> bool {
        let set_idx = ((addr >> self.line_bits) as usize) & (self.num_sets - 1);
        let tag = addr >> (self.line_bits + self.set_bits);
        let count = usize::from(self.counts[set_idx]);
        let base = set_idx * self.assoc;
        let set = &mut self.tags[base..base + self.assoc];

        for pos in 0..count {
            if set[pos] == tag {
                if pos > 0 {
                    set[..=pos].rotate_right(1);
                }
                return true;
            }
        }

        if count < self.assoc {
            if count > 0 {
                set[..=count].rotate_right(1);
            }
            self.counts[set_idx] += 1;
        } else if self.assoc > 1 {
            set.copy_within(0..self.assoc - 1, 1);
        }
        set[0] = tag;
        false
    }
}

pub(crate) struct TimingMemModel {
    l1d: TimingCacheLevel,
    l2: TimingCacheLevel,
}

impl TimingMemModel {
    pub(crate) fn new(config: TimingMemModelConfig) -> Self {
        Self {
            l1d: TimingCacheLevel::new(
                config.l1d.size_bytes,
                config.l1d.assoc,
                config.l1d.line_size,
            ),
            l2: TimingCacheLevel::new(config.l2.size_bytes, config.l2.assoc, config.l2.line_size),
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

#[inline(always)]
pub(crate) fn aarch64_timing_info_for<T: TimingModel>(
    decoded: &DecodedAarch64Insn,
    pc: u64,
) -> TimingInsnInfo {
    let mut info = TimingInsnInfo::new_basic(
        pc,
        decoded.timing_class,
        decoded.is_branch,
        decoded.timing_is_load,
        decoded.timing_is_store,
        decoded.timing_is_fp,
    );

    if T::model_caps().needs_operand_timing {
        let (src_regs, src_reg_count) = aarch64_timing_src_regs(&decoded.insn);
        let (dst_regs, dst_reg_count) = aarch64_timing_dst_regs(&decoded.insn);
        info.src_regs = src_regs;
        info.src_reg_count = src_reg_count;
        info.dst_regs = dst_regs;
        info.dst_reg_count = dst_reg_count;
    }

    info
}

#[inline(always)]
fn riscv_timing_info_for<T: TimingModel>(
    insn: &helm_arch::riscv::Instruction,
    timing_class: TimingInsnClass,
    pc: u64,
) -> TimingInsnInfo {
    let mut info = TimingInsnInfo::new_basic(
        pc,
        timing_class,
        insn.is_control_flow(),
        matches!(timing_class, TimingInsnClass::Load),
        matches!(timing_class, TimingInsnClass::Store),
        matches!(
            timing_class,
            TimingInsnClass::FpAlu | TimingInsnClass::SimdAlu
        ),
    );

    if T::model_caps().needs_operand_timing {
        let (src_regs, src_reg_count) = riscv_timing_src_regs(insn);
        let (dst_regs, dst_reg_count) = riscv_timing_dst_regs(insn);
        info.src_regs = src_regs;
        info.src_reg_count = src_reg_count;
        info.dst_regs = dst_regs;
        info.dst_reg_count = dst_reg_count;
    }

    info
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
    /// A JIT debug breakpoint was hit at the given PC.
    Breakpoint,
}

// ── InstrumentedMem ──────────────────────────────────────────────────────────

/// Maximum memory accesses recorded per instruction. 16 covers paired
/// load/store and basic SIMD (LD1/ST1 up to 4-register). SVE/SME with
/// wider vectors may exceed this — those accesses are silently dropped
/// since we don't yet instrument SVE element-wise.
const MAX_INSTRUMENTED_ACCESSES: usize = 16;

/// Stack-allocated memory access recorder for the plugin system.
///
/// Wraps `&mut FlatMem`, delegates all accesses, and records up to
/// `MAX_INSTRUMENTED_ACCESSES` entries for post-execute callback dispatch.
struct InstrumentedMem<'a> {
    inner: &'a mut FlatMem,
    records: [MemAccessRecord; MAX_INSTRUMENTED_ACCESSES],
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
            records: [MemAccessRecord::default(); MAX_INSTRUMENTED_ACCESSES],
            count: 0,
        }
    }

    fn push(&mut self, rec: MemAccessRecord) {
        if self.count < MAX_INSTRUMENTED_ACCESSES {
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
        self.inner.write(addr, size, val, ty)?;
        self.push(MemAccessRecord {
            vaddr: addr,
            size: size as u8,
            is_store: true,
            is_atomic,
            value_before: None,
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
    aarch64_decode_cache: Aarch64DecodeCache,
    pub events: EventQueue,

    /// Total instructions retired.
    pub insns_retired: u64,
    /// Lightweight JIT instrumentation counters for performance debugging.
    jit_stats: JitPerfStats,

    /// Countdown for the FS-mode periodic timer check (fires at 0, resets to TIMER_CHECK_INTERVAL).
    timer_countdown: u32,
    /// Countdown for IRQ line polling (poll every 16 instructions instead of every instruction).
    irq_poll_countdown: u8,
    /// Countdown for throttled SMP progress logging in FS mode.
    fs_status_countdown: u32,
    /// Index of the FS-mode vCPU selected by the most recent system-mode step.
    active_fs_vcpu: usize,
    /// Runtime selected for generic debug connections. Falls back to the
    /// execution-active runtime when unset.
    debug_runtime: Option<session::HelmCoreId>,

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
    /// RISC-V64 stencil JIT backend (separate from AArch64 backend).
    #[cfg(feature = "jit-stencil")]
    jit_rv64_backend: Option<Box<helm_jit::stencil::StencilBackendRv64>>,
    /// Whether JIT execution is enabled (set via `set_jit(true)`).
    #[cfg(feature = "jit")]
    jit_enabled: bool,
    /// Runtime policy knobs for shared JIT execution helpers.
    #[cfg(feature = "jit")]
    jit_runtime_config: helm_jit::runtime::JitRuntimeConfig,
    /// Reusable buffer for decoded instructions during JIT block compilation.
    /// Cleared and reused on each cache miss to avoid per-miss heap allocation.
    #[cfg(feature = "jit")]
    jit_decode_buf: Vec<helm_arch::aarch64::insn::Instruction>,
    /// SE-mode inline TLB for JIT blocks. Populated lazily; flushed on `brk`/`mmap`.
    #[cfg(feature = "jit")]
    pub jit_se_tlb: Option<Box<helm_jit::helpers::JitSeTlb>>,
    /// Conservative trace cache placeholder for future trace-JIT activation.
    #[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
    jit_trace_cache: Option<helm_jit::trace::exit::TraceCache>,
    /// Hot backward-branch tracker for future trace-JIT activation.
    #[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
    jit_trace_recorder: Option<helm_jit::trace::recorder::TraceRecorder>,
    /// JIT debug/trace controller (breakpoints, trace windows, insn-count triggers).
    #[cfg(feature = "jit")]
    pub jit_debug: helm_jit::debug::JitDebugController,
    /// JIT-specific typed probe bundle -- zero-cost in release builds.
    #[cfg(feature = "jit")]
    pub jit_probes: helm_probe::JitProbes,
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

    fn current_probe_runtime_id(&self) -> u64 {
        self.session.active_id().0 as u64
    }

    fn selected_debug_runtime_id(&self) -> session::HelmCoreId {
        let fallback = self.session.active_id();
        match self.debug_runtime {
            Some(id) if self.session.runtimes.runtime(id).is_some() => id,
            _ => fallback,
        }
    }

    fn debug_aarch64_state_for_selected_runtime(&self) -> Option<&Aarch64ArchState> {
        let core = self.session.runtimes.runtime(self.selected_debug_runtime_id())?;
        match core {
            session::HelmCore::Aarch64(runtime) => match runtime.mode() {
                Some(ExecMode::System) => runtime.state_for_vcpu(self.active_fs_vcpu),
                _ => runtime.state(),
            },
            session::HelmCore::Riscv(_) => None,
        }
    }

    fn debug_aarch64_state_mut_for_selected_runtime(&mut self) -> Option<&mut Aarch64ArchState> {
        let runtime_id = self.selected_debug_runtime_id();
        let active_fs_vcpu = self.active_fs_vcpu;
        let core = self.session.runtimes.runtime_mut(runtime_id)?;
        match core {
            session::HelmCore::Aarch64(runtime) => match runtime.mode() {
                Some(ExecMode::System) => runtime.state_mut_for_vcpu(active_fs_vcpu),
                _ => runtime.state_mut(),
            },
            session::HelmCore::Riscv(_) => None,
        }
    }

    fn debug_riscv_state_for_selected_runtime(&self) -> Option<&RiscvCore> {
        match self.session.runtimes.runtime(self.selected_debug_runtime_id())? {
            session::HelmCore::Riscv(runtime) => Some(runtime),
            session::HelmCore::Aarch64(_) => None,
        }
    }

    fn debug_riscv_state_mut_for_selected_runtime(&mut self) -> Option<&mut RiscvCore> {
        match self
            .session
            .runtimes
            .runtime_mut(self.selected_debug_runtime_id())?
        {
            session::HelmCore::Riscv(runtime) => Some(runtime),
            session::HelmCore::Aarch64(_) => None,
        }
    }

    fn aarch64_state_for_current_context(&self) -> Option<&Aarch64ArchState> {
        let core = self.session.aarch64()?;
        match core.mode() {
            Some(ExecMode::System) => core.state_for_vcpu(self.active_fs_vcpu),
            _ => core.state(),
        }
    }

    fn aarch64_state_mut_for_current_context(&mut self) -> Option<&mut Aarch64ArchState> {
        let active_fs_vcpu = self.active_fs_vcpu;
        let core = self.session.aarch64_mut()?;
        match core.mode() {
            Some(ExecMode::System) => core.state_mut_for_vcpu(active_fs_vcpu),
            _ => core.state_mut(),
        }
    }

    fn fault_arch_context(&self) -> helm_plugin::runtime::ArchContext {
        if let Some(a64) = self.aarch64_state_for_current_context() {
            helm_plugin::runtime::ArchContext::Aarch64 {
                x: a64.x,
                sp: a64.sp,
                pc: a64.pc,
                nzcv: a64.nzcv,
                current_el: a64.current_el,
                tpidrro_el0: a64.tpidrro_el0,
            }
        } else if let Some(rv) = self.session.riscv() {
            helm_plugin::runtime::ArchContext::RiscV {
                x: rv.iregs,
                pc: rv.pc,
            }
        } else {
            sim_warn!(
                component = "helm-engine",
                "fault callback missing ISA state; using ArchContext::None"
            );
            helm_plugin::runtime::ArchContext::None
        }
    }

    #[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
    fn invalidate_jit_traces(&mut self, event: helm_jit::trace::exit::TraceInvalidationEvent) {
        if let Some(cache) = &mut self.jit_trace_cache {
            let _ = cache.invalidate_for_event_with_stats(event, &mut self.jit_stats);
        }
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
        // Sync IRQ lines for ALL vCPUs at each context switch. This ensures
        // SGIs/IPIs are visible within one scheduling round (1 instruction
        // per vCPU) rather than waiting up to IRQ_POLL_INTERVAL instructions.
        // AtomicBool::load is cheap and SMP correctness requires prompt delivery.
        for i in 0..machine.vcpus.len() {
            machine.vcpus[i].fs.irq_pending = machine
                .irq_lines
                .get(i)
                .map_or(false, |l| l.load(std::sync::atomic::Ordering::Relaxed));
            // Auto-wake WFI-idle vCPUs when an IRQ arrives.
            if machine.vcpus[i].fs.wfi_idle && machine.vcpus[i].fs.irq_pending {
                machine.vcpus[i].fs.wfi_idle = false;
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
        let caller = &machine.vcpus[vcpu_idx].arch;
        let current_sp = caller.current_sp();
        let current_el = caller.current_el.max(1);
        let current_mpidr = caller.mpidr_el1;
        let current_pc = caller.pc;
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
                            let target_sp =
                                current_sp.wrapping_sub(((target_idx + 1) as u64) * 0x10000);
                            target.arch.sp_el1 = 0;
                            target.arch.sp_el2 = 0;
                            match current_el {
                                2 => {
                                    target.arch.sp_el2 = target_sp;
                                    target.arch.sctlr_el2 = 0x0000_0800;
                                    target.arch.id_aa64pfr0_el1 =
                                        (target.arch.id_aa64pfr0_el1 & !0xF00) | 0x100;
                                }
                                _ => {
                                    target.arch.sp_el1 = target_sp;
                                    target.arch.sctlr_el1 = 0x0000_0800;
                                }
                            }
                            target.arch.current_el = current_el;
                            target.arch.spsel = true;
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
            timing_mem_model: TimingMemModel::new(TimingMemModelConfig::default()),
            session: HelmMachine::from_runtimes(runtimes),
            mem_size,
            memory: FlatMem::new(mem_base, mem_size),
            aarch64_decode_cache: Aarch64DecodeCache::new(),
            events: EventQueue::new(),
            insns_retired: 0,
            jit_stats: JitPerfStats::default(),
            timer_countdown: TIMER_CHECK_INTERVAL,
            irq_poll_countdown: IRQ_POLL_INTERVAL,
            fs_status_countdown: 50_000_000,
            active_fs_vcpu: 0,
            debug_runtime: None,
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
            #[cfg(feature = "jit-stencil")]
            jit_rv64_backend: None,
            #[cfg(feature = "jit")]
            jit_enabled: false,
            #[cfg(feature = "jit")]
            jit_runtime_config: helm_jit::runtime::DEFAULT_RUNTIME_CONFIG,
            #[cfg(feature = "jit")]
            jit_decode_buf: Vec::with_capacity(64),
            #[cfg(feature = "jit")]
            jit_se_tlb: None,
            #[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
            jit_trace_cache: None,
            #[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
            jit_trace_recorder: None,
            #[cfg(feature = "jit")]
            jit_debug: helm_jit::debug::JitDebugController::new(),
            #[cfg(feature = "jit")]
            jit_probes: helm_probe::JitProbes::default(),
        }
        .with_initial_runtime_mode(mode)
    }

    pub fn with_timing_mem_model_config(mut self, config: TimingMemModelConfig) -> Self {
        self.timing_mem_model = TimingMemModel::new(config);
        self
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

    pub fn jit_perf_stats(&self) -> JitPerfStats {
        #[allow(unused_mut)]
        let mut stats = self.jit_stats.clone();
        #[cfg(feature = "jit")]
        {
            if let Some(cache) = &self.jit_cache {
                stats.cache_entries = cache.len();
                stats.cache_promotions = cache.promotions();
                stats.cache_evictions = cache.evictions();
            }
            #[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
            if let Some(cache) = &self.jit_trace_cache {
                stats.trace_cache_entries = cache.len();
            }
        }
        stats
    }

    pub fn jit_enabled(&self) -> bool {
        #[cfg(feature = "jit")]
        {
            return self.jit_enabled;
        }
        #[cfg(not(feature = "jit"))]
        {
            false
        }
    }

    pub fn system_board_has_quirk(&self, key: QuirkKey) -> Option<bool> {
        let machine = self.session.aarch64().and_then(Aarch64Core::machine)?;
        Some(machine.has_quirk(key))
    }

    pub fn user_stage2_insn_abort_stats(&self) -> Option<(u64, u64)> {
        let machine = self.session.aarch64().and_then(Aarch64Core::machine)?;
        let mut events = 0u64;
        let mut repeats = 0u64;
        for vcpu in &machine.vcpus {
            events = events.saturating_add(vcpu.fs.user_stage2_insn_abort_events);
            repeats = repeats.saturating_add(vcpu.fs.user_stage2_insn_abort_repeats);
        }
        Some((events, repeats))
    }

    pub fn aarch64_mmu_stats(&self) -> Option<helm_arch::aarch64::mmu::TlbStats> {
        let machine = self.session.aarch64().and_then(Aarch64Core::machine)?;
        let mut stats = helm_arch::aarch64::mmu::TlbStats::default();
        for vcpu in &machine.vcpus {
            let vcpu_stats = vcpu.fs.tlb.stats();
            stats.hits = stats.hits.saturating_add(vcpu_stats.hits);
            stats.misses = stats.misses.saturating_add(vcpu_stats.misses);
            stats.stage1_walks = stats.stage1_walks.saturating_add(vcpu_stats.stage1_walks);
            stats.stage2_walks = stats.stage2_walks.saturating_add(vcpu_stats.stage2_walks);
        }
        Some(stats)
    }

    pub fn with_system_memory_mut<R>(
        &mut self,
        f: impl FnOnce(&mut HelmAddressSpace) -> R,
    ) -> Option<R> {
        let machine = self
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::machine_mut)?;
        Some(f(machine.sys_mem.as_mut()))
    }

    pub fn with_system_memory<R>(&self, f: impl FnOnce(&HelmAddressSpace) -> R) -> Option<R> {
        let machine = self.session.aarch64().and_then(Aarch64Core::machine)?;
        Some(f(machine.sys_mem.as_ref()))
    }

    pub fn with_a64_state_mut<R>(
        &mut self,
        f: impl FnOnce(&mut Aarch64ArchState) -> R,
    ) -> Option<R> {
        let state = self.aarch64_state_mut_for_current_context()?;
        Some(f(state))
    }

    pub fn with_rv64_state_mut<R>(
        &mut self,
        f: impl FnOnce(&mut session::RiscvCore) -> R,
    ) -> Option<R> {
        let state = self.session.riscv_mut()?;
        Some(f(state))
    }

    pub fn install_test_aarch64_system_board(
        &mut self,
        sys_mem: HelmAddressSpace,
    ) -> Result<(), &'static str> {
        let board = build_single_vcpu_aarch64_system_board(
            sys_mem,
            crate::platform::arm_virt::ArmVirtDevices {
                gicd_idx: 0,
                gicc_idx: 0,
                uart_idx: 0,
                rtc_idx: None,
                smmu_idx: None,
            },
            Vec::new(),
            QuirkSet::default(),
            None,
            None,
        );
        self.install_built_system(BuiltSystem::Aarch64(BuiltAarch64System { board }))
    }

    pub fn install_aarch64_system_board_v2(
        &mut self,
        sys_mem: HelmAddressSpace,
        devs: crate::platform::arm_virt::ArmVirtDevices,
        irq_lines: Vec<std::sync::Arc<std::sync::atomic::AtomicBool>>,
        gic_state: Arc<Mutex<GicSharedState>>,
    ) -> Result<(), &'static str> {
        let board = build_single_vcpu_aarch64_system_board(
            sys_mem,
            devs,
            irq_lines,
            QuirkSet::default(),
            Some(HelmGic::V2(gic_state.clone())),
            Some(arm_virt::build_arm_virt_gicv2_pci_msi_emitter(gic_state)),
        );
        self.install_built_system(BuiltSystem::Aarch64(BuiltAarch64System { board }))
    }

    pub fn install_arm_virt_board(
        &mut self,
        mem_mib: usize,
        num_cpus: usize,
        gic_version: arm_virt::ArmVirtGicVersion,
        uart_backend: Box<dyn CharBackend>,
    ) -> Result<(), &'static str> {
        let built = arm_virt::build_arm_virt_system(mem_mib, num_cpus, gic_version, uart_backend);
        self.install_built_system(built)
    }

    fn install_built_system(&mut self, built: BuiltSystem) -> Result<(), &'static str> {
        match built {
            BuiltSystem::Aarch64(BuiltAarch64System { board }) => {
                if self.isa != Isa::AArch64 {
                    return Err("AArch64 system board helper requires AArch64 engine");
                }

                self.session
                    .replace_primary(HelmCore::Aarch64(Aarch64Core::System(board)));
                self.mode = ExecMode::System;
                self.active_fs_vcpu = 0;
                self.symbols.clear();

                if let Some(machine) = self.session.aarch64().and_then(Aarch64Core::machine) {
                    for idx in 0..machine.vcpus.len() {
                        self.plugins.fire_vcpu_init(idx);
                    }
                }
            }
        }
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
        F: FnMut(&mut HelmEngine<T>, u64, EventData) + Send + 'static,
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
        F: FnMut(&mut HelmEngine<T>, u64, EventData) + Send + 'static,
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
            let EventData::Boxed(boxed) = data else {
                return;
            };
            let event = *boxed
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
            let EventData::Boxed(boxed) = data else {
                return;
            };
            let event = *boxed
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
        if self.events.is_empty() {
            self.events.advance_to(target_tick);
            return;
        }
        if self
            .events
            .peek_next_tick()
            .is_some_and(|next_tick| next_tick > target_tick)
        {
            self.events.advance_to(target_tick);
            return;
        }

        let mut queue = std::mem::take(&mut self.events);
        let mut callbacks: Vec<Box<dyn FnOnce(&mut HelmEngine<T>) + Send>> = Vec::new();
        let mut pending_events: Vec<(u32, u64, EventData)> = Vec::new();

        queue.drain_until(target_tick, |class_id, owner_id, data| {
            if class_id == ENGINE_EVENT_CALLBACK_CLASS {
                if let EventData::Boxed(boxed) = data {
                    let event = *boxed
                        .downcast::<EngineCallbackEvent<T>>()
                        .expect("engine callback event payload must match timing specialization");
                    callbacks.push(event.callback);
                }
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

    #[inline(always)]
    fn sync_events_to_timing(&mut self) {
        self.events.advance_to(self.timing.current_cycles());
    }

    #[inline(always)]
    fn idealized_fast_run_eligible(&self) -> bool {
        let caps = T::model_caps();
        caps.idealized_fast_run
            && !helm_diag::is_monitor_active()
            && self.events.is_empty()
            && !self.probes.any_active()
            && !self.plugins.has_any_callbacks()
    }

    fn run_idealized_fast(&mut self, max_insns: u64) -> StopReason {
        for _ in 0..max_insns {
            helm_probe::update_probe_runtime_id(self.current_probe_runtime_id());
            let result = match self.session.active_isa().unwrap_or(self.isa) {
                Isa::RiscV => self.step_riscv(),
                Isa::AArch64 => {
                    if self.active_mode() == ExecMode::System {
                        self.step_aarch64_system()
                    } else {
                        self.step_aarch64()
                    }
                }
                Isa::AArch32 => {
                    self.sync_events_to_timing();
                    return StopReason::Unsupported;
                }
            };

            match result {
                Ok(()) => {
                    self.insns_retired += 1;
                    self.session.on_progress(RunStep::RetiredInstruction);
                }
                Err(exc) => {
                    let stop = self.handle_exception(exc);
                    let exit_code = self
                        .session
                        .aarch64()
                        .and_then(Aarch64Core::handler)
                        .and_then(|h| h.should_exit.then_some(h.exit_code));
                    if let Some(code) = exit_code {
                        self.sync_events_to_timing();
                        return StopReason::Exit { code };
                    }
                    match stop {
                        StopReason::Quantum => {
                            self.insns_retired += 1;
                            self.session.on_progress(RunStep::YieldedQuantum);
                        }
                        ref s @ StopReason::Exit { .. } | ref s @ StopReason::Exception(_) => {
                            self.sync_events_to_timing();
                            self.plugins.fire_vcpu_exit(0);
                            return s.clone();
                        }
                        other => {
                            self.sync_events_to_timing();
                            return other;
                        }
                    }
                }
            }
        }

        self.sync_events_to_timing();
        StopReason::Quantum
    }

    /// Run up to `max_insns` instructions. Returns the reason for stopping.
    pub fn run(&mut self, max_insns: u64) -> StopReason {
        if self.idealized_fast_run_eligible() {
            return self.run_idealized_fast(max_insns);
        }

        for _ in 0..max_insns {
            if helm_diag::is_monitor_active() {
                helm_diag::update_sim_ctx(self.insns_retired, 1_000_000_000);
            }
            helm_probe::update_probe_runtime_id(self.current_probe_runtime_id());
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

        // 2. Decode (cache lookup avoids redundant decode; copy is ~80 bytes but
        // returning a reference conflicts with later &mut self borrows).
        let decoded = if let Some(decoded) = self.aarch64_decode_cache.lookup(pc, raw) {
            decoded
        } else {
            let decoded = match DecodedAarch64Insn::decode(raw, pc) {
                Ok(decoded) => decoded,
                Err(DecodeError::Unknown { raw, pc }) => {
                    return Err(HartException::IllegalInstruction { pc, raw });
                }
                Err(DecodeError::Unimplemented) => {
                    self.note_unimplemented_instruction(pc, raw, "DecodeUnimplemented");
                    return Err(HartException::Unsupported);
                }
            };
            self.aarch64_decode_cache.insert(pc, decoded);
            decoded
        };

        // 3. Execute — instrument memory when timing or observers need access records.
        let use_mem_instrumentation = self.plugins.has_mem_callbacks()
            || self.probes.mem.has_listeners()
            || (T::model_caps().needs_mem_access_timing && decoded.records_mem_access);
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
            let exec_result = aarch64_execute(&decoded.insn, a64, &mut imem, Some(probes));
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
                        opcode_name: decoded.opcode_name,
                        class: decoded.class,
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
            pc_written = dispatch::dispatch(&decoded.insn, a64, &mut self.memory)?;
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
                insn_class: decoded.probe_class,
                is_stub: decoded.is_stub,
            }
        );

        // Probe: branch
        if decoded.is_branch {
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
                    kind: decoded.probe_branch_kind,
                }
            );
            self.timing
                .on_branch(pc_written, decoded.predict_branch(pc, target));
        }

        // 4. Timing
        let tinfo = aarch64_timing_info_for::<T>(&decoded, pc);
        self.timing.on_insn(&tinfo);

        // 5. Plugin callbacks
        if decoded.is_stub {
            self.note_unimplemented_instruction(pc, raw, decoded.opcode_name);
        }
        if self.plugins.has_insn_callbacks() {
            self.plugins.fire_insn_exec(
                0,
                &helm_plugin::runtime::PluginInsnInfo {
                    pc,
                    raw,
                    size: 4,
                    class: decoded.class,
                    opcode_name: decoded.opcode_name,
                    is_stub: decoded.is_stub,
                    context: if let Some(a) = self.session.aarch64().and_then(Aarch64Core::state) {
                        helm_plugin::runtime::ArchContext::Aarch64 {
                            x: a.x,
                            sp: a.sp,
                            pc: a.pc,
                            nzcv: a.nzcv,
                            current_el: a.current_el,
                            tpidrro_el0: a.tpidrro_el0,
                        }
                    } else {
                        helm_plugin::runtime::ArchContext::None
                    },
                },
            );
        }

        // 6. Branch callback
        if self.plugins.has_branch_callbacks() && decoded.is_branch {
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
                    kind: decoded.plugin_branch_kind,
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
            ref mut active_fs_vcpu,
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
                    if vcpu.arch.cnthp_ctl_el2 & 1 != 0 {
                        nearest = nearest.min(vcpu.arch.cnthp_cval_el2);
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
        if let Some(HelmGic::V2(shared)) = &machine.gic {
            shared.lock().unwrap().set_active_cpu(vcpu_idx);
        }
        *active_fs_vcpu = vcpu_idx;
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
        // Dynamic countdown: recomputed from next timer deadline after each check.
        self.timer_countdown -= 1;
        if self.timer_countdown == 0 {
            self.timer_countdown = next_timer_countdown(a64, fs_state);
            match machine.gic.as_ref() {
                Some(HelmGic::V3(shared)) => {
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
            machine.pci_msi.as_ref(),
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
            if a64.cnthp_ctl_el2 & 1 != 0 {
                nearest = nearest.min(a64.cnthp_cval_el2);
            }
            if nearest != u64::MAX && nearest > fs_state.tick {
                fs_state.tick = nearest;
                a64.cntvct_el0 = nearest;
                idle_fast_forward_to =
                    Some(idle_fast_forward_to.map_or(nearest, |current| current.max(nearest)));
            }
            match machine.gic.as_ref() {
                Some(HelmGic::V3(shared)) => {
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
        // Also flush other vCPUs' decode caches: code patching (alternatives,
        // modules) writes new instructions then issues TLBI+ISB. Without
        // flushing, other vCPUs execute stale cached decodes for patched PAs.
        if needs_tlbi_broadcast {
            for i in 0..machine.vcpus.len() {
                if i != vcpu_idx {
                    machine.vcpus[i].fs.tlb.flush();
                    machine.vcpus[i].fs.decode_cache.flush();
                }
            }
            #[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
            if let Some(cache) = &mut self.jit_trace_cache {
                let _ = cache.invalidate_for_event_with_stats(
                    helm_jit::trace::exit::TraceInvalidationEvent::CodePatch,
                    &mut self.jit_stats,
                );
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
    ) -> Result<(), EngineLoadError> {
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
    ) -> Result<(), EngineLoadError> {
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
        boot_el: Option<u8>,
    ) -> Result<(), EngineLoadError> {
        let boot_policy = arm_virt::arm_virt_boot_policy_from_override(boot_el)?;
        let built = arm_virt::build_loaded_arm_virt_system(
            kernel_path,
            dtb_path,
            initrd_path,
            append,
            self.mem_size / (1024 * 1024),
            num_cpus,
            gic_version,
            boot_policy,
            Box::new(arm_virt::StdioCharBackend),
        )?;
        self.install_built_system(built)
            .map_err(EngineLoadError::BoardInstall)
    }

    /// Load an ARM64 Linux Image and configure the engine for FS mode on
    /// arm-virt using a Rust-generated baseline DTB.
    pub fn load_aarch64_kernel_auto_dtb(
        &mut self,
        kernel_path: &str,
        initrd_path: Option<&str>,
        append: Option<&str>,
        num_cpus: usize,
        gic_version: arm_virt::ArmVirtGicVersion,
        boot_el: Option<u8>,
    ) -> Result<(), EngineLoadError> {
        let boot_policy = arm_virt::arm_virt_boot_policy_from_override(boot_el)?;
        let built = arm_virt::build_loaded_arm_virt_system_auto_dtb(
            kernel_path,
            initrd_path,
            append,
            self.mem_size / (1024 * 1024),
            num_cpus,
            gic_version,
            boot_policy,
            Box::new(arm_virt::StdioCharBackend),
        )?;
        self.install_built_system(built)
            .map_err(EngineLoadError::BoardInstall)
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
        boot_el: Option<u8>,
    ) -> Result<(), EngineLoadError> {
        let boot_policy = arm_virt::arm_virt_boot_policy_from_override(boot_el)?;
        let built = arm_virt::build_loaded_arm_virt_system_dtb_bytes(
            kernel_path,
            dtb_data,
            initrd_path,
            append,
            self.mem_size / (1024 * 1024),
            num_cpus,
            gic_version,
            boot_policy,
            Box::new(arm_virt::StdioCharBackend),
        )?;
        self.install_built_system(built)
            .map_err(EngineLoadError::BoardInstall)
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
        let info = riscv_timing_info_for::<T>(&insn, timing_class, pc);
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
                self.plugins.fire_fault(&helm_plugin::runtime::FaultInfo {
                    vcpu_idx: 0,
                    pc,
                    raw: raw_insn,
                    kind,
                    message,
                    insn_count: self.insns_retired,
                    context: self.fault_arch_context(),
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

                // Flush SE-mode JIT TLB after memory-layout-changing syscalls.
                // brk/munmap/mmap can change the guest PA → host pointer mapping.
                #[cfg(feature = "jit")]
                {
                    use crate::se::linux_aarch64::nr;
                    if matches!(nr, nr::BRK | nr::MUNMAP | nr::MMAP) {
                        if let Some(tlb) = &mut self.jit_se_tlb {
                            tlb.flush();
                        }
                        #[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
                        self.invalidate_jit_traces(
                            helm_jit::trace::exit::TraceInvalidationEvent::AddressSpaceChange,
                        );
                    }
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
                #[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
                {
                    if matches!(nr, 214 | 215 | 222) {
                        self.invalidate_jit_traces(
                            helm_jit::trace::exit::TraceInvalidationEvent::AddressSpaceChange,
                        );
                    }
                }
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

struct HelmSimGdbTarget<'a> {
    sim: RefCell<&'a mut HelmSim>,
}

#[derive(Clone, Copy)]
enum DebugArchDispatch {
    Aarch64,
    RiscV,
}

impl<'a> HelmSimGdbTarget<'a> {
    fn new(sim: &'a mut HelmSim) -> Self {
        Self {
            sim: RefCell::new(sim),
        }
    }

    fn active_arch(&self) -> Option<DebugArchDispatch> {
        let sim = self.sim.borrow();
        match sim.active_debug_arch() {
            Some(Isa::AArch64) => Some(DebugArchDispatch::Aarch64),
            Some(Isa::RiscV) => Some(DebugArchDispatch::RiscV),
            _ => None,
        }
    }
}

impl helm_debug::GdbTarget for HelmSimGdbTarget<'_> {
    fn read_register(&self, reg_num: usize) -> Option<u64> {
        let sim = self.sim.borrow();
        if reg_num == 32 {
            Some(sim.debug_pc())
        } else {
            sim.debug_read_gpr(reg_num)
        }
    }

    fn write_register(&mut self, reg_num: usize, val: u64) -> bool {
        let mut sim = self.sim.borrow_mut();
        if reg_num == 32 {
            return sim.debug_set_pc(val);
        }
        sim.debug_write_gpr(reg_num, val)
    }

    fn read_memory(&self, addr: u64, len: usize) -> Option<Vec<u8>> {
        let mut sim = self.sim.borrow_mut();
        let mut bytes = Vec::with_capacity(len);
        for offset in 0..len {
            let value = sim.read_mem(addr + offset as u64, 1);
            bytes.push(value as u8);
        }
        Some(bytes)
    }

    fn write_memory(&mut self, addr: u64, data: &[u8]) -> bool {
        self.sim.borrow_mut().load_bytes(addr, data);
        true
    }

    fn step(&mut self) -> u64 {
        let mut sim = self.sim.borrow_mut();
        let _ = sim.run(1);
        sim.pc()
    }

    fn continue_exec(&mut self) -> helm_debug::StopReason {
        let mut sim = self.sim.borrow_mut();
        loop {
            #[cfg(feature = "jit")]
            let stop = if sim.jit_enabled() {
                sim.run_jit(100_000)
            } else {
                sim.run(100_000)
            };
            #[cfg(not(feature = "jit"))]
            let stop = sim.run(100_000);

            match stop {
                StopReason::Quantum => continue,
                StopReason::Exit { code } => return helm_debug::StopReason::Exited(code),
                StopReason::Breakpoint => return helm_debug::StopReason::Breakpoint(sim.pc()),
                StopReason::Exception(_) => return helm_debug::StopReason::Signal(5),
                StopReason::Unsupported => return helm_debug::StopReason::Signal(4),
            }
        }
    }

    fn set_breakpoint(&mut self, addr: u64) -> bool {
        #[cfg(feature = "jit")]
        {
            return self.sim.borrow_mut().add_jit_breakpoint(addr);
        }
        #[cfg(not(feature = "jit"))]
        {
            let _ = addr;
            false
        }
    }

    fn remove_breakpoint(&mut self, addr: u64) -> bool {
        #[cfg(feature = "jit")]
        {
            return self.sim.borrow_mut().remove_jit_breakpoint(addr);
        }
        #[cfg(not(feature = "jit"))]
        {
            let _ = addr;
            false
        }
    }

    fn num_registers(&self) -> usize {
        match self.active_arch() {
            Some(DebugArchDispatch::Aarch64) | Some(DebugArchDispatch::RiscV) => 33,
            None => 0,
        }
    }

    fn read_pc(&self) -> u64 {
        self.sim.borrow().pc()
    }
}

fn debug_connection_arch_label(isa: Isa) -> String {
    match isa {
        Isa::AArch64 => "aarch64",
        Isa::RiscV => "riscv64",
        Isa::AArch32 => "aarch32",
    }
    .to_string()
}

fn debug_connection_mode_label(mode: ExecMode) -> String {
    match mode {
        ExecMode::Functional => "functional",
        ExecMode::Syscall => "syscall",
        ExecMode::System => "system",
    }
    .to_string()
}

fn debug_connection_role_label(role: session::HelmCoreRole) -> String {
    match role {
        session::HelmCoreRole::PrimaryCpu => "primary_cpu",
        session::HelmCoreRole::Cpu => "cpu",
        session::HelmCoreRole::Accelerator => "accelerator",
        session::HelmCoreRole::Service => "service",
    }
    .to_string()
}

impl HelmSim {
    pub fn run(&mut self, max_insns: u64) -> StopReason {
        match self {
            Self::VirtualTiming(e) => e.run(max_insns),
            Self::IntervalTiming(e) => e.run(max_insns),
            Self::AccurateTiming(e) => e.run(max_insns),
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

    pub fn jit_perf_stats(&self) -> JitPerfStats {
        match self {
            Self::VirtualTiming(e) => e.jit_perf_stats(),
            Self::IntervalTiming(e) => e.jit_perf_stats(),
            Self::AccurateTiming(e) => e.jit_perf_stats(),
        }
    }

    pub fn jit_enabled(&self) -> bool {
        match self {
            Self::VirtualTiming(e) => e.jit_enabled(),
            Self::IntervalTiming(e) => e.jit_enabled(),
            Self::AccurateTiming(e) => e.jit_enabled(),
        }
    }

    pub fn user_stage2_insn_abort_stats(&self) -> Option<(u64, u64)> {
        match self {
            Self::VirtualTiming(e) => e.user_stage2_insn_abort_stats(),
            Self::IntervalTiming(e) => e.user_stage2_insn_abort_stats(),
            Self::AccurateTiming(e) => e.user_stage2_insn_abort_stats(),
        }
    }

    pub fn aarch64_mmu_stats(&self) -> Option<helm_arch::aarch64::mmu::TlbStats> {
        match self {
            Self::VirtualTiming(e) => e.aarch64_mmu_stats(),
            Self::IntervalTiming(e) => e.aarch64_mmu_stats(),
            Self::AccurateTiming(e) => e.aarch64_mmu_stats(),
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
    ) -> Result<(), EngineLoadError> {
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
    ) -> Result<(), EngineLoadError> {
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
        boot_el: Option<u8>,
    ) -> Result<(), EngineLoadError> {
        match self {
            Self::VirtualTiming(e) => e.load_aarch64_kernel(
                kernel_path,
                dtb_path,
                initrd_path,
                append,
                num_cpus,
                gic_version,
                boot_el,
            ),
            Self::IntervalTiming(e) => e.load_aarch64_kernel(
                kernel_path,
                dtb_path,
                initrd_path,
                append,
                num_cpus,
                gic_version,
                boot_el,
            ),
            Self::AccurateTiming(e) => e.load_aarch64_kernel(
                kernel_path,
                dtb_path,
                initrd_path,
                append,
                num_cpus,
                gic_version,
                boot_el,
            ),
        }
    }

    /// Load an ARM64 Linux Image using a Rust-generated baseline DTB.
    pub fn load_aarch64_kernel_auto_dtb(
        &mut self,
        kernel_path: &str,
        initrd_path: Option<&str>,
        append: Option<&str>,
        num_cpus: usize,
        gic_version: arm_virt::ArmVirtGicVersion,
        boot_el: Option<u8>,
    ) -> Result<(), EngineLoadError> {
        match self {
            Self::VirtualTiming(e) => e.load_aarch64_kernel_auto_dtb(
                kernel_path,
                initrd_path,
                append,
                num_cpus,
                gic_version,
                boot_el,
            ),
            Self::IntervalTiming(e) => e.load_aarch64_kernel_auto_dtb(
                kernel_path,
                initrd_path,
                append,
                num_cpus,
                gic_version,
                boot_el,
            ),
            Self::AccurateTiming(e) => e.load_aarch64_kernel_auto_dtb(
                kernel_path,
                initrd_path,
                append,
                num_cpus,
                gic_version,
                boot_el,
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
        boot_el: Option<u8>,
    ) -> Result<(), EngineLoadError> {
        match self {
            Self::VirtualTiming(e) => e.load_aarch64_kernel_dtb_bytes(
                kernel_path,
                dtb_data,
                initrd_path,
                append,
                num_cpus,
                gic_version,
                boot_el,
            ),
            Self::IntervalTiming(e) => e.load_aarch64_kernel_dtb_bytes(
                kernel_path,
                dtb_data,
                initrd_path,
                append,
                num_cpus,
                gic_version,
                boot_el,
            ),
            Self::AccurateTiming(e) => e.load_aarch64_kernel_dtb_bytes(
                kernel_path,
                dtb_data,
                initrd_path,
                append,
                num_cpus,
                gic_version,
                boot_el,
            ),
        }
    }

    /// Get mutable reference to the legacy callback-plugin registry.
    ///
    /// Prefer probe/session-backed observation (`helm-probe` / `helm-spy`)
    /// for new instrumentation code. This accessor remains for compatibility
    /// with existing plugin-backed helpers.
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
            Self::VirtualTiming(e) => e.aarch64_state_for_current_context(),
            Self::IntervalTiming(e) => e.aarch64_state_for_current_context(),
            Self::AccurateTiming(e) => e.aarch64_state_for_current_context(),
        }
    }

    /// Current program counter.
    pub fn pc(&self) -> u64 {
        match self {
            Self::VirtualTiming(e) => e
                .aarch64_state_for_current_context()
                .map_or_else(|| e.session.riscv().map_or(0, |r| r.pc), |s| s.pc),
            Self::IntervalTiming(e) => e
                .aarch64_state_for_current_context()
                .map_or_else(|| e.session.riscv().map_or(0, |r| r.pc), |s| s.pc),
            Self::AccurateTiming(e) => e
                .aarch64_state_for_current_context()
                .map_or_else(|| e.session.riscv().map_or(0, |r| r.pc), |s| s.pc),
        }
    }

    pub fn active_debug_arch(&self) -> Option<Isa> {
        match self {
            Self::VirtualTiming(e) => e
                .session
                .runtimes
                .runtime(e.selected_debug_runtime_id())
                .map(session::HelmCore::isa),
            Self::IntervalTiming(e) => e
                .session
                .runtimes
                .runtime(e.selected_debug_runtime_id())
                .map(session::HelmCore::isa),
            Self::AccurateTiming(e) => e
                .session
                .runtimes
                .runtime(e.selected_debug_runtime_id())
                .map(session::HelmCore::isa),
        }
    }

    pub fn debug_pc(&self) -> u64 {
        match self.active_debug_arch() {
            Some(Isa::AArch64) => match self {
                Self::VirtualTiming(e) => e
                    .debug_aarch64_state_for_selected_runtime()
                    .map_or(0, |state| state.pc),
                Self::IntervalTiming(e) => e
                    .debug_aarch64_state_for_selected_runtime()
                    .map_or(0, |state| state.pc),
                Self::AccurateTiming(e) => e
                    .debug_aarch64_state_for_selected_runtime()
                    .map_or(0, |state| state.pc),
            },
            Some(Isa::RiscV) => match self {
                Self::VirtualTiming(e) => e
                    .debug_riscv_state_for_selected_runtime()
                    .map_or(0, |state| state.pc),
                Self::IntervalTiming(e) => e
                    .debug_riscv_state_for_selected_runtime()
                    .map_or(0, |state| state.pc),
                Self::AccurateTiming(e) => e
                    .debug_riscv_state_for_selected_runtime()
                    .map_or(0, |state| state.pc),
            },
            _ => 0,
        }
    }

    pub fn debug_read_gpr(&self, n: usize) -> Option<u64> {
        match self.active_debug_arch()? {
            Isa::AArch64 => match self {
                Self::VirtualTiming(e) => e.debug_aarch64_state_for_selected_runtime(),
                Self::IntervalTiming(e) => e.debug_aarch64_state_for_selected_runtime(),
                Self::AccurateTiming(e) => e.debug_aarch64_state_for_selected_runtime(),
            }
            .map(|state| {
                if n < 31 {
                    state.x[n]
                } else {
                    state.current_sp()
                }
            }),
            Isa::RiscV => match self {
                Self::VirtualTiming(e) => e.debug_riscv_state_for_selected_runtime(),
                Self::IntervalTiming(e) => e.debug_riscv_state_for_selected_runtime(),
                Self::AccurateTiming(e) => e.debug_riscv_state_for_selected_runtime(),
            }
            .and_then(|state| state.iregs.get(n).copied()),
            Isa::AArch32 => None,
        }
    }

    pub fn debug_sp(&self) -> Option<u64> {
        match self.active_debug_arch()? {
            Isa::AArch64 => self.debug_read_gpr(31),
            Isa::RiscV => self.debug_read_gpr(2),
            Isa::AArch32 => None,
        }
    }

    pub fn debug_set_pc(&mut self, pc: u64) -> bool {
        match self.active_debug_arch() {
            Some(Isa::AArch64) => match self {
                Self::VirtualTiming(e) => e.debug_aarch64_state_mut_for_selected_runtime(),
                Self::IntervalTiming(e) => e.debug_aarch64_state_mut_for_selected_runtime(),
                Self::AccurateTiming(e) => e.debug_aarch64_state_mut_for_selected_runtime(),
            }
            .map(|state| state.pc = pc)
            .is_some(),
            Some(Isa::RiscV) => match self {
                Self::VirtualTiming(e) => e.debug_riscv_state_mut_for_selected_runtime(),
                Self::IntervalTiming(e) => e.debug_riscv_state_mut_for_selected_runtime(),
                Self::AccurateTiming(e) => e.debug_riscv_state_mut_for_selected_runtime(),
            }
            .map(|state| state.pc = pc)
            .is_some(),
            _ => false,
        }
    }

    pub fn debug_write_gpr(&mut self, n: usize, val: u64) -> bool {
        match self.active_debug_arch() {
            Some(Isa::AArch64) => {
                if n > 31 {
                    return false;
                }
                match self {
                    Self::VirtualTiming(e) => e.debug_aarch64_state_mut_for_selected_runtime(),
                    Self::IntervalTiming(e) => e.debug_aarch64_state_mut_for_selected_runtime(),
                    Self::AccurateTiming(e) => e.debug_aarch64_state_mut_for_selected_runtime(),
                }
                .map(|state| {
                    if n < 31 {
                        state.x[n] = val;
                    } else {
                        state.write_xsp(31, val);
                    }
                })
                .is_some()
            }
            Some(Isa::RiscV) => {
                if n >= 32 {
                    return false;
                }
                match self {
                    Self::VirtualTiming(e) => e.debug_riscv_state_mut_for_selected_runtime(),
                    Self::IntervalTiming(e) => e.debug_riscv_state_mut_for_selected_runtime(),
                    Self::AccurateTiming(e) => e.debug_riscv_state_mut_for_selected_runtime(),
                }
                .map(|state| {
                    if n != 0 {
                        state.iregs[n] = val;
                    }
                })
                .is_some()
            }
            _ => false,
        }
    }

    pub fn debug_vn(&self, n: usize) -> Option<(u64, u64)> {
        match self {
            Self::VirtualTiming(e) => e.debug_aarch64_state_for_selected_runtime(),
            Self::IntervalTiming(e) => e.debug_aarch64_state_for_selected_runtime(),
            Self::AccurateTiming(e) => e.debug_aarch64_state_for_selected_runtime(),
        }
        .map(|state| {
            let val = state.v[n];
            (val as u64, (val >> 64) as u64)
        })
    }

    pub fn debug_nzcv(&self) -> Option<u32> {
        match self {
            Self::VirtualTiming(e) => e
                .debug_aarch64_state_for_selected_runtime()
                .map(|state| state.nzcv),
            Self::IntervalTiming(e) => e
                .debug_aarch64_state_for_selected_runtime()
                .map(|state| state.nzcv),
            Self::AccurateTiming(e) => e
                .debug_aarch64_state_for_selected_runtime()
                .map(|state| state.nzcv),
        }
    }

    pub fn debug_current_el(&self) -> Option<u8> {
        match self {
            Self::VirtualTiming(e) => e
                .debug_aarch64_state_for_selected_runtime()
                .map(|state| state.current_el),
            Self::IntervalTiming(e) => e
                .debug_aarch64_state_for_selected_runtime()
                .map(|state| state.current_el),
            Self::AccurateTiming(e) => e
                .debug_aarch64_state_for_selected_runtime()
                .map(|state| state.current_el),
        }
    }

    pub fn debug_daif(&self) -> Option<u32> {
        match self {
            Self::VirtualTiming(e) => e
                .debug_aarch64_state_for_selected_runtime()
                .map(|state| state.daif),
            Self::IntervalTiming(e) => e
                .debug_aarch64_state_for_selected_runtime()
                .map(|state| state.daif),
            Self::AccurateTiming(e) => e
                .debug_aarch64_state_for_selected_runtime()
                .map(|state| state.daif),
        }
    }

    pub fn save_debug_checkpoint_values(&self) -> Option<Vec<(String, u64)>> {
        match self.active_debug_arch()? {
            Isa::AArch64 => {
                let a64 = match self {
                    Self::VirtualTiming(e) => e.debug_aarch64_state_for_selected_runtime(),
                    Self::IntervalTiming(e) => e.debug_aarch64_state_for_selected_runtime(),
                    Self::AccurateTiming(e) => e.debug_aarch64_state_for_selected_runtime(),
                }?;
                let mut vals = Vec::with_capacity(34);
                vals.push(("pc".to_string(), a64.pc));
                vals.push(("sp".to_string(), a64.current_sp()));
                vals.push(("nzcv".to_string(), u64::from(a64.nzcv)));
                for (idx, reg) in a64.x.iter().enumerate() {
                    vals.push((format!("x{idx}"), *reg));
                }
                Some(vals)
            }
            Isa::RiscV => {
                let rv = match self {
                    Self::VirtualTiming(e) => e.debug_riscv_state_for_selected_runtime(),
                    Self::IntervalTiming(e) => e.debug_riscv_state_for_selected_runtime(),
                    Self::AccurateTiming(e) => e.debug_riscv_state_for_selected_runtime(),
                }?;
                let mut vals = Vec::with_capacity(33);
                vals.push(("pc".to_string(), rv.pc));
                for (idx, reg) in rv.iregs.iter().enumerate() {
                    vals.push((format!("x{idx}"), *reg));
                }
                Some(vals)
            }
            Isa::AArch32 => None,
        }
    }

    pub fn restore_debug_checkpoint_values(&mut self, restored: &[(String, u64)]) -> bool {
        match self.active_debug_arch() {
            Some(Isa::AArch64) => match self {
                Self::VirtualTiming(e) => e.debug_aarch64_state_mut_for_selected_runtime(),
                Self::IntervalTiming(e) => e.debug_aarch64_state_mut_for_selected_runtime(),
                Self::AccurateTiming(e) => e.debug_aarch64_state_mut_for_selected_runtime(),
            }
                .map(|a64| {
                    for (name, value) in restored {
                        if name.starts_with("debug.") {
                            continue;
                        } else if name == "pc" {
                            a64.pc = *value;
                        } else if name == "sp" {
                            a64.write_xsp(31, *value);
                        } else if name == "nzcv" {
                            a64.nzcv = *value as u32;
                        } else if let Some(idx) =
                            name.strip_prefix('x').and_then(|s| s.parse::<usize>().ok())
                        {
                            if idx < 31 {
                                a64.x[idx] = *value;
                            }
                        }
                    }
                })
                .is_some(),
            Some(Isa::RiscV) => match self {
                Self::VirtualTiming(e) => e.debug_riscv_state_mut_for_selected_runtime(),
                Self::IntervalTiming(e) => e.debug_riscv_state_mut_for_selected_runtime(),
                Self::AccurateTiming(e) => e.debug_riscv_state_mut_for_selected_runtime(),
            }
                .map(|rv| {
                    for (name, value) in restored {
                        if name.starts_with("debug.") {
                            continue;
                        } else if name == "pc" {
                            rv.pc = *value;
                        } else if let Some(idx) =
                            name.strip_prefix('x').and_then(|s| s.parse::<usize>().ok())
                        {
                            if idx < rv.iregs.len() && idx != 0 {
                                rv.iregs[idx] = *value;
                            }
                        }
                    }
                    rv.iregs[0] = 0;
                })
                .is_some(),
            _ => false,
        }
    }

    pub fn debug_connections(&self) -> Vec<helm_debug::DebugConnectionView> {
        let view = match self {
            Self::VirtualTiming(e) => e.session.machine_coordination_view(),
            Self::IntervalTiming(e) => e.session.machine_coordination_view(),
            Self::AccurateTiming(e) => e.session.machine_coordination_view(),
        };
        let selected = match self {
            Self::VirtualTiming(e) => e.selected_debug_runtime_id().0,
            Self::IntervalTiming(e) => e.selected_debug_runtime_id().0,
            Self::AccurateTiming(e) => e.selected_debug_runtime_id().0,
        };
        view.runtimes
            .into_iter()
            .map(|runtime| helm_debug::DebugConnectionView {
                runtime_id: runtime.id.0,
                label: runtime.label,
                arch: debug_connection_arch_label(runtime.isa),
                mode: runtime.mode.map(debug_connection_mode_label),
                role: debug_connection_role_label(runtime.role),
                domain: runtime.domain.0,
                active: runtime.id.0 == selected,
            })
            .collect()
    }

    pub fn active_debug_connection(&self) -> Option<helm_debug::DebugConnectionView> {
        self.debug_connections()
            .into_iter()
            .find(|runtime| runtime.active)
    }

    pub fn select_debug_connection(&mut self, runtime_id: usize) -> bool {
        let id = session::HelmCoreId(runtime_id);
        match self {
            Self::VirtualTiming(e) => {
                if e.session.runtimes.runtime(id).is_some() {
                    e.debug_runtime = Some(id);
                    true
                } else {
                    false
                }
            }
            Self::IntervalTiming(e) => {
                if e.session.runtimes.runtime(id).is_some() {
                    e.debug_runtime = Some(id);
                    true
                } else {
                    false
                }
            }
            Self::AccurateTiming(e) => {
                if e.session.runtimes.runtime(id).is_some() {
                    e.debug_runtime = Some(id);
                    true
                } else {
                    false
                }
            }
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

    /// Mutable reference to the JIT probe bundle.
    #[cfg(feature = "jit")]
    pub fn jit_probes_mut(&mut self) -> &mut helm_probe::JitProbes {
        match self {
            Self::VirtualTiming(e) => &mut e.jit_probes,
            Self::IntervalTiming(e) => &mut e.jit_probes,
            Self::AccurateTiming(e) => &mut e.jit_probes,
        }
    }

    /// Run a closure against the live AArch64 architectural state when present.
    pub fn with_a64_state_mut<R>(
        &mut self,
        f: impl FnOnce(&mut Aarch64ArchState) -> R,
    ) -> Option<R> {
        match self {
            Self::VirtualTiming(e) => e.with_a64_state_mut(f),
            Self::IntervalTiming(e) => e.with_a64_state_mut(f),
            Self::AccurateTiming(e) => e.with_a64_state_mut(f),
        }
    }

    /// Run a closure against the live RISC-V architectural state when present.
    pub fn with_rv64_state_mut<R>(
        &mut self,
        f: impl FnOnce(&mut session::RiscvCore) -> R,
    ) -> Option<R> {
        match self {
            Self::VirtualTiming(e) => e.with_rv64_state_mut(f),
            Self::IntervalTiming(e) => e.with_rv64_state_mut(f),
            Self::AccurateTiming(e) => e.with_rv64_state_mut(f),
        }
    }

    /// Run a closure against the live system-memory surface when a system board
    /// is currently realized.
    pub fn with_system_memory_mut<R>(
        &mut self,
        f: impl FnOnce(&mut HelmAddressSpace) -> R,
    ) -> Option<R> {
        match self {
            Self::VirtualTiming(e) => e.with_system_memory_mut(f),
            Self::IntervalTiming(e) => e.with_system_memory_mut(f),
            Self::AccurateTiming(e) => e.with_system_memory_mut(f),
        }
    }

    /// Run a closure against the live system-memory surface (immutable).
    pub fn with_system_memory<R>(&self, f: impl FnOnce(&HelmAddressSpace) -> R) -> Option<R> {
        match self {
            Self::VirtualTiming(e) => e.with_system_memory(f),
            Self::IntervalTiming(e) => e.with_system_memory(f),
            Self::AccurateTiming(e) => e.with_system_memory(f),
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

    /// Start a blocking GDB RSP server on localhost and serve the current simulator.
    pub fn serve_gdb(&mut self, port: u16) -> Result<(), helm_debug::DebugError> {
        let server = helm_debug::RspServer::new(port);
        let mut target = HelmSimGdbTarget::new(self);
        server
            .listen(&mut target)
            .map_err(helm_debug::DebugError::from)
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

    // ── Device introspection ────────────────────────────────────────────────

    /// Immutable reference to the RISC-V architectural state (if ISA == RiscV).
    pub fn rv64_state(&self) -> Option<&session::RiscvCore> {
        match self {
            Self::VirtualTiming(e) => e.session.riscv(),
            Self::IntervalTiming(e) => e.session.riscv(),
            Self::AccurateTiming(e) => e.session.riscv(),
        }
    }

    /// Read a general-purpose register (ISA-agnostic).
    ///
    /// For AArch64: x0-x30 plus x31=SP.
    /// For RISC-V: x0-x31 (x0 always 0).
    pub fn read_gpr(&self, n: usize) -> Option<u64> {
        if let Some(a64) = self.a64_state() {
            Some(if n < 31 { a64.x[n] } else { a64.current_sp() })
        } else if let Some(rv) = self.rv64_state() {
            if n < 32 {
                Some(rv.iregs[n])
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Query GICv2 pending interrupt mask for a given 32-IRQ register bank.
    ///
    /// `reg_index` selects which group of 32 IRQs: 0 = IRQs 0-31 (private to
    /// `cpu`), 1 = IRQs 32-63, etc. For private IRQs (reg 0), the per-CPU
    /// banked pending bits are returned.
    pub fn gic_pending_mask(&self, cpu: usize, reg_index: usize) -> Option<u32> {
        self.with_gicv2_state(|gic| {
            if reg_index == 0 {
                gic.private_pending_for_cpu(cpu)
            } else if reg_index < gic.dist.pending.len() {
                gic.dist.pending[reg_index]
            } else {
                0
            }
        })
    }

    /// Query GICv2 enabled interrupt mask for a given 32-IRQ register bank.
    pub fn gic_enabled_mask(&self, cpu: usize, reg_index: usize) -> Option<u32> {
        self.with_gicv2_state(|gic| {
            if reg_index == 0 {
                gic.private_enabled_for_cpu(cpu)
            } else if reg_index < gic.dist.enabled.len() {
                gic.dist.enabled[reg_index]
            } else {
                0
            }
        })
    }

    /// Query GICv2 active interrupt mask for a given 32-IRQ register bank.
    pub fn gic_active_mask(&self, cpu: usize, reg_index: usize) -> Option<u32> {
        self.with_gicv2_state(|gic| {
            if reg_index == 0 {
                gic.private_active_for_cpu(cpu)
            } else if reg_index < gic.dist.active.len() {
                gic.dist.active[reg_index]
            } else {
                0
            }
        })
    }

    /// Run a closure against the live GICv2 shared state (if present).
    fn with_gicv2_state<R>(&self, f: impl FnOnce(&GicSharedState) -> R) -> Option<R> {
        let board = self.board()?;
        match board.gic.as_ref()? {
            session::HelmGic::V2(shared) => {
                let state = shared.lock().ok()?;
                Some(f(&state))
            }
            session::HelmGic::V3(_) => None,
        }
    }

    // ── UART (PL011) introspection ─────────────────────────────────────

    /// Total bytes transmitted through the platform UART.
    pub fn uart_tx_count(&self) -> Option<u64> {
        let board = self.board()?;
        let uart_idx = board.devs.uart_idx;
        self.with_system_memory(|sys| sys.device_as::<Pl011>(uart_idx).map(|u| u.tx_count))?
    }

    /// Total bytes received (read from RX FIFO) through the platform UART.
    pub fn uart_rx_count(&self) -> Option<u64> {
        let board = self.board()?;
        let uart_idx = board.devs.uart_idx;
        self.with_system_memory(|sys| sys.device_as::<Pl011>(uart_idx).map(|u| u.rx_count))?
    }

    /// Whether the UART transmit FIFO is full (always false in simulation).
    pub fn uart_is_tx_full(&self) -> Option<bool> {
        let board = self.board()?;
        let uart_idx = board.devs.uart_idx;
        self.with_system_memory(|sys| sys.device_as::<Pl011>(uart_idx).map(|u| u.is_tx_full()))?
    }

    /// Whether the UART receive FIFO is empty.
    pub fn uart_is_rx_empty(&self) -> Option<bool> {
        let board = self.board()?;
        let uart_idx = board.devs.uart_idx;
        self.with_system_memory(|sys| sys.device_as::<Pl011>(uart_idx).map(|u| u.is_rx_empty()))?
    }

    /// Immutable reference to the FS-mode board (if present).
    fn board(&self) -> Option<&session::HelmBoard> {
        match self {
            Self::VirtualTiming(e) => e.session.aarch64().and_then(Aarch64Core::machine),
            Self::IntervalTiming(e) => e.session.aarch64().and_then(Aarch64Core::machine),
            Self::AccurateTiming(e) => e.session.aarch64().and_then(Aarch64Core::machine),
        }
    }

    /// Return the ISA of the current simulation.
    pub fn isa(&self) -> Isa {
        match self {
            Self::VirtualTiming(e) => e.isa,
            Self::IntervalTiming(e) => e.isa,
            Self::AccurateTiming(e) => e.isa,
        }
    }
}

// ── build_simulator ───────────────────────────────────────────────────────────

/// Constructor called from configuration code (or Rust tests).
pub fn build_simulator_from_request(request: SimulatorBuildRequest) -> HelmSim {
    let SimulatorBuildRequest {
        isa,
        mode,
        timing,
        platform,
        mem_base,
        mem_size,
        built_in_num_cpus,
        built_in_gic_version,
    } = request;

    match timing {
        TimingChoice::VirtualTiming { ipc } => {
            HelmSim::VirtualTiming(maybe_realize_builtin_platform(
                HelmEngine::new(isa, mode, VirtualTiming::new(ipc), mem_base, mem_size),
                platform,
                built_in_num_cpus,
                built_in_gic_version,
            ))
        }
        TimingChoice::IntervalTiming {
            ipc,
            interval_len,
            mem_model,
        } => HelmSim::IntervalTiming(maybe_realize_builtin_platform(
            HelmEngine::new(
                isa,
                mode,
                IntervalTiming::new(ipc, interval_len),
                mem_base,
                mem_size,
            )
            .with_timing_mem_model_config(mem_model),
            platform,
            built_in_num_cpus,
            built_in_gic_version,
        )),
        TimingChoice::AccurateTiming => HelmSim::AccurateTiming(maybe_realize_builtin_platform(
            HelmEngine::new(isa, mode, AccurateTiming::default(), mem_base, mem_size),
            platform,
            built_in_num_cpus,
            built_in_gic_version,
        )),
    }
}

/// Compatibility constructor called from older scalar-argument call sites.
///
/// `mem_base` and `mem_size` define the flat guest-physical memory window.
pub fn build_simulator(
    isa: Isa,
    mode: ExecMode,
    timing: TimingChoice,
    mem_base: u64,
    mem_size: usize,
) -> HelmSim {
    build_simulator_from_request(SimulatorBuildRequest::new(
        isa, mode, timing, mem_base, mem_size,
    ))
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
        SimdUmaxp | SimdSmaxp => (InsnClass::SimdAlu, "SimdReduce", true),
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
        SimdLd1 | SimdSt1 => (InsnClass::Load, "SimdLd1St1", false),
        SimdXtn => (InsnClass::SimdAlu, "SimdXtn", false),
        SimdLd2 | SimdSt2 | SimdLd3 | SimdSt3 | SimdLd4 | SimdSt4 | SimdLd1r => {
            (InsnClass::SimdAlu, "SimdMultiStruct", true)
        }

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
        PacHint | PacReg | PacRegZ | AutReg | AutRegZ | Xpac => (InsnClass::System, "PAC", false),
        RetAut => (InsnClass::Branch, "RetAut", false),
        BrAut | BrAutZ => (InsnClass::Branch, "BrAut", false),
        BlrAut | BlrAutZ => (InsnClass::Branch, "BlrAut", false),
        EretAut => (InsnClass::Branch, "EretAut", false),
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
        Ret | Eret | RetAut | EretAut => ProbeBranchKind::Return,
        Br | BrAut | BrAutZ => ProbeBranchKind::IndirectJump,
        Blr | BlrAut | BlrAutZ => ProbeBranchKind::IndirectCall,
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
        Ret | RetAut | Eret | EretAut => BranchKind::Return,
        Br | BrAut | BrAutZ => BranchKind::IndirectJump,
        Blr | BlrAut | BlrAutZ => BranchKind::IndirectCall,
        BCond | Cbz | Cbnz | Tbz | Tbnz => BranchKind::DirectCond,
        _ => BranchKind::DirectUncond,
    }
}

/// Timing configuration passed to `build_simulator`.
#[derive(Debug, Clone, Copy)]
pub enum TimingChoice {
    VirtualTiming {
        ipc: f64,
    },
    IntervalTiming {
        ipc: f64,
        interval_len: u64,
        mem_model: TimingMemModelConfig,
    },
    AccurateTiming,
}

/// Frozen request for constructing one simulator instance.
///
/// This is the shared typed build input between configuration/discovery code
/// and the engine-side constructor. It narrows the construction boundary away
/// from ad hoc scalar argument plumbing.
#[derive(Debug, Clone, Copy)]
pub struct SimulatorBuildRequest {
    /// Selected guest ISA.
    pub isa: Isa,
    /// Requested execution mode.
    pub mode: ExecMode,
    /// Timing model configuration.
    pub timing: TimingChoice,
    /// Optional built-in platform to realize for system-mode execution.
    pub platform: Option<BuiltInPlatform>,
    /// Requested guest-visible RAM base.
    pub mem_base: u64,
    /// Requested guest-visible RAM size in bytes.
    pub mem_size: usize,
    /// Built-in platform CPU count hint used when realizing default system boards.
    pub built_in_num_cpus: usize,
    /// Built-in platform GIC version hint used when realizing default arm-virt boards.
    pub built_in_gic_version: arm_virt::ArmVirtGicVersion,
}

impl SimulatorBuildRequest {
    pub fn new(
        isa: Isa,
        mode: ExecMode,
        timing: TimingChoice,
        mem_base: u64,
        mem_size: usize,
    ) -> Self {
        Self {
            isa,
            mode,
            timing,
            platform: None,
            mem_base,
            mem_size,
            built_in_num_cpus: 1,
            built_in_gic_version: arm_virt::ArmVirtGicVersion::V3,
        }
    }

    pub fn with_platform(mut self, platform: BuiltInPlatform) -> Self {
        self.platform = Some(platform);
        self
    }

    pub fn with_arm_virt_defaults(
        mut self,
        num_cpus: usize,
        gic_version: arm_virt::ArmVirtGicVersion,
    ) -> Self {
        self.built_in_num_cpus = num_cpus.max(1);
        self.built_in_gic_version = gic_version;
        self
    }
}

/// Frozen simulator config after discovery and validation, before construction.
#[derive(Debug, Clone)]
pub struct FrozenSimulatorConfig {
    /// Scalar simulator build request.
    pub request: SimulatorBuildRequest,
    /// Discovered built-in mappings preserved for later projection.
    pub mappings: Vec<helm_platform::BuiltInMappedDevice>,
}

fn maybe_realize_builtin_platform<T: TimingModel>(
    mut engine: HelmEngine<T>,
    platform: Option<BuiltInPlatform>,
    built_in_num_cpus: usize,
    built_in_gic_version: arm_virt::ArmVirtGicVersion,
) -> HelmEngine<T> {
    let Some(platform) = platform else {
        return engine;
    };

    match (engine.isa, engine.mode, platform) {
        (Isa::AArch64, ExecMode::System, BuiltInPlatform::ArmVirt) => {
            let mem_mib = engine.mem_size.div_ceil(1024 * 1024).max(1);
            engine
                .install_arm_virt_board(
                    mem_mib,
                    built_in_num_cpus,
                    built_in_gic_version,
                    Box::new(arm_virt::StdioCharBackend),
                )
                .expect("built-in arm-virt realization should succeed");
            engine
        }
        _ => engine,
    }
}

#[cfg(test)]
mod tests;

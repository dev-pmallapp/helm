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

pub mod fs;
pub mod loader;
mod machine;
pub mod platform;
pub mod session;
pub mod se;
pub mod system_mem;

use helm_arch::{
    aarch64_decode, aarch64_execute, riscv_decode, riscv_execute, Aarch64ArchState, DecodeError,
};
pub use helm_core::{AccessType, MemFault, MemInterface};
use helm_core::{ExecContext, HartException};
use helm_event::EventQueue;
pub use helm_memory::FlatMem;
use helm_timing::{Accurate, InsnInfo, Interval, TimingModel, Virtual};

pub use helm_plugin;
use helm_plugin::PluginRegistry;

use helm_probe::{
    probe, BranchEvent, BranchKind as ProbeBranchKind, CpuProbes, CpuStepEvent, MemAccessEvent,
};

use crate::platform::arm_virt::{self};
use crate::session::{
    Aarch64FsMachine, Aarch64Runtime, Aarch64Vcpu, RiscvRuntime, Runtime, RuntimeSet,
    SessionProgress, SimulationSession,
};
use helm_diag;
use helm_diag::sim_info;
use se::{LinuxAarch64SyscallHandler, LinuxRiscv64SyscallHandler, SyscallArgs, SyscallHandler};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct UnimplementedInstructionSite {
    pc: u64,
    raw: u32,
    opcode_name: &'static str,
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
}

impl Default for MemAccessRecord {
    fn default() -> Self {
        Self {
            vaddr: 0,
            size: 0,
            is_store: false,
            is_atomic: false,
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

    fn push(&mut self, vaddr: u64, size: u8, is_store: bool, is_atomic: bool) {
        if self.count < 8 {
            self.records[self.count] = MemAccessRecord {
                vaddr,
                size,
                is_store,
                is_atomic,
            };
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
        self.push(addr, size as u8, false, is_atomic);
        self.inner.read(addr, size, ty)
    }

    fn write(&mut self, addr: u64, size: usize, val: u64, ty: AccessType) -> Result<(), MemFault> {
        let is_atomic = ty == AccessType::Atomic;
        self.push(addr, size as u8, true, is_atomic);
        self.inner.write(addr, size, val, ty)
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

    /// Runtime session state. Today this wraps a homogeneous runtime
    /// collection, but the shape is intended to evolve toward heterogeneous
    /// systems.
    session: SimulationSession,

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
    pub plugins: PluginRegistry,

    /// Typed probe bundle — zero-cost in release builds.
    pub probes: CpuProbes,

    /// ELF symbol table (populated after load_aarch64_elf).
    pub symbols: Vec<loader::ElfSymbol>,

    /// Unique stubbed instruction sites encountered during execution.
    unimplemented_instruction_sites: std::collections::HashSet<UnimplementedInstructionSite>,
}

impl<T: TimingModel> HelmEngine<T> {
    fn riscv(&self) -> &RiscvRuntime {
        self.session.riscv().expect("riscv runtime missing")
    }

    fn riscv_mut(&mut self) -> &mut RiscvRuntime {
        self.session.riscv_mut().expect("riscv runtime missing")
    }

    fn active_mode(&self) -> ExecMode {
        self.session.active_mode().unwrap_or(self.mode)
    }

    fn online_fs_cpus(machine: &Aarch64FsMachine) -> Vec<usize> {
        machine
            .vcpus
            .iter()
            .enumerate()
            .filter_map(|(idx, vcpu)| vcpu.powered_on.then_some(idx))
            .collect()
    }

    fn pick_next_fs_vcpu(machine: &mut Aarch64FsMachine) -> Option<usize> {
        if machine.vcpus.is_empty() {
            return None;
        }
        let start = machine.next_vcpu % machine.vcpus.len();
        for off in 0..machine.vcpus.len() {
            let idx = (start + off) % machine.vcpus.len();
            if machine.vcpus[idx].powered_on {
                machine.next_vcpu = (idx + 1) % machine.vcpus.len();
                return Some(idx);
            }
        }
        None
    }

    fn handle_fs_psci_call(
        machine: &mut Aarch64FsMachine,
        vcpu_idx: usize,
        conduit: &str,
        function: u32,
        arg1: u64,
        arg2: u64,
        arg3: u64,
    ) -> Result<(), HartException> {
        let current_sp_el1 = machine.vcpus[vcpu_idx].arch.sp_el1;
        let current_mpidr = machine.vcpus[vcpu_idx].arch.mpidr_el1;
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
                            "PSCI CPU_ON rejected: src_cpu{} mpidr={:#x} target_cpu{} target_mpidr={:#x} already_on",
                            vcpu_idx,
                            current_mpidr,
                            target_idx,
                            machine.vcpus[target_idx].arch.mpidr_el1
                        );
                        -4
                    } else {
                        let target_mpidr;
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
                            target.arch.psci_via_engine = true;
                            target.powered_on = true;
                            target_mpidr = target.arch.mpidr_el1;
                        }
                        let online = Self::online_fs_cpus(machine);
                        sim_info!(
                            component = "aarch64-fs-smp",
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
            (Isa::RiscV, _) => RuntimeSet::new_primary(Runtime::Riscv(RiscvRuntime::default())),
            (Isa::AArch64, ExecMode::Functional) => RuntimeSet::new_primary(Runtime::Aarch64(
                Aarch64Runtime::Functional(Aarch64ArchState::new()),
            )),
            (Isa::AArch64, _) => {
                RuntimeSet::new_primary(Runtime::Aarch64(Aarch64Runtime::Disabled))
            }
            (Isa::AArch32, _) => RuntimeSet::default(),
        };
        Self {
            isa,
            mode,
            timing,
            session: SimulationSession::from_runtimes(runtimes),
            mem_size,
            memory: FlatMem::new(mem_base, mem_size),
            events: EventQueue::new(),
            insns_retired: 0,
            timer_countdown: 1024,
            irq_poll_countdown: 16,
            fs_status_countdown: 50_000_000,
            plugins: PluginRegistry::new(),
            probes: CpuProbes::default(),
            symbols: Vec::new(),
            unimplemented_instruction_sites: std::collections::HashSet::new(),
        }
        .with_initial_runtime_mode(mode)
    }

    fn with_initial_runtime_mode(mut self, mode: ExecMode) -> Self {
        if let Some(riscv) = self.session.riscv_mut() {
            riscv.mode = mode;
            self.session.refresh_active_runtime_cache();
        }
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
        if let Some(machine) = self.session.aarch64().and_then(Aarch64Runtime::machine) {
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
        self.riscv_mut().pc = pc;
        if let Some(a64) = self.session.aarch64_mut().and_then(Aarch64Runtime::state_mut) {
            a64.pc = pc;
        }
        if let Some(machine) = self.session.aarch64_mut().and_then(Aarch64Runtime::machine_mut) {
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
        self.riscv_mut().syscall_handler = Some(h);
        self.riscv_mut().mode = ExecMode::Syscall;
        self.session.refresh_active_runtime_cache();
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

    /// Run up to `max_insns` instructions. Returns the reason for stopping.
    pub fn run(&mut self, max_insns: u64) -> StopReason {
        for _ in 0..max_insns {
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
                    self.session.on_progress(SessionProgress::RetiredInstruction);
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
                }
                Err(exc) => {
                    let stop = self.handle_exception(exc);
                    // Check if AArch64 handler requested exit
                    if let Some(h) = self.session.aarch64().and_then(Aarch64Runtime::handler) {
                        if h.should_exit {
                            return StopReason::Exit { code: h.exit_code };
                        }
                    }
                    match stop {
                        // Syscall handled OK — count it and keep running.
                        StopReason::Quantum => {
                            self.insns_retired += 1;
                            self.session.on_progress(SessionProgress::YieldedQuantum);
                            self.maybe_log_fs_smp_progress();
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
            .and_then(Aarch64Runtime::state)
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

        // 3. Execute — use InstrumentedMem when mem callbacks are registered
        let use_mem_instrumentation = self.plugins.has_mem_callbacks();
        let pc_written;

        if use_mem_instrumentation {
            // Destructure to satisfy borrow checker: borrow AArch64 runtime and memory separately.
            let HelmEngine {
                ref mut session,
                ref mut memory,
                ref plugins,
                ref probes,
                ..
            } = *self;
            let a64 = session
                .aarch64_mut()
                .and_then(Aarch64Runtime::state_mut)
                .ok_or(HartException::Unsupported)?;
            let mut imem = InstrumentedMem::new(memory);
            let exec_result = aarch64_execute(&insn, a64, &mut imem);
            for rec in imem.recorded() {
                plugins.fire_mem_access(
                    0,
                    &helm_plugin::runtime::MemInfo {
                        vaddr: rec.vaddr,
                        size: rec.size,
                        is_store: rec.is_store,
                        is_atomic: rec.is_atomic,
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
                .and_then(Aarch64Runtime::state_mut)
                .ok_or(HartException::Unsupported)?;
            pc_written = aarch64_execute(&insn, a64, &mut self.memory)?;
            if !pc_written {
                a64.pc = a64.pc.wrapping_add(4);
            }
        }

        // Classify opcode (used by both probe and plugin callbacks)
        let (class, opcode_name, is_stub) = classify_aarch64_opcode(insn.opcode);

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
                .and_then(Aarch64Runtime::state)
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
        }

        // 4. Timing
        let tinfo = InsnInfo {
            pc,
            is_branch: insn.is_branch(),
            is_load: insn.is_mem_access(),
            is_store: insn.is_mem_access(),
            is_fp: false,
        };
        self.timing.on_insn(&tinfo);

        // 5. Plugin callbacks
        if is_stub {
            self.note_unimplemented_instruction(pc, raw, opcode_name);
        }
        if self.plugins.has_insn_callbacks() {
            self.plugins.fire_insn_exec(
                0,
                &helm_plugin::runtime::InsnInfo {
                    pc,
                    raw,
                    size: 4,
                    class,
                    opcode_name,
                    is_stub,
                    context: if let Some(a) = self
                        .session
                        .aarch64()
                        .and_then(Aarch64Runtime::state)
                    {
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
                .and_then(Aarch64Runtime::state)
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
            ref probes,
            ref plugins,
            ..
        } = *self;
        let machine = session
            .aarch64_mut()
            .and_then(Aarch64Runtime::machine_mut)
            .ok_or(HartException::Unsupported)?;
        let vcpu_idx = Self::pick_next_fs_vcpu(machine).ok_or(HartException::WaitForInterrupt)?;
        if let Some(gic) = &machine.gic {
            gic.lock().unwrap().set_active_cpu(vcpu_idx);
        }
        let (a64, fs_state) = {
            let vcpu = &mut machine.vcpus[vcpu_idx];
            (&mut vcpu.arch, &mut vcpu.fs)
        };

        // Record PC before step so we can detect and log branches afterwards.
        // pc_before is retained for future branch probe wiring (Phase 2).
        let _pc_before = a64.pc;

        // Physical timer (PPI 30, INTID 30) — level-triggered signal.
        // Countdown replaces the previous `% 1024` modulo (avoids integer division).
        self.timer_countdown -= 1;
        if self.timer_countdown == 0 {
            self.timer_countdown = 1024;
            arm_virt::inject_timers(a64, fs_state, &mut machine.sys_mem);
        }

        // Sync irq_pending from the GIC IRQ line (level-triggered, not edge).
        // Polled every 16 instructions instead of every instruction to avoid
        // the AtomicBool load overhead on the critical path. IRQ latency is at
        // most 16 instructions (~3 ns at current speed — negligible for Linux).
        //
        // Critical: ASSIGN (not OR) so irq_pending tracks the line exactly.
        // When the kernel reads GICC_IAR the line drops; on the next poll
        // irq_pending becomes false, preventing spurious re-interrupts after ERET.
        self.irq_poll_countdown -= 1;
        if self.irq_poll_countdown == 0 {
            self.irq_poll_countdown = 16;
            fs_state.irq_pending = machine
                .irq_lines
                .get(vcpu_idx)
                .map_or(false, |l| l.load(std::sync::atomic::Ordering::Relaxed));
        }

        let result = fs::step_aarch64_fs(
            a64,
            &mut machine.sys_mem,
            fs_state,
            probes,
            plugins,
            vcpu_idx,
        );

        // WFI fast-forward: when the kernel executes WFI, fs.tick stops advancing
        // (step returned early before tick += 1). Jump tick forward to the nearest
        // armed timer deadline so the next timer-check interval fires immediately
        // rather than waiting for real instruction-count progress to catch up.
        // WFI fast-forward: advance the virtual tick to the nearest timer deadline
        // so the next inject_timers() call fires immediately.
        if matches!(result, Err(helm_core::HartException::WaitForInterrupt)) {
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
            }
            arm_virt::inject_timers(a64, fs_state, &mut machine.sys_mem);
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

        self.session.replace_primary(Runtime::Aarch64(Aarch64Runtime::Syscall {
            state,
            handler,
        }));
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
            .replace_primary(Runtime::Riscv(RiscvRuntime::default()));
        self.riscv_mut().mode = ExecMode::Syscall;
        self.riscv_mut().pc = loaded.entry_point;
        self.riscv_mut().iregs[2] = loaded.initial_sp; // sp
        self.riscv_mut().iregs[4] = tp; // tp
        self.session.refresh_active_runtime_cache();
        self.isa = Isa::RiscV;
        self.mode = ExecMode::Syscall;
        self.symbols = loaded.symbols;

        let mut handler = LinuxRiscv64SyscallHandler::new(loaded.brk_base);
        handler.binary_path = path.to_string();
        self.riscv_mut().syscall_handler = Some(Box::new(handler));

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
    ) -> Result<(), String> {
        let (boot_vcpus, sys_mem, devs, irq_lines, gic_state) =
            arm_virt::setup_arm_virt_boot_with_cpus(
                kernel_path,
                dtb_path,
                initrd_path,
                append,
                self.mem_size / (1024 * 1024),
                num_cpus,
                Box::new(arm_virt::StdioCharBackend),
            )?;

        self.session
            .replace_primary(Runtime::Aarch64(Aarch64Runtime::System(
            Aarch64FsMachine {
            sys_mem,
            vcpus: boot_vcpus
                .into_iter()
                .enumerate()
                .map(|(idx, (arch, fs))| Aarch64Vcpu {
                    arch,
                    fs,
                    powered_on: idx == 0,
                })
                .collect(),
            next_vcpu: 0,
            devs,
            irq_lines,
            gic: Some(gic_state),
        },
        )));
        self.mode = ExecMode::System;
        self.symbols.clear();

        if let Some(machine) = self.session.aarch64().and_then(Aarch64Runtime::machine) {
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
    ) -> Result<(), String> {
        let (boot_vcpus, sys_mem, devs, irq_lines, gic_state) =
            arm_virt::setup_arm_virt_boot_with_cpus_dtb_bytes(
                kernel_path,
                dtb_data,
                initrd_path,
                append,
                self.mem_size / (1024 * 1024),
                num_cpus,
                Box::new(arm_virt::StdioCharBackend),
            )?;

        self.session
            .replace_primary(Runtime::Aarch64(Aarch64Runtime::System(
            Aarch64FsMachine {
            sys_mem,
            vcpus: boot_vcpus
                .into_iter()
                .enumerate()
                .map(|(idx, (arch, fs))| Aarch64Vcpu {
                    arch,
                    fs,
                    powered_on: idx == 0,
                })
                .collect(),
            next_vcpu: 0,
            devs,
            irq_lines,
            gic: Some(gic_state),
        },
        )));
        self.mode = ExecMode::System;
        self.symbols.clear();

        if let Some(machine) = self.session.aarch64().and_then(Aarch64Runtime::machine) {
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

        // 4. Timing
        let info = InsnInfo {
            pc,
            is_branch: insn.is_control_flow(),
            is_load: insn.is_mem_access(),
            is_store: insn.is_mem_access(),
            is_fp: false, // TODO: add is_fp() to Instruction
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
                let context = if let Some(a) = self
                    .session
                    .aarch64()
                    .and_then(Aarch64Runtime::state)
                {
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
                .and_then(Aarch64Runtime::state)
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
            .and_then(Aarch64Runtime::handler_mut)
        {
            h.handle(nr, args, &mut self.memory)
        } else {
            Ok(-38) // -ENOSYS if no handler
        };

        match result {
            Ok(ret) => {
                if let Some(a) = self
                    .session
                    .aarch64_mut()
                    .and_then(Aarch64Runtime::state_mut)
                {
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
        self.memory.read(addr, size, ty)
    }

    #[inline(always)]
    fn write_mem(
        &mut self,
        addr: u64,
        size: usize,
        val: u64,
        ty: AccessType,
    ) -> Result<(), MemFault> {
        self.memory.write(addr, size, val, ty)
    }
}

// ── HelmSim ───────────────────────────────────────────────────────────────────

/// The PyO3 boundary — one enum variant per timing model.
///
/// Python calls `build_simulator()` which returns a `HelmSim`.
/// All Python-facing methods dispatch into the appropriate arm.
pub enum HelmSim {
    Virtual(HelmEngine<Virtual>),
    Interval(HelmEngine<Interval>),
    Accurate(HelmEngine<Accurate>),
}

impl HelmSim {
    pub fn run(&mut self, max_insns: u64) -> StopReason {
        match self {
            Self::Virtual(e) => e.run(max_insns),
            Self::Interval(e) => e.run(max_insns),
            Self::Accurate(e) => e.run(max_insns),
        }
    }

    pub fn insns_retired(&self) -> u64 {
        match self {
            Self::Virtual(e) => e.insns_retired,
            Self::Interval(e) => e.insns_retired,
            Self::Accurate(e) => e.insns_retired,
        }
    }

    pub fn set_pc(&mut self, pc: u64) {
        match self {
            Self::Virtual(e) => e.set_pc(pc),
            Self::Interval(e) => e.set_pc(pc),
            Self::Accurate(e) => e.set_pc(pc),
        }
    }

    pub fn load_bytes(&mut self, addr: u64, bytes: &[u8]) {
        match self {
            Self::Virtual(e) => e.load_bytes(addr, bytes),
            Self::Interval(e) => e.load_bytes(addr, bytes),
            Self::Accurate(e) => e.load_bytes(addr, bytes),
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
            Self::Virtual(e) => e.load_aarch64_elf(path, argv, envp),
            Self::Interval(e) => e.load_aarch64_elf(path, argv, envp),
            Self::Accurate(e) => e.load_aarch64_elf(path, argv, envp),
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
            Self::Virtual(e) => e.load_riscv64_elf(path, argv, envp),
            Self::Interval(e) => e.load_riscv64_elf(path, argv, envp),
            Self::Accurate(e) => e.load_riscv64_elf(path, argv, envp),
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
    ) -> Result<(), String> {
        match self {
            Self::Virtual(e) => {
                e.load_aarch64_kernel(kernel_path, dtb_path, initrd_path, append, num_cpus)
            }
            Self::Interval(e) => {
                e.load_aarch64_kernel(kernel_path, dtb_path, initrd_path, append, num_cpus)
            }
            Self::Accurate(e) => {
                e.load_aarch64_kernel(kernel_path, dtb_path, initrd_path, append, num_cpus)
            }
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
    ) -> Result<(), String> {
        match self {
            Self::Virtual(e) => e.load_aarch64_kernel_dtb_bytes(
                kernel_path,
                dtb_data,
                initrd_path,
                append,
                num_cpus,
            ),
            Self::Interval(e) => e.load_aarch64_kernel_dtb_bytes(
                kernel_path,
                dtb_data,
                initrd_path,
                append,
                num_cpus,
            ),
            Self::Accurate(e) => e.load_aarch64_kernel_dtb_bytes(
                kernel_path,
                dtb_data,
                initrd_path,
                append,
                num_cpus,
            ),
        }
    }

    /// Get mutable reference to the plugin registry.
    pub fn plugins_mut(&mut self) -> &mut PluginRegistry {
        match self {
            Self::Virtual(e) => &mut e.plugins,
            Self::Interval(e) => &mut e.plugins,
            Self::Accurate(e) => &mut e.plugins,
        }
    }

    /// Get the loaded ELF symbol table.
    pub fn symbols(&self) -> &[loader::ElfSymbol] {
        match self {
            Self::Virtual(e) => &e.symbols,
            Self::Interval(e) => &e.symbols,
            Self::Accurate(e) => &e.symbols,
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
            Self::Virtual(e) => e.has_unimplemented_instructions(),
            Self::Interval(e) => e.has_unimplemented_instructions(),
            Self::Accurate(e) => e.has_unimplemented_instructions(),
        }
    }

    pub fn unimplemented_instruction_count(&self) -> usize {
        match self {
            Self::Virtual(e) => e.unimplemented_instruction_count(),
            Self::Interval(e) => e.unimplemented_instruction_count(),
            Self::Accurate(e) => e.unimplemented_instruction_count(),
        }
    }

    /// Immutable reference to the AArch64 architectural state (if ISA == AArch64).
    pub fn a64_state(&self) -> Option<&Aarch64ArchState> {
        match self {
            Self::Virtual(e) => e.session.aarch64().and_then(Aarch64Runtime::state),
            Self::Interval(e) => e.session.aarch64().and_then(Aarch64Runtime::state),
            Self::Accurate(e) => e.session.aarch64().and_then(Aarch64Runtime::state),
        }
    }

    /// Current program counter.
    pub fn pc(&self) -> u64 {
        match self {
            Self::Virtual(e) => e
                .session
                .aarch64()
                .and_then(Aarch64Runtime::state)
                .map_or_else(|| e.session.riscv().map_or(0, |r| r.pc), |s| s.pc),
            Self::Interval(e) => e
                .session
                .aarch64()
                .and_then(Aarch64Runtime::state)
                .map_or_else(|| e.session.riscv().map_or(0, |r| r.pc), |s| s.pc),
            Self::Accurate(e) => e
                .session
                .aarch64()
                .and_then(Aarch64Runtime::state)
                .map_or_else(|| e.session.riscv().map_or(0, |r| r.pc), |s| s.pc),
        }
    }

    /// Mutable reference to the CPU probe bundle.
    pub fn probes_mut(&mut self) -> &mut CpuProbes {
        match self {
            Self::Virtual(e) => &mut e.probes,
            Self::Interval(e) => &mut e.probes,
            Self::Accurate(e) => &mut e.probes,
        }
    }

    /// Read `size` bytes from guest memory (for debugging).
    pub fn read_mem(&mut self, addr: u64, size: usize) -> u64 {
        use helm_core::{AccessType, MemInterface};
        match self {
            Self::Virtual(e) => e
                .memory
                .read(addr, size, AccessType::Load)
                .unwrap_or(0xDEAD),
            Self::Interval(e) => e
                .memory
                .read(addr, size, AccessType::Load)
                .unwrap_or(0xDEAD),
            Self::Accurate(e) => e
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
            Self::Virtual(e) => {
                if let Some(s) = e.session.aarch64_mut().and_then(Aarch64Runtime::state_mut) {
                    m.apply(s);
                }
                if let Some(machine) = e
                    .session
                    .aarch64_mut()
                    .and_then(Aarch64Runtime::machine_mut)
                {
                    for vcpu in &mut machine.vcpus {
                        m.apply(&mut vcpu.arch);
                    }
                }
            }
            Self::Interval(e) => {
                if let Some(s) = e.session.aarch64_mut().and_then(Aarch64Runtime::state_mut) {
                    m.apply(s);
                }
                if let Some(machine) = e
                    .session
                    .aarch64_mut()
                    .and_then(Aarch64Runtime::machine_mut)
                {
                    for vcpu in &mut machine.vcpus {
                        m.apply(&mut vcpu.arch);
                    }
                }
            }
            Self::Accurate(e) => {
                if let Some(s) = e.session.aarch64_mut().and_then(Aarch64Runtime::state_mut) {
                    m.apply(s);
                }
                if let Some(machine) = e
                    .session
                    .aarch64_mut()
                    .and_then(Aarch64Runtime::machine_mut)
                {
                    for vcpu in &mut machine.vcpus {
                        m.apply(&mut vcpu.arch);
                    }
                }
            }
        }
        Ok(())
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
        TimingChoice::Virtual { ipc } => HelmSim::Virtual(HelmEngine::new(
            isa,
            mode,
            Virtual::new(ipc),
            mem_base,
            mem_size,
        )),
        TimingChoice::Interval { ipc, interval_len } => HelmSim::Interval(HelmEngine::new(
            isa,
            mode,
            Interval::new(ipc, interval_len),
            mem_base,
            mem_size,
        )),
        TimingChoice::Accurate => HelmSim::Accurate(HelmEngine::new(
            isa,
            mode,
            Accurate::default(),
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
    Virtual { ipc: f64 },
    Interval { ipc: f64, interval_len: u64 },
    Accurate,
}

#[cfg(test)]
mod tests {
    use super::{classify_aarch64_opcode, Aarch64Runtime, RiscvRuntime};
    use crate::fs::FsState;
    use crate::session::{
        Aarch64FsMachine, Aarch64Vcpu, DomainProgress, ProgressAdvancePolicy,
        Runtime, RuntimeCoordinationDomain, RuntimeId, RuntimeRole,
        RuntimeSelectionPolicy, RuntimeSelectionScope, SessionProgress,
        SimulationSession,
    };
    use crate::{system_mem::SystemMem, ExecMode, FlatMem, HelmEngine, Isa, Virtual};
    use helm_arch::aarch64::insn::Opcode;
    use helm_arch::Aarch64ArchState;

    #[test]
    fn classify_implemented_simd_ops_as_non_stub() {
        for opcode in [
            Opcode::SimdAdd,
            Opcode::SimdSub,
            Opcode::SimdMul,
            Opcode::SimdAnd,
            Opcode::SimdOrr,
            Opcode::SimdEor,
            Opcode::SimdBic,
            Opcode::SimdNot,
            Opcode::SimdNeg,
            Opcode::SimdAbs,
            Opcode::SimdCmeq,
            Opcode::SimdCmgt,
            Opcode::SimdCmge,
            Opcode::SimdCmhi,
            Opcode::SimdCmhs,
            Opcode::SimdUmaxv,
            Opcode::SimdUminv,
        ] {
            let (_, _, is_stub) = classify_aarch64_opcode(opcode);
            assert!(!is_stub, "{opcode:?} should not be classified as a stub");
        }
    }

    #[test]
    fn unimplemented_instruction_tracking_deduplicates_by_site() {
        let mut engine = HelmEngine::new(
            Isa::AArch64,
            ExecMode::Syscall,
            Virtual::new(1.0),
            0,
            1 << 20,
        );

        assert!(engine.note_unimplemented_instruction(0x1000, 0xDEADBEEF, "SimdOther"));
        assert!(!engine.note_unimplemented_instruction(0x1000, 0xDEADBEEF, "SimdOther"));
        assert!(engine.note_unimplemented_instruction(0x1004, 0xDEADBEEF, "SimdOther"));
        assert_eq!(engine.unimplemented_instruction_count(), 2);
        assert!(engine.has_unimplemented_instructions());
    }

    #[test]
    fn runtime_set_tracks_active_runtime() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let aarch64_id = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));

        assert!(matches!(session.runtimes.active(), Some(Runtime::Riscv(_))));
        assert_eq!(session.active_id(), RuntimeId(0));

        assert!(session.set_active(aarch64_id));
        assert!(matches!(session.runtimes.active(), Some(Runtime::Aarch64(_))));
        assert_eq!(session.active_id(), aarch64_id);
    }

    #[test]
    fn runtime_set_rejects_invalid_active_index() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        assert!(!session.set_active(RuntimeId(99)));
        assert_eq!(session.active_id(), RuntimeId(0));
        assert!(matches!(session.runtimes.active(), Some(Runtime::Riscv(_))));
    }

    #[test]
    fn session_active_runtime_cache_tracks_switches() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let aarch64_id = session.push(Runtime::Aarch64(Aarch64Runtime::Functional(
            Aarch64ArchState::new(),
        )));

        assert_eq!(session.active_isa(), Some(Isa::RiscV));
        assert_eq!(session.active_mode(), Some(ExecMode::Functional));

        assert!(session.set_active(aarch64_id));
        assert_eq!(session.active_isa(), Some(Isa::AArch64));
        assert_eq!(session.active_mode(), Some(ExecMode::Functional));

        session.set_selection_policy(RuntimeSelectionPolicy::round_robin());
        session.on_progress(SessionProgress::RetiredInstruction);
        assert_eq!(session.active_isa(), Some(Isa::RiscV));
        assert_eq!(session.active_mode(), Some(ExecMode::Functional));
    }

    #[test]
    fn riscv_constructor_syncs_session_mode() {
        let engine = HelmEngine::new(Isa::RiscV, ExecMode::Syscall, Virtual::new(1.0), 0, 0x1000);
        assert_eq!(engine.active_mode(), ExecMode::Syscall);
    }

    #[test]
    fn session_fixed_policy_tracks_explicit_active_runtime() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let aarch64_id = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));

        session.set_selection_policy(RuntimeSelectionPolicy::Fixed(aarch64_id));

        assert!(matches!(
            session.selection_policy(),
            RuntimeSelectionPolicy::Fixed(id) if *id == aarch64_id
        ));
        assert_eq!(session.active_id(), aarch64_id);
        assert!(matches!(session.runtimes.active(), Some(Runtime::Aarch64(_))));
    }

    #[test]
    fn session_round_robin_advances_active_runtime() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let aarch64_id = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));

        session.set_selection_policy(RuntimeSelectionPolicy::round_robin());
        session.advance_selection();
        assert_eq!(session.active_id(), aarch64_id);
        assert!(matches!(session.runtimes.active(), Some(Runtime::Aarch64(_))));

        session.advance_selection();
        assert_eq!(session.active_id(), RuntimeId(0));
        assert!(matches!(session.runtimes.active(), Some(Runtime::Riscv(_))));
    }

    #[test]
    fn session_progress_hook_advances_round_robin_policy() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let aarch64_id = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));
        session.set_selection_policy(RuntimeSelectionPolicy::round_robin());

        session.on_progress(SessionProgress::RetiredInstruction);
        assert_eq!(session.active_id(), aarch64_id);

        session.on_progress(SessionProgress::YieldedQuantum);
        assert_eq!(session.active_id(), RuntimeId(0));
    }

    #[test]
    fn session_round_robin_skips_non_cpu_roles() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let accel_id = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));
        let cpu_id = session.push(Runtime::Riscv(RiscvRuntime::default()));

        assert!(session.set_runtime_role(accel_id, RuntimeRole::Accelerator));
        session.set_selection_policy(RuntimeSelectionPolicy::round_robin());

        session.advance_selection();
        assert_eq!(session.active_id(), cpu_id);

        session.advance_selection();
        assert_eq!(session.active_id(), RuntimeId(0));
    }

    #[test]
    fn session_round_robin_resyncs_when_active_role_changes() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let cpu_id = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));

        assert!(session.set_active(cpu_id));
        session.set_selection_policy(RuntimeSelectionPolicy::round_robin());
        assert_eq!(session.active_id(), cpu_id);

        assert!(session.set_runtime_role(cpu_id, RuntimeRole::Service));
        assert_eq!(session.active_id(), RuntimeId(0));
    }

    #[test]
    fn session_round_robin_can_target_specific_runtime_roles() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let service0 = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));
        let service1 = session.push(Runtime::Riscv(RiscvRuntime::default()));

        assert!(session.set_runtime_role(service0, RuntimeRole::Service));
        assert!(session.set_runtime_role(service1, RuntimeRole::Service));
        session.set_selection_policy(RuntimeSelectionPolicy::round_robin_scope(
            RuntimeSelectionScope::Role(RuntimeRole::Service),
        ));

        assert_eq!(session.active_id(), service0);

        session.advance_selection();
        assert_eq!(session.active_id(), service1);

        session.advance_selection();
        assert_eq!(session.active_id(), service0);
    }

    #[test]
    fn session_progress_hook_can_limit_round_robin_to_quantum_yields() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let cpu1 = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));
        session.set_selection_policy(RuntimeSelectionPolicy::round_robin_with(
            RuntimeSelectionScope::Compute,
            ProgressAdvancePolicy::YieldedQuantum,
        ));

        session.on_progress(SessionProgress::RetiredInstruction);
        assert_eq!(session.active_id(), RuntimeId(0));

        session.on_progress(SessionProgress::YieldedQuantum);
        assert_eq!(session.active_id(), cpu1);
    }

    #[test]
    fn session_progress_hook_can_limit_all_scope_round_robin_to_retired_instructions() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let service_id = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));

        assert!(session.set_runtime_role(service_id, RuntimeRole::Service));
        session.set_selection_policy(RuntimeSelectionPolicy::round_robin_with(
            RuntimeSelectionScope::All,
            ProgressAdvancePolicy::RetiredInstruction,
        ));

        session.on_progress(SessionProgress::YieldedQuantum);
        assert_eq!(session.active_id(), RuntimeId(0));

        session.on_progress(SessionProgress::RetiredInstruction);
        assert_eq!(session.active_id(), service_id);
    }

    #[test]
    fn session_push_refreshes_scoped_scheduler_topology() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));

        assert!(session.set_runtime_role(RuntimeId(0), RuntimeRole::Service));
        session.set_selection_policy(RuntimeSelectionPolicy::round_robin_scope(
            RuntimeSelectionScope::Compute,
        ));
        assert_eq!(session.active_id(), RuntimeId(0));

        let cpu_id = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));
        assert_eq!(session.active_id(), cpu_id);
    }

    #[test]
    fn session_round_robin_can_target_specific_coordination_domains() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let domain1_cpu0 = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));
        let domain1_cpu1 = session.push(Runtime::Riscv(RiscvRuntime::default()));

        assert!(session.set_runtime_domain(domain1_cpu0, RuntimeCoordinationDomain(1)));
        assert!(session.set_runtime_domain(domain1_cpu1, RuntimeCoordinationDomain(1)));
        session.set_selection_policy(RuntimeSelectionPolicy::round_robin_scope(
            RuntimeSelectionScope::Domain(RuntimeCoordinationDomain(1)),
        ));

        assert_eq!(session.active_id(), domain1_cpu0);

        session.advance_selection();
        assert_eq!(session.active_id(), domain1_cpu1);

        session.advance_selection();
        assert_eq!(session.active_id(), domain1_cpu0);
    }

    #[test]
    fn session_domain_changes_resync_domain_scoped_scheduler() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let cpu1 = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));
        let cpu2 = session.push(Runtime::Riscv(RiscvRuntime::default()));

        assert!(session.set_runtime_domain(cpu1, RuntimeCoordinationDomain(1)));
        assert!(session.set_runtime_domain(cpu2, RuntimeCoordinationDomain(1)));
        session.set_selection_policy(RuntimeSelectionPolicy::round_robin_scope(
            RuntimeSelectionScope::Domain(RuntimeCoordinationDomain(1)),
        ));
        assert_eq!(session.active_id(), cpu1);

        assert!(session.set_runtime_domain(cpu1, RuntimeCoordinationDomain(2)));
        assert_eq!(session.active_id(), cpu2);
    }

    #[test]
    fn session_round_robin_can_target_compute_within_a_domain() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let domain1_cpu0 = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));
        let domain1_service = session.push(Runtime::Riscv(RiscvRuntime::default()));
        let domain1_cpu1 = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));

        assert!(session.set_runtime_domain(domain1_cpu0, RuntimeCoordinationDomain(1)));
        assert!(session.set_runtime_domain(domain1_service, RuntimeCoordinationDomain(1)));
        assert!(session.set_runtime_domain(domain1_cpu1, RuntimeCoordinationDomain(1)));
        assert!(session.set_runtime_role(domain1_service, RuntimeRole::Service));

        session.set_selection_policy(RuntimeSelectionPolicy::round_robin_scope(
            RuntimeSelectionScope::ComputeInDomain(RuntimeCoordinationDomain(1)),
        ));

        assert_eq!(session.active_id(), domain1_cpu0);

        session.advance_selection();
        assert_eq!(session.active_id(), domain1_cpu1);

        session.advance_selection();
        assert_eq!(session.active_id(), domain1_cpu0);
    }

    #[test]
    fn session_compute_domain_scope_resyncs_when_domain_compute_membership_changes() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let domain1_cpu0 = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));
        let domain1_cpu1 = session.push(Runtime::Riscv(RiscvRuntime::default()));

        assert!(session.set_runtime_domain(domain1_cpu0, RuntimeCoordinationDomain(1)));
        assert!(session.set_runtime_domain(domain1_cpu1, RuntimeCoordinationDomain(1)));
        session.set_selection_policy(RuntimeSelectionPolicy::round_robin_scope(
            RuntimeSelectionScope::ComputeInDomain(RuntimeCoordinationDomain(1)),
        ));
        assert_eq!(session.active_id(), domain1_cpu0);

        assert!(session.set_runtime_role(domain1_cpu0, RuntimeRole::Service));
        assert_eq!(session.active_id(), domain1_cpu1);
    }

    #[test]
    fn session_domain_progress_tracks_active_runtime_domain() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let cpu1 = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));

        assert!(session.set_runtime_domain(cpu1, RuntimeCoordinationDomain(1)));

        session.on_progress(SessionProgress::RetiredInstruction);
        assert_eq!(
            session.domain_progress(RuntimeCoordinationDomain::SYSTEM),
            Some(DomainProgress {
                retired_instructions: 1,
                yielded_quanta: 0,
            })
        );
        assert_eq!(
            session.domain_progress(RuntimeCoordinationDomain(1)),
            Some(DomainProgress::default())
        );

        assert!(session.set_active(cpu1));
        session.on_progress(SessionProgress::YieldedQuantum);
        assert_eq!(
            session.domain_progress(RuntimeCoordinationDomain(1)),
            Some(DomainProgress {
                retired_instructions: 0,
                yielded_quanta: 1,
            })
        );
    }

    #[test]
    fn session_domain_progress_follows_domain_reassignment() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));

        session.on_progress(SessionProgress::RetiredInstruction);
        assert_eq!(
            session.domain_progress(RuntimeCoordinationDomain::SYSTEM),
            Some(DomainProgress {
                retired_instructions: 1,
                yielded_quanta: 0,
            })
        );

        assert!(session.set_runtime_domain(RuntimeId(0), RuntimeCoordinationDomain(3)));
        session.on_progress(SessionProgress::RetiredInstruction);
        assert_eq!(
            session.domain_progress(RuntimeCoordinationDomain::SYSTEM),
            Some(DomainProgress {
                retired_instructions: 1,
                yielded_quanta: 0,
            })
        );
        assert_eq!(
            session.domain_progress(RuntimeCoordinationDomain(3)),
            Some(DomainProgress {
                retired_instructions: 1,
                yielded_quanta: 0,
            })
        );
    }

    #[test]
    fn session_replace_primary_rebuilds_coordination_state() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));

        assert!(session.set_runtime_domain(RuntimeId(0), RuntimeCoordinationDomain(3)));
        session.on_progress(SessionProgress::RetiredInstruction);
        assert_eq!(
            session.domain_progress(RuntimeCoordinationDomain(3)),
            Some(DomainProgress {
                retired_instructions: 1,
                yielded_quanta: 0,
            })
        );

        session.replace_primary(Runtime::Aarch64(Aarch64Runtime::Disabled));

        assert_eq!(session.active_id(), RuntimeId(0));
        assert_eq!(
            session.runtime_domain(RuntimeId(0)),
            Some(RuntimeCoordinationDomain::SYSTEM)
        );
        assert_eq!(
            session.domain_progress(RuntimeCoordinationDomain::SYSTEM),
            Some(DomainProgress::default())
        );
        assert_eq!(session.domain_progress(RuntimeCoordinationDomain(3)), None);
    }

    #[test]
    fn session_machine_coordination_view_reports_runtime_and_domain_state() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let cpu1 = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));
        let svc = session.push(Runtime::Riscv(RiscvRuntime::default()));

        assert!(session.set_runtime_domain(cpu1, RuntimeCoordinationDomain(1)));
        assert!(session.set_runtime_domain(svc, RuntimeCoordinationDomain(1)));
        assert!(session.set_runtime_role(svc, RuntimeRole::Service));
        assert!(session.set_runtime_label(svc, "svc0"));
        assert!(session.set_active(cpu1));
        session.on_progress(SessionProgress::RetiredInstruction);

        let view = session.machine_coordination_view();
        assert_eq!(view.active_runtime, cpu1);
        assert_eq!(view.runtimes.len(), 3);

        let svc_view = view
            .runtimes
            .iter()
            .find(|runtime| runtime.id == svc)
            .expect("service runtime missing from machine view");
        assert_eq!(svc_view.label, "svc0");
        assert_eq!(svc_view.role, RuntimeRole::Service);
        assert_eq!(svc_view.domain, RuntimeCoordinationDomain(1));
        assert!(!svc_view.active);

        let domain1 = view
            .domains
            .iter()
            .find(|domain| domain.domain == RuntimeCoordinationDomain(1))
            .expect("domain 1 missing from machine view");
        assert_eq!(domain1.runtime_ids, vec![cpu1, svc]);
        assert_eq!(domain1.compute_runtime_ids, vec![cpu1]);
        assert_eq!(
            domain1.progress,
            DomainProgress {
                retired_instructions: 1,
                yielded_quanta: 0,
            }
        );
    }

    #[test]
    fn session_machine_coordination_view_retains_progress_for_empty_domains() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));

        assert!(session.set_runtime_domain(RuntimeId(0), RuntimeCoordinationDomain(4)));
        session.on_progress(SessionProgress::RetiredInstruction);
        assert!(session.set_runtime_domain(RuntimeId(0), RuntimeCoordinationDomain::SYSTEM));

        let view = session.machine_coordination_view();
        let domain4 = view
            .domains
            .iter()
            .find(|domain| domain.domain == RuntimeCoordinationDomain(4))
            .expect("domain 4 progress should remain visible");
        assert!(domain4.runtime_ids.is_empty());
        assert!(domain4.compute_runtime_ids.is_empty());
        assert_eq!(
            domain4.progress,
            DomainProgress {
                retired_instructions: 1,
                yielded_quanta: 0,
            }
        );
    }

    #[test]
    fn session_machine_coordination_state_summarizes_domains() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let cpu1 = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));
        let svc = session.push(Runtime::Riscv(RiscvRuntime::default()));
        let accel = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));

        assert!(session.set_runtime_domain(cpu1, RuntimeCoordinationDomain(1)));
        assert!(session.set_runtime_domain(svc, RuntimeCoordinationDomain(1)));
        assert!(session.set_runtime_domain(accel, RuntimeCoordinationDomain(2)));
        assert!(session.set_runtime_role(svc, RuntimeRole::Service));
        assert!(session.set_runtime_role(accel, RuntimeRole::Accelerator));
        assert!(session.set_active(cpu1));
        session.on_progress(SessionProgress::RetiredInstruction);
        session.on_progress(SessionProgress::RetiredInstruction);
        assert!(session.set_active(accel));
        session.on_progress(SessionProgress::YieldedQuantum);

        let state = session.machine_coordination_state();
        assert_eq!(state.total_runtime_count(), 4);
        assert_eq!(state.total_compute_runtime_count(), 2);

        let domain1 = state
            .domain_summary(RuntimeCoordinationDomain(1))
            .expect("domain 1 summary missing");
        assert_eq!(domain1.runtime_count, 2);
        assert_eq!(domain1.compute_runtime_count, 1);
        assert_eq!(domain1.primary_cpu_count, 0);
        assert_eq!(domain1.cpu_count, 1);
        assert_eq!(domain1.service_count, 1);
        assert_eq!(domain1.accelerator_count, 0);
        assert_eq!(
            domain1.progress,
            DomainProgress {
                retired_instructions: 2,
                yielded_quanta: 0,
            }
        );

        let busiest = state
            .busiest_domain_by_retired_instructions()
            .expect("busiest domain missing");
        assert_eq!(busiest.domain, RuntimeCoordinationDomain(1));
    }

    #[test]
    fn session_machine_policy_feedback_prefers_busiest_compute_domain() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let cpu1 = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));
        let cpu2 = session.push(Runtime::Riscv(RiscvRuntime::default()));

        assert!(session.set_runtime_domain(cpu1, RuntimeCoordinationDomain(1)));
        assert!(session.set_runtime_domain(cpu2, RuntimeCoordinationDomain(2)));

        assert!(session.set_active(cpu1));
        session.on_progress(SessionProgress::RetiredInstruction);
        session.on_progress(SessionProgress::RetiredInstruction);
        assert!(session.set_active(cpu2));
        session.on_progress(SessionProgress::RetiredInstruction);

        let feedback = session.machine_policy_feedback();
        assert_eq!(
            feedback.preferred_scope,
            Some(RuntimeSelectionScope::ComputeInDomain(
                RuntimeCoordinationDomain(1)
            ))
        );
        assert_eq!(feedback.busiest_domain, Some(RuntimeCoordinationDomain(1)));
        assert_eq!(
            feedback.busiest_domain_progress,
            Some(DomainProgress {
                retired_instructions: 2,
                yielded_quanta: 0,
            })
        );
    }

    #[test]
    fn session_machine_policy_feedback_falls_back_to_global_compute_scope() {
        let session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));

        let feedback = session.machine_policy_feedback();
        assert_eq!(feedback.preferred_scope, Some(RuntimeSelectionScope::Compute));
        assert_eq!(feedback.busiest_domain, Some(RuntimeCoordinationDomain::SYSTEM));
        assert_eq!(
            feedback.busiest_domain_progress,
            Some(DomainProgress::default())
        );
    }

    #[test]
    fn session_tracks_runtime_labels_and_roles() {
        let mut session = SimulationSession::new_primary(Runtime::Riscv(RiscvRuntime::default()));
        let accel_id = session.push(Runtime::Aarch64(Aarch64Runtime::Disabled));

        assert_eq!(session.runtime_label(RuntimeId(0)), Some("runtime-0"));
        assert_eq!(session.runtime_role(RuntimeId(0)), Some(RuntimeRole::PrimaryCpu));
        assert_eq!(
            session.runtime_domain(RuntimeId(0)),
            Some(RuntimeCoordinationDomain::SYSTEM)
        );
        assert_eq!(session.runtime_role(accel_id), Some(RuntimeRole::Cpu));
        assert_eq!(
            session.runtime_domain(accel_id),
            Some(RuntimeCoordinationDomain::SYSTEM)
        );

        assert!(session.set_runtime_label(accel_id, "gpu0"));
        assert!(session.set_runtime_role(accel_id, RuntimeRole::Accelerator));
        assert!(session.set_runtime_domain(accel_id, RuntimeCoordinationDomain(7)));

        assert_eq!(session.runtime_label(accel_id), Some("gpu0"));
        assert_eq!(session.runtime_role(accel_id), Some(RuntimeRole::Accelerator));
        assert_eq!(session.runtime_domain(accel_id), Some(RuntimeCoordinationDomain(7)));
    }

    #[test]
    fn psci_cpu_on_powers_secondary_vcpu() {
        let mut cpu0 = Aarch64ArchState::new();
        cpu0.mpidr_el1 = 0x8000_0000;
        cpu0.sp_el1 = 0x8000_0000;
        cpu0.psci_via_engine = true;

        let mut cpu1 = Aarch64ArchState::new();
        cpu1.mpidr_el1 = 0x8000_0001;
        cpu1.psci_via_engine = true;

        let mut machine = Aarch64FsMachine {
            sys_mem: SystemMem::new(FlatMem::new(0, 0)),
            vcpus: vec![
                Aarch64Vcpu {
                    arch: cpu0,
                    fs: FsState::new(),
                    powered_on: true,
                },
                Aarch64Vcpu {
                    arch: cpu1,
                    fs: FsState::new(),
                    powered_on: false,
                },
            ],
            next_vcpu: 0,
            devs: crate::platform::arm_virt::ArmVirtDevices {
                gicd_idx: 0,
                gicc_idx: 0,
                uart_idx: 0,
            },
            irq_lines: Vec::new(),
            gic: None,
        };

        HelmEngine::<Virtual>::handle_fs_psci_call(
            &mut machine,
            0,
            "smc",
            0x8400_0003,
            0x8000_0001,
            0x1234_0000,
            0x55AA,
        )
        .unwrap();

        assert!(machine.vcpus[1].powered_on);
        assert_eq!(machine.vcpus[1].arch.pc, 0x1234_0000);
        assert_eq!(machine.vcpus[1].arch.x[0], 0x55AA);
        assert_eq!(machine.vcpus[0].arch.x[0], 0);
    }

    #[test]
    fn fs_irq_polling_uses_selected_vcpu_irq_line() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;

        let mut cpu0 = Aarch64ArchState::new();
        cpu0.pc = 0;
        cpu0.current_el = 1;
        cpu0.spsel = true;

        let mut cpu1 = Aarch64ArchState::new();
        cpu1.pc = 0;
        cpu1.current_el = 1;
        cpu1.spsel = true;

        let mut sys_mem = SystemMem::new(FlatMem::new(0, 0x1000));
        sys_mem.ram.load_bytes(0, &0xD503_201Fu32.to_le_bytes()); // NOP

        let machine = Aarch64FsMachine {
            sys_mem,
            vcpus: vec![
                Aarch64Vcpu {
                    arch: cpu0,
                    fs: FsState::new(),
                    powered_on: false,
                },
                Aarch64Vcpu {
                    arch: cpu1,
                    fs: FsState::new(),
                    powered_on: true,
                },
            ],
            next_vcpu: 0,
            devs: crate::platform::arm_virt::ArmVirtDevices {
                gicd_idx: 0,
                gicc_idx: 0,
                uart_idx: 0,
            },
            irq_lines: vec![
                Arc::new(AtomicBool::new(true)),
                Arc::new(AtomicBool::new(false)),
            ],
            gic: None,
        };

        let mut engine =
            HelmEngine::new(Isa::AArch64, ExecMode::System, Virtual::new(1.0), 0, 0x1000);
        engine.session = SimulationSession::new_primary(Runtime::Aarch64(Aarch64Runtime::System(machine)));
        engine.irq_poll_countdown = 1;

        engine
            .step_aarch64_system()
            .expect("secondary vCPU should execute a NOP");

        let machine = engine
            .session
            .aarch64()
            .and_then(Aarch64Runtime::machine)
            .expect("machine should remain present");
        assert!(
            !machine.vcpus[1].fs.irq_pending,
            "CPU1 must not inherit CPU0's IRQ line state"
        );
    }
}

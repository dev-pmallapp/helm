//! `helm-engine` — simulation kernel.
//!
//! # Key types
//! - [`HelmEngine<T>`] — generic simulation kernel; `T` is the `TimingModel`
//! - [`HelmSim`]       — enum wrapping all timing variants; the PyO3 boundary
//! - [`Isa`]           — which ISA is active (dispatch once per `run()` call)
//! - [`ExecMode`]      — functional / syscall-emulation / full-system
//! - [`FlatMem`]       — Phase 0 flat memory (replaced by `MemoryMap` in Phase 1)
//! - [`StopReason`]    — why `run()` returned
//!
//! # Inner loop contract
//! The inner loop (`step_*`) is hot. No allocations, no trait objects, no
//! dynamic dispatch. All cross-component refs are stored during `elaborate()`.

#![allow(missing_docs)]

pub mod fs;
pub mod loader;
pub mod platform;
pub mod se;
pub mod system_mem;

use helm_arch::{
    aarch64_decode, aarch64_execute, Aarch64ArchState,
    riscv_decode, riscv_execute, DecodeError,
};
pub use helm_core::{AccessType, MemFault, MemInterface};
use helm_core::{ExecContext, HartException};
use helm_event::EventQueue;
use helm_timing::{Accurate, InsnInfo, Interval, TimingModel, Virtual};

use helm_plugin::PluginRegistry;
pub use helm_plugin;

use crate::fs::FsState;
use helm_debug::sim_trace;
use helm_hw_intc::GicState;
use crate::platform::arm_virt::{self, ArmVirtDevices};
use crate::system_mem::SystemMem;
use se::{LinuxAarch64SyscallHandler, SyscallArgs, SyscallHandler};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct UnimplementedInstructionSite {
    pc: u64,
    raw: u32,
    opcode_name: &'static str,
}

struct Aarch64FsMachine {
    sys_mem: SystemMem,
    fs: FsState,
    #[allow(dead_code)]
    devs: ArmVirtDevices,
    /// IRQ line from the GIC — raised when any enabled SPI/PPI is pending.
    irq_line: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Shared GIC state — used to assert device interrupts (e.g. timer PPI 30).
    gic: Option<std::sync::Arc<std::sync::Mutex<GicState>>>,
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

// ── FlatMem ───────────────────────────────────────────────────────────────────

/// Phase 0 sparse-page memory.
///
/// Allocates 4 KiB pages on first access, so code at `0x400000` and the stack
/// at `0x7FFF_FFE0_0000` coexist without a multi-TiB allocation.
///
/// Replace with `helm_memory::MemoryMap` in Phase 1.
pub struct FlatMem {
    pages: std::collections::HashMap<u64, Box<[u8; Self::PAGE_SIZE]>>,
}

impl FlatMem {
    const PAGE_SIZE: usize = 4096;
    const PAGE_MASK: u64   = !(Self::PAGE_SIZE as u64 - 1);

    pub fn new(_base: u64, _size: usize) -> Self {
        Self { pages: std::collections::HashMap::new() }
    }

    fn page_mut(&mut self, page_addr: u64) -> &mut [u8; Self::PAGE_SIZE] {
        self.pages.entry(page_addr).or_insert_with(|| Box::new([0u8; Self::PAGE_SIZE]))
    }

    #[allow(dead_code)]
    fn page_ref(&self, page_addr: u64) -> Option<&[u8; Self::PAGE_SIZE]> {
        self.pages.get(&page_addr).map(|b| b.as_ref())
    }

    /// Load bytes into memory (e.g. ELF segment).
    pub fn load_bytes(&mut self, addr: u64, bytes: &[u8]) {
        let mut off = 0usize;
        let mut va  = addr;
        while off < bytes.len() {
            let page_addr  = va & Self::PAGE_MASK;
            let page_off   = (va - page_addr) as usize;
            let chunk      = (bytes.len() - off).min(Self::PAGE_SIZE - page_off);
            let page       = self.page_mut(page_addr);
            page[page_off..page_off + chunk].copy_from_slice(&bytes[off..off + chunk]);
            off += chunk;
            va  += chunk as u64;
        }
    }
}

impl MemInterface for FlatMem {
    fn read(&mut self, addr: u64, size: usize, _ty: AccessType) -> Result<u64, MemFault> {
        debug_assert!(size <= 8);
        let page_addr = addr & Self::PAGE_MASK;
        let page_off  = (addr - page_addr) as usize;

        // Fast path: access within one page
        if page_off + size <= Self::PAGE_SIZE {
            let page = self.page_mut(page_addr);
            let mut buf = [0u8; 8];
            buf[..size].copy_from_slice(&page[page_off..page_off + size]);
            return Ok(u64::from_le_bytes(buf));
        }

        // Slow path: straddles page boundary
        let mut buf = [0u8; 8];
        for i in 0..size {
            let va = addr + i as u64;
            let pa = va & Self::PAGE_MASK;
            let po = (va - pa) as usize;
            buf[i] = self.page_mut(pa)[po];
        }
        Ok(u64::from_le_bytes(buf))
    }

    fn write(&mut self, addr: u64, size: usize, val: u64, _ty: AccessType) -> Result<(), MemFault> {
        debug_assert!(size <= 8);
        let bytes = val.to_le_bytes();
        let page_addr = addr & Self::PAGE_MASK;
        let page_off  = (addr - page_addr) as usize;

        // Fast path: within one page
        if page_off + size <= Self::PAGE_SIZE {
            let page = self.page_mut(page_addr);
            page[page_off..page_off + size].copy_from_slice(&bytes[..size]);
            return Ok(());
        }

        // Slow path: straddles page boundary
        for i in 0..size {
            let va = addr + i as u64;
            let pa = va & Self::PAGE_MASK;
            let po = (va - pa) as usize;
            self.page_mut(pa)[po] = bytes[i];
        }
        Ok(())
    }
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
        Self { vaddr: 0, size: 0, is_store: false, is_atomic: false }
    }
}

impl<'a> InstrumentedMem<'a> {
    fn new(inner: &'a mut FlatMem) -> Self {
        Self { inner, records: [MemAccessRecord::default(); 8], count: 0 }
    }

    fn push(&mut self, vaddr: u64, size: u8, is_store: bool, is_atomic: bool) {
        if self.count < 8 {
            self.records[self.count] = MemAccessRecord { vaddr, size, is_store, is_atomic };
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
    pub isa:  Isa,
    pub mode: ExecMode,
    pub timing: T,

    // RISC-V arch state (Phase 0 — will become an enum in Phase 2 for multi-ISA)
    pub iregs: [u64; 32],
    pub fregs: [u64; 32],
    pub csrs:  Box<[u64; 4096]>,
    pub pc:    u64,

    /// Reservation address for LR/SC atomics.
    pub lr_addr: Option<u64>,

    /// AArch64 architectural state (populated when isa == AArch64).
    pub a64_state: Option<Aarch64ArchState>,
    /// AArch64 Linux syscall handler (populated when isa == AArch64, mode == Syscall).
    pub a64_handler: Option<LinuxAarch64SyscallHandler>,
    /// AArch64 full-system machine state (populated when mode == System).
    a64_fs: Option<Aarch64FsMachine>,

    mem_size: usize,
    pub memory: FlatMem,
    pub events: EventQueue,

    pub syscall_handler: Option<Box<dyn SyscallHandler>>,

    /// Total instructions retired.
    pub insns_retired: u64,

    /// Plugin callback registry.
    pub plugins: PluginRegistry,

    /// ELF symbol table (populated after load_aarch64_elf).
    pub symbols: Vec<loader::ElfSymbol>,

    /// Unique stubbed instruction sites encountered during execution.
    unimplemented_instruction_sites: std::collections::HashSet<UnimplementedInstructionSite>,
}

impl<T: TimingModel> HelmEngine<T> {
    pub fn new(isa: Isa, mode: ExecMode, timing: T, mem_base: u64, mem_size: usize) -> Self {
        Self {
            isa,
            mode,
            timing,
            iregs: [0u64; 32],
            fregs: [0u64; 32],
            csrs:  Box::new([0u64; 4096]),
            pc:    0,
            lr_addr: None,
            a64_state: None,
            a64_handler: None,
            a64_fs: None,
            mem_size,
            memory: FlatMem::new(mem_base, mem_size),
            events: EventQueue::new(),
            syscall_handler: None,
            insns_retired: 0,
            plugins: PluginRegistry::new(),
            symbols: Vec::new(),
            unimplemented_instruction_sites: std::collections::HashSet::new(),
        }
    }

    /// Set the program counter (reset vector).
    pub fn set_pc(&mut self, pc: u64) { self.pc = pc; }

    /// Load bytes into memory (e.g. from ELF loader).
    pub fn load_bytes(&mut self, addr: u64, bytes: &[u8]) {
        self.memory.load_bytes(addr, bytes);
    }

    /// Attach a syscall handler (required for `ExecMode::Syscall`).
    pub fn set_syscall_handler(&mut self, h: Box<dyn SyscallHandler>) {
        self.syscall_handler = Some(h);
    }

    fn note_unimplemented_instruction(&mut self, pc: u64, raw: u32, opcode_name: &'static str) -> bool {
        let site = UnimplementedInstructionSite { pc, raw, opcode_name };
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

    /// Run up to `max_insns` instructions. Returns the reason for stopping.
    pub fn run(&mut self, max_insns: u64) -> StopReason {
        for _ in 0..max_insns {
            let result = match self.isa {
                Isa::RiscV   => self.step_riscv(),
                Isa::AArch64 => {
                    if self.mode == ExecMode::System {
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
                    // Update sim-trace context only when a MonitorSink is active.
                    // The RefCell::borrow_mut() in update_sim_ctx has measurable
                    // overhead at simulation speeds; skip it when no one is listening.
                    if helm_debug::sim_trace::is_monitor_active() {
                        sim_trace::update_sim_ctx(self.insns_retired, 1_000_000_000);
                    }
                    if self.plugins.has_timer_callbacks() {
                        self.plugins.fire_timer(0, self.insns_retired);
                    }
                }
                Err(exc) => {
                    let stop = self.handle_exception(exc);
                    // Check if AArch64 handler requested exit
                    if let Some(h) = &self.a64_handler {
                        if h.should_exit {
                            return StopReason::Exit { code: h.exit_code };
                        }
                    }
                    match stop {
                        // Syscall handled OK — count it and keep running.
                        StopReason::Quantum => { self.insns_retired += 1; }
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
        let pc = self.a64_state.as_ref().ok_or(HartException::Unsupported)?.pc;

        // 1. Fetch
        let raw = self.memory.fetch32(pc).map_err(|_| {
            HartException::InstructionAccessFault { addr: pc }
        })?;

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
            // Destructure to satisfy borrow checker: borrow a64_state and memory separately.
            let HelmEngine { ref mut a64_state, ref mut memory, ref plugins, .. } = *self;
            let a64 = a64_state.as_mut().unwrap();
            let mut imem = InstrumentedMem::new(memory);
            pc_written = aarch64_execute(&insn, a64, &mut imem)?;
            if !pc_written {
                a64.pc = a64.pc.wrapping_add(4);
            }
            // Fire mem_access callbacks for each recorded access
            for rec in imem.recorded() {
                plugins.fire_mem_access(0, &helm_plugin::runtime::MemInfo {
                    vaddr: rec.vaddr, size: rec.size, is_store: rec.is_store, is_atomic: rec.is_atomic,
                });
            }
        } else {
            let a64 = self.a64_state.as_mut().unwrap();
            pc_written = aarch64_execute(&insn, a64, &mut self.memory)?;
            if !pc_written {
                a64.pc = a64.pc.wrapping_add(4);
            }
        }

        // 4. Timing
        let tinfo = InsnInfo {
            pc,
            is_branch: insn.is_branch(),
            is_load:   insn.is_mem_access(),
            is_store:  insn.is_mem_access(),
            is_fp:     false,
        };
        self.timing.on_insn(&tinfo);

        // 5. Plugin callbacks
        let (class, opcode_name, is_stub) = classify_aarch64_opcode(insn.opcode);
        if is_stub {
            self.note_unimplemented_instruction(pc, raw, opcode_name);
        }
        if self.plugins.has_insn_callbacks() {
            self.plugins.fire_insn_exec(0, &helm_plugin::runtime::InsnInfo {
                pc, raw, size: 4, class, opcode_name, is_stub,
                context: if let Some(a) = &self.a64_state {
                    helm_plugin::runtime::ArchContext::Aarch64 {
                        x: a.x, sp: a.sp, pc: a.pc, nzcv: a.nzcv,
                    }
                } else {
                    helm_plugin::runtime::ArchContext::None
                },
            });
        }

        // 6. Branch callback
        if self.plugins.has_branch_callbacks() && insn.is_branch() {
            let target = self.a64_state.as_ref().unwrap().pc;
            self.plugins.fire_branch(0, &helm_plugin::runtime::BranchInfo {
                pc,
                target,
                taken: pc_written,
                kind: classify_branch_kind(insn.opcode),
            });
        }

        Ok(())
    }

    fn step_aarch64_system(&mut self) -> Result<(), HartException> {
        let HelmEngine { ref mut a64_state, ref mut a64_fs, .. } = *self;
        let a64 = a64_state.as_mut().ok_or(HartException::Unsupported)?;
        let machine = a64_fs.as_mut().ok_or(HartException::Unsupported)?;

        // Record PC before step so we can detect and log branches afterwards.
        let pc_before = a64.pc;

        // Physical timer (PPI 30, INTID 30) — level-triggered signal.
        //
        // Checked every TIMER_CHECK_INTERVAL steps (not every instruction):
        //  - avoid taking the GIC Mutex on every step (massive overhead)
        //  - 1024-instruction granularity is fine for timer IRQ latency
        //
        // Both assert AND deassert on every check so that physical_level[]
        // in GicState is kept in sync with the timer condition. Without
        // deassert, cpu_eoi() re-pends the timer permanently, trapping the
        // kernel in an infinite timer interrupt loop.
        //
        // Reference: ../helm.git uses inject_timers() every 1024 blocks.
        const TIMER_CHECK_INTERVAL: u64 = 1024;
        if self.insns_retired % TIMER_CHECK_INTERVAL == 0 {
            if let Some(ref gic) = machine.gic {
                let mut g = gic.lock().unwrap();
                if fs::check_timer(a64, &mut machine.fs) {
                    g.assert_irq(30);   // timer expired: IRQ line HIGH
                } else {
                    g.deassert_irq(30); // deadline in future: IRQ line LOW
                }
            }
        }

        // Sync irq_pending from the GIC IRQ line (level-triggered, not edge).
        //
        // Critical: we ASSIGN (not OR) so that irq_pending tracks the GIC line
        // exactly. When the kernel reads GICC_IAR the GIC line drops, and on the
        // next step irq_pending becomes false — preventing spurious re-interrupts
        // after ERET when the kernel restores DAIF (unmasks IRQs).
        machine.fs.irq_pending = machine.irq_line
            .as_ref()
            .map_or(false, |l| l.load(std::sync::atomic::Ordering::Relaxed));

        let result = fs::step_aarch64_fs(a64, &mut machine.sys_mem, &mut machine.fs);

        // Emit branch event via sim_trace when PC changed non-linearly.
        // Covers taken branches, calls, returns, and exception entry/return.
        let pc_after = a64.pc;
        if pc_after != pc_before.wrapping_add(4) {
            helm_debug::sim_branch!(pc = pc_before, target = pc_after);
        }

        result
    }

    /// Load a static AArch64 ELF binary and set up the engine for SE mode.
    ///
    /// Initialises `a64_state`, sets PC and SP, and configures the syscall handler.
    pub fn load_aarch64_elf(&mut self, path: &str, argv: &[&str], envp: &[&str]) -> Result<(), String> {
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

        self.a64_state   = Some(state);
        self.a64_handler = Some(handler);
        self.a64_fs      = None;
        self.mode        = ExecMode::Syscall;
        self.symbols     = loaded.symbols;

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
    ) -> Result<(), String> {
        let (state, sys_mem, fs, devs, irq_line, gic_state) = arm_virt::setup_arm_virt_boot(
            kernel_path,
            dtb_path,
            initrd_path,
            append,
            self.mem_size / (1024 * 1024),
            Box::new(arm_virt::StdioCharBackend),
        )?;

        self.a64_state = Some(state);
        self.a64_handler = None;
        self.a64_fs = Some(Aarch64FsMachine { sys_mem, fs, devs, irq_line: Some(irq_line), gic: Some(gic_state) });
        self.mode = ExecMode::System;
        self.symbols.clear();

        self.plugins.fire_vcpu_init(0);
        Ok(())
    }

    fn step_riscv(&mut self) -> Result<(), HartException> {
        let pc = self.pc;

        // 1. Fetch
        let raw = self.memory.fetch32(pc).map_err(|_| {
            HartException::InstructionAccessFault { addr: pc }
        })?;

        // 2. Decode
        let insn = riscv_decode(raw, pc).map_err(|e| match e {
            DecodeError::Unknown { raw, pc } => HartException::IllegalInstruction { pc, raw },
            DecodeError::Unimplemented      => HartException::Unsupported,
        })?;

        // 3. Execute (writes PC itself)
        riscv_execute(insn, self)?;

        // 4. Timing
        let info = InsnInfo {
            pc,
            is_branch: insn.is_control_flow(),
            is_load:   insn.is_mem_access(),
            is_store:  insn.is_mem_access(),
            is_fp:     false, // TODO: add is_fp() to Instruction
        };
        self.timing.on_insn(&info);

        Ok(())
    }

    fn handle_exception(&mut self, exc: HartException) -> StopReason {
        match exc {
            HartException::EnvironmentCall { pc: _, nr } => {
                if self.mode == ExecMode::Syscall {
                    // AArch64: syscall number from X8 (passed in `nr`), args from X0-X5
                    if self.isa == Isa::AArch64 {
                        return self.dispatch_aarch64_syscall(nr);
                    }
                    // RISC-V: forward to generic SyscallHandler
                    if let Some(handler) = &mut self.syscall_handler {
                        let args = SyscallArgs {
                            a0: self.iregs[10], a1: self.iregs[11],
                            a2: self.iregs[12], a3: self.iregs[13],
                            a4: self.iregs[14], a5: self.iregs[15],
                        };
                        match handler.handle(nr, args) {
                            Ok(ret) => { self.iregs[10] = ret as u64; }
                            Err(e)  => return self.handle_exception(e),
                        }
                    }
                    return StopReason::Quantum;
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
            HartException::Unsupported => StopReason::Unsupported,
            other => {
                // Fire plugin fault callback before returning.
                let (pc, raw_insn, kind, message) = match &other {
                    HartException::IllegalInstruction { pc, raw } => {
                        (*pc, *raw, helm_plugin::runtime::FaultKind::IllegalInstruction,
                         format!("illegal instruction at {pc:#x} (raw={raw:#010x})"))
                    }
                    HartException::Breakpoint { pc } => {
                        let raw = self.memory.fetch32(*pc).unwrap_or(0);
                        (*pc, raw, helm_plugin::runtime::FaultKind::Breakpoint,
                         format!("breakpoint at {pc:#x}"))
                    }
                    HartException::InstructionAddressMisaligned { addr } => {
                        (*addr, 0, helm_plugin::runtime::FaultKind::WildJump,
                         format!("instruction address misaligned: {addr:#x}"))
                    }
                    HartException::LoadAccessFault { addr } => {
                        (0, 0, helm_plugin::runtime::FaultKind::MemoryFault,
                         format!("load access fault at {addr:#x}"))
                    }
                    HartException::StoreAccessFault { addr } => {
                        (0, 0, helm_plugin::runtime::FaultKind::MemoryFault,
                         format!("store/AMO access fault at {addr:#x}"))
                    }
                    HartException::InstructionAccessFault { addr } => {
                        (*addr, 0, helm_plugin::runtime::FaultKind::MemoryFault,
                         format!("instruction access fault at {addr:#x}"))
                    }
                    _ => (0, 0, helm_plugin::runtime::FaultKind::IllegalInstruction,
                          format!("{other}"))
                };
                let context = if let Some(a) = &self.a64_state {
                    helm_plugin::runtime::ArchContext::Aarch64 {
                        x: a.x, sp: a.sp, pc: a.pc, nzcv: a.nzcv,
                    }
                } else {
                    helm_plugin::runtime::ArchContext::RiscV {
                        x: self.iregs, pc: self.pc,
                    }
                };
                self.plugins.fire_fault(&helm_plugin::runtime::FaultInfo {
                    vcpu_idx: 0, pc, raw: raw_insn, kind, message,
                    insn_count: self.insns_retired, context,
                });
                StopReason::Exception(other)
            }
        }
    }

    /// Dispatch one AArch64 SVC syscall to `LinuxAarch64SyscallHandler`.
    fn dispatch_aarch64_syscall(&mut self, nr: u64) -> StopReason {
        // Borrow arch state and handler separately — can't borrow self twice.
        let (x0, x1, x2, x3, x4, x5) = {
            let a = self.a64_state.as_ref().expect("a64_state missing");
            (a.x[0], a.x[1], a.x[2], a.x[3], a.x[4], a.x[5])
        };
        let args = SyscallArgs { a0: x0, a1: x1, a2: x2, a3: x3, a4: x4, a5: x5 };

        // Fire plugin pre-syscall event
        self.plugins.fire_syscall(&helm_plugin::runtime::SyscallInfo {
            vcpu_idx: 0, number: nr, args: [x0, x1, x2, x3, x4, x5],
        });

        let result = if let Some(h) = &mut self.a64_handler {
            h.handle(nr, args, &mut self.memory)
        } else {
            Ok(-38) // -ENOSYS if no handler
        };

        match result {
            Ok(ret) => {
                if let Some(a) = &mut self.a64_state {
                    a.x[0] = ret as u64;
                    // Advance PC past the SVC instruction
                    a.pc = a.pc.wrapping_add(4);
                }
                // Fire plugin post-syscall event
                self.plugins.fire_syscall_ret(&helm_plugin::runtime::SyscallRetInfo {
                    vcpu_idx: 0, number: nr, ret_value: ret as u64,
                });
                StopReason::Quantum
            }
            Err(HartException::Exit { code }) => StopReason::Exit { code },
            Err(e) => StopReason::Exception(e),
        }
    }
}

// ── ExecContext impl for HelmEngine<T> ───────────────────────────────────────

impl<T: TimingModel> ExecContext for HelmEngine<T> {
    #[inline(always)]
    fn read_int_reg(&self, idx: usize) -> u64 { self.iregs[idx] }

    #[inline(always)]
    fn write_int_reg(&mut self, idx: usize, val: u64) {
        if idx != 0 { self.iregs[idx] = val; }
    }

    #[inline(always)]
    fn read_float_reg_bits(&self, idx: usize) -> u64 { self.fregs[idx] }

    #[inline(always)]
    fn write_float_reg_bits(&mut self, idx: usize, val: u64) { self.fregs[idx] = val; }

    #[inline(always)]
    fn read_csr(&self, addr: u16) -> u64 { self.csrs[addr as usize] }

    #[inline(always)]
    fn write_csr(&mut self, addr: u16, val: u64) { self.csrs[addr as usize] = val; }

    #[inline(always)]
    fn read_pc(&self) -> u64 { self.pc }

    #[inline(always)]
    fn write_pc(&mut self, val: u64) { self.pc = val; }

    #[inline(always)]
    fn read_mem(&mut self, addr: u64, size: usize, ty: AccessType) -> Result<u64, MemFault> {
        self.memory.read(addr, size, ty)
    }

    #[inline(always)]
    fn write_mem(&mut self, addr: u64, size: usize, val: u64, ty: AccessType) -> Result<(), MemFault> {
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
            Self::Virtual(e)  => e.run(max_insns),
            Self::Interval(e) => e.run(max_insns),
            Self::Accurate(e) => e.run(max_insns),
        }
    }

    pub fn insns_retired(&self) -> u64 {
        match self {
            Self::Virtual(e)  => e.insns_retired,
            Self::Interval(e) => e.insns_retired,
            Self::Accurate(e) => e.insns_retired,
        }
    }

    pub fn set_pc(&mut self, pc: u64) {
        match self {
            Self::Virtual(e)  => e.set_pc(pc),
            Self::Interval(e) => e.set_pc(pc),
            Self::Accurate(e) => e.set_pc(pc),
        }
    }

    pub fn load_bytes(&mut self, addr: u64, bytes: &[u8]) {
        match self {
            Self::Virtual(e)  => e.load_bytes(addr, bytes),
            Self::Interval(e) => e.load_bytes(addr, bytes),
            Self::Accurate(e) => e.load_bytes(addr, bytes),
        }
    }

    /// Load an AArch64 ELF binary and configure the engine for SE mode.
    pub fn load_aarch64_elf(&mut self, path: &str, argv: &[&str], envp: &[&str]) -> Result<(), String> {
        match self {
            Self::Virtual(e)  => e.load_aarch64_elf(path, argv, envp),
            Self::Interval(e) => e.load_aarch64_elf(path, argv, envp),
            Self::Accurate(e) => e.load_aarch64_elf(path, argv, envp),
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
    ) -> Result<(), String> {
        match self {
            Self::Virtual(e)  => e.load_aarch64_kernel(kernel_path, dtb_path, initrd_path, append),
            Self::Interval(e) => e.load_aarch64_kernel(kernel_path, dtb_path, initrd_path, append),
            Self::Accurate(e) => e.load_aarch64_kernel(kernel_path, dtb_path, initrd_path, append),
        }
    }

    /// Get mutable reference to the plugin registry.
    pub fn plugins_mut(&mut self) -> &mut PluginRegistry {
        match self {
            Self::Virtual(e)  => &mut e.plugins,
            Self::Interval(e) => &mut e.plugins,
            Self::Accurate(e) => &mut e.plugins,
        }
    }

    /// Get the loaded ELF symbol table.
    pub fn symbols(&self) -> &[loader::ElfSymbol] {
        match self {
            Self::Virtual(e)  => &e.symbols,
            Self::Interval(e) => &e.symbols,
            Self::Accurate(e) => &e.symbols,
        }
    }

    /// Resolve a symbol name to its address. Returns None if not found.
    pub fn resolve_symbol(&self, name: &str) -> Option<u64> {
        self.symbols().iter().find(|s| s.name == name).map(|s| s.addr)
    }

    pub fn has_unimplemented_instructions(&self) -> bool {
        match self {
            Self::Virtual(e)  => e.has_unimplemented_instructions(),
            Self::Interval(e) => e.has_unimplemented_instructions(),
            Self::Accurate(e) => e.has_unimplemented_instructions(),
        }
    }

    pub fn unimplemented_instruction_count(&self) -> usize {
        match self {
            Self::Virtual(e)  => e.unimplemented_instruction_count(),
            Self::Interval(e) => e.unimplemented_instruction_count(),
            Self::Accurate(e) => e.unimplemented_instruction_count(),
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
            Self::Virtual(e)  => { if let Some(s) = e.a64_state.as_mut() { m.apply(s); } }
            Self::Interval(e) => { if let Some(s) = e.a64_state.as_mut() { m.apply(s); } }
            Self::Accurate(e) => { if let Some(s) = e.a64_state.as_mut() { m.apply(s); } }
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
        TimingChoice::Virtual { ipc } => {
            HelmSim::Virtual(HelmEngine::new(isa, mode, Virtual::new(ipc), mem_base, mem_size))
        }
        TimingChoice::Interval { ipc, interval_len } => HelmSim::Interval(HelmEngine::new(
            isa,
            mode,
            Interval::new(ipc, interval_len),
            mem_base,
            mem_size,
        )),
        TimingChoice::Accurate => {
            HelmSim::Accurate(HelmEngine::new(isa, mode, Accurate::default(), mem_base, mem_size))
        }
    }
}

/// Classify an AArch64 opcode for the plugin system.
/// Returns (InsnClass, opcode_name, is_stub).
fn classify_aarch64_opcode(
    op: helm_arch::aarch64::insn::Opcode,
) -> (helm_plugin::runtime::InsnClass, &'static str, bool) {
    use helm_arch::aarch64::insn::Opcode::*;
    use helm_plugin::runtime::InsnClass;

    match op {
        // Data processing
        Adr | Adrp | AddImm | SubImm | AddsImm | SubsImm
        | AndImm | OrrImm | EorImm | AndsImm
        | Movn | Movz | Movk | Sbfm | Bfm | Ubfm | Extr
        | AddReg | SubReg | AddsReg | SubsReg
        | AddExt | SubExt | AddsExt | SubsExt
        | AndReg | OrrReg | EorReg | AndsReg | BicReg | OrnReg | EonReg | BicsReg
        | Adc | Adcs | Sbc | Sbcs
        | Lsl | Lsr | Asr | Ror
        | Cls | Clz | Rev | Rev16 | Rev32 | Rbit
        | Csel | Csinc | Csinv | Csneg | Ccmn | Ccmp
            => (InsnClass::IntAlu, "IntAlu", false),

        Mul | Madd | Msub | Mneg | Smulh | Umulh | Sdiv | Udiv
        | Smaddl | Smsubl | Umaddl | Umsubl
            => (InsnClass::IntMul, "IntMul", false),

        Crc32 | Crc32c => (InsnClass::IntAlu, "Crc32", true), // stub

        // Branch
        B | Bl | Br | Blr | Ret | BCond | Cbz | Cbnz | Tbz | Tbnz
        | Svc | Hvc | Smc | Eret
            => (InsnClass::Branch, "Branch", false),

        // Load/Store
        Ldr | Ldrb | Ldrh | Ldrsb | Ldrsh | Ldrsw | LdrLit | LdrswLit
        | Ldp | Ldur | Ldurb | Ldurh | Ldursb | Ldursh | Ldursw
        | Ldxr | Ldaxr | Ldar
        | LdrSimd | LdpSimd | LdurSimd
            => (InsnClass::Load, "Load", false),

        Str | Strb | Strh | Stp | Stur | Sturb | Sturh
        | Stxr | Stlxr | Stlr
        | StrSimd | StpSimd | SturSimd
            => (InsnClass::Store, "Store", false),

        // Atomics
        Ldadd | Ldclr | Ldeor | Ldset | LdSmax | LdSmin | LdUmax | LdUmin | Swp | Cas | Casp
            => (InsnClass::Atomic, "Atomic", false),

        // FP
        FmovImm | FmovReg | FmovGpr
        | Fadd | Fsub | Fmul | Fdiv | Fsqrt | Fabs | Fneg | Fnmul
        | Fmax | Fmin | Fmaxnm | Fminnm
        | Fmadd | Fmsub | Fnmadd | Fnmsub
        | Fcmp | Fcmpe | Fccmp | Fccmpe | Fcvt | Fsel
        | FcvtzsGpr | FcvtzuGpr | ScvtfGpr | UcvtfGpr
        | FcvtnsGpr | FcvtnuGpr | FcvtmsGpr | FcvtmuGpr
        | FcvtpsGpr | FcvtpuGpr | FcvtasGpr | FcvtauGpr
            => (InsnClass::FpAlu, "FpAlu", false),

        // System
        Nop | Wfi | Wfe | Sev | Sevl | Yield | Dmb | Dsb | Isb
        | Brk | Mrs | Msr | MsrImm | Sys | Clrex | Prfm
            => (InsnClass::System, "System", false),

        DcZva => (InsnClass::System, "DcZva", false),

        // SIMD — implemented
        SimdDup | SimdIns | SimdUmov | SimdSmov | SimdMovi
        | SimdAdd | SimdSub | SimdMul
        | SimdAnd | SimdOrr | SimdEor | SimdBic
        | SimdNot | SimdNeg | SimdAbs
        | SimdCmeq | SimdCmgt | SimdCmge | SimdCmhi | SimdCmhs
        | SimdUmaxv | SimdUminv
            => (InsnClass::SimdAlu, "SimdImpl", false),

        // SIMD — stubs (silently skipped)
        SimdOther => (InsnClass::SimdAlu, "SimdOther", true),
        SimdBif | SimdBit | SimdBsl | SimdOrrImm
            => (InsnClass::SimdAlu, "SimdLogic", true),
        SimdCmgt0 | SimdCmeq0 | SimdCmlt0 | SimdCmge0 | SimdCmle0
            => (InsnClass::SimdAlu, "SimdCmpZero", false),
        SimdCmtst
            => (InsnClass::SimdAlu, "SimdCmp", true),
        SimdAddp | SimdAddv
            => (InsnClass::SimdAlu, "SimdReduce", true),
        SimdSshl | SimdUshl | SimdSshr | SimdUshr | SimdShl
            => (InsnClass::SimdAlu, "SimdShift", true),
        SimdTbl | SimdTbx => (InsnClass::SimdAlu, "SimdTbl", true),
        SimdZip1 | SimdZip2 | SimdUzp1 | SimdUzp2 | SimdTrn1 | SimdTrn2
            => (InsnClass::SimdAlu, "SimdPermute", true),
        SimdExt => (InsnClass::SimdAlu, "SimdExt", true),
        SimdRev64 | SimdRev32 | SimdRev16 => (InsnClass::SimdAlu, "SimdRev", true),
        SimdCnt | SimdClz => (InsnClass::SimdAlu, "SimdBitCount", true),
        SimdSxtl | SimdUxtl => (InsnClass::SimdAlu, "SimdExtend", true),
        SimdSmin | SimdUmin | SimdSmax | SimdUmax => (InsnClass::SimdAlu, "SimdMinMax", true),
        SimdFadd | SimdFsub | SimdFmul | SimdFdiv
        | SimdFabs | SimdFneg | SimdFsqrt
        | SimdFcmeq | SimdFcmgt | SimdFcmge
        | SimdFcvtzs | SimdFcvtzu | SimdScvtf | SimdUcvtf
        | SimdFrintm | SimdFrintn | SimdFrintp | SimdFrintz
            => (InsnClass::SimdAlu, "SimdFp", true),
        SimdMvni | SimdFmov => (InsnClass::SimdAlu, "SimdMov", true),
        SimdLd1 | SimdSt1 | SimdLd2 | SimdSt2 | SimdLd3 | SimdSt3 | SimdLd4 | SimdSt4
        | SimdLd1r
            => (InsnClass::SimdAlu, "SimdMultiStruct", true),

        FcvtzsVec | FcvtzuVec => (InsnClass::SimdAlu, "SimdVecCvt", true),

        ScalarAddp => (InsnClass::SimdAlu, "ScalarAddp", false),
        // v8.3/v8.4 new opcodes
        Ldapr | Ldaprh | Ldaprb => (InsnClass::Load, "LdaprRcpc", false),
        LdapurB | LdapurH | Ldapur => (InsnClass::Load, "LdapurRcpc2", false),
        StlurB | StlurH | Stlur => (InsnClass::Store, "StlurRcpc2", false),
        Fjcvtzs => (InsnClass::FpAlu, "Fjcvtzs", false),
        Fcadd | Fcmla => (InsnClass::SimdAlu, "FcmaComplex", false),
        Sdot | Udot => (InsnClass::SimdAlu, "DotProduct", false),
        Setf8 | Setf16 | Cfinv | Rmif => (InsnClass::IntAlu, "FlagM", false),
        Bti => (InsnClass::System, "Bti", false),
        Sha3 | Sha512 | Sm3 | Sm4 => (InsnClass::SimdAlu, "CryptoStub", true),

        Undefined => (InsnClass::Unknown, "Undefined", false),
    }
}

/// Classify an AArch64 branch opcode into a `BranchKind`.
fn classify_branch_kind(
    op: helm_arch::aarch64::insn::Opcode,
) -> helm_plugin::runtime::BranchKind {
    use helm_arch::aarch64::insn::Opcode::*;
    use helm_plugin::runtime::BranchKind;

    match op {
        B                                       => BranchKind::DirectUncond,
        Bl                                      => BranchKind::Call,
        Ret                                     => BranchKind::Return,
        Br                                      => BranchKind::IndirectJump,
        Blr                                     => BranchKind::IndirectCall,
        BCond | Cbz | Cbnz | Tbz | Tbnz        => BranchKind::DirectCond,
        _                                       => BranchKind::DirectUncond,
    }
}

/// Timing configuration passed to `build_simulator`.
pub enum TimingChoice {
    Virtual  { ipc: f64 },
    Interval { ipc: f64, interval_len: u64 },
    Accurate,
}

#[cfg(test)]
mod tests {
    use super::classify_aarch64_opcode;
    use crate::{ExecMode, HelmEngine, Isa, Virtual};
    use helm_arch::aarch64::insn::Opcode;

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
}

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

pub mod loader;
pub mod se;

use helm_arch::{
    aarch64_decode, aarch64_execute, Aarch64ArchState,
    riscv_decode, riscv_execute, DecodeError,
};
use helm_core::{AccessType, ExecContext, HartException, MemFault, MemInterface};
use helm_event::EventQueue;
use helm_timing::{Accurate, InsnInfo, Interval, TimingModel, Virtual};

use se::{LinuxAarch64SyscallHandler, SyscallArgs, SyscallHandler};

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

    pub memory: FlatMem,
    pub events: EventQueue,

    pub syscall_handler: Option<Box<dyn SyscallHandler>>,

    /// Total instructions retired.
    pub insns_retired: u64,
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
            memory: FlatMem::new(mem_base, mem_size),
            events: EventQueue::new(),
            syscall_handler: None,
            insns_retired: 0,
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

    /// Run up to `max_insns` instructions. Returns the reason for stopping.
    pub fn run(&mut self, max_insns: u64) -> StopReason {
        for _ in 0..max_insns {
            let result = match self.isa {
                Isa::RiscV   => self.step_riscv(),
                Isa::AArch64 => self.step_aarch64(),
                Isa::AArch32 => return StopReason::Unsupported,
            };
            match result {
                Ok(()) => { self.insns_retired += 1; }
                Err(exc) => {
                    let stop = self.handle_exception(exc);
                    // Check if AArch64 handler requested exit
                    if let Some(h) = &self.a64_handler {
                        if h.should_exit {
                            return StopReason::Exit { code: h.exit_code };
                        }
                    }
                    return stop;
                }
            }
        }
        StopReason::Quantum
    }

    /// Single-step one AArch64 instruction.
    fn step_aarch64(&mut self) -> Result<(), HartException> {
        let pc = self.a64_state.as_ref().ok_or(HartException::Unsupported)?.pc;

        // 1. Fetch
        let raw = self.memory.fetch32(pc).map_err(|_| {
            HartException::InstructionAccessFault { addr: pc }
        })?;

        // 2. Decode
        let insn = aarch64_decode(raw, pc).map_err(|e| match e {
            DecodeError::Unknown { raw, pc } => HartException::IllegalInstruction { pc, raw },
            DecodeError::Unimplemented      => HartException::Unsupported,
        })?;

        // 3. Execute
        let a64 = self.a64_state.as_mut().unwrap();
        let pc_written = aarch64_execute(&insn, a64, &mut self.memory)?;
        if !pc_written {
            a64.pc = a64.pc.wrapping_add(4);
        }

        // 4. Timing
        let info = InsnInfo {
            pc,
            is_branch: insn.is_branch(),
            is_load:   insn.is_mem_access(),
            is_store:  insn.is_mem_access(),
            is_fp:     false,
        };
        self.timing.on_insn(&info);

        Ok(())
    }

    /// Load a static AArch64 ELF binary and set up the engine for SE mode.
    ///
    /// Initialises `a64_state`, sets PC and SP, and configures the syscall handler.
    pub fn load_aarch64_elf(&mut self, path: &str, argv: &[&str], envp: &[&str]) -> Result<(), String> {
        use loader::load_elf;

        let loaded = load_elf(path, argv, envp, &mut self.memory)?;

        let mut state = Aarch64ArchState::new();
        state.pc = loaded.entry_point;
        state.sp = loaded.initial_sp;

        let mut handler = LinuxAarch64SyscallHandler::new(loaded.brk_base);
        handler.binary_path = path.to_string();

        self.a64_state   = Some(state);
        self.a64_handler = Some(handler);
        self.mode        = ExecMode::Syscall;

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
            HartException::Unsupported => StopReason::Unsupported,
            other => StopReason::Exception(other),
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

/// Timing configuration passed to `build_simulator`.
pub enum TimingChoice {
    Virtual  { ipc: f64 },
    Interval { ipc: f64, interval_len: u64 },
    Accurate,
}

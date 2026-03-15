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

pub mod se;

use helm_arch::{riscv_decode, riscv_execute, DecodeError};
use helm_core::{AccessType, ExecContext, HartException, MemFault, MemInterface};
use helm_event::EventQueue;
use helm_timing::{Accurate, InsnInfo, Interval, MemAccess, TimingModel, Virtual};

use se::SyscallHandler;

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

/// Phase 0 flat memory: a single `Vec<u8>` representing the full guest address space.
///
/// Replace with `helm_memory::MemoryMap` in Phase 1.
pub struct FlatMem {
    data: Vec<u8>,
    base: u64,
}

impl FlatMem {
    pub fn new(base: u64, size: usize) -> Self {
        Self { data: vec![0u8; size], base }
    }

    /// Load bytes into the flat memory (e.g. ELF segment).
    pub fn load_bytes(&mut self, addr: u64, bytes: &[u8]) {
        let offset = (addr - self.base) as usize;
        self.data[offset..offset + bytes.len()].copy_from_slice(bytes);
    }
}

impl MemInterface for FlatMem {
    fn read(&mut self, addr: u64, size: usize, _ty: AccessType) -> Result<u64, MemFault> {
        let offset = addr.checked_sub(self.base).ok_or(MemFault::AccessFault { addr })? as usize;
        let end = offset + size;
        if end > self.data.len() { return Err(MemFault::AccessFault { addr }); }
        let mut buf = [0u8; 8];
        buf[..size].copy_from_slice(&self.data[offset..end]);
        Ok(u64::from_le_bytes(buf))
    }

    fn write(&mut self, addr: u64, size: usize, val: u64, _ty: AccessType) -> Result<(), MemFault> {
        let offset = addr.checked_sub(self.base).ok_or(MemFault::AccessFault { addr })? as usize;
        let end = offset + size;
        if end > self.data.len() { return Err(MemFault::AccessFault { addr }); }
        self.data[offset..end].copy_from_slice(&val.to_le_bytes()[..size]);
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
                Isa::RiscV  => self.step_riscv(),
                Isa::AArch64 | Isa::AArch32 => return StopReason::Unsupported,
            };
            match result {
                Ok(()) => { self.insns_retired += 1; }
                Err(exc) => return self.handle_exception(exc),
            }
        }
        StopReason::Quantum
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
        match &exc {
            HartException::EnvironmentCall { pc, nr } => {
                if self.mode == ExecMode::Syscall {
                    if let Some(handler) = &mut self.syscall_handler {
                        // TODO(phase-0): pass ThreadContext wrapper to handler
                        let _ = (pc, nr);
                    }
                    return StopReason::Quantum; // continue after syscall
                }
                StopReason::Exception(exc)
            }
            HartException::Exit { code } => StopReason::Exit { code: *code },
            HartException::Unsupported => StopReason::Unsupported,
            _ => StopReason::Exception(exc),
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

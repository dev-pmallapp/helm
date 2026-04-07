//! `helm-core` — foundational types, traits, and abstractions shared by all crates.
//!
//! Zero internal helm-* dependencies. Every other crate depends on this one.
//!
//! # Module layout
//! - [`attr`]    — named attribute registry (state exposure for checkpointing)
//! - [`error`]   — `HartException` (exceptions raised during execution)
//! - [`mem`]     — `MemFault`, `AccessType`, `MemInterface`, `ByteMem`
//! - [`sysreg`]  — system register map for `AArch64` dispatch
//!
//! # Key traits
//! - [`ArchState`]       — ISA register file + PC; implemented per ISA
//! - [`ExecContext`]     — hot-path execution interface; implemented by `HelmEngine<T>`
//! - [`ThreadContext`]   — cold-path introspection; may be boxed as `dyn ThreadContext`
//! - [`TimerScheduler`]  — schedule callbacks at future simulation ticks
//! - [`PowerController`] — controls CPU power state (PSCI)
//! - [`DmaPort`]         — DMA-capable memory access interface

#![allow(clippy::module_name_repetitions)]

pub mod attr;
pub mod error;
pub mod mem;
pub mod sysreg;

pub use attr::{AttrRegistry, AttrValue};
pub use error::HartException;
pub use mem::{
    AccessType, ByteMem, MemFault, MemInterface, MemoryMap, MemoryMapRange, MemoryMapRangeKind,
};
pub use sysreg::{SysRegEntry, SysRegHandler, SysRegKey, SysRegMap};

// ── ArchState ────────────────────────────────────────────────────────────────

/// ISA-specific architectural register file + PC.
///
/// Implemented once per ISA (e.g. `RiscvArchState`, `Aarch64ArchState`).
/// Always statically dispatched — never boxed.
pub trait ArchState: Send + 'static {
    /// Read an integer (general-purpose) register. Idx 0 must always return 0 for RISC-V.
    fn read_int_reg(&self, idx: usize) -> u64;
    /// Write an integer register. Implementations must ignore writes to idx 0 for RISC-V.
    fn write_int_reg(&mut self, idx: usize, val: u64);
    /// Read the program counter.
    fn read_pc(&self) -> u64;
    /// Write the program counter.
    fn write_pc(&mut self, val: u64);
    /// Register all architectural state fields into `registry` for checkpointing.
    fn register_attrs(&self, registry: &mut AttrRegistry);
    /// Reset to the post-reset architectural state (`PC=reset_vector`, regs=0).
    fn reset(&mut self, reset_vector: u64);
}

// ── ExecContext ───────────────────────────────────────────────────────────────

/// Hot-path execution interface. **Never boxed** — always `&mut impl ExecContext`.
///
/// Implemented by `HelmEngine<T>`. Passed directly to ISA `execute()` functions
/// so that integer-register reads, memory accesses, and PC updates are inlined.
pub trait ExecContext {
    /// Read an integer register by architectural index.
    fn read_int_reg(&self, idx: usize) -> u64;
    /// Write an integer register by architectural index.
    fn write_int_reg(&mut self, idx: usize, val: u64);

    /// Read a floating-point register as raw bits.
    ///
    /// Any NaN-boxing or lane interpretation is handled by the ISA layer.
    fn read_float_reg_bits(&self, idx: usize) -> u64;
    /// Write a floating-point register as raw bits.
    fn write_float_reg_bits(&mut self, idx: usize, val: u64);

    /// Read a control/status register by architectural address.
    fn read_csr(&self, addr: u16) -> u64;
    /// Write a control/status register by architectural address.
    fn write_csr(&mut self, addr: u16, val: u64);

    /// Read the current program counter.
    fn read_pc(&self) -> u64;
    /// Write the current program counter.
    fn write_pc(&mut self, val: u64);

    /// Read guest memory.
    ///
    /// `size` is in bytes and is typically 1, 2, 4, or 8.
    fn read_mem(&mut self, addr: u64, size: usize, ty: AccessType) -> Result<u64, MemFault>;
    /// Write guest memory.
    ///
    /// `size` is in bytes and is typically 1, 2, 4, or 8.
    fn write_mem(
        &mut self,
        addr: u64,
        size: usize,
        val: u64,
        ty: AccessType,
    ) -> Result<(), MemFault>;
}

// ── ThreadContext ─────────────────────────────────────────────────────────────

/// Cold-path introspection + control interface.
///
/// Extends `ExecContext` and may be boxed as `dyn ThreadContext`. Passed to
/// syscall handlers, the GDB stub, and Python-facing APIs. Never on the hot path.
pub trait ThreadContext: ExecContext {
    /// The hart (hardware thread) identifier.
    fn hart_id(&self) -> u64;
    /// Human-readable ISA name (e.g. `"riscv64"`, `"aarch64"`).
    fn isa_name(&self) -> &'static str;
    /// Pause the hart (e.g. waiting on I/O or a lock).
    fn pause(&mut self);
    /// Resume a paused hart.
    fn resume(&mut self);
}

// ── TimerScheduler ──────────────────────────────────────────────────────────

/// Trait for scheduling callbacks at future simulation ticks.
///
/// Implemented by `EventQueue` in `helm-event`. Devices store
/// `Arc<dyn TimerScheduler>` populated at `elaborate()`.
pub trait TimerScheduler: Send + Sync + 'static {
    /// Schedule a callback to fire after `delay_ticks` from now.
    /// Returns an opaque ID that can be used for cancellation.
    fn schedule_callback(&self, delay_ticks: u64, class_id: u32, owner_id: u64) -> u64;

    /// Return the current simulation tick.
    fn current_tick(&self) -> u64;

    /// Cancel a previously scheduled callback by its ID.
    fn cancel(&self, event_id: u64) -> bool;
}

// ── PowerController ─────────────────────────────────────────────────────────

/// Controls CPU power state. Implemented by `helm-engine`.
///
/// PSCI SMC/HVC handler receives `Arc<dyn PowerController>` at `elaborate()`.
pub trait PowerController: Send + Sync {
    /// Power on a CPU identified by target MPIDR.
    fn cpu_on(
        &self,
        target_mpidr: u64,
        entry_point: u64,
        context_id: u64,
    ) -> Result<(), PowerError>;

    /// Power off the calling CPU.
    fn cpu_off(&self, this_mpidr: u64) -> Result<(), PowerError>;

    /// Perform a system-level reset.
    fn system_reset(&self) -> Result<(), PowerError>;
}

/// Errors from power operations.
#[derive(Debug)]
pub enum PowerError {
    /// The target CPU is already on.
    AlreadyOn,
    /// The target CPU was not found.
    InvalidTarget,
    /// The operation is not supported.
    NotSupported,
    /// Internal error.
    Internal(String),
}

impl std::fmt::Display for PowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyOn => write!(f, "CPU already powered on"),
            Self::InvalidTarget => write!(f, "invalid target CPU"),
            Self::NotSupported => write!(f, "operation not supported"),
            Self::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for PowerError {}

// ── DmaPort ─────────────────────────────────────────────────────────────────

/// DMA-capable memory access interface.
///
/// Current runtime work should route this through the live physical-memory
/// surface, which today is typically a shared
/// [`helm_memory::HelmAddressSpace`] wrapper such as
/// [`helm_memory::SharedDmaPort`].
pub trait DmaPort: Send + Sync {
    /// Read `buf.len()` bytes from guest physical address `addr`.
    fn dma_read(&self, addr: u64, buf: &mut [u8]) -> Result<(), MemFault>;

    /// Write `buf` bytes to guest physical address `addr`.
    fn dma_write(&self, addr: u64, buf: &[u8]) -> Result<(), MemFault>;
}

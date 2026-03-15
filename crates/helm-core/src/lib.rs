//! `helm-core` — foundational types, traits, and abstractions shared by all crates.
//!
//! Zero internal helm-* dependencies. Every other crate depends on this one.
//!
//! # Module layout
//! - [`attr`]   — named attribute registry (state exposure for checkpointing)
//! - [`error`]  — `HartException` (exceptions raised during execution)
//! - [`mem`]    — `MemFault`, `AccessType`, `MemInterface` trait
//!
//! # Key traits
//! - [`ArchState`]     — ISA register file + PC; implemented per ISA
//! - [`ExecContext`]   — hot-path execution interface; implemented by `HelmEngine<T>`
//! - [`ThreadContext`] — cold-path introspection; may be boxed as `dyn ThreadContext`

#![warn(missing_docs)]
#![allow(clippy::module_name_repetitions)]

pub mod attr;
pub mod error;
pub mod mem;

pub use attr::{AttrRegistry, AttrValue};
pub use error::HartException;
pub use mem::{AccessType, MemFault, MemInterface};

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
    /// Reset to the post-reset architectural state (PC=reset_vector, regs=0).
    fn reset(&mut self, reset_vector: u64);
}

// ── ExecContext ───────────────────────────────────────────────────────────────

/// Hot-path execution interface. **Never boxed** — always `&mut impl ExecContext`.
///
/// Implemented by `HelmEngine<T>`. Passed directly to ISA `execute()` functions
/// so that integer-register reads, memory accesses, and PC updates are inlined.
pub trait ExecContext {
    // Integer registers
    fn read_int_reg(&self, idx: usize) -> u64;
    fn write_int_reg(&mut self, idx: usize, val: u64);

    // Floating-point registers (stored as raw bits; NaN-boxing handled by ISA layer)
    fn read_float_reg_bits(&self, idx: usize) -> u64;
    fn write_float_reg_bits(&mut self, idx: usize, val: u64);

    // Control/status registers
    fn read_csr(&self, addr: u16) -> u64;
    fn write_csr(&mut self, addr: u16, val: u64);

    // Program counter
    fn read_pc(&self) -> u64;
    fn write_pc(&mut self, val: u64);

    // Memory access — size in bytes (1, 2, 4, 8)
    fn read_mem(&mut self, addr: u64, size: usize, ty: AccessType) -> Result<u64, MemFault>;
    fn write_mem(&mut self, addr: u64, size: usize, val: u64, ty: AccessType) -> Result<(), MemFault>;
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

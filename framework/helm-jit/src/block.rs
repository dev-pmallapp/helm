//! Compiled translation block wrapper.

#![allow(missing_docs)]

use std::any::Any;
use std::pin::Pin;

/// Exit codes returned by JIT-compiled blocks in `rax`.
pub const EXIT_END_OF_BLOCK: u64 = 0;
pub const EXIT_CHAIN: u64 = 1;
pub const EXIT_SYSCALL: u64 = 2;
pub const EXIT_EXCEPTION: u64 = 3;

/// Function pointer type for compiled block entry points.
///
/// # Arguments
/// - `regs`: `*mut u64` — pointer to the flat register array (`[u64; 48]`)
/// - `mem`:  `*mut u8`  — opaque pointer to `FlatMem` (passed to memory helpers)
///
/// # Returns
/// Exit code in `rax` (see `EXIT_*` constants).
pub type JitBlockFn = unsafe extern "C" fn(regs: *mut u64, mem: *mut u8) -> u64;

/// A compiled translation block.
///
/// The buffer is `Pin<Box<...>>` to guarantee the backing executable memory
/// is never moved after construction — `entry` is a raw function pointer that
/// points into it.
pub struct CompiledBlock {
    /// Keeps the executable pages alive and pinned. The concrete type depends
    /// on the backend (e.g. `dynasmrt::ExecutableBuffer` for dynasm).
    /// Pinned so that `entry` (which points into this allocation) remains valid.
    _buf: Pin<Box<dyn Any + Send + Sync>>,
    /// Entry point function pointer (points into `_buf`).
    pub entry: JitBlockFn,
    /// Guest PC at which this block starts.
    pub guest_pc: u64,
    /// Number of guest instructions compiled into this block.
    pub insn_count: u32,
}

impl CompiledBlock {
    /// Create a new compiled block from a backend-produced buffer.
    ///
    /// # Safety
    /// - `entry` must point into valid executable memory owned by `buf`.
    /// - The code at `entry` must follow the JIT calling convention
    ///   (rdi=regs, rsi=mem, returns exit code in rax).
    #[allow(unsafe_code)]
    pub unsafe fn new(
        buf: impl Any + Send + Sync,
        entry: JitBlockFn,
        guest_pc: u64,
        insn_count: u32,
    ) -> Self {
        Self {
            _buf: Box::pin(buf),
            entry,
            guest_pc,
            insn_count,
        }
    }
}

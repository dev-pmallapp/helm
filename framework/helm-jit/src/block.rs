//! Compiled translation block wrapper.

#![allow(missing_docs)]

use dynasmrt::ExecutableBuffer;

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
pub struct CompiledBlock {
    /// The executable buffer holding the generated x86-64 code.
    _buf: ExecutableBuffer,
    /// Entry point function pointer (points into `_buf`).
    pub entry: JitBlockFn,
    /// Guest PC at which this block starts.
    pub guest_pc: u64,
    /// Number of guest instructions compiled into this block.
    pub insn_count: u32,
}

impl CompiledBlock {
    /// Create a new compiled block from an assembled buffer.
    ///
    /// # Safety
    /// The buffer must contain valid x86-64 code that follows the JIT calling
    /// convention (rdi=regs, rsi=mem, returns exit code in rax).
    #[allow(unsafe_code)]
    pub unsafe fn new(buf: ExecutableBuffer, guest_pc: u64, insn_count: u32) -> Self {
        let entry: JitBlockFn = std::mem::transmute(buf.ptr(dynasmrt::AssemblyOffset(0)));
        Self {
            _buf: buf,
            entry,
            guest_pc,
            insn_count,
        }
    }
}

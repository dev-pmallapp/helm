//! Compiled translation block wrapper.

#![allow(missing_docs, unsafe_code)]

use dynasmrt::ExecutableBuffer;
use std::any::Any;
use std::pin::Pin;

/// Exit codes returned by JIT-compiled blocks in `rax`.
pub const EXIT_END_OF_BLOCK: u64 = 0;
pub const EXIT_SYSCALL: u64 = 1;
pub const EXIT_EXCEPTION: u64 = 2;

/// Function pointer type for compiled block entry points.
///
/// # Arguments
/// - `regs`: `*mut u64` — pointer to the flat register array (`[u64; 48]`)
/// - `mem`:  `*mut u8`  — opaque pointer to `FlatMem` (passed to memory helpers)
///
/// # Returns
/// Exit code in `rax` (see `EXIT_*` constants).
pub type JitBlockFn = unsafe extern "C" fn(regs: *mut u64, mem: *mut u8) -> u64;

// ── Patch sites (Phase 2-B: block chaining) ─────────────────────────────────

/// A patchable 5-byte exit slot at the end of a compiled block.
///
/// In the unlinked state the slot holds `ret + nop×4`.
/// When the target block is compiled, `JitCache::link_waiters()` overwrites
/// the slot with `jmp rel32` pointing at the target's entry.
#[derive(Debug, Clone)]
pub struct PatchSite {
    /// Byte offset of the 5-byte slot within the block's `ExecutableBuffer`.
    pub byte_offset: usize,
    /// Guest PC that this exit should jump to when linked.
    pub target_pc: u64,
    /// Whether this site is currently patched with `jmp rel32` (true) or
    /// the default `ret+nop×4` (false).
    pub linked: bool,
}

// ── Inline cache patches (Phase 2-E) ────────────────────────────────────────

/// A future inline-cache site in a compiled block.
///
/// This metadata is not active today. It is retained as the intended shape
/// for Phase 2-E once the runtime can arm IC patch contexts and the emitters
/// actually record specialisable memory-access sites.
#[derive(Debug, Clone)]
pub struct IcPatch {
    /// Byte offset within the block where the 8-byte host-pointer imm64 lives.
    pub imm64_offset: usize,
    /// Whether this IC has been specialised.
    pub specialized: bool,
    /// Guest page (guest_addr >> 12) this IC is valid for. Used for invalidation.
    pub guest_page: u64,
}

// ── CompiledBlock ────────────────────────────────────────────────────────────

/// A compiled translation block with chaining and IC metadata.
///
/// Two storage variants are supported:
/// - **Legacy**: backed by an arbitrary `dyn Any + Send + Sync` buffer (used by
///   the stencil backend and any non-patchable dynasm block).
/// - **Patchable**: backed by a dynasmrt `ExecutableBuffer` with a stored
///   entry-point offset, enabling `JitCache` to call `CodeArena::write_jmp_rel32`
///   or `CodeArena::patch` on demand.
///
/// The patchable variant is used for all dynasm-compiled blocks that participate
/// in block chaining (Phase 2-B) or IC specialisation (Phase 2-E).
pub struct CompiledBlock {
    inner: BlockInner,
    /// Entry point function pointer.
    pub entry: JitBlockFn,
    /// Guest PC at which this block starts.
    pub guest_pc: u64,
    /// Number of guest instructions compiled into this block.
    pub insn_count: u32,
    /// Patchable exit slots for block chaining (Phase 2-B).
    /// The current chaining model patches `ret+nop×4` into `jmp rel32` rather
    /// than returning a dedicated chain exit code to the runtime.
    pub patch_sites: Vec<PatchSite>,
    /// Future inline-cache sites for speculative memory specialisation (Phase 2-E).
    pub ic_patches: Vec<IcPatch>,
}

#[allow(dead_code)]
enum BlockInner {
    /// Opaque buffer (stencil or non-patchable dynasm).
    Opaque(Pin<Box<dyn Any + Send + Sync>>),
    /// dynasmrt `ExecutableBuffer` with the entry offset for pointer arithmetic.
    Patchable {
        buf: ExecutableBuffer,
        entry_offset: usize,
    },
}

impl CompiledBlock {
    /// Create a block from an opaque backend buffer.
    ///
    /// # Safety
    /// `entry` must point into valid executable memory owned by `buf`.
    pub unsafe fn new(
        buf: impl Any + Send + Sync,
        entry: JitBlockFn,
        guest_pc: u64,
        insn_count: u32,
    ) -> Self {
        Self {
            inner: BlockInner::Opaque(Box::pin(buf)),
            entry,
            guest_pc,
            insn_count,
            patch_sites: Vec::new(),
            ic_patches: Vec::new(),
        }
    }

    /// Create a patchable block from a dynasmrt `ExecutableBuffer`.
    ///
    /// `entry_offset` is the `AssemblyOffset` value of the entry label,
    /// used to compute `site_rx` pointers for `CodeArena::write_jmp_rel32`.
    ///
    /// # Safety
    /// The code at `entry_offset` inside `buf` must follow the JIT calling
    /// convention (rdi=regs, rsi=mem, returns exit code in rax).
    pub unsafe fn new_patchable(
        buf: ExecutableBuffer,
        entry_offset: usize,
        guest_pc: u64,
        insn_count: u32,
    ) -> Self {
        let entry_ptr = buf.ptr(dynasmrt::AssemblyOffset(entry_offset));
        let entry: JitBlockFn = std::mem::transmute(entry_ptr);
        Self {
            inner: BlockInner::Patchable { buf, entry_offset },
            entry,
            guest_pc,
            insn_count,
            patch_sites: Vec::new(),
            ic_patches: Vec::new(),
        }
    }

    /// Returns the `ExecutableBuffer` if this is a patchable block, or `None`.
    pub fn executable_buffer(&self) -> Option<&ExecutableBuffer> {
        match &self.inner {
            BlockInner::Patchable { buf, .. } => Some(buf),
            BlockInner::Opaque(_) => None,
        }
    }

    /// Take the `ExecutableBuffer` out of a patchable block for patching.
    ///
    /// After patching, restore it with `restore_buffer`. The block is
    /// temporarily non-callable during the patch window.
    pub fn take_buffer(&mut self) -> Option<(ExecutableBuffer, usize)> {
        // We do a swap trick: replace Patchable with a sentinel Opaque variant.
        let sentinel_fn: JitBlockFn = unsafe { std::mem::transmute(usize::MAX as *const u8) };
        match &mut self.inner {
            BlockInner::Patchable { .. } => {
                // Swap out via a dummy replace
                let old = std::mem::replace(&mut self.inner, BlockInner::Opaque(Box::pin(())));
                if let BlockInner::Patchable { buf, entry_offset } = old {
                    // entry pointer stays valid (we'll restore soon)
                    let _ = sentinel_fn; // suppress unused warning
                    Some((buf, entry_offset))
                } else {
                    unreachable!()
                }
            }
            BlockInner::Opaque(_) => None,
        }
    }

    /// Restore the `ExecutableBuffer` after patching (see `take_buffer`).
    pub fn restore_buffer(&mut self, buf: ExecutableBuffer, entry_offset: usize) {
        let entry_ptr = buf.ptr(dynasmrt::AssemblyOffset(entry_offset));
        self.entry = unsafe { std::mem::transmute::<*const u8, JitBlockFn>(entry_ptr) };
        self.inner = BlockInner::Patchable { buf, entry_offset };
    }

    /// Pointer to the first byte of the RX (executable) view.
    /// Used as the `site_rx` base for computing `jmp rel32` offsets.
    pub fn rx_base(&self) -> Option<*const u8> {
        match &self.inner {
            BlockInner::Patchable { buf, .. } => Some(buf.ptr(dynasmrt::AssemblyOffset(0))),
            BlockInner::Opaque(_) => None,
        }
    }
}

// SAFETY: `ExecutableBuffer` is backed by a memory-mapped region.
// The raw `*const u8` inside dynasmrt's buffer is stable as long as the
// buffer is not dropped, and we only access it through `CompiledBlock`.
unsafe impl Send for CompiledBlock {}
unsafe impl Sync for CompiledBlock {}

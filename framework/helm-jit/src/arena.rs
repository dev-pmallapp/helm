//! W^X code arena for JIT-compiled blocks.
//!
//! Wraps dynasmrt's `ExecutableBuffer ↔ MutableBuffer` round-trip to provide
//! a patching surface for block chaining.
//!
//! # Design
//!
//! dynasmrt uses `memmap2`-backed allocations and exposes `make_mut()` /
//! `make_exec()` to toggle between writable and executable views. We model
//! this as a `CodeArena` that:
//! 1. Holds the `ExecutableBuffer` for execution.
//! 2. Tracks per-block patch sites so the block chainer knows where to write.
//! 3. Provides `patch_block()` which calls `make_mut()`, writes, and calls
//!    `make_exec()` — one mprotect pair per link operation (acceptable since
//!    linking happens O(1) times per block, not per execution).
//!
//! # Safety
//!
//! Patch-site writes are always exactly 5 bytes (`jmp rel32` or `ret+nop×4`)
//! at a known byte offset. The offset is computed and stored at compile time
//! by the dynasm emitter, so there is no out-of-bounds risk.

#![allow(unsafe_code, missing_docs)]

use dynasmrt::ExecutableBuffer;

/// x86-64 near return instruction (`ret`).
const X86_RET_OPCODE: u8 = 0xC3;

/// Default arena capacity in bytes (64 MiB).
pub const DEFAULT_ARENA_CAPACITY: usize = 64 * 1024 * 1024;

/// Lightweight wrapper around an `ExecutableBuffer` that supports in-place
/// patching of specific byte offsets without a separate memory-file.
///
/// Patching protocol:
/// 1. Call `patch(buf, byte_offset, &bytes)` with the 5-byte payload.
/// 2. The method temporarily converts the buffer to writable, writes, and
///    converts back — one `mprotect` pair per patch.
///
/// This is the "single-mmap" variant of the W^X arena. It is simpler than a
/// `memfd_create` double-map approach and works with dynasmrt's existing API.
pub struct CodeArena;

impl CodeArena {
    /// Patch `len` bytes at `byte_offset` inside `buf`.
    ///
    /// Temporarily makes the buffer writable, writes `data`, then re-protects
    /// it as executable.
    ///
    /// # Panics
    /// Panics if `byte_offset + data.len()` exceeds the buffer size.
    pub fn patch(buf: ExecutableBuffer, byte_offset: usize, data: &[u8]) -> ExecutableBuffer {
        assert!(
            byte_offset + data.len() <= buf.size(),
            "patch_site out of bounds: offset={byte_offset} len={} buf_size={}",
            data.len(),
            buf.size()
        );
        let mut mutable = buf
            .make_mut()
            .expect("CodeArena: make_mut failed (mprotect error)");
        mutable[byte_offset..byte_offset + data.len()].copy_from_slice(data);
        mutable
            .make_exec()
            .expect("CodeArena: make_exec failed (mprotect error)")
    }

    /// Write a direct `jmp rel32` at `byte_offset` in `buf`, jumping to `target_rx`.
    ///
    /// `site_rx` is the address of the first byte of the 5-byte slot inside the
    /// RX (executable) view. Used to compute the relative offset.
    ///
    /// # Safety
    /// - `site_rx` must be the actual runtime address of the patch site.
    /// - The 5-byte slot must be within `buf` at `byte_offset`.
    pub fn write_jmp_rel32(
        buf: ExecutableBuffer,
        byte_offset: usize,
        site_rx: *const u8,
        target_rx: *const u8,
    ) -> ExecutableBuffer {
        // rel32 = target - (site + 5)
        let from = site_rx as i64 + 5;
        let to = target_rx as i64;
        let rel = (to - from) as i32;
        let mut patch = [0u8; 5];
        patch[0] = 0xE9; // JMP rel32
        patch[1..5].copy_from_slice(&rel.to_le_bytes());
        Self::patch(buf, byte_offset, &patch)
    }

    /// Restore the unlinked `ret + nop×4` sequence at `byte_offset`.
    pub fn write_ret_nop4(buf: ExecutableBuffer, byte_offset: usize) -> ExecutableBuffer {
        Self::patch(buf, byte_offset, &[X86_RET_OPCODE, 0x90, 0x90, 0x90, 0x90])
    }
}

#[cfg(test)]
#[cfg(feature = "backend-dynasm")]
mod tests {
    use super::*;
    use dynasm::dynasm;

    /// Build a minimal block: `nop×5; xor rax,rax; ret`.
    /// Returns the buffer and the offset of the 5-byte nop region (byte 0).
    fn make_patchable_block() -> (ExecutableBuffer, usize) {
        let mut ops = dynasmrt::x64::Assembler::new().unwrap();
        // 5-byte patch site at byte 0 (will hold ret+nop4 or jmp rel32)
        dynasm!(ops
            ; nop  // 5× NOP as placeholder for the patch site
            ; nop
            ; nop
            ; nop
            ; nop
            ; xor rax, rax
            ; ret
        );
        let buf = ops.finalize().unwrap();
        (buf, 0)
    }

    #[test]
    fn write_and_read_via_patch() {
        let (buf, offset) = make_patchable_block();
        // Write a recognisable 5-byte sequence through the patch API.
        let sentinel = [0x90u8, 0x01, 0x02, 0x03, 0x04];
        let patched = CodeArena::patch(buf, offset, &sentinel);
        // Read back through the executable view.
        let bytes: &[u8] = &patched;
        assert_eq!(&bytes[offset..offset + 5], &sentinel);
    }

    #[test]
    fn restore_ret_nop4() {
        let (buf, offset) = make_patchable_block();
        // First smash with a junk payload, then restore.
        let junk = [0u8; 5];
        let buf = CodeArena::patch(buf, offset, &junk);
        let buf = CodeArena::write_ret_nop4(buf, offset);
        let bytes: &[u8] = &buf;
        assert_eq!(bytes[offset], X86_RET_OPCODE); // ret
        assert!(bytes[offset + 1..offset + 5].iter().all(|&b| b == 0x90)); // nop×4
    }

    #[test]
    #[should_panic(expected = "patch_site out of bounds")]
    fn patch_out_of_bounds_panics() {
        let (buf, _) = make_patchable_block();
        let size = buf.size();
        CodeArena::patch(buf, size, &[0u8; 5]); // one byte past end → panic
    }
}

//! `extern "C"` helper functions callable from JIT-compiled code.
//!
//! These provide guest memory access. The `mem` pointer is a `*mut FlatMem`
//! transmuted to `*mut u8` — JIT code passes it from `rsi` (set up by the
//! block entry prologue).
//!
//! # Return convention
//! - `jit_mem_read`: returns the loaded value on success. On fault, returns 0
//!   and sets a fault flag (the block should check and exit).
//! - `jit_mem_write`: returns 0 on success, 1 on fault.

#![allow(missing_docs)]
#![allow(unsafe_code)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use helm_core::{AccessType, MemInterface};
use helm_memory::FlatMem;

/// Read a value from guest memory.
///
/// # Safety
/// `mem` must be a valid pointer to a `FlatMem` instance. Called from JIT code.
///
/// # Arguments
/// - `mem`: opaque pointer to `FlatMem`
/// - `addr`: guest physical address
/// - `size`: access width in bytes (1, 2, 4, or 8)
/// - `out`: pointer to store the result
///
/// # Returns
/// 0 on success (value written to `*out`), 1 on fault.
#[no_mangle]
pub extern "C" fn jit_mem_read(mem: *mut u8, addr: u64, size: u32, out: *mut u64) -> u64 {
    let flat = unsafe { &mut *(mem as *mut FlatMem) };
    match flat.read(addr, size as usize, AccessType::Load) {
        Ok(val) => {
            unsafe { *out = val };
            0
        }
        Err(_) => 1,
    }
}

/// Write a value to guest memory.
///
/// # Safety
/// `mem` must be a valid pointer to a `FlatMem` instance. Called from JIT code.
///
/// # Arguments
/// - `mem`: opaque pointer to `FlatMem`
/// - `addr`: guest physical address
/// - `val`: value to write
/// - `size`: access width in bytes (1, 2, 4, or 8)
///
/// # Returns
/// 0 on success, 1 on fault.
#[no_mangle]
pub extern "C" fn jit_mem_write(mem: *mut u8, addr: u64, val: u64, size: u32) -> u64 {
    let flat = unsafe { &mut *(mem as *mut FlatMem) };
    match flat.write(addr, size as usize, val, AccessType::Store) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

//! `extern "C"` helper functions callable from JIT-compiled code.
//!
//! Two sets of helpers:
//! - **SE mode**: `jit_mem_read` / `jit_mem_write` — direct `FlatMem` access (physical addresses)
//! - **FS mode**: `jit_fs_mem_read` / `jit_fs_mem_write` — VA→PA translation via MMU + `HelmAddressSpace`
//!
//! The `mem` pointer (`rsi` in the JIT calling convention) is either:
//! - `*mut FlatMem` for SE mode
//! - `*mut JitFsContext` for FS mode
//!
//! The engine populates register-array slots 46/47 with the appropriate
//! helper function pointers.

#![allow(missing_docs)]
#![allow(unsafe_code)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use helm_core::{AccessType, MemInterface};
use helm_memory::FlatMem;

// ── SE-mode helpers (physical address, FlatMem) ─────────────────────────────

/// Read a value from guest memory (SE mode — physical address).
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

/// Write a value to guest memory (SE mode — physical address).
#[no_mangle]
pub extern "C" fn jit_mem_write(mem: *mut u8, addr: u64, val: u64, size: u32) -> u64 {
    let flat = unsafe { &mut *(mem as *mut FlatMem) };
    match flat.write(addr, size as usize, val, AccessType::Store) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

// ── FS-mode helpers (virtual address, MMU translation) ──────────────────────

use helm_arch::aarch64::mmu::{self, MmuConfig, MmuAccess, Tlb};
use helm_memory::HelmAddressSpace;

/// Opaque context for FS-mode JIT memory access.
///
/// Passed as the `mem` parameter (`rsi`) to JIT blocks in FS mode.
/// Contains everything needed for VA→PA translation + memory access.
#[repr(C)]
pub struct JitFsContext {
    /// Pointer to the system address space (RAM + MMIO devices).
    pub sys_mem: *mut HelmAddressSpace,
    /// Pointer to the software TLB (shared, mutable).
    pub tlb: *mut Tlb,
    /// Snapshotted MMU configuration (sctlr, tcr, ttbr, el, hcr).
    pub mmu_cfg: MmuConfig,
}

/// Read a value from guest memory (FS mode — virtual address with MMU translation).
#[no_mangle]
pub extern "C" fn jit_fs_mem_read(ctx: *mut u8, addr: u64, size: u32, out: *mut u64) -> u64 {
    let ctx = unsafe { &mut *(ctx as *mut JitFsContext) };
    let sys_mem = unsafe { &mut *ctx.sys_mem };
    let tlb = unsafe { &mut *ctx.tlb };

    // Translate VA → PA
    let pa = if !ctx.mmu_cfg.mmu_enabled() {
        addr // Identity mapping when MMU is off
    } else {
        match mmu::translate_cfg(&ctx.mmu_cfg, addr, MmuAccess::Read, sys_mem, Some(tlb)) {
            Ok(pa) => pa,
            Err(_) => return 1, // Translation fault
        }
    };

    // Read from physical address
    match sys_mem.read(pa, size as usize, AccessType::Load) {
        Ok(val) => {
            unsafe { *out = val };
            0
        }
        Err(_) => 1,
    }
}

// ── RISC-V64 FS-mode helpers (virtual address, Sv39/Sv48 translation) ────────

use helm_arch::riscv::mmu::{self as rv_mmu, RiscvMmuConfig, RiscvTlb};

/// Opaque context for RISC-V64 FS-mode JIT memory access.
#[repr(C)]
pub struct JitFsContextRv64 {
    /// Pointer to the system address space.
    pub sys_mem: *mut HelmAddressSpace,
    /// Pointer to the RISC-V software TLB.
    pub tlb: *mut RiscvTlb,
    /// Snapshotted MMU configuration (satp-derived).
    pub mmu_cfg: RiscvMmuConfig,
}

/// Read a value from guest memory (RISC-V64 FS mode — Sv39/Sv48 translation).
#[no_mangle]
pub extern "C" fn jit_rv64_fs_mem_read(ctx: *mut u8, addr: u64, size: u32, out: *mut u64) -> u64 {
    let ctx = unsafe { &mut *(ctx as *mut JitFsContextRv64) };
    let sys_mem = unsafe { &mut *ctx.sys_mem };
    let tlb = unsafe { &mut *ctx.tlb };

    let pa = if !ctx.mmu_cfg.mmu_enabled() {
        addr
    } else {
        match rv_mmu::translate_cached(&ctx.mmu_cfg, addr, AccessType::Load, sys_mem, tlb) {
            Ok(pa) => pa,
            Err(_) => return 1,
        }
    };

    match sys_mem.read(pa, size as usize, AccessType::Load) {
        Ok(val) => {
            unsafe { *out = val };
            0
        }
        Err(_) => 1,
    }
}

/// Write a value to guest memory (RISC-V64 FS mode — Sv39/Sv48 translation).
#[no_mangle]
pub extern "C" fn jit_rv64_fs_mem_write(ctx: *mut u8, addr: u64, val: u64, size: u32) -> u64 {
    let ctx = unsafe { &mut *(ctx as *mut JitFsContextRv64) };
    let sys_mem = unsafe { &mut *ctx.sys_mem };
    let tlb = unsafe { &mut *ctx.tlb };

    let pa = if !ctx.mmu_cfg.mmu_enabled() {
        addr
    } else {
        match rv_mmu::translate_cached(&ctx.mmu_cfg, addr, AccessType::Store, sys_mem, tlb) {
            Ok(pa) => pa,
            Err(_) => return 1,
        }
    };

    match sys_mem.write(pa, size as usize, val, AccessType::Store) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// Write a value to guest memory (FS mode — virtual address with MMU translation).
#[no_mangle]
pub extern "C" fn jit_fs_mem_write(ctx: *mut u8, addr: u64, val: u64, size: u32) -> u64 {
    let ctx = unsafe { &mut *(ctx as *mut JitFsContext) };
    let sys_mem = unsafe { &mut *ctx.sys_mem };
    let tlb = unsafe { &mut *ctx.tlb };

    // Translate VA → PA
    let pa = if !ctx.mmu_cfg.mmu_enabled() {
        addr
    } else {
        match mmu::translate_cfg(&ctx.mmu_cfg, addr, MmuAccess::Write, sys_mem, Some(tlb)) {
            Ok(pa) => pa,
            Err(_) => return 1, // Translation fault
        }
    };

    // Write to physical address
    match sys_mem.write(pa, size as usize, val, AccessType::Store) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

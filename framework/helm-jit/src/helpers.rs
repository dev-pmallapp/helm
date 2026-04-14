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

// ── SE-mode inline TLB ──────────────────────────────────────────────────────

/// One SE-mode JIT TLB entry (16 bytes, cache-line friendly).
///
/// The TLB is direct-mapped with 256 entries indexed by `(guest_addr >> 12) & 0xFF`.
#[repr(C, align(16))]
pub struct JitSeTlbEntry {
    /// Guest virtual page number (`guest_addr >> 12`).
    /// `u64::MAX` = invalid (miss).
    pub va_tag: u64,
    /// Host pointer to the start of the corresponding 4KB page.
    pub host_ptr: u64, // *mut u8 as u64
}

/// 256-entry direct-mapped TLB for SE-mode JIT memory accesses.
///
/// Eliminates the `jit_mem_read`/`jit_mem_write` C helper call overhead on
/// the TLB hot path (~99% of SE-mode accesses hit in 256 entries).
///
/// Lives in `HelmEngine` alongside `FlatMem`. The base pointer is stored in
/// flat array slot 44 so JIT-compiled blocks can access it directly.
#[repr(C)]
pub struct JitSeTlb {
    pub entries: Box<[JitSeTlbEntry; 256]>,
}

impl JitSeTlb {
    pub fn new() -> Self {
        Self {
            entries: Box::new(std::array::from_fn(|_| JitSeTlbEntry {
                va_tag: u64::MAX,
                host_ptr: 0,
            })),
        }
    }

    /// Flush all TLB entries (e.g. after `brk`/`mmap` changes the memory layout).
    pub fn flush(&mut self) {
        for e in self.entries.iter_mut() {
            e.va_tag = u64::MAX;
        }
    }
}

impl Default for JitSeTlb {
    fn default() -> Self {
        Self::new()
    }
}

/// Fill a SE-mode TLB entry from `FlatMem` and perform the memory read.
///
/// Called from JIT code on a TLB miss. Fills the TLB entry for the page,
/// then reads `size` bytes from `addr`.
///
/// # Safety
/// - `mem` must point to a valid `FlatMem`.
/// - `tlb` must point to the first `JitSeTlbEntry` in a valid `JitSeTlb`.
/// - `out` must point to valid writable storage.
#[no_mangle]
pub extern "C" fn jit_se_tlb_fill_and_read(
    mem: *mut u8,
    tlb: *mut u8,
    addr: u64,
    size: u32,
    out: *mut u64,
) -> u64 {
    let flat = unsafe { &mut *(mem as *mut FlatMem) };
    let tlb_entries = unsafe { std::slice::from_raw_parts_mut(tlb as *mut JitSeTlbEntry, 256) };

    // Try to fill the TLB entry from FlatMem's page table.
    if let Some(host) = flat.host_ptr_for_page(addr) {
        let idx = ((addr >> 12) & 0xFF) as usize;
        tlb_entries[idx].va_tag = addr >> 12;
        tlb_entries[idx].host_ptr = host as u64;
    }

    // Read via the normal slow path (guaranteed correct even on fill failure).
    match flat.read(addr, size as usize, helm_core::AccessType::Load) {
        Ok(val) => {
            unsafe { *out = val };
            0
        }
        Err(_) => 1,
    }
}

/// Fill a SE-mode TLB entry from `FlatMem` and perform the memory write.
#[no_mangle]
pub extern "C" fn jit_se_tlb_fill_and_write(
    mem: *mut u8,
    tlb: *mut u8,
    addr: u64,
    val: u64,
    size: u32,
) -> u64 {
    let flat = unsafe { &mut *(mem as *mut FlatMem) };
    let tlb_entries = unsafe { std::slice::from_raw_parts_mut(tlb as *mut JitSeTlbEntry, 256) };

    // Fill TLB entry.
    if let Some(host) = flat.host_ptr_for_page(addr) {
        let idx = ((addr >> 12) & 0xFF) as usize;
        tlb_entries[idx].va_tag = addr >> 12;
        tlb_entries[idx].host_ptr = host as u64;
    }

    match flat.write(addr, size as usize, val, helm_core::AccessType::Store) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

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

use helm_arch::aarch64::mmu::{self, MmuAccess, MmuConfig, Tlb};
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

// -- System register helpers (MRS/MSR from JIT-compiled code) ----------------

use helm_arch::aarch64::arch_state::Aarch64ArchState;
use helm_arch::aarch64::execute::helpers as arch_helpers;

/// Read a system register from `Aarch64ArchState`.
///
/// Called from JIT-compiled MRS instructions. `arch_state` is the raw pointer
/// stored in flat_regs[REG_JIT_ARCH_STATE].
#[no_mangle]
pub extern "C" fn jit_sysreg_read(arch_state: *mut u8, encoding: u32) -> u64 {
    let a = unsafe { &*(arch_state as *const Aarch64ArchState) };
    let redirected = arch_helpers::redirect_sysreg(a, encoding);
    arch_helpers::read_sysreg(a, redirected)
}

/// Write a system register in `Aarch64ArchState`.
///
/// Called from JIT-compiled MSR instructions. Returns 0 on success.
#[no_mangle]
pub extern "C" fn jit_sysreg_write(arch_state: *mut u8, encoding: u32, value: u64) -> u64 {
    let a = unsafe { &mut *(arch_state as *mut Aarch64ArchState) };
    let redirected = arch_helpers::redirect_sysreg(a, encoding);
    arch_helpers::write_sysreg(a, redirected, value);
    0
}

/// Write SPSel field in `Aarch64ArchState`.
///
/// Called from JIT-compiled MsrImm (SPSel) instructions.
#[no_mangle]
pub extern "C" fn jit_spsel_write(arch_state: *mut u8, value: u32) {
    let a = unsafe { &mut *(arch_state as *mut Aarch64ArchState) };
    a.spsel = value != 0;
}

/// Execute a SYS instruction (TLBI, AT, IC, DC, barriers, hints).
///
/// Handles TLBI by setting `tlb_flush_pending` with the correct flush
/// parameters. AT sets `par_el1`. Other SYS ops are no-ops.
#[no_mangle]
pub extern "C" fn jit_sys_exec(arch_state: *mut u8, raw: u32, xt_value: u64) {
    let a = unsafe { &mut *(arch_state as *mut Aarch64ArchState) };
    let op0 = (raw >> 19) & 0x3;
    let op1 = (raw >> 16) & 0x7;
    let crn = (raw >> 12) & 0xF;
    let crm = (raw >> 8) & 0xF;
    let op2 = (raw >> 5) & 0x7;

    // TLBI: op0=01, CRn=1000
    if op0 == 0b01 && crn == 0b1000 {
        a.tlb_flush_pending = true;
        a.tlb_flush_broadcast = true;
        a.tlb_flush_asid = None;
        // VA-targeted forms encode page number in Xt[55:12].
        a.tlb_flush_va = match (op1, crm, op2) {
            (0, 3, 1) | (0, 7, 1) | (0, 3, 5) | (0, 7, 5)
            | (0, 3, 3) | (0, 7, 3) | (0, 3, 7) | (0, 7, 7)
            | (4, 3, 1) | (4, 7, 1) | (4, 3, 5) | (4, 7, 5)
            | (6, 3, 1) | (6, 7, 1) | (6, 3, 5) | (6, 7, 5) => {
                let raw_va = xt_value << 12;
                Some(if raw_va & (1u64 << 55) != 0 {
                    raw_va | 0xFF00_0000_0000_0000
                } else {
                    raw_va
                })
            }
            (0, 3, 2) | (0, 7, 2) => {
                let asid_mask = if (a.tcr_el1 >> 36) & 1 != 0 { 0xFFFF } else { 0x00FF };
                a.tlb_flush_asid = Some(((xt_value >> 48) as u16) & asid_mask);
                None
            }
            _ => None,
        };
        return;
    }

    // AT S1E1R/W, S1E0R/W: identity in SE mode.
    if crn == 0b0111 && crm == 0b1000 && op0 <= 0b01 {
        let rt = raw & 0x1F;
        let va = a.read_x(rt);
        a.par_el1 = va & 0x0000_FFFF_FFFF_F000;
        return;
    }

    // IC, DC, and other maintenance ops: no-op.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn se_tlb_fill_and_read_uses_entries_base_pointer() {
        let mut mem = FlatMem::new(0, 0x2000);
        mem.load_bytes(0x1000, &[0x78, 0x56, 0x34, 0x12]);

        let mut tlb = JitSeTlb::new();
        let mut out = 0u64;

        assert_eq!(
            jit_se_tlb_fill_and_read(
                (&mut mem as *mut FlatMem).cast::<u8>(),
                tlb.entries.as_mut_ptr().cast::<u8>(),
                0x1000,
                4,
                &mut out,
            ),
            0
        );
        assert_eq!(out, 0x1234_5678);

        let idx = ((0x1000 >> 12) & 0xFF) as usize;
        assert_eq!(tlb.entries[idx].va_tag, 0x1000 >> 12);
        assert_ne!(tlb.entries[idx].host_ptr, 0);
    }

    #[test]
    fn se_tlb_fill_and_write_uses_entries_base_pointer() {
        let mut mem = FlatMem::new(0, 0x2000);
        let mut tlb = JitSeTlb::new();

        assert_eq!(
            jit_se_tlb_fill_and_write(
                (&mut mem as *mut FlatMem).cast::<u8>(),
                tlb.entries.as_mut_ptr().cast::<u8>(),
                0x1000,
                0xABCD_EF01,
                4,
            ),
            0
        );
        assert_eq!(
            mem.read(0x1000, 4, AccessType::Load).expect("memory read"),
            0xABCD_EF01
        );

        let idx = ((0x1000 >> 12) & 0xFF) as usize;
        assert_eq!(tlb.entries[idx].va_tag, 0x1000 >> 12);
        assert_ne!(tlb.entries[idx].host_ptr, 0);
    }
}

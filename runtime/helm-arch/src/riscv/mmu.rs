//! RISC-V Sv39/Sv48 page table walker for FS-mode JIT.
//!
//! Implements the two-level (Sv39, 3 levels) and three-level (Sv48, 4 levels)
//! virtual address translation defined in the RISC-V Privileged Specification.

#![allow(missing_docs)]

use helm_core::{AccessType, MemFault, MemInterface};

/// Translation mode extracted from SATP CSR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SatpMode {
    /// No translation (bare physical addresses).
    Bare,
    /// Sv39: 39-bit virtual address, 3-level page table.
    Sv39,
    /// Sv48: 48-bit virtual address, 4-level page table.
    Sv48,
}

/// Snapshotted MMU configuration for JIT use.
///
/// Taken from CSRs at the start of each quantum to avoid borrow conflicts.
#[derive(Debug, Clone, Copy)]
pub struct RiscvMmuConfig {
    /// Translation mode (Bare/Sv39/Sv48).
    pub mode: SatpMode,
    /// Physical page number of the root page table (from SATP.PPN).
    pub root_ppn: u64,
    /// SATP.ASID for TLB tagging.
    pub asid: u16,
}

impl RiscvMmuConfig {
    /// Extract MMU configuration from the SATP CSR value.
    pub fn from_satp(satp: u64) -> Self {
        let mode_bits = (satp >> 60) & 0xF;
        let mode = match mode_bits {
            0 => SatpMode::Bare,
            8 => SatpMode::Sv39,
            9 => SatpMode::Sv48,
            _ => SatpMode::Bare, // Unknown modes treated as bare
        };
        let asid = ((satp >> 44) & 0xFFFF) as u16;
        let root_ppn = satp & 0xFFF_FFFF_FFFF; // 44-bit PPN
        Self { mode, root_ppn, asid }
    }

    /// Whether address translation is enabled.
    pub fn mmu_enabled(&self) -> bool {
        self.mode != SatpMode::Bare
    }
}

/// Page table entry bits (common to Sv39 and Sv48).
const PTE_V: u64 = 1 << 0; // Valid
const PTE_R: u64 = 1 << 1; // Readable
const PTE_W: u64 = 1 << 2; // Writable
const PTE_X: u64 = 1 << 3; // Executable
#[allow(dead_code)]
const PTE_U: u64 = 1 << 4; // User-accessible
const PTE_A: u64 = 1 << 6; // Accessed
const PTE_D: u64 = 1 << 7; // Dirty

const PAGE_SHIFT: u64 = 12;
const PAGE_SIZE: u64 = 1 << PAGE_SHIFT;
const PTE_SIZE: u64 = 8; // 64-bit PTEs

/// Translate a virtual address to physical using the Sv39/Sv48 page table walker.
///
/// `mem` provides physical memory access for reading page table entries.
pub fn translate(
    cfg: &RiscvMmuConfig,
    va: u64,
    access: AccessType,
    mem: &mut dyn MemInterface,
) -> Result<u64, MemFault> {
    if !cfg.mmu_enabled() {
        return Ok(va); // Bare mode — identity mapping
    }

    let levels = match cfg.mode {
        SatpMode::Sv39 => 3,
        SatpMode::Sv48 => 4,
        SatpMode::Bare => return Ok(va),
    };

    let vpn_bits = 9; // 9 bits per VPN level
    let mut ppn = cfg.root_ppn;

    for level in (0..levels).rev() {
        let vpn_shift = PAGE_SHIFT + (level as u64) * vpn_bits;
        let vpn = (va >> vpn_shift) & 0x1FF;

        let pte_addr = (ppn << PAGE_SHIFT) + vpn * PTE_SIZE;
        let pte = mem.read(pte_addr, 8, AccessType::Load)
            .map_err(|_| MemFault::AccessFault { addr: va })?;

        // Check valid bit
        if pte & PTE_V == 0 {
            return Err(MemFault::PageFault { addr: va, iss: 0 });
        }

        let r = pte & PTE_R != 0;
        let w = pte & PTE_W != 0;
        let x = pte & PTE_X != 0;

        if !r && !w && !x {
            // Non-leaf PTE — descend to next level
            ppn = (pte >> 10) & 0xFFF_FFFF_FFFF; // 44-bit PPN
            continue;
        }

        // Leaf PTE — check permissions
        match access {
            AccessType::Fetch if !x => return Err(MemFault::PageFault { addr: va, iss: 0 }),
            AccessType::Load if !r => return Err(MemFault::PageFault { addr: va, iss: 0 }),
            AccessType::Store | AccessType::Atomic if !w => {
                return Err(MemFault::PageFault { addr: va, iss: 0 })
            }
            _ => {}
        }

        // Check accessed/dirty bits
        if pte & PTE_A == 0 {
            return Err(MemFault::PageFault { addr: va, iss: 0 });
        }
        if matches!(access, AccessType::Store | AccessType::Atomic) && pte & PTE_D == 0 {
            return Err(MemFault::PageFault { addr: va, iss: 0 });
        }

        // Compute physical address
        let pte_ppn = (pte >> 10) & 0xFFF_FFFF_FFFF;

        if level > 0 {
            // Superpage — check alignment (lower PPN bits must be zero)
            let superpage_mask = (1u64 << (level as u64 * vpn_bits)) - 1;
            if pte_ppn & superpage_mask != 0 {
                return Err(MemFault::PageFault { addr: va, iss: 0 }); // Misaligned superpage
            }
            // PA = pte_ppn[upper bits] | va[lower bits]
            let offset_mask = (1u64 << (PAGE_SHIFT + level as u64 * vpn_bits)) - 1;
            let pa = (pte_ppn << PAGE_SHIFT) | (va & offset_mask);
            return Ok(pa);
        }

        // Regular 4KB page
        let offset = va & (PAGE_SIZE - 1);
        let pa = (pte_ppn << PAGE_SHIFT) | offset;
        return Ok(pa);
    }

    // Exhausted all levels without finding a leaf — page fault
    Err(MemFault::PageFault { addr: va, iss: 0 })
}

/// Software TLB entry for RISC-V.
#[derive(Clone, Copy, Default)]
pub struct RiscvTlbEntry {
    pub vpn: u64,
    pub ppn: u64,
    pub valid: bool,
    pub asid: u16,
    pub permissions: u8, // RWX bits
}

/// Direct-mapped software TLB for RISC-V.
pub struct RiscvTlb {
    entries: Vec<RiscvTlbEntry>,
    mask: usize,
}

impl RiscvTlb {
    pub fn new(size: usize) -> Self {
        let size = size.next_power_of_two();
        Self {
            entries: vec![RiscvTlbEntry::default(); size],
            mask: size - 1,
        }
    }

    fn index(&self, vpn: u64) -> usize {
        (vpn as usize) & self.mask
    }

    pub fn lookup(&self, va: u64, asid: u16) -> Option<u64> {
        let vpn = va >> PAGE_SHIFT;
        let entry = &self.entries[self.index(vpn)];
        if entry.valid && entry.vpn == vpn && entry.asid == asid {
            Some((entry.ppn << PAGE_SHIFT) | (va & (PAGE_SIZE - 1)))
        } else {
            None
        }
    }

    pub fn insert(&mut self, va: u64, pa: u64, asid: u16, permissions: u8) {
        let vpn = va >> PAGE_SHIFT;
        let ppn = pa >> PAGE_SHIFT;
        let idx = self.index(vpn);
        self.entries[idx] = RiscvTlbEntry {
            vpn,
            ppn,
            valid: true,
            asid,
            permissions,
        };
    }

    pub fn flush(&mut self) {
        for e in &mut self.entries {
            e.valid = false;
        }
    }

    pub fn flush_asid(&mut self, asid: u16) {
        for e in &mut self.entries {
            if e.asid == asid {
                e.valid = false;
            }
        }
    }
}

/// Translate with TLB acceleration.
pub fn translate_cached(
    cfg: &RiscvMmuConfig,
    va: u64,
    access: AccessType,
    mem: &mut dyn MemInterface,
    tlb: &mut RiscvTlb,
) -> Result<u64, MemFault> {
    if !cfg.mmu_enabled() {
        return Ok(va);
    }

    // TLB hit?
    if let Some(pa) = tlb.lookup(va, cfg.asid) {
        return Ok(pa);
    }

    // TLB miss — walk the page table
    let pa = translate(cfg, va, access, mem)?;

    // Insert into TLB
    let perms = 0; // Simplified — full impl would extract from PTE
    tlb.insert(va, pa, cfg.asid, perms);

    Ok(pa)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn satp_bare_mode() {
        let cfg = RiscvMmuConfig::from_satp(0);
        assert_eq!(cfg.mode, SatpMode::Bare);
        assert!(!cfg.mmu_enabled());
    }

    #[test]
    fn satp_sv39_mode() {
        let satp = (8u64 << 60) | (42u64 << 44) | 0x12345;
        let cfg = RiscvMmuConfig::from_satp(satp);
        assert_eq!(cfg.mode, SatpMode::Sv39);
        assert_eq!(cfg.asid, 42);
        assert_eq!(cfg.root_ppn, 0x12345);
        assert!(cfg.mmu_enabled());
    }

    #[test]
    fn bare_mode_identity() {
        struct DummyMem;
        impl MemInterface for DummyMem {
            fn read(&mut self, _: u64, _: usize, _: AccessType) -> Result<u64, MemFault> { Ok(0) }
            fn write(&mut self, _: u64, _: usize, _: u64, _: AccessType) -> Result<(), MemFault> { Ok(()) }
        }
        let cfg = RiscvMmuConfig::from_satp(0);
        assert_eq!(translate(&cfg, 0xDEAD, AccessType::Load, &mut DummyMem).unwrap(), 0xDEAD);
    }

    #[test]
    fn tlb_hit_and_miss() {
        let mut tlb = RiscvTlb::new(256);
        assert!(tlb.lookup(0x1000, 0).is_none());
        tlb.insert(0x1000, 0x8000_0000, 0, 0);
        assert_eq!(tlb.lookup(0x1000, 0), Some(0x8000_0000));
        assert!(tlb.lookup(0x1000, 1).is_none()); // Different ASID
    }

    #[test]
    fn tlb_flush() {
        let mut tlb = RiscvTlb::new(256);
        tlb.insert(0x1000, 0x8000_0000, 0, 0);
        tlb.flush();
        assert!(tlb.lookup(0x1000, 0).is_none());
    }
}

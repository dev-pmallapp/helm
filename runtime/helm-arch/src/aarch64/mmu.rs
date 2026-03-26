//! AArch64 MMU page table walker (4KB granule, EL1 only).
//!
//! Implements a 4-level page table walk for VA->PA translation.
//! Table walks use physical addresses (bypass the MMU) via `mem.read()`.
//!
//! # TLB
//! `Tlb` is a direct-mapped 256-entry software TLB indexed by VA bits [19:12].
//! Each entry stores the VA page base, PA page base, and a TTBR tag to detect
//! stale entries after context switches (TTBR write → `tlb.flush()`).

use helm_core::{AccessType, MemInterface};

use super::arch_state::Aarch64ArchState;

// ---------------------------------------------------------------------------
// TLB
// ---------------------------------------------------------------------------

/// Direct-mapped software TLB — 256 entries, indexed by VA bits [19:12].
///
/// Tag = TTBR (upper or lower) used at translation time.  A tag mismatch
/// (e.g. after a context switch that writes TTBR0/1) acts as an implicit
/// flush for that entry.  Call `flush()` explicitly on SCTLR/TCR writes.
pub struct Tlb {
    /// (va_page, pa_page) pairs.  `va_page` = VA with page-offset zeroed.
    entries: Box<[(u64, u64); 256]>,
    /// TTBR tag per entry — invalidates stale entries on context switch.
    tags: Box<[u64; 256]>,
}

impl Tlb {
    pub fn new() -> Self {
        Self {
            entries: Box::new([(0, 0); 256]),
            tags: Box::new([u64::MAX; 256]),
        }
    }

    #[inline]
    fn idx(va: u64) -> usize {
        ((va >> 12) & 0xFF) as usize
    }

    /// Look up a VA. Returns the PA if cached and tag matches.
    #[inline]
    pub fn lookup(&self, va: u64, ttbr: u64) -> Option<u64> {
        let i = Self::idx(va);
        let va_page = va & !0xFFF;
        if self.entries[i].0 == va_page && self.tags[i] == ttbr {
            Some(self.entries[i].1 | (va & 0xFFF))
        } else {
            None
        }
    }

    /// Insert a (VA page, PA page) pair with the given TTBR tag.
    #[inline]
    pub fn insert(&mut self, va: u64, pa: u64, ttbr: u64) {
        let i = Self::idx(va);
        self.entries[i] = (va & !0xFFF, pa & !0xFFF);
        self.tags[i] = ttbr;
    }

    /// Invalidate all TLB entries.
    #[inline]
    pub fn flush(&mut self) {
        for tag in self.tags.iter_mut() {
            *tag = u64::MAX;
        }
    }
}

impl Default for Tlb {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// MmuConfig (used by translate_cfg to avoid dummy ArchState allocation)
// ---------------------------------------------------------------------------

/// Minimal snapshot of MMU configuration registers needed for VA→PA translation.
/// Used by `translate_cfg()` to avoid passing a full `Aarch64ArchState`.
#[derive(Clone, Copy)]
pub struct MmuConfig {
    pub sctlr_el1: u64,
    pub tcr_el1: u64,
    pub ttbr0_el1: u64,
    pub ttbr1_el1: u64,
}

impl MmuConfig {
    pub fn from_arch(a: &Aarch64ArchState) -> Self {
        Self {
            sctlr_el1: a.sctlr_el1,
            tcr_el1: a.tcr_el1,
            ttbr0_el1: a.ttbr0_el1,
            ttbr1_el1: a.ttbr1_el1,
        }
    }

    #[inline]
    pub fn mmu_enabled(&self) -> bool {
        self.sctlr_el1 & 1 != 0
    }
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Access type for MMU permission checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmuAccess {
    Read,
    Write,
    Execute,
}

/// Result of a successful address translation.
#[derive(Debug, Clone, Copy)]
pub struct TranslateResult {
    /// Physical address.
    pub pa: u64,
    /// Memory attributes index (from descriptor AttrIndx[2:0]).
    pub attr_idx: u8,
    /// Access permission bits AP[2:1].
    pub ap: u8,
}

/// MMU translation fault.
#[derive(Debug, Clone, Copy)]
pub struct MmuFault {
    /// Faulting virtual address.
    pub va: u64,
    /// Page table walk level where the fault occurred (0-3).
    pub level: u8,
    /// Fault type.
    pub kind: FaultKind,
}

/// Classification of MMU faults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    /// Translation fault -- invalid descriptor at this level.
    Translation,
    /// Access flag fault -- AF bit not set.
    AccessFlag,
    /// Permission fault -- AP bits deny the access.
    Permission,
    /// Address size fault -- address outside supported range.
    AddressSize,
}

// ---------------------------------------------------------------------------
// Syndrome encoding
// ---------------------------------------------------------------------------

impl MmuFault {
    /// Encode as ISS for ESR_EL1 Data Abort syndrome.
    ///
    /// ISS[5:0] = DFSC (Data Fault Status Code) encoding:
    /// - Translation fault = `0b0001xx` where `xx` = level
    /// - Access flag fault = `0b0010xx`
    /// - Permission fault  = `0b0011xx`
    /// - Address size fault = `0b0000xx`
    ///
    /// ISS[6] = WnR (1 for write, 0 for read).
    pub fn iss_data(&self, is_write: bool) -> u32 {
        let dfsc = self.fault_status_code();
        let wnr = if is_write { 1 << 6 } else { 0 };
        dfsc | wnr
    }

    /// Encode as ISS for ESR_EL1 Instruction Abort syndrome.
    ///
    /// ISS[5:0] = IFSC (same encoding as DFSC).
    pub fn iss_insn(&self) -> u32 {
        self.fault_status_code()
    }

    /// Common DFSC/IFSC encoding: fault-class bits [5:2] | level [1:0].
    fn fault_status_code(&self) -> u32 {
        let class = match self.kind {
            FaultKind::AddressSize => 0b000000,
            FaultKind::Translation => 0b000100,
            FaultKind::AccessFlag => 0b001000,
            FaultKind::Permission => 0b001100,
        };
        class | (self.level as u32 & 3)
    }
}

// ---------------------------------------------------------------------------
// Top-level translate entry point
// ---------------------------------------------------------------------------

/// Translate a virtual address to a physical address.
///
/// If SCTLR_EL1.M = 0 (MMU disabled), returns identity mapping (PA = VA).
/// Otherwise performs a 4-level page table walk using the appropriate TTBR.
/// On a successful walk the result is inserted into `tlb` (if provided).
///
/// `mem` is used for physical memory reads (table walk accesses).
pub fn translate(
    a: &Aarch64ArchState,
    va: u64,
    access: MmuAccess,
    mem: &mut impl MemInterface,
    tlb: Option<&mut Tlb>,
) -> Result<TranslateResult, MmuFault> {
    translate_inner(
        a.sctlr_el1, a.tcr_el1, a.ttbr0_el1, a.ttbr1_el1,
        va, access, mem, tlb,
    )
}

/// Like `translate()` but takes a `MmuConfig` snapshot instead of a full
/// `Aarch64ArchState`.  Returns just the PA (sufficient for FS-mode dispatch).
/// Avoids allocating a dummy `Aarch64ArchState` in `TranslatingMem`.
/// Like `translate()` but takes a `MmuConfig` snapshot instead of a full
/// `Aarch64ArchState`.  Returns `(PA, MmuFault)` so callers can propagate
/// the exact fault level and kind into ESR ISS fields.
pub fn translate_cfg(
    cfg: &MmuConfig,
    va: u64,
    access: MmuAccess,
    mem: &mut impl MemInterface,
    tlb: Option<&mut Tlb>,
) -> Result<u64, MmuFault> {
    translate_inner(
        cfg.sctlr_el1, cfg.tcr_el1, cfg.ttbr0_el1, cfg.ttbr1_el1,
        va, access, mem, tlb,
    )
    .map(|r| r.pa)
}

/// Shared core of `translate()` and `translate_cfg()`.
#[inline]
fn translate_inner(
    sctlr_el1: u64,
    tcr_el1: u64,
    ttbr0_el1: u64,
    ttbr1_el1: u64,
    va: u64,
    access: MmuAccess,
    mem: &mut impl MemInterface,
    tlb: Option<&mut Tlb>,
) -> Result<TranslateResult, MmuFault> {
    // Fast path: MMU disabled => identity translation.
    if sctlr_el1 & 1 == 0 {
        return Ok(TranslateResult { pa: va, attr_idx: 0, ap: 0 });
    }

    // Determine which TTBR to use based on VA[63].
    let is_upper = va >> 63 != 0;
    let (ttbr, tsz) = if !is_upper {
        (ttbr0_el1, (tcr_el1 & 0x3F) as u32)
    } else {
        (ttbr1_el1, ((tcr_el1 >> 16) & 0x3F) as u32)
    };

    // TLB fast path.
    if let Some(ref tlb_ref) = tlb {
        if let Some(pa) = tlb_ref.lookup(va, ttbr) {
            return Ok(TranslateResult { pa, attr_idx: 0, ap: 0 });
        }
    }

    // Table base address from TTBR: bits [47:12] for 4KB granule.
    let table_base = ttbr & 0x0000_FFFF_FFFF_F000;

    // Starting level: 4KB granule, each level resolves 9 VA bits.
    let va_bits = 64 - tsz;
    let start_level = if va_bits > 39 { 0u8 } else if va_bits > 30 { 1 } else if va_bits > 21 { 2 } else { 3 };

    let result = walk(va, table_base, start_level, access, mem, is_upper)?;

    // TLB fill on success.
    if let Some(tlb_mut) = tlb {
        tlb_mut.insert(va, result.pa, ttbr);
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Internal page table walk
// ---------------------------------------------------------------------------

/// Perform the page table walk from `start_level` to level 3.
///
/// Each level indexes into a 512-entry table (9 bits of VA) using 8-byte
/// descriptors read from physical memory via `mem.read()`.
fn walk(
    va: u64,
    table_base: u64,
    start_level: u8,
    access: MmuAccess,
    mem: &mut impl MemInterface,
    is_upper: bool,
) -> Result<TranslateResult, MmuFault> {
    let mut table_addr = table_base;

    for level in start_level..=3 {
        // VA index bits for this level:
        //   Level 0: VA[47:39]  (shift = 39)
        //   Level 1: VA[38:30]  (shift = 30)
        //   Level 2: VA[29:21]  (shift = 21)
        //   Level 3: VA[20:12]  (shift = 12)
        let shift = ((3 - level) as u64) * 9 + 12;
        let index = (va >> shift) & 0x1FF;

        // Read the 8-byte descriptor at physical address.
        let desc_addr = table_addr + index * 8;
        let desc = mem
            .read(desc_addr, 8, AccessType::Load)
            .map_err(|_| MmuFault {
                va,
                level,
                kind: FaultKind::Translation,
            })?;

        // Bit 0 = valid.
        if desc & 1 == 0 {
            return Err(MmuFault {
                va,
                level,
                kind: FaultKind::Translation,
            });
        }

        if level == 3 {
            // Level 3: must be a page descriptor (bits[1:0] = 0b11).
            if desc & 0x3 != 0x3 {
                return Err(MmuFault {
                    va,
                    level,
                    kind: FaultKind::Translation,
                });
            }
            return finish_page(va, desc, level, access, is_upper, 12);
        }

        // Levels 0-2: bit 1 distinguishes table (1) vs block (0).
        if desc & 0x2 == 0 {
            // Block descriptor (valid at level 1 and level 2 only).
            return match level {
                1 => finish_page(va, desc, level, access, is_upper, 30), // 1 GB block
                2 => finish_page(va, desc, level, access, is_upper, 21), // 2 MB block
                _ => Err(MmuFault {
                    va,
                    level,
                    kind: FaultKind::Translation,
                }),
            };
        }

        // Table descriptor: next-level table address = desc[47:12].
        table_addr = desc & 0x0000_FFFF_FFFF_F000;
    }

    // Should never reach here -- the loop covers levels start..=3.
    Err(MmuFault {
        va,
        level: 3,
        kind: FaultKind::Translation,
    })
}

/// Extract the physical address and attributes from a page/block descriptor,
/// check permissions, and return the `TranslateResult`.
///
/// `page_shift` is 12 for 4KB pages, 21 for 2MB blocks, 30 for 1GB blocks.
fn finish_page(
    va: u64,
    desc: u64,
    level: u8,
    access: MmuAccess,
    is_upper: bool,
    page_shift: u32,
) -> Result<TranslateResult, MmuFault> {
    // Physical base address from descriptor bits [47:page_shift].
    let pa_mask = 0x0000_FFFF_FFFF_F000u64 & !((1u64 << page_shift) - 1);
    let pa_base = desc & pa_mask;

    // Offset within the page/block.
    let offset_mask = (1u64 << page_shift) - 1;
    let pa = pa_base | (va & offset_mask);

    // Descriptor attribute fields.
    let attr_idx = ((desc >> 2) & 0x7) as u8; // AttrIndx[2:0]
    let ap = ((desc >> 6) & 0x3) as u8; // AP[2:1]

    // Permission check.
    check_permissions(va, level, ap, access, is_upper)?;

    Ok(TranslateResult { pa, attr_idx, ap })
}

// ---------------------------------------------------------------------------
// Permission checks
// ---------------------------------------------------------------------------

/// Check access permissions from AP[2:1] bits.
///
/// AP[2:1] encoding (EL1 accesses, 4KB granule):
///
/// | AP[2:1] | EL1       | EL0       |
/// |---------|-----------|-----------|
/// | `00`    | RW        | No access |
/// | `01`    | RW        | RW        |
/// | `10`    | RO        | No access |
/// | `11`    | RO        | RO        |
///
/// Currently only EL1 access is checked.
fn check_permissions(
    va: u64,
    level: u8,
    ap: u8,
    access: MmuAccess,
    _is_upper: bool,
) -> Result<(), MmuFault> {
    // EL1 can always read (all AP encodings allow EL1 read).
    // EL1 can write only when AP[2] = 0 (i.e. ap & 0x2 == 0).
    match access {
        MmuAccess::Read => {
            // Always allowed for EL1.
            Ok(())
        }
        MmuAccess::Write => {
            if ap & 0x2 != 0 {
                Err(MmuFault {
                    va,
                    level,
                    kind: FaultKind::Permission,
                })
            } else {
                Ok(())
            }
        }
        MmuAccess::Execute => {
            // PXN/UXN checks would go here; for now allow all EL1 execution.
            Ok(())
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use helm_core::{AccessType, MemFault, MemInterface};

    // -----------------------------------------------------------------------
    // Test memory: sparse physical memory backed by a HashMap.
    // -----------------------------------------------------------------------

    struct TestMem {
        data: std::collections::HashMap<u64, u64>,
    }

    impl TestMem {
        fn new() -> Self {
            Self {
                data: std::collections::HashMap::new(),
            }
        }

        /// Store a u64 at the given physical address (8-byte aligned).
        fn store_u64(&mut self, addr: u64, val: u64) {
            self.data.insert(addr, val);
        }
    }

    impl MemInterface for TestMem {
        fn read(&mut self, addr: u64, size: usize, _ty: AccessType) -> Result<u64, MemFault> {
            if size == 8 {
                Ok(*self.data.get(&addr).unwrap_or(&0))
            } else {
                Ok(0)
            }
        }

        fn write(
            &mut self,
            _addr: u64,
            _size: usize,
            _val: u64,
            _ty: AccessType,
        ) -> Result<(), MemFault> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helpers to construct arch state with MMU on / off.
    // -----------------------------------------------------------------------

    fn make_state_mmu_off() -> Aarch64ArchState {
        let mut a = Aarch64ArchState::new();
        a.sctlr_el1 = 0; // MMU off
        a
    }

    fn make_state_mmu_on() -> Aarch64ArchState {
        let mut a = Aarch64ArchState::new();
        a.sctlr_el1 = 1; // MMU on (bit 0 = M)
        a.current_el = 1;
        a
    }

    // -----------------------------------------------------------------------
    // Test: MMU disabled => identity translation.
    // -----------------------------------------------------------------------

    #[test]
    fn mmu_off_identity_translation() {
        let a = make_state_mmu_off();
        let mut mem = TestMem::new();
        let result = translate(&a, 0x4000_0000, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.pa, 0x4000_0000);
    }

    // -----------------------------------------------------------------------
    // Test: Level 2 block descriptor (2 MB).
    // -----------------------------------------------------------------------

    #[test]
    fn level2_block_2mb() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 16; // T0SZ=16 => 48-bit VA, start at level 0
        let mut mem = TestMem::new();

        let pgd_base: u64 = 0x1_0000;
        a.ttbr0_el1 = pgd_base;

        // L0[0] -> table at 0x2_0000
        let l1_table: u64 = 0x2_0000;
        mem.store_u64(pgd_base, l1_table | 0x3); // valid table

        // L1[0] -> table at 0x3_0000
        let l2_table: u64 = 0x3_0000;
        mem.store_u64(l1_table, l2_table | 0x3); // valid table

        // L2[0] -> 2 MB block at PA 0x8000_0000
        let block_pa: u64 = 0x8000_0000;
        mem.store_u64(l2_table, block_pa | 0x1); // valid block (bit1=0, bit0=1)

        // VA 0x10_0000: L0 idx=0, L1 idx=0, L2 idx=0, offset=0x10_0000
        let va = 0x0000_0000_0010_0000;
        let result = translate(&a, va, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.pa, block_pa | (va & 0x1F_FFFF));
    }

    // -----------------------------------------------------------------------
    // Test: Level 3 page descriptor (4 KB).
    // -----------------------------------------------------------------------

    #[test]
    fn level3_page_4kb() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 25; // T0SZ=25 => 39-bit VA, start at level 1
        let mut mem = TestMem::new();

        let l1_base: u64 = 0x1_0000;
        a.ttbr0_el1 = l1_base;

        // L1[0] -> table at 0x2_0000
        let l2_table: u64 = 0x2_0000;
        mem.store_u64(l1_base, l2_table | 0x3);

        // L2[0] -> table at 0x3_0000
        let l3_table: u64 = 0x3_0000;
        mem.store_u64(l2_table, l3_table | 0x3);

        // L3[0] -> 4KB page at PA 0x5_0000
        let page_pa: u64 = 0x5_0000;
        mem.store_u64(l3_table, page_pa | 0x3); // valid page

        let va = 0x0000_0000_0000_0042;
        let result = translate(&a, va, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.pa, page_pa | 0x42);
    }

    // -----------------------------------------------------------------------
    // Test: Translation fault from invalid descriptor.
    // -----------------------------------------------------------------------

    #[test]
    fn translation_fault_invalid_descriptor() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 25;
        let mut mem = TestMem::new();

        let l1_base: u64 = 0x1_0000;
        a.ttbr0_el1 = l1_base;

        // L1[0] = 0 (invalid).
        mem.store_u64(l1_base, 0);

        let result = translate(&a, 0x0, MmuAccess::Read, &mut mem, None);
        assert!(result.is_err());
        let fault = result.unwrap_err();
        assert_eq!(fault.kind, FaultKind::Translation);
        assert_eq!(fault.level, 1);
    }

    // -----------------------------------------------------------------------
    // Test: Permission fault -- write to read-only mapping.
    // -----------------------------------------------------------------------

    #[test]
    fn permission_fault_write_to_ro() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 25;
        let mut mem = TestMem::new();

        let l1_base: u64 = 0x1_0000;
        a.ttbr0_el1 = l1_base;

        // L1[0] -> table
        let l2_table: u64 = 0x2_0000;
        mem.store_u64(l1_base, l2_table | 0x3);

        // L2[0] -> table
        let l3_table: u64 = 0x3_0000;
        mem.store_u64(l2_table, l3_table | 0x3);

        // L3[0] -> page with AP=0b10 (RO at EL1)
        let page_pa: u64 = 0x5_0000;
        let ap_ro: u64 = 0b10 << 6;
        mem.store_u64(l3_table, page_pa | ap_ro | 0x3);

        // Read should succeed.
        let result = translate(&a, 0, MmuAccess::Read, &mut mem, None);
        assert!(result.is_ok());

        // Write should fail with Permission fault.
        let result = translate(&a, 0, MmuAccess::Write, &mut mem, None);
        assert!(result.is_err());
        let fault = result.unwrap_err();
        assert_eq!(fault.kind, FaultKind::Permission);
    }

    // -----------------------------------------------------------------------
    // Test: TTBR1 selected for upper-half (kernel) addresses.
    // -----------------------------------------------------------------------

    #[test]
    fn ttbr1_for_upper_addresses() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 25 | (25 << 16); // T0SZ=25, T1SZ=25
        let mut mem = TestMem::new();

        // TTBR0 for lower half.
        let ttbr0_base: u64 = 0x1_0000;
        a.ttbr0_el1 = ttbr0_base;

        // TTBR1 for upper half.
        let ttbr1_base: u64 = 0xA_0000;
        a.ttbr1_el1 = ttbr1_base;

        // Use a kernel VA whose L1/L2/L3 indices are all 0 within the
        // TTBR1 address space.  With T1SZ=25 the VA range starts at
        // 0xFFFF_FF80_0000_0000.  L1[0], L2[0], L3[0].
        let va = 0xFFFF_FF80_0000_0000u64;

        // Set up TTBR1 page tables for kernel VA.
        let l2_table: u64 = 0xB_0000;
        mem.store_u64(ttbr1_base, l2_table | 0x3); // L1[0]
        let l3_table: u64 = 0xC_0000;
        mem.store_u64(l2_table, l3_table | 0x3); // L2[0]
        let page_pa: u64 = 0xD_0000;
        mem.store_u64(l3_table, page_pa | 0x3); // L3[0]

        // Upper-half VA (bit63=1): should use TTBR1.
        let result = translate(&a, va, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.pa, page_pa | (va & 0xFFF));
    }

    // -----------------------------------------------------------------------
    // Test: Level 1 block descriptor (1 GB).
    // -----------------------------------------------------------------------

    #[test]
    fn level1_block_1gb() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 16; // T0SZ=16 => 48-bit VA, start at level 0
        let mut mem = TestMem::new();

        let pgd_base: u64 = 0x1_0000;
        a.ttbr0_el1 = pgd_base;

        // L0[0] -> table at 0x2_0000
        let l1_table: u64 = 0x2_0000;
        mem.store_u64(pgd_base, l1_table | 0x3);

        // L1[0] -> 1 GB block at PA 0x4000_0000
        let block_pa: u64 = 0x4000_0000;
        mem.store_u64(l1_table, block_pa | 0x1); // valid block

        let va = 0x1234_5678u64;
        let result = translate(&a, va, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.pa, block_pa | (va & 0x3FFF_FFFF));
    }

    // -----------------------------------------------------------------------
    // Test: Fault syndrome encoding carries the correct level.
    // -----------------------------------------------------------------------

    #[test]
    fn mmu_fault_encodes_level() {
        let fault = MmuFault {
            va: 0x1000,
            level: 2,
            kind: FaultKind::Translation,
        };
        let iss = fault.iss_data(false);
        // Translation fault level 2: 0b000110 = 0x06
        assert_eq!(iss & 0x3F, 0b000110);
    }

    // -----------------------------------------------------------------------
    // Test: Data-abort WnR bit set for write faults.
    // -----------------------------------------------------------------------

    #[test]
    fn data_abort_wnr_bit() {
        let fault = MmuFault {
            va: 0x2000,
            level: 3,
            kind: FaultKind::Permission,
        };

        // Read fault: WnR = 0.
        let iss_read = fault.iss_data(false);
        assert_eq!(iss_read & (1 << 6), 0);

        // Write fault: WnR = 1.
        let iss_write = fault.iss_data(true);
        assert_ne!(iss_write & (1 << 6), 0);
    }

    // -----------------------------------------------------------------------
    // Test: Instruction-abort syndrome encoding.
    // -----------------------------------------------------------------------

    #[test]
    fn insn_abort_syndrome() {
        let fault = MmuFault {
            va: 0x3000,
            level: 1,
            kind: FaultKind::Permission,
        };
        let iss = fault.iss_insn();
        // Permission fault level 1: 0b001101 = 0x0D
        assert_eq!(iss & 0x3F, 0b001101);
    }

    // -----------------------------------------------------------------------
    // Test: Address size fault encoding.
    // -----------------------------------------------------------------------

    #[test]
    fn address_size_fault_encoding() {
        let fault = MmuFault {
            va: 0,
            level: 0,
            kind: FaultKind::AddressSize,
        };
        let iss = fault.iss_data(false);
        // Address size fault level 0: 0b000000
        assert_eq!(iss & 0x3F, 0b000000);
    }

    // -----------------------------------------------------------------------
    // Test: Access flag fault encoding.
    // -----------------------------------------------------------------------

    #[test]
    fn access_flag_fault_encoding() {
        let fault = MmuFault {
            va: 0,
            level: 2,
            kind: FaultKind::AccessFlag,
        };
        let iss = fault.iss_data(false);
        // Access flag fault level 2: 0b001010
        assert_eq!(iss & 0x3F, 0b001010);
    }

    // -----------------------------------------------------------------------
    // Test: Non-zero page offset preserved through 4KB translation.
    // -----------------------------------------------------------------------

    #[test]
    fn page_offset_preserved() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 25; // T0SZ=25 => 39-bit VA
        let mut mem = TestMem::new();

        let l1_base: u64 = 0x1_0000;
        a.ttbr0_el1 = l1_base;

        let l2_table: u64 = 0x2_0000;
        mem.store_u64(l1_base, l2_table | 0x3);
        let l3_table: u64 = 0x3_0000;
        mem.store_u64(l2_table, l3_table | 0x3);
        let page_pa: u64 = 0x7_0000;
        mem.store_u64(l3_table, page_pa | 0x3);

        // Translate with various offsets within the page.
        for offset in [0x000u64, 0x100, 0x7FF, 0xFFF] {
            let va = offset;
            let result = translate(&a, va, MmuAccess::Read, &mut mem, None).unwrap();
            assert_eq!(
                result.pa,
                page_pa | offset,
                "offset {offset:#x} not preserved"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test: Execute access allowed (PXN not yet enforced).
    // -----------------------------------------------------------------------

    #[test]
    fn execute_access_allowed() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 25;
        let mut mem = TestMem::new();

        let l1_base: u64 = 0x1_0000;
        a.ttbr0_el1 = l1_base;

        let l2_table: u64 = 0x2_0000;
        mem.store_u64(l1_base, l2_table | 0x3);
        let l3_table: u64 = 0x3_0000;
        mem.store_u64(l2_table, l3_table | 0x3);
        // RO page (AP=0b10) -- execute should still succeed.
        let page_pa: u64 = 0x5_0000;
        let ap_ro: u64 = 0b10 << 6;
        mem.store_u64(l3_table, page_pa | ap_ro | 0x3);

        let result = translate(&a, 0, MmuAccess::Execute, &mut mem, None);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Test: Attribute index extracted correctly.
    // -----------------------------------------------------------------------

    #[test]
    fn attr_index_extracted() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 25;
        let mut mem = TestMem::new();

        let l1_base: u64 = 0x1_0000;
        a.ttbr0_el1 = l1_base;

        let l2_table: u64 = 0x2_0000;
        mem.store_u64(l1_base, l2_table | 0x3);
        let l3_table: u64 = 0x3_0000;
        mem.store_u64(l2_table, l3_table | 0x3);

        // Page with AttrIndx = 5 (bits[4:2] = 0b101).
        let page_pa: u64 = 0x5_0000;
        let attr_idx_val: u64 = 5 << 2; // AttrIndx[2:0] in bits[4:2]
        mem.store_u64(l3_table, page_pa | attr_idx_val | 0x3);

        let result = translate(&a, 0, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.attr_idx, 5);
    }
}

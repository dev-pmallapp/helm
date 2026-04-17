//! IOMMU software TLB — direct-mapped translation cache.
//!
//! Shared across all IOMMU variants (ARM SMMU, AMD-Vi, RISC-V IOMMU).
//! The TLB structure is architecture-independent: it caches
//! (`stream_id`, `asid`, VA) to (PA, size, prot) mappings.

/// Number of TLB entries (must be power of 2).
const IOMMU_TLB_SIZE: usize = 256;

/// A single TLB entry caching one translation result.
#[derive(Debug, Clone, Default)]
pub struct IommuTlbEntry {
    /// Entry is valid.
    pub valid: bool,
    /// Stream/device ID that produced this translation.
    pub stream_id: u32,
    /// Address Space ID (ASID/PASID/GSCID depending on architecture).
    pub asid: u16,
    /// Input virtual address (page-aligned).
    pub va: u64,
    /// Output physical address (page-aligned).
    pub pa: u64,
    /// Page size: 4KB, 2MB, or 1GB.
    pub size: u64,
    /// Protection flags — bit 0 = read, bit 1 = write, bit 2 = execute.
    pub prot: u32,
}

/// Direct-mapped software TLB for IOMMU translations.
pub struct IommuTlb {
    entries: Vec<IommuTlbEntry>,
}

impl IommuTlb {
    /// Create a new TLB with `IOMMU_TLB_SIZE` invalid entries.
    pub fn new() -> Self {
        Self {
            entries: vec![IommuTlbEntry::default(); IOMMU_TLB_SIZE],
        }
    }

    /// Compute the index for a given (`stream_id`, `asid`, va) triple.
    ///
    /// Including ASID in the hash avoids pathological eviction when
    /// different address spaces translate the same VA through the same
    /// stream ID — without ASID in the hash the second `fill` would
    /// unconditionally evict the first.
    fn index(stream_id: u32, asid: u16, va: u64) -> usize {
        ((stream_id as usize) ^ (asid as usize) ^ ((va >> 12) as usize)) & (IOMMU_TLB_SIZE - 1)
    }

    /// Look up a translation. Returns `Some(&entry)` on hit, `None` on miss.
    pub fn lookup(&self, stream_id: u32, asid: u16, va: u64) -> Option<&IommuTlbEntry> {
        let idx = Self::index(stream_id, asid, va);
        let e = &self.entries[idx];
        if e.valid && e.stream_id == stream_id && e.asid == asid {
            let page_mask = !(e.size - 1);
            if (va & page_mask) == e.va {
                return Some(e);
            }
        }
        None
    }

    /// Insert a translation into the TLB (overwrites any existing entry at that index).
    pub fn fill(&mut self, stream_id: u32, asid: u16, va: u64, pa: u64, size: u64, prot: u32) {
        let page_mask = !(size - 1);
        let idx = Self::index(stream_id, asid, va);
        self.entries[idx] = IommuTlbEntry {
            valid: true,
            stream_id,
            asid,
            va: va & page_mask,
            pa: pa & page_mask,
            size,
            prot,
        };
    }

    /// Invalidate all entries.
    pub fn flush_all(&mut self) {
        for e in &mut self.entries {
            e.valid = false;
        }
    }

    /// Invalidate all entries matching a given ASID.
    pub fn flush_by_asid(&mut self, asid: u16) {
        for e in &mut self.entries {
            if e.valid && e.asid == asid {
                e.valid = false;
            }
        }
    }

    /// Invalidate the entry matching (ASID, VA).
    pub fn flush_by_va_asid(&mut self, asid: u16, va: u64) {
        for e in &mut self.entries {
            if e.valid && e.asid == asid {
                let page_mask = !(e.size - 1);
                if (va & page_mask) == e.va {
                    e.valid = false;
                }
            }
        }
    }

    /// Invalidate all entries matching a given stream ID.
    pub fn flush_by_sid(&mut self, stream_id: u32) {
        for e in &mut self.entries {
            if e.valid && e.stream_id == stream_id {
                e.valid = false;
            }
        }
    }

    /// Invalidate entries for a range of stream IDs: [sid, sid + count).
    pub fn flush_by_sid_range(&mut self, sid: u32, count: u32) {
        for e in &mut self.entries {
            if e.valid && e.stream_id >= sid && e.stream_id < sid.wrapping_add(count) {
                e.valid = false;
            }
        }
    }
}

impl Default for IommuTlb {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tlb_has_no_valid_entries() {
        let tlb = IommuTlb::new();
        for e in &tlb.entries {
            assert!(!e.valid);
        }
    }

    #[test]
    fn fill_and_lookup_hit() {
        let mut tlb = IommuTlb::new();
        tlb.fill(1, 42, 0x1000, 0x8_0000, 0x1000, 0x3);
        let hit = tlb.lookup(1, 42, 0x1000);
        assert!(hit.is_some());
        let e = hit.unwrap();
        assert_eq!(e.pa, 0x8_0000);
        assert_eq!(e.asid, 42);
        assert_eq!(e.prot, 0x3);
    }

    #[test]
    fn lookup_miss_wrong_sid() {
        let mut tlb = IommuTlb::new();
        tlb.fill(1, 42, 0x1000, 0x8_0000, 0x1000, 0x3);
        assert!(tlb.lookup(2, 42, 0x1000).is_none());
    }

    #[test]
    fn lookup_miss_wrong_asid() {
        let mut tlb = IommuTlb::new();
        tlb.fill(1, 42, 0x1000, 0x8_0000, 0x1000, 0x3);
        assert!(tlb.lookup(1, 7, 0x1000).is_none());
    }

    #[test]
    fn lookup_miss_wrong_va() {
        let mut tlb = IommuTlb::new();
        tlb.fill(1, 42, 0x1000, 0x8_0000, 0x1000, 0x3);
        assert!(tlb.lookup(1, 42, 0x2000).is_none());
    }

    #[test]
    fn flush_all_invalidates_everything() {
        let mut tlb = IommuTlb::new();
        tlb.fill(1, 42, 0x1000, 0x8_0000, 0x1000, 0x3);
        tlb.fill(2, 43, 0x2000, 0x9_0000, 0x1000, 0x3);
        tlb.flush_all();
        assert!(tlb.lookup(1, 42, 0x1000).is_none());
        assert!(tlb.lookup(2, 43, 0x2000).is_none());
    }

    #[test]
    fn flush_by_asid_selective() {
        let mut tlb = IommuTlb::new();
        tlb.fill(1, 42, 0x1000, 0x8_0000, 0x1000, 0x3);
        tlb.fill(2, 43, 0x2000, 0x9_0000, 0x1000, 0x3);
        tlb.flush_by_asid(42);
        assert!(tlb.lookup(1, 42, 0x1000).is_none());
        assert!(tlb.lookup(2, 43, 0x2000).is_some());
    }

    #[test]
    fn flush_by_va_asid() {
        let mut tlb = IommuTlb::new();
        tlb.fill(1, 42, 0x1000, 0x8_0000, 0x1000, 0x3);
        tlb.fill(1, 42, 0x5000, 0xA_0000, 0x1000, 0x3);
        tlb.flush_by_va_asid(42, 0x1000);
        assert!(tlb.lookup(1, 42, 0x1000).is_none());
    }

    #[test]
    fn flush_by_sid_range() {
        let mut tlb = IommuTlb::new();
        tlb.fill(10, 1, 0x1000, 0x8_0000, 0x1000, 0x3);
        tlb.fill(11, 1, 0x2000, 0x9_0000, 0x1000, 0x3);
        tlb.fill(20, 1, 0x3000, 0xA_0000, 0x1000, 0x3);
        tlb.flush_by_sid_range(10, 2);
        assert!(tlb.lookup(10, 1, 0x1000).is_none());
        assert!(tlb.lookup(11, 1, 0x2000).is_none());
        assert!(tlb.lookup(20, 1, 0x3000).is_some());
    }

    #[test]
    fn superpage_lookup_base_va() {
        let mut tlb = IommuTlb::new();
        tlb.fill(1, 42, 0x20_0000, 0x100_0000, 0x20_0000, 0x3);
        let hit = tlb.lookup(1, 42, 0x20_0000);
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().pa, 0x100_0000);
        assert_eq!(hit.unwrap().size, 0x20_0000);
    }

    // ── ASID-sensitive regression tests ─────────────────────────────────

    /// Same (stream_id, VA) with different ASIDs must never hit each other.
    #[test]
    fn same_sid_va_different_asid_no_false_hit() {
        let mut tlb = IommuTlb::new();
        tlb.fill(1, 10, 0x1000, 0xA_0000, 0x1000, 0x3);
        tlb.fill(1, 20, 0x1000, 0xB_0000, 0x1000, 0x3);

        // ASID 10 must not see ASID 20's mapping and vice versa.
        let hit10 = tlb.lookup(1, 10, 0x1000);
        let hit20 = tlb.lookup(1, 20, 0x1000);

        // Both entries coexist because ASID is part of the index hash.
        assert!(hit10.is_some(), "ASID 10 entry should be present");
        assert!(hit20.is_some(), "ASID 20 entry should be present");
        assert_eq!(hit10.unwrap().pa, 0xA_0000);
        assert_eq!(hit20.unwrap().pa, 0xB_0000);
    }

    /// ASID-specific invalidation must not affect a different ASID.
    #[test]
    fn flush_by_asid_does_not_affect_other_asid() {
        let mut tlb = IommuTlb::new();
        tlb.fill(1, 10, 0x1000, 0xA_0000, 0x1000, 0x3);
        tlb.fill(1, 20, 0x1000, 0xB_0000, 0x1000, 0x3);

        tlb.flush_by_asid(10);

        assert!(tlb.lookup(1, 10, 0x1000).is_none(), "ASID 10 must be gone");
        assert!(tlb.lookup(1, 20, 0x1000).is_some(), "ASID 20 must survive");
    }

    /// VA+ASID invalidation with different ASIDs at the same VA.
    #[test]
    fn flush_by_va_asid_precise() {
        let mut tlb = IommuTlb::new();
        tlb.fill(1, 10, 0x1000, 0xA_0000, 0x1000, 0x3);
        tlb.fill(1, 20, 0x1000, 0xB_0000, 0x1000, 0x3);

        tlb.flush_by_va_asid(10, 0x1000);

        assert!(tlb.lookup(1, 10, 0x1000).is_none(), "flushed ASID 10 entry");
        assert!(tlb.lookup(1, 20, 0x1000).is_some(), "ASID 20 untouched");
    }

    /// Superpage entries must preserve ASID — lookup with wrong ASID must miss.
    #[test]
    fn superpage_lookup_preserves_asid() {
        let mut tlb = IommuTlb::new();
        // 2MB superpage
        tlb.fill(1, 42, 0x20_0000, 0x100_0000, 0x20_0000, 0x3);

        // Same VA + stream_id, wrong ASID → miss
        assert!(tlb.lookup(1, 99, 0x20_0000).is_none());
        // Correct ASID → hit
        let hit = tlb.lookup(1, 42, 0x20_0000);
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().pa, 0x100_0000);
        assert_eq!(hit.unwrap().asid, 42);
    }
}

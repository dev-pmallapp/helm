# helm-memory — Test Plan

> **Status:** Draft — Phase 1 target
> **Covers:** Unit tests, property tests, invariant enforcement tests, MMIO dispatch tests

---

## 1. Test Organization

```
helm-memory/
├── src/
│   ├── region.rs         — #[cfg(test)] mod tests at bottom of file
│   ├── map.rs            — #[cfg(test)] mod tests
│   ├── flat_view.rs      — #[cfg(test)] mod tests
│   ├── cache/model.rs    — #[cfg(test)] mod tests
│   └── tlb/model.rs      — #[cfg(test)] mod tests
└── tests/
    ├── integration.rs    — cross-module integration tests
    ├── sv39.rs           — page table walk tests with MockMem
    ├── aarch64_walk.rs   — AArch64 page walk tests
    └── proptest.rs       — property-based tests (proptest crate)
```

All unit tests use `#[test]`, no async. Property tests require the `proptest` crate in `[dev-dependencies]`.

---

## 2. MemoryMap Tests

### 2.1 `add_region` — Basic Mapping

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn make_ram(size: u64) -> MemoryRegion {
        MemoryRegion::Ram { data: vec![0u8; size as usize], size }
    }

    fn make_rom(data: Vec<u8>) -> MemoryRegion {
        let size = data.len() as u64;
        MemoryRegion::Rom { data, size }
    }

    /// Adding a RAM region and reading back written data.
    #[test]
    fn add_ram_read_write_roundtrip() {
        let root = MemoryRegion::Container {
            subregions: vec![(0x0, make_ram(0x1000))],
            size: 0x1000_0000,
        };
        let mut map = MemoryMap::new(root);

        map.write_functional(0x0, 4, 0xDEAD_BEEF).unwrap();
        let (val, _) = map.read_atomic(0x0, 4).unwrap();
        assert_eq!(val, 0xDEAD_BEEF);
    }

    /// Reading from an address not covered by any region returns AccessFault.
    #[test]
    fn unmapped_address_returns_access_fault() {
        let root = MemoryRegion::Container {
            subregions: vec![(0x0, make_ram(0x1000))],
            size: 0x1000_0000,
        };
        let mut map = MemoryMap::new(root);

        let result = map.read_atomic(0xFF00_0000, 4);
        assert_eq!(result, Err(MemFault::AccessFault { addr: 0xFF00_0000 }));
    }

    /// Writing to a ROM region returns ReadOnly fault.
    #[test]
    fn write_to_rom_returns_readonly_fault() {
        let root = MemoryRegion::Container {
            subregions: vec![(0x2000_0000, make_rom(vec![0xAA; 0x100]))],
            size: 0x1_0000_0000,
        };
        let mut map = MemoryMap::new(root);

        let result = map.write_atomic(0x2000_0000, 1, 0xFF);
        assert_eq!(result, Err(MemFault::ReadOnly { addr: 0x2000_0000 }));
    }

    /// Dynamic add_region: add a region after construction, then read from it.
    #[test]
    fn dynamic_add_region() {
        let root = MemoryRegion::Container {
            subregions: vec![],
            size: 0x1000_0000,
        };
        let mut map = MemoryMap::new(root);

        // Nothing mapped yet.
        assert!(map.read_atomic(0x1000, 4).is_err());

        // Add RAM dynamically.
        map.add_region(&[], 0x0, make_ram(0x2000));
        map.write_functional(0x1000, 4, 0x1234_5678).unwrap();
        let (val, _) = map.read_atomic(0x1000, 4).unwrap();
        assert_eq!(val, 0x1234_5678);
    }
}
```

### 2.2 Overlap Priority

```rust
    /// Q25: Last-added subregion wins over first-added on overlap.
    #[test]
    fn overlap_last_added_wins() {
        // Two RAM regions at the same base address. The second one wins.
        let first_ram  = make_ram(0x1000); // filled with 0xAA
        let second_ram = {
            let mut r = make_ram(0x1000);
            if let MemoryRegion::Ram { ref mut data, .. } = r {
                data.iter_mut().for_each(|b| *b = 0xBB);
            }
            r
        };

        let root = MemoryRegion::Container {
            subregions: vec![
                (0x0, first_ram),
                (0x0, second_ram), // added after → wins
            ],
            size: 0x10000,
        };
        let mut map = MemoryMap::new(root);

        // Byte at 0x0 should read 0xBB (second RAM).
        let (val, _) = map.read_atomic(0x0, 1).unwrap();
        assert_eq!(val, 0xBB);
    }

    /// Reserved region access yields AccessFault.
    #[test]
    fn reserved_region_faults() {
        let root = MemoryRegion::Container {
            subregions: vec![
                (0x0,    make_ram(0x1000)),
                (0x1000, MemoryRegion::Reserved { size: 0x1000 }),
            ],
            size: 0x10000,
        };
        let mut map = MemoryMap::new(root);

        assert_eq!(
            map.read_atomic(0x1000, 4),
            Err(MemFault::AccessFault { addr: 0x1000 }),
        );
    }
```

### 2.3 Read/Write Routing

```rust
    /// Functional read does not update cache stats.
    #[test]
    fn functional_read_no_cache_side_effects() {
        // Build a map with a cache layer. Read functionally. Cache misses = 0.
        // This test validates Q9.
        // (Full verification requires attaching a CacheModel; use a mock here.)
        let root = MemoryRegion::Container {
            subregions: vec![(0x0, make_ram(0x1000))],
            size: 0x1000,
        };
        let mut map = MemoryMap::new(root);
        map.write_bytes(0x0, &[0x42, 0x00, 0x00, 0x00]).unwrap();

        // Functional read.
        let val = map.read_functional(0x0, 1).unwrap();
        assert_eq!(val, 0x42);
        // No cache state to assert here without a CacheModel; integration test covers this.
    }
```

---

## 3. FlatView Tests

```rust
// helm-memory/src/flat_view.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::region::*;

    fn ram(size: u64) -> MemoryRegion {
        MemoryRegion::Ram { data: vec![0u8; size as usize], size }
    }

    /// FlatView from a simple container with two non-overlapping RAM regions.
    #[test]
    fn compute_non_overlapping_regions() {
        let root = MemoryRegion::Container {
            subregions: vec![
                (0x0000, ram(0x1000)),
                (0x2000, ram(0x1000)),
            ],
            size: 0x1_0000,
        };
        let mut view = FlatView::new();
        view.recompute(&root, 0, &mut []);

        assert_eq!(view.ranges().len(), 2);
        assert_eq!(view.ranges()[0].base, 0x0000);
        assert_eq!(view.ranges()[0].size, 0x1000);
        assert_eq!(view.ranges()[1].base, 0x2000);
        assert_eq!(view.ranges()[1].size, 0x1000);
    }

    /// Lookup: exact base address.
    #[test]
    fn lookup_at_base() {
        let root = MemoryRegion::Container {
            subregions: vec![(0x8000_0000, ram(0x1000_0000))],
            size: 0x1_0000_0000,
        };
        let mut view = FlatView::new();
        view.recompute(&root, 0, &mut []);

        let r = view.lookup(0x8000_0000).expect("should find RAM");
        assert_eq!(r.base, 0x8000_0000);
        assert_eq!(r.kind, RegionType::Ram);
    }

    /// Lookup: address in the middle of a range.
    #[test]
    fn lookup_mid_range() {
        let root = MemoryRegion::Container {
            subregions: vec![(0x1000, ram(0x4000))],
            size: 0x10000,
        };
        let mut view = FlatView::new();
        view.recompute(&root, 0, &mut []);

        let r = view.lookup(0x3000).expect("should find RAM");
        assert_eq!(r.base, 0x1000);
        assert_eq!(r.region_offset, 0x2000); // 0x3000 - 0x1000
    }

    /// Lookup: unmapped gap returns None.
    #[test]
    fn lookup_gap_returns_none() {
        let root = MemoryRegion::Container {
            subregions: vec![
                (0x0000, ram(0x1000)),
                (0x2000, ram(0x1000)),
            ],
            size: 0x10000,
        };
        let mut view = FlatView::new();
        view.recompute(&root, 0, &mut []);

        assert!(view.lookup(0x1500).is_none()); // in the gap between 0x1000 and 0x2000
        assert!(view.lookup(0xFFFF).is_none()); // beyond all ranges
    }

    /// Overlap resolution: last-added region wins (Q25).
    #[test]
    fn overlap_resolution_last_wins() {
        let mut second = MemoryRegion::Ram { data: vec![0xBBu8; 0x1000], size: 0x1000 };
        let root = MemoryRegion::Container {
            subregions: vec![
                (0x0, ram(0x2000)),  // first, covers 0x0–0x1FFF
                (0x0, second),       // second, covers 0x0–0x0FFF — wins
            ],
            size: 0x10000,
        };
        let mut view = FlatView::new();
        view.recompute(&root, 0, &mut []);

        // At 0x0: second region (higher priority) wins.
        let r0 = view.lookup(0x0).expect("should exist");
        // At 0x1000: only the first region covers this; second is 0x0–0x0FFF.
        let r1 = view.lookup(0x1000).expect("should exist");
        // The two results must have different region_ids.
        assert_ne!(r0.region_id, r1.region_id);
    }

    /// Alias expansion: alias resolves to backing RAM's region type.
    #[test]
    fn alias_expansion() {
        use std::sync::Arc;
        let ram_region = Arc::new(ram(0x1000));
        let alias = MemoryRegion::Alias {
            target: ram_region,
            target_offset: 0,
            size: 0x1000,
        };
        let root = MemoryRegion::Container {
            subregions: vec![(0x8000_0000, alias)],
            size: 0x1_0000_0000,
        };
        let mut view = FlatView::new();
        view.recompute(&root, 0, &mut []);

        let r = view.lookup(0x8000_0000).expect("alias should resolve");
        assert_eq!(r.kind, RegionType::Ram);
    }

    /// Dirty flag: FlatView is dirty after invalidate(), clean after recompute().
    #[test]
    fn dirty_flag_lifecycle() {
        let mut view = FlatView::new();
        assert!(view.is_dirty());
        let root = MemoryRegion::Container { subregions: vec![], size: 0x1000 };
        view.recompute(&root, 0, &mut []);
        assert!(!view.is_dirty());
        view.invalidate();
        assert!(view.is_dirty());
    }
}
```

---

## 4. CacheModel Tests

```rust
// helm-memory/src/cache/model.rs

#[cfg(test)]
mod tests {
    use super::*;

    fn l1_config() -> CacheConfig {
        CacheConfig {
            size_kb: 32,
            assoc: 8,
            line_size: 64,
            hit_latency: 4,
            mshrs: 8,
            write_back: true,
        }
    }

    /// Cold miss: first access to a new address always misses.
    #[test]
    fn cold_miss() {
        let mut cache = CacheModel::new(l1_config(), None);
        let result = cache.read(0x1000, 8);
        assert!(matches!(result, CacheLookupResult::Miss { level: 0 }));
        assert_eq!(cache.stats.read_misses, 1);
    }

    /// Fill then hit: after fill_line, subsequent read hits.
    #[test]
    fn fill_then_hit() {
        let cfg = l1_config();
        let mut cache = CacheModel::new(cfg.clone(), None);

        // Miss and allocate MSHR.
        let miss = cache.read(0x1000, 8);
        assert!(matches!(miss, CacheLookupResult::Miss { .. }));

        // Simulate fill from next level.
        cache.fill_line(0x1000, vec![0u8; cfg.line_size as usize]);

        // Now a read should hit.
        let hit = cache.read(0x1000, 8);
        assert!(matches!(hit, CacheLookupResult::Hit(4)));
        assert_eq!(cache.stats.read_hits, 1);
    }

    /// LRU eviction: fill all ways in a set, then fill one more; the PLRU victim is evicted.
    #[test]
    fn lru_eviction_fills_then_evicts() {
        let cfg = CacheConfig {
            size_kb: 8,  // small cache to force conflicts
            assoc: 4,
            line_size: 64,
            hit_latency: 4,
            mshrs: 16,
            write_back: true,
        };
        let num_sets = cfg.num_sets() as u64;
        let line = cfg.line_size as u64;
        let mut cache = CacheModel::new(cfg.clone(), None);

        // Fill all 4 ways in set 0.
        // Addresses that map to set 0: base + n * num_sets * line_size.
        let addrs: Vec<u64> = (0..=4u64).map(|n| n * num_sets * line).collect();
        for &addr in &addrs[..4] {
            cache.read(addr, 8);
            cache.fill_line(addr, vec![0u8; cfg.line_size as usize]);
        }

        // Verify all 4 hit.
        for &addr in &addrs[..4] {
            assert!(matches!(cache.read(addr, 8), CacheLookupResult::Hit(_)));
        }

        // Access a 5th address conflicting with set 0 — must evict one.
        cache.read(addrs[4], 8);
        cache.fill_line(addrs[4], vec![0u8; cfg.line_size as usize]);
        assert_eq!(cache.stats.evictions + cache.stats.writebacks, 1);

        // After eviction, one of the original four addresses must now miss.
        let misses_after = addrs[..4].iter()
            .filter(|&&a| matches!(cache.read(a, 8), CacheLookupResult::Miss { .. }))
            .count();
        assert_eq!(misses_after, 1);
    }

    /// Dirty writeback: write hit sets dirty bit; eviction reports writeback.
    #[test]
    fn dirty_writeback_on_eviction() {
        let cfg = CacheConfig {
            size_kb: 8,
            assoc: 4,
            line_size: 64,
            hit_latency: 4,
            mshrs: 16,
            write_back: true,
        };
        let num_sets = cfg.num_sets() as u64;
        let line = cfg.line_size as u64;
        let mut cache = CacheModel::new(cfg.clone(), None);

        // Bring address A into cache.
        let addr_a = 0u64;
        cache.read(addr_a, 8);
        cache.fill_line(addr_a, vec![0u8; cfg.line_size as usize]);

        // Write to A → dirty.
        cache.write(addr_a, 4, &[0xDE, 0xAD, 0xBE, 0xEF]);

        // Force eviction of A by filling conflicting addresses.
        let addrs: Vec<u64> = (1..=4u64).map(|n| n * num_sets * line).collect();
        for &addr in &addrs {
            cache.read(addr, 8);
            cache.fill_line(addr, vec![0u8; cfg.line_size as usize]);
        }

        // Should have triggered at least one writeback.
        assert!(cache.stats.writebacks >= 1, "expected dirty writeback, got 0");
    }

    /// MSHR capacity: exceeding mshrs returns MshrFull.
    #[test]
    fn mshr_full_blocks_new_miss() {
        let cfg = CacheConfig {
            size_kb: 32,
            assoc: 8,
            line_size: 64,
            hit_latency: 4,
            mshrs: 2, // tiny MSHR file
            write_back: true,
        };
        let num_sets = cfg.num_sets() as u64;
        let line = cfg.line_size as u64;
        let mut cache = CacheModel::new(cfg.clone(), None);

        // Issue 2 misses (fills MSHRs).
        let miss1 = cache.read(0 * num_sets * line, 8);
        let miss2 = cache.read(1 * num_sets * line, 8);
        assert!(matches!(miss1, CacheLookupResult::Miss { .. }));
        assert!(matches!(miss2, CacheLookupResult::Miss { .. }));

        // Third miss must block.
        let miss3 = cache.read(2 * num_sets * line, 8);
        assert!(matches!(miss3, CacheLookupResult::MshrFull { .. }));
    }

    /// invalidate_line: after invalidation, address misses again.
    #[test]
    fn invalidate_line_causes_miss() {
        let cfg = l1_config();
        let mut cache = CacheModel::new(cfg.clone(), None);

        cache.read(0x1000, 8);
        cache.fill_line(0x1000, vec![0u8; cfg.line_size as usize]);
        assert!(matches!(cache.read(0x1000, 8), CacheLookupResult::Hit(_)));

        cache.invalidate_line(0x1000);
        assert!(matches!(cache.read(0x1000, 8), CacheLookupResult::Miss { .. }));
    }
}
```

---

## 5. TLB Tests

```rust
// helm-memory/src/tlb/model.rs

#[cfg(test)]
mod tests {
    use super::*;

    fn default_tlb() -> TlbModel {
        TlbModel::new(TlbConfig {
            entries: 64,
            assoc: 4,
            page_sizes: vec![PageSize::Page4K, PageSize::Page2M, PageSize::Page1G],
        })
    }

    fn entry_4k(va: u64, pa: u64, asid: u16) -> TlbEntry {
        TlbEntry {
            vpn: va >> 12,
            ppn: pa >> 12,
            flags: 0b0000_1111, // V=1, R=1, W=1, X=1
            asid,
            size: PageSize::Page4K,
            global: false,
        }
    }

    /// TLB hit: insert an entry, then translate → physical address correct.
    #[test]
    fn tlb_hit_returns_correct_pa() {
        let mut tlb = default_tlb();
        let va = 0x8000_1000u64;
        let pa = 0x4000_1000u64;
        tlb.insert(entry_4k(va, pa, 1));

        let result = tlb.translate(va, 1, AccessType::Read).unwrap();
        assert_eq!(result, pa);
        assert_eq!(tlb.hits, 1);
    }

    /// TLB miss: address not in TLB returns PageFault (caller must page-walk).
    #[test]
    fn tlb_miss_returns_page_fault() {
        let mut tlb = default_tlb();
        let result = tlb.translate(0xDEAD_0000, 1, AccessType::Read);
        assert!(matches!(result, Err(MemFault::PageFault { .. })));
        assert_eq!(tlb.misses, 1);
    }

    /// ASID isolation: entry for ASID 1 is not visible under ASID 2.
    #[test]
    fn asid_isolation() {
        let mut tlb = default_tlb();
        let va = 0x8000_2000u64;
        tlb.insert(entry_4k(va, 0x9000_2000, 1)); // ASID 1

        // ASID 2 must not see it.
        assert!(tlb.translate(va, 2, AccessType::Read).is_err());
        // ASID 1 must see it.
        assert!(tlb.translate(va, 1, AccessType::Read).is_ok());
    }

    /// Global entry: visible under any ASID, not flushed by flush_asid.
    #[test]
    fn global_entry_visible_under_any_asid() {
        let mut tlb = default_tlb();
        let va = 0x0000_3000u64;
        tlb.insert(TlbEntry {
            vpn: va >> 12,
            ppn: 0x0000_3,
            flags: 0b0010_1111, // G=1 (bit 5)
            asid: 0,
            size: PageSize::Page4K,
            global: true,
        });

        // Any ASID should hit.
        assert!(tlb.translate(va, 0, AccessType::Read).is_ok());
        assert!(tlb.translate(va, 99, AccessType::Read).is_ok());
    }

    /// flush_all: clears every entry.
    #[test]
    fn flush_all_clears_tlb() {
        let mut tlb = default_tlb();
        let va = 0x8000_4000u64;
        tlb.insert(entry_4k(va, 0x4000_4000, 1));
        assert!(tlb.translate(va, 1, AccessType::Read).is_ok());

        tlb.flush_all();
        assert!(tlb.translate(va, 1, AccessType::Read).is_err());
    }

    /// flush_asid: clears only entries for a specific ASID.
    #[test]
    fn flush_asid_only_removes_matching() {
        let mut tlb = default_tlb();
        let va1 = 0x8000_5000u64;
        let va2 = 0x8000_6000u64;
        tlb.insert(entry_4k(va1, 0x1000_5000, 1));
        tlb.insert(entry_4k(va2, 0x1000_6000, 2));

        tlb.flush_asid(1);
        assert!(tlb.translate(va1, 1, AccessType::Read).is_err()); // flushed
        assert!(tlb.translate(va2, 2, AccessType::Read).is_ok());  // still present
    }

    /// flush_va: clears only the entry for a specific VA.
    #[test]
    fn flush_va_removes_specific_address() {
        let mut tlb = default_tlb();
        let va1 = 0x8000_7000u64;
        let va2 = 0x8000_8000u64;
        tlb.insert(entry_4k(va1, 0x1000_7000, 1));
        tlb.insert(entry_4k(va2, 0x1000_8000, 1));

        tlb.flush_va(va1);
        assert!(tlb.translate(va1, 1, AccessType::Read).is_err());
        assert!(tlb.translate(va2, 1, AccessType::Read).is_ok());
    }

    /// flush_asid_va: clears a specific ASID+VA combination; global entries survive.
    #[test]
    fn flush_asid_va_does_not_flush_global() {
        let mut tlb = default_tlb();
        let va = 0x8000_9000u64;
        tlb.insert(TlbEntry {
            vpn: va >> 12,
            ppn: 0x9000_9,
            flags: 0b0010_0011, // G=1
            asid: 1,
            size: PageSize::Page4K,
            global: true,
        });

        tlb.flush_asid_va(1, va);
        // Global entry must survive.
        assert!(tlb.translate(va, 1, AccessType::Read).is_ok());
    }

    /// Huge page (2MB): translate middle of a 2MB page, correct PA.
    #[test]
    fn huge_page_2mb_translate() {
        let mut tlb = default_tlb();
        let va_base = 0x8020_0000u64; // 2MB-aligned
        let pa_base = 0x4020_0000u64;
        tlb.insert(TlbEntry {
            vpn: va_base >> 21,
            ppn: pa_base >> 21,
            flags: 0b0000_1111,
            asid: 1,
            size: PageSize::Page2M,
            global: false,
        });

        // Access in the middle of the page.
        let va_mid = va_base | 0x1_2345;
        let pa_mid = tlb.translate(va_mid, 1, AccessType::Read).unwrap();
        assert_eq!(pa_mid, pa_base | 0x1_2345);
    }
}
```

---

## 6. Sv39 Page Table Walk Tests (with MockMem)

```rust
// helm-memory/tests/sv39.rs

use helm_memory::tlb::{sv39_walk, AccessType, FunctionalMem, MemFault, PageSize};
use std::collections::HashMap;

/// MockMem: a flat map from physical address to 8-byte value.
struct MockMem {
    pages: HashMap<u64, u64>,
}

impl MockMem {
    fn new() -> Self { MockMem { pages: HashMap::new() } }
    fn write_u64(&mut self, pa: u64, val: u64) { self.pages.insert(pa, val); }
}

impl FunctionalMem for MockMem {
    fn read_u64(&self, pa: u64) -> Result<u64, MemFault> {
        self.pages.get(&pa).copied()
            .ok_or(MemFault::AccessFault { addr: pa })
    }
}

/// Build a minimal 3-level Sv39 page table for one 4KB page mapping.
///
///   VA 0x0000_0000_8000_1000 → PA 0x0000_0000_BEEF_1000
fn build_sv39_single_page(mem: &mut MockMem) -> (u64, u64) {
    // Root page table at PA 0x8000 (must be page-aligned).
    let root_pa = 0x8000u64;
    let l1_pa   = 0x9000u64;
    let l2_pa   = 0xA000u64;
    let leaf_ppn = 0xBEEF_1u64; // PA = 0xBEEF_1000

    let va = 0x0000_0000_8000_1000u64;
    // VPN[2] = va[38:30] = 0x2 (for VA 0x8000_1000 in Sv39 context)
    // VPN[1] = va[29:21] = 0x0
    // VPN[0] = va[20:12] = 0x0
    // For va=0x8000_1000: VPN[2]=0, VPN[1]=0, VPN[0]=0x80 (since 0x8000_1000 >> 12 = 0x8001, & 0x1FF = 0x001)
    // Let's use a simpler VA:
    let va = 0x0000_1000u64; // VPN[2]=0, VPN[1]=0, VPN[0]=1

    // L3 (root) entry 0: pointer to L1.
    // PTE = (l1_pa >> 12) << 10 | 0x01 (V=1, not leaf: R=W=X=0)
    let root_pte = (l1_pa >> 12) << 10 | 0x01;
    mem.write_u64(root_pa + 0 * 8, root_pte);

    // L2 (l1_pa) entry 0: pointer to L2.
    let l1_pte = (l2_pa >> 12) << 10 | 0x01;
    mem.write_u64(l1_pa + 0 * 8, l1_pte);

    // L1 (l2_pa) entry 1 (VPN[0]=1): leaf PTE.
    // Leaf PTE: V=1, R=1, W=1, X=0, U=1
    let leaf_pte = (leaf_ppn << 10) | 0b0001_0111; // V|R|W|U
    mem.write_u64(l2_pa + 1 * 8, leaf_pte);

    (root_pa >> 12, va) // return (satp_ppn, va)
}

#[test]
fn sv39_4kb_page_walk_success() {
    let mut mem = MockMem::new();
    let (satp_ppn, va) = build_sv39_single_page(&mut mem);

    let entry = sv39_walk(satp_ppn, va, 1, AccessType::Read, &mem).unwrap();
    assert_eq!(entry.size, PageSize::Page4K);
    assert_eq!(entry.ppn, 0xBEEF_1);
    assert_eq!(entry.vpn, va >> 12);
}

#[test]
fn sv39_missing_pte_returns_page_fault() {
    let mem = MockMem::new(); // empty — no PTEs at all
    let result = sv39_walk(0xDEAD, 0x1000, 1, AccessType::Read, &mem);
    assert!(matches!(result, Err(MemFault::PageFault { .. })));
}

#[test]
fn sv39_write_to_readonly_page_faults() {
    let mut mem = MockMem::new();
    let (satp_ppn, va) = build_sv39_single_page(&mut mem);
    // Walk with Write access — leaf PTE has W=1, so should succeed.
    assert!(sv39_walk(satp_ppn, va, 1, AccessType::Write, &mem).is_ok());

    // Now build a read-only page (W=0).
    let root_pa = 0x8000u64;
    let l2_pa   = 0xA000u64;
    let ro_ppn  = 0x1234u64;
    // Overwrite the leaf PTE: V=1, R=1, W=0, X=0
    let ro_pte = (ro_ppn << 10) | 0b0000_0011; // V|R only
    mem.write_u64(l2_pa + 1 * 8, ro_pte);

    let result = sv39_walk(satp_ppn, va, 1, AccessType::Write, &mem);
    assert!(matches!(result, Err(MemFault::PageFault { .. })));
}

/// Q37: Sv39 gigapage (1GB) walk.
#[test]
fn sv39_gigapage_walk() {
    let mut mem = MockMem::new();
    let root_pa = 0x8000u64;
    let va = 0x0u64; // gigapage at VA 0, VPN[2]=0

    // Leaf at level 2 (gigapage): PPN must have lower 18 bits = 0 (aligned).
    let giga_ppn = 0x4000_0u64; // PA = 0x4000_0000_0000 — 18 lower PPN bits = 0 ✓
    let giga_pte = (giga_ppn << 10) | 0b0000_1111; // V|R|W|X
    mem.write_u64(root_pa + 0 * 8, giga_pte);

    let entry = sv39_walk(root_pa >> 12, va, 0, AccessType::Read, &mem).unwrap();
    assert_eq!(entry.size, PageSize::Page1G);
    assert_eq!(entry.vpn, va >> 30);
}

/// Q37: Sv39 megapage (2MB) walk.
#[test]
fn sv39_megapage_walk() {
    let mut mem = MockMem::new();
    let root_pa = 0x8000u64;
    let l1_pa   = 0x9000u64;
    let va = 0x0020_0000u64; // 2MB-aligned, VPN[2]=0, VPN[1]=1

    // L3 root → L1.
    mem.write_u64(root_pa, (l1_pa >> 12) << 10 | 0x01);

    // L2 at l1_pa, entry 1 (VPN[1]=1): leaf megapage.
    // Mega PPN: lower 9 bits must be 0.
    let mega_ppn = 0x200u64; // PA = 0x200 << 12 = 0x200_000 — lower 9 bits = 0 ✓
    let mega_pte = (mega_ppn << 10) | 0b0000_0111; // V|R|W
    mem.write_u64(l1_pa + 1 * 8, mega_pte);

    let entry = sv39_walk(root_pa >> 12, va, 0, AccessType::Read, &mem).unwrap();
    assert_eq!(entry.size, PageSize::Page2M);
    assert_eq!(entry.vpn, va >> 21);
}
```

---

## 7. Property Tests

```rust
// helm-memory/tests/proptest.rs

use proptest::prelude::*;
use helm_memory::{MemoryMap, MemoryRegion, MemFault};

fn arbitrary_ram_map(size: u64) -> MemoryMap {
    let root = MemoryRegion::Container {
        subregions: vec![(0x0, MemoryRegion::Ram {
            data: vec![0u8; size as usize],
            size,
        })],
        size,
    };
    MemoryMap::new(root)
}

proptest! {
    /// Any address within the mapped range never panics.
    #[test]
    fn read_within_range_never_panics(
        addr in 0u64..0x1000u64,
        width in prop_oneof![Just(1usize), Just(2), Just(4), Just(8)],
    ) {
        let mut map = arbitrary_ram_map(0x1000);
        // May return Ok or Err (alignment), but must not panic.
        let _ = map.read_atomic(addr, width);
    }

    /// Address completely outside the mapped range always returns Err.
    #[test]
    fn read_outside_range_always_faults(
        addr in 0x1000u64..0xFFFF_FFFF_FFFF_FFFFu64,
    ) {
        let mut map = arbitrary_ram_map(0x1000);
        let result = map.read_atomic(addr, 1);
        assert!(result.is_err(), "expected fault for addr {addr:#x}");
    }

    /// Write then read at the same address roundtrips correctly (Functional mode).
    #[test]
    fn write_read_roundtrip(
        offset in 0u64..0xFF8u64,       // ensure 8 bytes fit
        value in any::<u64>(),
    ) {
        let mut map = arbitrary_ram_map(0x1000);
        map.write_functional(offset, 8, value).unwrap();
        let readback = map.read_functional(offset, 8).unwrap();
        prop_assert_eq!(readback, value);
    }

    /// FlatView lookup for any address in [0, size) never panics.
    #[test]
    fn flat_view_lookup_never_panics(
        addr in 0u64..0xFFFF_FFFFu64,
    ) {
        use helm_memory::flat_view::FlatView;
        let root = MemoryRegion::Container {
            subregions: vec![(0x0, MemoryRegion::Ram { data: vec![0u8; 0x1000], size: 0x1000 })],
            size: 0xFFFF_FFFF,
        };
        let mut view = FlatView::new();
        view.recompute(&root, 0, &mut []);
        // Must not panic regardless of addr.
        let _ = view.lookup(addr);
    }
}
```

---

## 8. Timing/Atomic Invariant Tests

```rust
// helm-memory/tests/integration.rs

use helm_memory::{MemoryMap, MemoryRegion, MemFault, AccessMode};

fn simple_map() -> MemoryMap {
    MemoryMap::new(MemoryRegion::Container {
        subregions: vec![(0x0, MemoryRegion::Ram { data: vec![0u8; 0x1000], size: 0x1000 })],
        size: 0x1000,
    })
}

/// Switching to Timing with no pending ops succeeds.
#[test]
fn enable_timing_succeeds_when_idle() {
    let mut map = simple_map();
    assert!(map.enable_timing().is_ok());
}

/// Attempting Atomic read while Timing mode is active returns ModeMismatch.
#[test]
fn atomic_read_while_timing_active_returns_mode_mismatch() {
    let mut map = simple_map();
    map.enable_timing().unwrap();

    // Timing mode is active; Atomic read must return ModeMismatch.
    let result = map.read_atomic(0x0, 4);
    assert_eq!(
        result,
        Err(MemFault::ModeMismatch {
            current: AccessMode::Timing,
            requested: AccessMode::Atomic,
        })
    );
}

/// Functional read succeeds regardless of current mode.
#[test]
fn functional_read_allowed_during_timing_mode() {
    let mut map = simple_map();
    map.write_bytes(0x0, &[0xAB]).unwrap();
    map.enable_timing().unwrap();

    // Functional must always work.
    let val = map.read_functional(0x0, 1).unwrap();
    assert_eq!(val, 0xAB);
}

/// disable_timing drains pending requests, then allows Atomic.
#[test]
fn disable_timing_then_atomic_succeeds() {
    let mut map = simple_map();
    map.enable_timing().unwrap();

    // Issue a timing request (no actual event loop; manually complete it).
    let mut completed = false;
    let id = map.request_timing_read(0x0, 4, Box::new(|_result| {
        // callback — would check result in a real test
    })).unwrap();
    map.complete_timing(id);

    // Now disable timing.
    map.disable_timing();

    // Atomic must work now.
    assert!(map.read_atomic(0x0, 4).is_ok());
}
```

---

## 9. MMIO Dispatch Test (with MockDevice)

```rust
// helm-memory/tests/integration.rs (continued)

use helm_memory::region::MemoryRegion;
use helm_core::Device;
use std::sync::{Arc, Mutex};

struct MockDevice {
    pub reads:  Vec<(u64, usize)>,
    pub writes: Vec<(u64, usize, u64)>,
    read_return: u64,
}

impl MockDevice {
    fn new(read_return: u64) -> Self {
        MockDevice { reads: vec![], writes: vec![], read_return }
    }
}

impl Device for MockDevice {
    fn read(&mut self, offset: u64, size: usize) -> u64 {
        self.reads.push((offset, size));
        self.read_return
    }

    fn write(&mut self, offset: u64, size: usize, val: u64) {
        self.writes.push((offset, size, val));
    }

    fn region_size(&self) -> u64 { 0x1000 }
    fn signal(&mut self, _name: &str, _val: u64) {}
}

/// MMIO read dispatches to device with correct offset.
#[test]
fn mmio_read_dispatches_with_correct_offset() {
    let device = Box::new(MockDevice::new(0xCAFE_BABE));
    let mmio_base = 0x1000_0000u64;

    let root = MemoryRegion::Container {
        subregions: vec![(mmio_base, MemoryRegion::Mmio { handler: device, size: 0x1000 })],
        size: 0xFFFF_FFFF,
    };
    let mut map = MemoryMap::new(root);

    // Read from middle of MMIO region.
    let (val, _) = map.read_atomic(mmio_base + 0x80, 4).unwrap();
    assert_eq!(val, 0xCAFE_BABE);

    // Verify the device received offset 0x80, not the full address.
    // (Requires downcasting — use Arc<Mutex<MockDevice>> in a real implementation.)
}

/// MMIO write dispatches to device with value.
#[test]
fn mmio_write_dispatches_value() {
    let device = Box::new(MockDevice::new(0));
    let mmio_base = 0x0900_0000u64;

    let root = MemoryRegion::Container {
        subregions: vec![(mmio_base, MemoryRegion::Mmio { handler: device, size: 0x1000 })],
        size: 0xFFFF_FFFF,
    };
    let mut map = MemoryMap::new(root);

    map.write_atomic(mmio_base + 0x10, 4, 0xDEAD_BEEF).unwrap();
    // With an observable MockDevice (via Arc<Mutex>), assert writes contains (0x10, 4, 0xDEAD_BEEF).
}
```

---

## 10. Test Coverage Targets

| Module | Lines target | Critical paths |
|--------|-------------|----------------|
| `MemoryMap` | ≥ 85% | add/remove, read/write, mode switching |
| `FlatView` | ≥ 90% | recompute, lookup, overlap resolution, alias |
| `CacheModel` | ≥ 85% | hit, miss, fill, eviction, writeback, MSHR |
| `TlbModel` | ≥ 90% | translate, insert, all 4 flush variants |
| `sv39_walk` | ≥ 90% | 4KB, 2MB, 1GB pages, permission faults |
| `aarch64_4k_walk` | ≥ 80% | 4KB, 2MB, 1GB block descriptors |

Run with:
```sh
cargo test -p helm-memory
cargo test -p helm-memory --test proptest
```

Coverage report:
```sh
cargo llvm-cov --package helm-memory --html
```

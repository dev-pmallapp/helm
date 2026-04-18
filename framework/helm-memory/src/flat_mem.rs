//! Sparse flat memory backend with a page-table fast path.

#![allow(missing_docs)]

use helm_core::{AccessType, MemFault, MemInterface, MemoryBackend, MemoryMapRange, MemoryMapRangeKind};

/// Sparse memory backend using contiguous mapped regions and a flat page table.
///
/// Ported from the reference implementation (`helm.git/crates/helm-memory/src/
/// address_space.rs`). The key design points:
///
/// - `map(base, size)` allocates one contiguous `Vec<u8>` per region.
/// - A flat page table (`Vec<*mut u8>`, indexed by `(PA - base) >> 12`) gives
///   O(1) host-pointer lookups for single-page accesses.
/// - Page table is rebuilt after each `map()` call.
/// - Reads to unmapped addresses return 0.
/// - Raw pointers point into owned region buffers and are accessed only
///   through `&mut self`.
pub struct FlatMem {
    regions: Vec<FlatMemRegion>,
    /// Flat page table: each entry is a host pointer to the start of that 4KB
    /// page within the owning region's data buffer. Null = unmapped.
    page_table: Vec<*mut u8>,
    /// Lowest PA covered by the page table.
    page_table_base: u64,
    /// Number of 4KB entries in the page table.
    page_table_pages: usize,
    /// When true, page table is stale and must be rebuilt before next access.
    page_table_dirty: bool,
    /// Preserved for RAM fast-path checks in higher-level memory compositions.
    pub base: u64,
    pub size_bytes: u64,
}

struct FlatMemRegion {
    base: u64,
    size: u64,
    data: Vec<u8>,
}

// Safety: raw pointers in page_table point into FlatMemRegion::data Vec<u8>
// buffers. They are valid for the lifetime of this FlatMem. All access goes
// through &mut self, so no concurrent aliasing is possible.
#[allow(unsafe_code)]
unsafe impl Send for FlatMem {}

const FM_PAGE_SHIFT: u32 = 12;
const FM_PAGE_SIZE: u64 = 1 << FM_PAGE_SHIFT;
const FM_PAGE_MASK: u64 = FM_PAGE_SIZE - 1;
/// 1M pages = 4 GiB coverage. Stays as 8 MiB table. SE-mode scattered segments
/// may exceed this and fall through to the region-scan slow path.
const FM_MAX_PAGES: usize = 1 << 20;

#[allow(unsafe_code)]
impl FlatMem {
    pub fn new(base: u64, size: usize) -> Self {
        let mut fm = Self {
            regions: Vec::new(),
            page_table: Vec::new(),
            page_table_base: 0,
            page_table_pages: 0,
            page_table_dirty: false,
            base,
            size_bytes: size as u64,
        };
        if size > 0 {
            fm.map(base, size as u64);
            fm.ensure_page_table();
        }
        fm
    }

    /// Map a contiguous region. Existing regions are preserved.
    /// The page table is rebuilt lazily on next access.
    pub fn map(&mut self, base: u64, size: u64) {
        self.regions.push(FlatMemRegion {
            base,
            size,
            data: vec![0u8; size as usize],
        });
        self.page_table_dirty = true;
    }

    /// Ensure the page table is up-to-date. Called before any read/write.
    #[inline]
    fn ensure_page_table(&mut self) {
        if self.page_table_dirty {
            self.rebuild_page_table();
            self.page_table_dirty = false;
        }
    }

    fn rebuild_page_table(&mut self) {
        use std::ptr;
        if self.regions.is_empty() {
            self.page_table.clear();
            self.page_table_base = 0;
            self.page_table_pages = 0;
            return;
        }
        let min_base = self.regions.iter().map(|r| r.base).min().unwrap();
        let max_end = self.regions.iter().map(|r| r.base + r.size).max().unwrap();
        let base_page = min_base >> FM_PAGE_SHIFT;
        let end_page = (max_end + FM_PAGE_MASK) >> FM_PAGE_SHIFT;
        let num_pages = (end_page - base_page) as usize;

        if num_pages > FM_MAX_PAGES {
            self.page_table.clear();
            self.page_table_base = 0;
            self.page_table_pages = 0;
            return;
        }
        self.page_table_base = base_page << FM_PAGE_SHIFT;
        self.page_table_pages = num_pages;
        self.page_table = vec![ptr::null_mut(); num_pages];

        for region in &self.regions {
            if region.base & FM_PAGE_MASK != 0 || region.size < FM_PAGE_SIZE {
                continue;
            }
            let pages = (region.size >> FM_PAGE_SHIFT) as usize;
            let data_ptr = region.data.as_ptr() as *mut u8;
            let start_idx = ((region.base >> FM_PAGE_SHIFT) - base_page) as usize;
            for p in 0..pages {
                let idx = start_idx + p;
                if idx < num_pages {
                    self.page_table[idx] = unsafe { data_ptr.add(p << FM_PAGE_SHIFT as usize) };
                }
            }
        }
    }

    /// Load bytes into a mapped region (e.g. from an ELF loader).
    pub fn load_bytes(&mut self, addr: u64, bytes: &[u8]) {
        self.ensure_page_table();
        let mut off: usize = 0;
        let mut va = addr;
        while off < bytes.len() {
            let page_off = (va & FM_PAGE_MASK) as usize;
            let chunk = (bytes.len() - off).min(FM_PAGE_SIZE as usize - page_off);
            if va >= self.page_table_base {
                let idx = ((va - self.page_table_base) >> FM_PAGE_SHIFT) as usize;
                if idx < self.page_table_pages {
                    let host = self.page_table[idx];
                    if !host.is_null() {
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                bytes[off..].as_ptr(),
                                host.add(page_off),
                                chunk,
                            );
                        }
                        off += chunk;
                        va += chunk as u64;
                        continue;
                    }
                }
            }
            let written = self.write_region(va, &bytes[off..off + chunk]);
            if !written {
                let page_base = va & !FM_PAGE_MASK;
                self.map(page_base, FM_PAGE_SIZE);
                self.write_region(va, &bytes[off..off + chunk]);
            }
            off += chunk;
            va += chunk as u64;
        }
    }

    fn write_region(&mut self, addr: u64, data: &[u8]) -> bool {
        for region in &mut self.regions {
            if addr >= region.base && addr.saturating_add(data.len() as u64) <= region.base.saturating_add(region.size) {
                let off = (addr - region.base) as usize;
                region.data[off..off + data.len()].copy_from_slice(data);
                return true;
            }
        }
        false
    }

    /// O(1) read of up to 8 bytes. Falls back to region scan on page-table miss.
    #[inline]
    fn read_inner(&self, addr: u64, size: usize) -> u64 {
        let page_off = (addr & FM_PAGE_MASK) as usize;
        if page_off + size <= FM_PAGE_SIZE as usize && addr >= self.page_table_base {
            let idx = ((addr - self.page_table_base) >> FM_PAGE_SHIFT) as usize;
            if idx < self.page_table_pages {
                let host = self.page_table[idx];
                if !host.is_null() {
                    let mut buf = [0u8; 8];
                    unsafe {
                        std::ptr::copy_nonoverlapping(host.add(page_off), buf.as_mut_ptr(), size);
                    }
                    return u64::from_le_bytes(buf);
                }
            }
        }
        for region in &self.regions {
            if addr >= region.base && addr.saturating_add(size as u64) <= region.base.saturating_add(region.size) {
                let off = (addr - region.base) as usize;
                let mut buf = [0u8; 8];
                buf[..size].copy_from_slice(&region.data[off..off + size]);
                return u64::from_le_bytes(buf);
            }
        }
        0
    }

    /// O(1) write of up to 8 bytes.
    #[inline]
    /// Return the host pointer for the 4KB page containing `addr`, or `None`
    /// if the page is not mapped.
    ///
    /// The returned pointer points to the **start of the page** (i.e. the host
    /// byte at `addr & !0xFFF`). The caller must add the page offset to reach
    /// the target byte. The pointer is valid as long as no new `map()` call is
    /// made.
    pub fn host_ptr_for_page(&mut self, addr: u64) -> Option<*mut u8> {
        self.ensure_page_table();
        if addr < self.page_table_base {
            return None;
        }
        let idx = ((addr - self.page_table_base) >> FM_PAGE_SHIFT) as usize;
        if idx < self.page_table_pages {
            let host = self.page_table[idx];
            if !host.is_null() {
                return Some(host);
            }
        }
        // Region-scan fallback for pages outside the page table range.
        for region in &self.regions {
            let page_base = addr & !FM_PAGE_MASK;
            if page_base >= region.base && page_base + FM_PAGE_SIZE <= region.base + region.size {
                let off = (page_base - region.base) as usize;
                return Some(unsafe { region.data.as_ptr().add(off) as *mut u8 });
            }
        }
        None
    }

    /// O(1) write of up to 8 bytes.
    #[inline]
    fn write_inner(&mut self, addr: u64, size: usize, val: u64) {
        let bytes = val.to_le_bytes();
        let page_off = (addr & FM_PAGE_MASK) as usize;
        if page_off + size <= FM_PAGE_SIZE as usize && addr >= self.page_table_base {
            let idx = ((addr - self.page_table_base) >> FM_PAGE_SHIFT) as usize;
            if idx < self.page_table_pages {
                let host = self.page_table[idx];
                if !host.is_null() {
                    unsafe {
                        std::ptr::copy_nonoverlapping(bytes.as_ptr(), host.add(page_off), size);
                    }
                    return;
                }
            }
        }
        for region in &mut self.regions {
            if addr >= region.base && addr.saturating_add(size as u64) <= region.base.saturating_add(region.size) {
                let off = (addr - region.base) as usize;
                region.data[off..off + size].copy_from_slice(&bytes[..size]);
                return;
            }
        }
        let page_base = addr & !FM_PAGE_MASK;
        self.map(page_base, FM_PAGE_SIZE);
        self.write_inner(addr, size, val);
    }
}

impl MemInterface for FlatMem {
    #[inline]
    fn read(&mut self, addr: u64, size: usize, _ty: AccessType) -> Result<u64, MemFault> {
        debug_assert!(size <= 8);
        self.ensure_page_table();
        Ok(self.read_inner(addr, size))
    }

    #[inline]
    fn write(&mut self, addr: u64, size: usize, val: u64, _ty: AccessType) -> Result<(), MemFault> {
        debug_assert!(size <= 8);
        self.ensure_page_table();
        self.write_inner(addr, size, val);
        Ok(())
    }
}

impl MemoryBackend for FlatMem {
    fn mapped_ranges(&mut self) -> Vec<MemoryMapRange> {
        self.regions
            .iter()
            .map(|r| MemoryMapRange {
                base: r.base,
                size: r.size,
                kind: MemoryMapRangeKind::Ram,
            })
            .collect()
    }

    fn backend_name(&self) -> &'static str {
        "FlatMem"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_mem_round_trips_basic_words() {
        let mut mem = FlatMem::new(0x1000, 0x2000);
        mem.write(0x1000, 4, 0x1122_3344, AccessType::Store)
            .unwrap();
        assert_eq!(mem.read(0x1000, 4, AccessType::Load).unwrap(), 0x1122_3344);
    }

    #[test]
    fn flat_mem_auto_maps_sparse_writes() {
        let mut mem = FlatMem::new(0, 0);
        mem.write(0x4000_1000, 8, 0xDEAD_BEEF_CAFE_BABE, AccessType::Store)
            .unwrap();
        assert_eq!(
            mem.read(0x4000_1000, 8, AccessType::Load).unwrap(),
            0xDEAD_BEEF_CAFE_BABE
        );
    }

    #[test]
    fn load_bytes_populates_mapped_region() {
        let mut mem = FlatMem::new(0x8000, 0x1000);
        mem.load_bytes(0x8004, &[1, 2, 3, 4]);
        assert_eq!(mem.read(0x8004, 4, AccessType::Load).unwrap(), 0x0403_0201);
    }

    // -- MemoryBackend tests --

    #[test]
    fn flatmem_backend_name() {
        let mem = FlatMem::new(0x1000, 0x2000);
        assert_eq!(mem.backend_name(), "FlatMem");
    }

    #[test]
    fn flatmem_mapped_ranges_reports_regions() {
        let mut mem = FlatMem::new(0x1000, 0x2000);
        let ranges = mem.mapped_ranges();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].base, 0x1000);
        assert_eq!(ranges[0].size, 0x2000);
        assert_eq!(ranges[0].kind, MemoryMapRangeKind::Ram);
    }

    #[test]
    fn flatmem_mapped_ranges_reports_demand_mapped_pages() {
        let mut mem = FlatMem::new(0, 0);
        // Write to an unmapped address triggers demand-map.
        mem.write(0x5000, 4, 0xAB, AccessType::Store).unwrap();
        let ranges = mem.mapped_ranges();
        assert!(!ranges.is_empty());
        assert!(ranges.iter().any(|r| r.base <= 0x5000 && r.base + r.size > 0x5000));
    }

    #[test]
    fn flatmem_contains_checks_range() {
        let mut mem = FlatMem::new(0x1000, 0x2000);
        assert!(mem.contains(0x1000));
        assert!(mem.contains(0x2FFF));
        assert!(!mem.contains(0x3000));
        assert!(!mem.contains(0x0FFF));
    }

    #[test]
    fn flatmem_works_through_dyn_backend() {
        let mut mem = FlatMem::new(0x1000, 0x1000);
        let backend: &mut dyn MemoryBackend = &mut mem;
        backend
            .write(0x1000, 4, 0xCAFE_BABE, AccessType::Store)
            .unwrap();
        let val = backend.read(0x1000, 4, AccessType::Load).unwrap();
        assert_eq!(val, 0xCAFE_BABE);
        assert_eq!(backend.backend_name(), "FlatMem");
    }

    #[test]
    fn flatmem_byte_mem_through_backend() {
        use helm_core::ByteMem;
        let mut mem = FlatMem::new(0x2000, 0x1000);
        mem.write_bytes(0x2010, &[0x11, 0x22, 0x33]).unwrap();
        let mut buf = [0u8; 3];
        mem.read_bytes(0x2010, &mut buf).unwrap();
        assert_eq!(buf, [0x11, 0x22, 0x33]);
    }
}

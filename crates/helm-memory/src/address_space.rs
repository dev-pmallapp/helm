//! Guest address space with optional I/O fallback for FS mode.

use helm_core::types::Addr;
use helm_core::HelmResult;
use std::ptr;

const PAGE_SHIFT: u32 = 12;
const PAGE_SIZE: u64 = 1 << PAGE_SHIFT;
const PAGE_MASK: u64 = PAGE_SIZE - 1;
/// Max page table entries (1M pages = 4GB PA coverage, 8MB table).
/// If the PA span exceeds this, the page table is not built.
const MAX_PAGE_TABLE_PAGES: usize = 1 << 20;

/// Memory region descriptor.
#[derive(Debug, Clone)]
pub struct MemRegion {
    pub base: Addr,
    pub size: u64,
    pub data: Vec<u8>,
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
}

/// I/O fallback handler for addresses not backed by RAM.
///
/// When a read/write misses all RAM regions, the AddressSpace calls
/// this handler. Used in FS mode to route device MMIO accesses.
pub trait IoHandler {
    /// Read `size` bytes from I/O address. Returns the value, or None
    /// if no device is mapped at this address.
    fn io_read(&mut self, addr: Addr, size: usize) -> Option<u64>;
    /// Write `size` bytes to I/O address. Returns true if handled.
    fn io_write(&mut self, addr: Addr, size: usize, value: u64) -> bool;
}

/// A flat address space with optional I/O fallback.
///
/// Maintains a flat page table (`page_table`) for O(1) PA→host-pointer
/// lookups.  The table is indexed by `(PA - page_table_base) >> 12`.
/// Null entries fall through to the region scan or I/O handler.
pub struct AddressSpace {
    regions: Vec<MemRegion>,
    /// Optional I/O handler for device MMIO in FS mode.
    io: Option<Box<dyn IoHandler>>,
    /// Flat page table: indexed by `(PA - page_table_base) >> 12`.
    /// Each entry is a host pointer to the start of that 4KB page
    /// within the owning region's `data` Vec.  Null = unmapped/IO.
    page_table: Vec<*mut u8>,
    /// Base PA of the page table coverage (page-aligned).
    page_table_base: u64,
    /// Number of 4KB pages covered by the page table.
    page_table_pages: usize,
}

// Safety: the raw pointers in page_table point into heap-allocated
// Vec<u8> buffers owned by MemRegion.  They remain valid as long as
// the AddressSpace (and its regions) is alive, and we never hand out
// references that could alias — all access goes through &mut self.
unsafe impl Send for AddressSpace {}

impl Default for AddressSpace {
    fn default() -> Self {
        Self::new()
    }
}

impl AddressSpace {
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            io: None,
            page_table: Vec::new(),
            page_table_base: 0,
            page_table_pages: 0,
        }
    }

    /// Set the I/O fallback handler for unmapped addresses.
    pub fn set_io_handler(&mut self, handler: Box<dyn IoHandler>) {
        self.io = Some(handler);
    }

    /// Map a new region.
    pub fn map(&mut self, base: Addr, size: u64, rwx: (bool, bool, bool)) {
        self.regions.push(MemRegion {
            base,
            size,
            data: vec![0u8; size as usize],
            readable: rwx.0,
            writable: rwx.1,
            executable: rwx.2,
        });
        self.rebuild_page_table();
    }

    /// Rebuild the flat page table from the current region list.
    ///
    /// Only fully-backed pages (where the entire 4KB is within a region)
    /// get an entry.  Sub-page or partial pages fall through to the
    /// region-scan slow path.
    fn rebuild_page_table(&mut self) {
        if self.regions.is_empty() {
            self.page_table.clear();
            self.page_table_base = 0;
            self.page_table_pages = 0;
            return;
        }

        let min_base = self.regions.iter().map(|r| r.base).min().unwrap();
        let max_end = self.regions.iter().map(|r| r.base + r.size).max().unwrap();

        let base_page = min_base >> PAGE_SHIFT;
        let end_page = (max_end + PAGE_MASK) >> PAGE_SHIFT;
        let num_pages = (end_page - base_page) as usize;

        // Skip if PA range is too large (e.g., SE mode with scattered segments)
        if num_pages > MAX_PAGE_TABLE_PAGES {
            self.page_table.clear();
            self.page_table_base = 0;
            self.page_table_pages = 0;
            return;
        }

        self.page_table_base = base_page << PAGE_SHIFT;
        self.page_table_pages = num_pages;
        self.page_table = vec![ptr::null_mut(); num_pages];

        for region in &self.regions {
            // First fully-backed page: round up region.base to next page boundary
            let first_full = (region.base + PAGE_MASK) & !PAGE_MASK;
            // Last address still in region
            let region_end = region.base + region.size;
            // Last fully-backed page start: round down region_end to page boundary
            let last_full_end = region_end & !PAGE_MASK;

            if first_full >= last_full_end {
                // Region too small to contain any fully-backed page.
                // Special case: region is page-aligned and exactly page-sized
                if region.base & PAGE_MASK == 0 && region.size >= PAGE_SIZE {
                    let pages = region.size >> PAGE_SHIFT;
                    let data_ptr = region.data.as_ptr() as *mut u8;
                    for p in 0..pages {
                        let pa = region.base + (p << PAGE_SHIFT);
                        let idx = ((pa >> PAGE_SHIFT) - base_page) as usize;
                        self.page_table[idx] = unsafe { data_ptr.add((p << PAGE_SHIFT) as usize) };
                    }
                }
                continue;
            }

            let data_ptr = region.data.as_ptr() as *mut u8;
            let mut pa = first_full;
            while pa < last_full_end {
                let data_offset = (pa - region.base) as usize;
                let idx = ((pa >> PAGE_SHIFT) - base_page) as usize;
                self.page_table[idx] = unsafe { data_ptr.add(data_offset) };
                pa += PAGE_SIZE;
            }
        }
    }

    /// Return the host pointer for a page-aligned PA, or None if the
    /// page is not RAM-backed.  Used to compute TLB addends.
    #[inline]
    pub fn host_ptr_for_pa(&self, pa: u64) -> Option<*mut u8> {
        debug_assert!(pa & PAGE_MASK == 0, "PA must be page-aligned");
        if pa >= self.page_table_base {
            let idx = ((pa - self.page_table_base) >> PAGE_SHIFT) as usize;
            if idx < self.page_table_pages {
                let p = self.page_table[idx];
                if !p.is_null() {
                    return Some(p);
                }
            }
        }
        None
    }

    /// Read bytes from the address space.
    #[inline]
    pub fn read(&mut self, addr: Addr, buf: &mut [u8]) -> HelmResult<()> {
        // Fast path: single-page access via page table
        let page_off = (addr & PAGE_MASK) as usize;
        if page_off + buf.len() <= PAGE_SIZE as usize && addr >= self.page_table_base {
            let idx = ((addr - self.page_table_base) >> PAGE_SHIFT) as usize;
            if idx < self.page_table_pages {
                let host = self.page_table[idx];
                if !host.is_null() {
                    unsafe {
                        ptr::copy_nonoverlapping(host.add(page_off), buf.as_mut_ptr(), buf.len());
                    }
                    return Ok(());
                }
            }
        }

        // Slow path: region scan
        for region in &self.regions {
            if addr >= region.base && addr + buf.len() as u64 <= region.base + region.size {
                let offset = (addr - region.base) as usize;
                buf.copy_from_slice(&region.data[offset..offset + buf.len()]);
                return Ok(());
            }
        }
        // I/O fallback
        if let Some(ref mut io) = self.io {
            if let Some(val) = io.io_read(addr, buf.len()) {
                let bytes = val.to_le_bytes();
                buf.copy_from_slice(&bytes[..buf.len()]);
                return Ok(());
            }
        }
        Err(helm_core::HelmError::Memory {
            addr,
            reason: "unmapped address".into(),
        })
    }

    /// Read bytes from physical address (no I/O fallback).
    /// Used by the MMU page table walker to read descriptors from RAM.
    #[inline]
    pub fn read_phys(&self, addr: Addr, buf: &mut [u8]) -> HelmResult<()> {
        // Fast path: single-page access via page table
        let page_off = (addr & PAGE_MASK) as usize;
        if page_off + buf.len() <= PAGE_SIZE as usize && addr >= self.page_table_base {
            let idx = ((addr - self.page_table_base) >> PAGE_SHIFT) as usize;
            if idx < self.page_table_pages {
                let host = self.page_table[idx];
                if !host.is_null() {
                    unsafe {
                        ptr::copy_nonoverlapping(host.add(page_off), buf.as_mut_ptr(), buf.len());
                    }
                    return Ok(());
                }
            }
        }

        // Slow path: region scan
        for region in &self.regions {
            if addr >= region.base && addr + buf.len() as u64 <= region.base + region.size {
                let offset = (addr - region.base) as usize;
                buf.copy_from_slice(&region.data[offset..offset + buf.len()]);
                return Ok(());
            }
        }
        Err(helm_core::HelmError::Memory {
            addr,
            reason: "unmapped physical address".into(),
        })
    }

    /// Write bytes into the address space.
    #[inline]
    pub fn write(&mut self, addr: Addr, data: &[u8]) -> HelmResult<()> {
        // Fast path: single-page access via page table
        let page_off = (addr & PAGE_MASK) as usize;
        if page_off + data.len() <= PAGE_SIZE as usize && addr >= self.page_table_base {
            let idx = ((addr - self.page_table_base) >> PAGE_SHIFT) as usize;
            if idx < self.page_table_pages {
                let host = self.page_table[idx];
                if !host.is_null() {
                    unsafe {
                        ptr::copy_nonoverlapping(data.as_ptr(), host.add(page_off), data.len());
                    }
                    return Ok(());
                }
            }
        }

        // Slow path: region scan
        for region in &mut self.regions {
            if addr >= region.base && addr + data.len() as u64 <= region.base + region.size {
                let offset = (addr - region.base) as usize;
                region.data[offset..offset + data.len()].copy_from_slice(data);
                return Ok(());
            }
        }
        // I/O fallback
        if let Some(ref mut io) = self.io {
            let val = if data.len() <= 8 {
                let mut buf = [0u8; 8];
                buf[..data.len()].copy_from_slice(data);
                u64::from_le_bytes(buf)
            } else {
                0 // large writes (e.g., DC ZVA 64-byte zeroing) — value not meaningful
            };
            if io.io_write(addr, data.len(), val) {
                return Ok(());
            }
        }
        Err(helm_core::HelmError::Memory {
            addr,
            reason: "unmapped address".into(),
        })
    }
}

//! Translation Lookaside Buffer with ASID tagging and variable page sizes.

use crate::mmu::Permissions;
use helm_core::types::Addr;

/// A single TLB entry mapping a VA page/block to a PA page/block.
#[derive(Clone)]
pub struct TlbEntry {
    /// VA aligned to the block/page boundary.
    pub va_page: u64,
    /// PA aligned to the block/page boundary.
    pub pa_page: u64,
    /// Block/page size in bytes (4K, 2M, 1G, etc.).
    pub size: u64,
    /// Access permissions.
    pub perms: Permissions,
    /// MAIR attribute index.
    pub attr_indx: u32,
    /// Address Space Identifier.
    pub asid: u16,
    /// Global entry (matches any ASID).
    pub global: bool,
    /// Valid flag.
    valid: bool,
}

impl TlbEntry {
    fn empty() -> Self {
        Self {
            va_page: 0,
            pa_page: 0,
            size: 0,
            perms: Permissions { readable: false, writable: false, el1_executable: false, el0_executable: false },
            attr_indx: 0,
            asid: 0,
            global: false,
            valid: false,
        }
    }
}

/// ASID-tagged TLB with variable page sizes and round-robin eviction.
pub struct Tlb {
    entries: Vec<TlbEntry>,
    capacity: usize,
    next_evict: usize,
}

impl Tlb {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: vec![TlbEntry::empty(); capacity],
            capacity,
            next_evict: 0,
        }
    }

    /// Look up a VA in the TLB. Returns (PA, permissions) on hit.
    pub fn lookup(&self, va: Addr, asid: u16) -> Option<(Addr, Permissions)> {
        for e in &self.entries {
            if !e.valid {
                continue;
            }
            // Check ASID match (global entries match any ASID)
            if !e.global && e.asid != asid {
                continue;
            }
            // Check VA falls within this entry's page/block
            let offset = va.wrapping_sub(e.va_page);
            if offset < e.size {
                let pa = e.pa_page + offset;
                return Some((pa, e.perms));
            }
        }
        None
    }

    /// Insert a new TLB entry. Overwrites matching VA or evicts round-robin.
    pub fn insert(&mut self, entry: TlbEntry) {
        // Check if we already have an entry for this VA+ASID — overwrite it
        for e in &mut self.entries {
            if e.valid && e.va_page == entry.va_page && e.size == entry.size
                && (e.global == entry.global)
                && (entry.global || e.asid == entry.asid)
            {
                *e = entry;
                return;
            }
        }

        // Find an invalid slot first
        for e in &mut self.entries {
            if !e.valid {
                *e = entry;
                return;
            }
        }

        // Round-robin eviction
        let idx = self.next_evict % self.capacity;
        self.entries[idx] = entry;
        self.next_evict = idx + 1;
    }

    /// Create a valid TLB entry from walk result fields.
    pub fn make_entry(
        va: Addr,
        pa: Addr,
        size: u64,
        perms: Permissions,
        attr_indx: u32,
        asid: u16,
        global: bool,
    ) -> TlbEntry {
        let mask = !(size - 1);
        TlbEntry {
            va_page: va & mask,
            pa_page: pa & mask,
            size,
            perms,
            attr_indx,
            asid,
            global,
            valid: true,
        }
    }

    /// Flush all entries.
    pub fn flush_all(&mut self) {
        for e in &mut self.entries {
            e.valid = false;
        }
    }

    /// Flush non-global entries matching a specific ASID.
    pub fn flush_asid(&mut self, asid: u16) {
        for e in &mut self.entries {
            if e.valid && !e.global && e.asid == asid {
                e.valid = false;
            }
        }
    }

    /// Flush entries matching a specific VA (any ASID).
    pub fn flush_va(&mut self, va: Addr) {
        for e in &mut self.entries {
            if e.valid {
                let offset = va.wrapping_sub(e.va_page);
                if offset < e.size {
                    e.valid = false;
                }
            }
        }
    }

    /// Flush entries matching a specific VA and ASID.
    pub fn flush_va_asid(&mut self, va: Addr, asid: u16) {
        for e in &mut self.entries {
            if e.valid && (e.global || e.asid == asid) {
                let offset = va.wrapping_sub(e.va_page);
                if offset < e.size {
                    e.valid = false;
                }
            }
        }
    }
}

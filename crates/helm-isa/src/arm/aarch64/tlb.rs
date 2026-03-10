//! Translation Lookaside Buffer with ASID tagging and variable page sizes.
//!
//! Three-level structure:
//! - **Inline TLB**: 1024-entry direct-mapped, packed for JIT inline checks.
//!   Each entry is 24 bytes: `addr_read`, `addr_write`, `addend`.  The JIT
//!   emits a single compare + branch per memory access (QEMU-style fast path).
//! - **Fast TLB**: 1024-entry direct-mapped hash table for O(1) lookup of 4KB
//!   pages.  Stores a pre-computed `addend` (host_ptr − va_page) so the JIT
//!   can resolve VA → host address in a single add.
//! - **Slow TLB**: 256-entry fully-associative array with round-robin eviction,
//!   supporting variable page sizes (4K / 2M / 1G) and all ASID/VMID combinations.

use super::mmu::Permissions;
use helm_core::types::Addr;

// ── Inline (JIT-optimized) TLB ─────────────────────────────────────

pub const FAST_TLB_BITS: usize = 10;
pub const FAST_TLB_SIZE: usize = 1 << FAST_TLB_BITS;
pub const FAST_TLB_MASK: usize = FAST_TLB_SIZE - 1;

/// Sentinel tag value for invalid inline TLB entries.
/// Any value with bits above the 48-bit VA space works.
const INLINE_INVALID: u64 = !0u64;

/// Packed TLB entry optimized for JIT inline checks.
///
/// The JIT emits a single `cmp + brif` per memory access:
/// - **Load**: compare `addr_read` with `va >> 12`; on match, use `addend`.
/// - **Store**: compare `addr_write` with `va >> 12`; on match, use `addend`.
///
/// `addr_read` / `addr_write` are set to `va_page >> 12` when the entry is
/// valid for that access type (readable + has host backing, etc.), or
/// [`INLINE_INVALID`] otherwise.  This packs permission + validity + tag
/// into a single compare — matching QEMU's `CPUTLBEntry` design.
///
/// Layout: 24 bytes per entry.  1024 entries = 24 KB (fits in L1 cache).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct InlineTlbEntry {
    /// VA page tag for read access, or `!0` if not readable/not backed.
    pub addr_read: u64,
    /// VA page tag for write access, or `!0` if not writable/not backed.
    pub addr_write: u64,
    /// Pre-computed host addend: `host_ptr as isize - va_page as isize`.
    pub addend: isize,
}

impl InlineTlbEntry {
    /// Size of one entry in bytes (for JIT offset calculations).
    pub const SIZE: usize = 24;

    const EMPTY: Self = Self {
        addr_read: INLINE_INVALID,
        addr_write: INLINE_INVALID,
        addend: 0,
    };
}

// ── Fast (direct-mapped) TLB ────────────────────────────────────────

/// Direct-mapped fast TLB entry for 4KB pages.
///
/// Indexed by `(va >> 12) & FAST_TLB_MASK`.  `va_tag == 0` means invalid.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct FastTlbEntry {
    /// VA page number (VA >> 12).  0 = invalid sentinel.
    pub va_tag: u64,
    /// PA page base (PA & !0xFFF).
    pub pa_page: u64,
    /// Pre-computed host addend: `host_ptr as isize - va_page as isize`.
    /// Adding `va` to this yields the host address directly.
    /// 0 if the page is not RAM-backed (IO).
    pub addend: isize,
    /// ASID tag.
    pub asid: u16,
    /// Global flag (matches any ASID).
    pub global: bool,
    /// Read permission.
    pub perm_read: bool,
    /// Write permission.
    pub perm_write: bool,
    /// EL0 execute permission.
    pub perm_el0_exec: bool,
    /// EL1 execute permission.
    pub perm_el1_exec: bool,
    /// True if `addend` is valid (page has a host backing).
    pub has_addend: bool,
}

impl FastTlbEntry {
    const EMPTY: Self = Self {
        va_tag: 0,
        pa_page: 0,
        addend: 0,
        asid: 0,
        global: false,
        perm_read: false,
        perm_write: false,
        perm_el0_exec: false,
        perm_el1_exec: false,
        has_addend: false,
    };
}

// ── Slow (fully-associative) TLB ────────────────────────────────────

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
    /// Virtual Machine Identifier (from VTTBR_EL2).
    pub vmid: u16,
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
            perms: Permissions {
                readable: false,
                writable: false,
                el1_executable: false,
                el0_executable: false,
            },
            attr_indx: 0,
            asid: 0,
            vmid: 0,
            global: false,
            valid: false,
        }
    }
}

// ── Combined TLB ────────────────────────────────────────────────────

/// Three-level TLB: inline (JIT) + fast direct-mapped + slow fully-associative.
pub struct Tlb {
    entries: Vec<TlbEntry>,
    capacity: usize,
    next_evict: usize,
    /// Fast direct-mapped TLB for 4KB pages (O(1) lookup).
    pub fast_entries: Box<[FastTlbEntry; FAST_TLB_SIZE]>,
    /// Packed inline TLB for JIT-generated code (1 compare per access).
    pub inline_entries: Box<[InlineTlbEntry; FAST_TLB_SIZE]>,
}

impl Tlb {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: vec![TlbEntry::empty(); capacity],
            capacity,
            next_evict: 0,
            fast_entries: Box::new([FastTlbEntry::EMPTY; FAST_TLB_SIZE]),
            inline_entries: Box::new([InlineTlbEntry::EMPTY; FAST_TLB_SIZE]),
        }
    }

    // ── Fast TLB operations ─────────────────────────────────────────

    /// O(1) lookup in the direct-mapped fast TLB.
    /// Returns `(PA, Permissions)` on hit, `None` on miss.
    #[inline(always)]
    pub fn lookup_fast(&self, va: Addr, asid: u16) -> Option<(Addr, Permissions)> {
        let va_tag = va >> 12;
        let idx = (va_tag as usize) & FAST_TLB_MASK;
        let e = unsafe { self.fast_entries.get_unchecked(idx) };
        if e.va_tag == va_tag && (e.global || e.asid == asid) {
            let pa = e.pa_page | (va & 0xFFF);
            let perms = Permissions {
                readable: e.perm_read,
                writable: e.perm_write,
                el0_executable: e.perm_el0_exec,
                el1_executable: e.perm_el1_exec,
            };
            return Some((pa, perms));
        }
        None
    }

    /// Insert a 4KB entry into the fast TLB with a pre-computed addend.
    ///
    /// `host_ptr` is the host pointer to the start of the PA page (from
    /// `AddressSpace::host_ptr_for_pa`).  If `None`, the entry is still
    /// inserted but `has_addend` will be false (IO page).
    ///
    /// Also populates the corresponding [`InlineTlbEntry`] for JIT use.
    pub fn insert_fast(&mut self, entry: &TlbEntry, host_ptr: Option<*mut u8>) {
        if entry.size != 4096 || !entry.valid {
            return;
        }
        let va_page = entry.va_page;
        let va_tag = va_page >> 12;
        let idx = (va_tag as usize) & FAST_TLB_MASK;
        let addend = host_ptr.map(|p| p as isize - va_page as isize).unwrap_or(0);
        let has_backing = host_ptr.is_some();
        self.fast_entries[idx] = FastTlbEntry {
            va_tag,
            pa_page: entry.pa_page,
            addend,
            asid: entry.asid,
            global: entry.global,
            perm_read: entry.perms.readable,
            perm_write: entry.perms.writable,
            perm_el0_exec: entry.perms.el0_executable,
            perm_el1_exec: entry.perms.el1_executable,
            has_addend: has_backing,
        };
        // Populate packed inline entry for JIT fast path.
        // addr_read/addr_write = va_tag only if permission + host backing;
        // otherwise INLINE_INVALID so the JIT compare always misses.
        self.inline_entries[idx] = InlineTlbEntry {
            addr_read: if entry.perms.readable && has_backing {
                va_tag
            } else {
                INLINE_INVALID
            },
            addr_write: if entry.perms.writable && has_backing {
                va_tag
            } else {
                INLINE_INVALID
            },
            addend,
        };
    }

    /// Invalidate all fast + inline TLB entries.
    fn flush_fast_all(&mut self) {
        for e in self.fast_entries.iter_mut() {
            e.va_tag = 0;
        }
        for e in self.inline_entries.iter_mut() {
            *e = InlineTlbEntry::EMPTY;
        }
    }

    /// Invalidate the fast + inline TLB entry matching a VA (if any).
    fn flush_fast_va(&mut self, va: Addr) {
        let va_tag = va >> 12;
        let idx = (va_tag as usize) & FAST_TLB_MASK;
        if self.fast_entries[idx].va_tag == va_tag {
            self.fast_entries[idx].va_tag = 0;
        }
        // Inline: always invalidate at this index (conservative).
        self.inline_entries[idx] = InlineTlbEntry::EMPTY;
    }

    /// Invalidate fast + inline TLB entries matching a specific ASID (non-global only).
    fn flush_fast_asid(&mut self, asid: u16) {
        for (i, e) in self.fast_entries.iter_mut().enumerate() {
            if e.va_tag != 0 && !e.global && e.asid == asid {
                e.va_tag = 0;
                self.inline_entries[i] = InlineTlbEntry::EMPTY;
            }
        }
    }

    // ── Slow TLB operations ─────────────────────────────────────────

    /// Look up a VA in the slow TLB. Returns (PA, permissions) on hit.
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

    /// Insert a new TLB entry into the slow TLB.
    /// Overwrites matching VA or evicts round-robin.
    pub fn insert(&mut self, entry: TlbEntry) {
        // Check if we already have an entry for this VA+ASID — overwrite it
        for e in &mut self.entries {
            if e.valid
                && e.va_page == entry.va_page
                && e.size == entry.size
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
            vmid: 0,
            global,
            valid: true,
        }
    }

    /// Create a valid TLB entry tagged with a VMID (for stage-2 or VM-aware caching).
    pub fn make_entry_vmid(
        va: Addr,
        pa: Addr,
        size: u64,
        perms: Permissions,
        attr_indx: u32,
        asid: u16,
        vmid: u16,
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
            vmid,
            global,
            valid: true,
        }
    }

    /// Flush all entries (fast + slow).
    pub fn flush_all(&mut self) {
        for e in &mut self.entries {
            e.valid = false;
        }
        self.flush_fast_all();
    }

    /// Flush non-global entries matching a specific ASID (fast + slow).
    pub fn flush_asid(&mut self, asid: u16) {
        for e in &mut self.entries {
            if e.valid && !e.global && e.asid == asid {
                e.valid = false;
            }
        }
        self.flush_fast_asid(asid);
    }

    /// Flush entries matching a specific VA (fast + slow).
    pub fn flush_va(&mut self, va: Addr) {
        for e in &mut self.entries {
            if e.valid {
                let offset = va.wrapping_sub(e.va_page);
                if offset < e.size {
                    e.valid = false;
                }
            }
        }
        self.flush_fast_va(va);
    }

    /// Flush entries matching a specific VA and ASID (fast + slow).
    pub fn flush_va_asid(&mut self, va: Addr, asid: u16) {
        for e in &mut self.entries {
            if e.valid && (e.global || e.asid == asid) {
                let offset = va.wrapping_sub(e.va_page);
                if offset < e.size {
                    e.valid = false;
                }
            }
        }
        // Fast TLB: invalidate if tag matches (don't check ASID — conservative)
        self.flush_fast_va(va);
    }

    /// Flush all entries matching a specific VMID (fast + inline + slow).
    pub fn flush_vmid(&mut self, vmid: u16) {
        for e in &mut self.entries {
            if e.valid && e.vmid == vmid {
                e.valid = false;
            }
        }
        // Fast/inline TLBs don't track VMID — flush everything conservatively
        if vmid == 0 {
            // VMID 0 is the default — many entries could match
            self.flush_fast_all();
        }
    }
}

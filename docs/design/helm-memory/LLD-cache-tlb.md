# helm-memory — LLD: Cache Model and TLB

> **Status:** Draft — Phase 1 target
> **Covers:** `CacheModel`, `CacheSet`, `CacheLine`, MSHR, `TlbModel`, `TlbEntry`, page table walkers

---

## 1. Cache Model

### 1.1 `CacheConfig`

Immutable configuration. Loaded from Python params at elaborate time.

```rust
/// Configuration for one cache level (L1I, L1D, L2, LLC).
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Total cache size in kilobytes.
    pub size_kb: u32,
    /// Number of ways (associativity). Must be a power of two.
    pub assoc: u32,
    /// Cache line size in bytes. Typical: 64.
    pub line_size: u32,
    /// Hit latency in cycles (returned by `lookup` on a hit).
    pub hit_latency: u64,
    /// Number of Miss Status Holding Registers (MSHRs).
    /// Limits outstanding misses. Typical: 8–32 (Q34).
    pub mshrs: u32,
    /// Q31: True = write-back (dirty bit, evict on replacement).
    ///       False = write-through (no dirty bit, write propagates immediately).
    pub write_back: bool,
}

impl CacheConfig {
    /// Number of sets = size_bytes / (assoc * line_size).
    pub fn num_sets(&self) -> u32 {
        (self.size_kb * 1024) / (self.assoc * self.line_size)
    }

    /// Number of bits for the block offset within a line.
    pub fn offset_bits(&self) -> u32 {
        self.line_size.trailing_zeros()
    }

    /// Number of bits for the set index.
    pub fn index_bits(&self) -> u32 {
        self.num_sets().trailing_zeros()
    }

    /// Number of tag bits = 64 - index_bits - offset_bits.
    pub fn tag_bits(&self) -> u32 {
        64 - self.index_bits() - self.offset_bits()
    }
}
```

### 1.2 `CacheLine`

```rust
/// One cache line (one way within a set).
#[derive(Debug, Clone)]
pub struct CacheLine {
    /// Tag bits for address matching.
    pub tag: u64,
    /// Line is valid (contains data).
    pub valid: bool,
    /// Line has been written since fill (write-back model, Q31).
    pub dirty: bool,
    /// Actual cached bytes. Length = `CacheConfig::line_size`.
    pub data: Vec<u8>,
}

impl CacheLine {
    pub fn new(line_size: u32) -> Self {
        CacheLine {
            tag: 0,
            valid: false,
            dirty: false,
            data: vec![0u8; line_size as usize],
        }
    }

    pub fn invalidate(&mut self) {
        self.valid = false;
        self.dirty = false;
    }
}
```

### 1.3 `CacheSet` with Pseudo-LRU

Q30: Pseudo-LRU (PLRU) via a binary tree of bits. For an N-way set, `N-1` bits represent a tournament tree. Each access updates the tree in O(1) to point away from the most recently used way. On eviction, follow the tree to find the least-recently-used way.

```rust
/// One set of an N-way set-associative cache.
pub struct CacheSet {
    /// N cache lines, one per way.
    pub ways: Vec<CacheLine>,
    /// PLRU tree state. For N ways, N-1 bits are used.
    /// Stored as a u64 bitmask (supports up to 64-way associativity).
    /// Bit interpretation: bit i=1 means "prefer the right subtree" at node i.
    plru_bits: u64,
}

impl CacheSet {
    pub fn new(assoc: u32, line_size: u32) -> Self {
        CacheSet {
            ways: (0..assoc).map(|_| CacheLine::new(line_size)).collect(),
            plru_bits: 0,
        }
    }

    /// Find the way index for `tag`, or `None` on miss.
    pub fn lookup_way(&self, tag: u64) -> Option<usize> {
        self.ways.iter().position(|w| w.valid && w.tag == tag)
    }

    /// Update PLRU tree to record that `way` was most recently used.
    /// Traverses from the root of the PLRU binary tree to the leaf for `way`,
    /// flipping each bit to point away from the path taken.
    pub fn touch(&mut self, way: usize) {
        let n = self.ways.len();
        let mut node = 0usize;
        let mut remaining = n;
        let mut way_idx = way;
        while remaining > 1 {
            let half = remaining / 2;
            if way_idx < half {
                // went left — set bit to prefer right (point away from used)
                self.plru_bits |= 1 << node;
                node = 2 * node + 1; // left child
                remaining = half;
            } else {
                // went right — clear bit to prefer left (point away from used)
                self.plru_bits &= !(1 << node);
                node = 2 * node + 2; // right child
                way_idx -= half;
                remaining -= half;
            }
        }
    }

    /// Find the victim way for eviction using PLRU.
    /// Follows the tree bits from root to leaf; each bit points toward the LRU subtree.
    pub fn plru_victim(&self) -> usize {
        let n = self.ways.len();
        let mut node = 0usize;
        let mut remaining = n;
        let mut victim = 0usize;
        while remaining > 1 {
            let half = remaining / 2;
            // Bit=1 means "right is LRU" (we prefer right = go right to find victim).
            // Bit=0 means "left is LRU".
            if (self.plru_bits >> node) & 1 == 0 {
                // prefer left = LRU is on the left
                node = 2 * node + 1;
                remaining = half;
            } else {
                victim += half;
                node = 2 * node + 2;
                remaining -= half;
            }
        }
        victim
    }

    /// Evict the PLRU victim and fill with new data.
    /// Returns the evicted line (caller must writeback if dirty and write-back policy).
    pub fn fill(&mut self, tag: u64, data: Vec<u8>) -> Option<CacheLine> {
        // Prefer an invalid way first.
        let victim = self.ways.iter().position(|w| !w.valid)
            .unwrap_or_else(|| self.plru_victim());
        let evicted = if self.ways[victim].valid {
            Some(self.ways[victim].clone())
        } else {
            None
        };
        self.ways[victim] = CacheLine { tag, valid: true, dirty: false, data };
        self.touch(victim);
        evicted
    }
}
```

### 1.4 `CacheLookupResult`

```rust
/// Result of a cache lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheLookupResult {
    /// Hit. Contains the hit latency in cycles.
    Hit(u64),
    /// Miss at this cache level. Must be forwarded to next level.
    /// `level` is the level that missed (0 = L1, 1 = L2, …).
    Miss { level: usize },
    /// MSHR capacity exceeded — too many outstanding misses.
    /// Caller must stall until an MSHR is freed.
    MshrFull { addr: u64 },
}
```

### 1.5 `CacheStats`

```rust
/// Accumulated statistics for one cache level.
#[derive(Debug, Default, Clone)]
pub struct CacheStats {
    pub reads:       u64,
    pub writes:      u64,
    pub read_hits:   u64,
    pub write_hits:  u64,
    pub read_misses: u64,
    pub write_misses:u64,
    pub evictions:   u64,
    pub writebacks:  u64,
}

impl CacheStats {
    pub fn hit_rate(&self) -> f64 {
        let total = self.reads + self.writes;
        if total == 0 { return 0.0; }
        (self.read_hits + self.write_hits) as f64 / total as f64
    }
}
```

### 1.6 `CacheModel`

```rust
/// One level of a set-associative cache.
///
/// Q32: Cache state persists between Interval timing intervals.
/// Q33: Shared LLC is wrapped in Arc<Mutex<CacheModel>> by the caller.
pub struct CacheModel {
    pub config: CacheConfig,
    sets: Vec<CacheSet>,
    pub stats: CacheStats,
    mshr: MshrFile,
    /// Next-level cache or None if this is the LLC.
    next_level: Option<Arc<Mutex<CacheModel>>>,
}

impl CacheModel {
    pub fn new(config: CacheConfig, next_level: Option<Arc<Mutex<CacheModel>>>) -> Self {
        let num_sets = config.num_sets() as usize;
        CacheModel {
            sets: (0..num_sets).map(|_| CacheSet::new(config.assoc, config.line_size)).collect(),
            stats: CacheStats::default(),
            mshr: MshrFile::new(config.mshrs as usize),
            next_level,
            config,
        }
    }

    fn set_index(&self, addr: u64) -> usize {
        ((addr >> self.config.offset_bits()) as usize) & (self.config.num_sets() as usize - 1)
    }

    fn tag(&self, addr: u64) -> u64 {
        addr >> (self.config.offset_bits() + self.config.index_bits())
    }

    fn line_base(&self, addr: u64) -> u64 {
        addr & !((self.config.line_size as u64) - 1)
    }

    /// Read `width` bytes from `addr`.
    ///
    /// Returns `CacheLookupResult::Hit(latency)` on hit.
    /// Returns `CacheLookupResult::Miss` on miss, and allocates an MSHR.
    /// Returns `CacheLookupResult::MshrFull` if no MSHR slot available.
    pub fn read(&mut self, addr: u64, width: usize) -> CacheLookupResult {
        self.stats.reads += 1;
        let set_idx = self.set_index(addr);
        let tag = self.tag(addr);
        if let Some(way) = self.sets[set_idx].lookup_way(tag) {
            self.sets[set_idx].touch(way);
            self.stats.read_hits += 1;
            CacheLookupResult::Hit(self.config.hit_latency)
        } else {
            self.stats.read_misses += 1;
            let line_base = self.line_base(addr);
            if self.mshr.is_pending(line_base) {
                // MSHR merge: already outstanding miss for this line.
                return CacheLookupResult::Miss { level: 0 };
            }
            match self.mshr.allocate(line_base) {
                Ok(()) => CacheLookupResult::Miss { level: 0 },
                Err(()) => CacheLookupResult::MshrFull { addr },
            }
        }
    }

    /// Write `width` bytes to `addr`.
    ///
    /// Write-back: marks the line dirty if present. On miss, allocates MSHR
    /// (write-allocate policy assumed for write-back caches).
    pub fn write(&mut self, addr: u64, width: usize, data: &[u8]) -> CacheLookupResult {
        self.stats.writes += 1;
        let set_idx = self.set_index(addr);
        let tag = self.tag(addr);
        if let Some(way) = self.sets[set_idx].lookup_way(tag) {
            self.sets[set_idx].touch(way);
            self.sets[set_idx].ways[way].dirty = self.config.write_back;
            self.stats.write_hits += 1;
            CacheLookupResult::Hit(self.config.hit_latency)
        } else {
            self.stats.write_misses += 1;
            let line_base = self.line_base(addr);
            match self.mshr.allocate(line_base) {
                Ok(()) => CacheLookupResult::Miss { level: 0 },
                Err(()) => CacheLookupResult::MshrFull { addr },
            }
        }
    }

    /// Fill a cache line after a miss is satisfied. Frees the MSHR for `addr`.
    ///
    /// If the evicted line is dirty, the caller must writeback to next level.
    pub fn fill_line(&mut self, addr: u64, data: Vec<u8>) -> Option<(u64, Vec<u8>)> {
        let line_base = self.line_base(addr);
        let set_idx = self.set_index(addr);
        let tag = self.tag(addr);
        self.mshr.free(line_base);
        if let Some(evicted) = self.sets[set_idx].fill(tag, data) {
            if evicted.dirty {
                self.stats.writebacks += 1;
                // Reconstruct the evicted address from tag + set index.
                let evicted_addr = (evicted.tag << (self.config.offset_bits() + self.config.index_bits()))
                    | ((set_idx as u64) << self.config.offset_bits());
                return Some((evicted_addr, evicted.data));
            }
            self.stats.evictions += 1;
        }
        None
    }

    /// Invalidate the cache line containing `addr` (used by MemoryListener).
    pub fn invalidate_line(&mut self, addr: u64) {
        let set_idx = self.set_index(addr);
        let tag = self.tag(addr);
        if let Some(way) = self.sets[set_idx].lookup_way(tag) {
            self.sets[set_idx].ways[way].invalidate();
        }
    }
}
```

---

## 2. MSHR (Miss Status Holding Registers)

Q34: MSHRs are modeled per cache level. Capacity is enforced; excess misses stall.

```rust
/// Tracks outstanding cache misses (one entry per in-flight cache line fetch).
///
/// Capacity is `CacheConfig::mshrs`. Attempting to allocate when full
/// returns `Err(())`, which propagates as `CacheLookupResult::MshrFull`.
pub struct MshrFile {
    /// Set of cache-line-aligned addresses with outstanding fetches.
    pending: HashSet<u64>,
    /// Maximum number of concurrent outstanding misses.
    capacity: usize,
}

impl MshrFile {
    pub fn new(capacity: usize) -> Self {
        MshrFile { pending: HashSet::new(), capacity }
    }

    /// True if a fetch is already in progress for `line_addr`.
    pub fn is_pending(&self, line_addr: u64) -> bool {
        self.pending.contains(&line_addr)
    }

    /// Allocate an MSHR entry. Returns `Err(())` if at capacity.
    pub fn allocate(&mut self, line_addr: u64) -> Result<(), ()> {
        if self.pending.len() >= self.capacity {
            return Err(());
        }
        self.pending.insert(line_addr);
        Ok(())
    }

    /// Free the MSHR entry when a fetch completes.
    pub fn free(&mut self, line_addr: u64) {
        self.pending.remove(&line_addr);
    }

    /// Current number of outstanding misses.
    pub fn outstanding(&self) -> usize {
        self.pending.len()
    }
}
```

---

## 3. TLB Model

### 3.1 `PageSize`

```rust
/// Supported page sizes for both RISC-V and AArch64.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSize {
    /// 4 KiB — standard page.
    Page4K,
    /// 2 MiB — Sv39 megapage / AArch64 2MB block (Q37).
    Page2M,
    /// 1 GiB — Sv39 gigapage / AArch64 1GB block (Q37).
    Page1G,
}

impl PageSize {
    pub fn bytes(self) -> u64 {
        match self {
            Self::Page4K => 4 * 1024,
            Self::Page2M => 2 * 1024 * 1024,
            Self::Page1G => 1024 * 1024 * 1024,
        }
    }

    pub fn offset_mask(self) -> u64 {
        self.bytes() - 1
    }
}
```

### 3.2 `TlbEntry`

```rust
/// One TLB entry: maps a virtual page number to a physical page number.
///
/// For RISC-V: `flags` is the raw PTE flags byte (V, R, W, X, U, G, A, D).
/// For AArch64: `flags` encodes AP[2:1], XN, UXN, AF, nG bits.
#[derive(Debug, Clone)]
pub struct TlbEntry {
    /// Virtual page number (VPN), right-shifted by page offset bits.
    /// For a 4KB page: `vpn = va >> 12`.
    /// For a 2MB page: `vpn = va >> 21`.
    /// For a 1GB page: `vpn = va >> 30`.
    pub vpn: u64,
    /// Physical page number (PPN), same granularity as vpn.
    pub ppn: u64,
    /// ISA-specific permission and attribute flags.
    pub flags: u64,
    /// Address Space Identifier (ASID). Q35: used for SFENCE.VMA isolation.
    pub asid: u16,
    /// Page size mapped by this entry (Q37).
    pub size: PageSize,
    /// True for global entries (G bit set); global entries are not flushed on ASID-specific SFENCE.VMA.
    pub global: bool,
}

impl TlbEntry {
    pub fn is_readable(&self)    -> bool { self.flags & 0b0010 != 0 }
    pub fn is_writable(&self)    -> bool { self.flags & 0b0100 != 0 }
    pub fn is_executable(&self)  -> bool { self.flags & 0b1000 != 0 }
    pub fn is_user(&self)        -> bool { self.flags & 0b0001_0000 != 0 }
    pub fn is_global(&self)      -> bool { self.global }
    pub fn is_accessed(&self)    -> bool { self.flags & 0b0100_0000 != 0 }
    pub fn is_dirty(&self)       -> bool { self.flags & 0b1000_0000 != 0 }
}
```

### 3.3 `TlbConfig`

```rust
#[derive(Debug, Clone)]
pub struct TlbConfig {
    /// Total number of TLB entries.
    pub entries: u32,
    /// Associativity (ways). Fully associative = entries.
    pub assoc: u32,
    /// Supported page sizes (subset of `PageSize` variants).
    pub page_sizes: Vec<PageSize>,
}
```

### 3.4 `TlbModel`

```rust
/// Per-hart TLB. ASID-aware, huge-page aware.
///
/// Internally organized as a set-associative structure indexed by VPN.
/// All four SFENCE.VMA variants are supported (Q35).
pub struct TlbModel {
    config: TlbConfig,
    /// Sets of entries: `sets[vpn % num_sets]` contains `assoc` entries.
    sets: Vec<Vec<Option<TlbEntry>>>,
    /// Pseudo-LRU replacement per set (same PLRU tree as CacheSet).
    plru: Vec<u64>,
    /// Hit counter.
    pub hits: u64,
    /// Miss counter.
    pub misses: u64,
}

impl TlbModel {
    pub fn new(config: TlbConfig) -> Self {
        let num_sets = (config.entries / config.assoc) as usize;
        TlbModel {
            sets: vec![vec![None; config.assoc as usize]; num_sets],
            plru: vec![0u64; num_sets],
            hits: 0,
            misses: 0,
            config,
        }
    }

    fn num_sets(&self) -> usize {
        self.sets.len()
    }

    fn set_index(&self, vpn: u64) -> usize {
        (vpn as usize) % self.num_sets()
    }

    /// Translate virtual address `va` using ASID `asid`.
    ///
    /// Returns the physical address on success.
    /// Returns `Err(PageFault { va, level: 0 })` on TLB miss (caller must page-walk).
    pub fn translate(&mut self, va: u64, asid: u16, access: AccessType)
        -> Result<u64, MemFault>
    {
        // Try each supported page size from largest to smallest.
        // A hit on a larger page preempts a hit on a smaller one (standard TLB behavior).
        for &page_size in self.config.page_sizes.iter().rev() {
            let vpn = va >> page_size.bytes().trailing_zeros();
            let set_idx = self.set_index(vpn);
            let set = &self.sets[set_idx];
            for (way, entry) in set.iter().enumerate() {
                if let Some(e) = entry {
                    if e.vpn == vpn && (e.asid == asid || e.global) && e.size == page_size {
                        // Permission check.
                        match access {
                            AccessType::Read    if !e.is_readable()   => {
                                return Err(MemFault::PageFault { va, level: 0 });
                            }
                            AccessType::Write   if !e.is_writable()   => {
                                return Err(MemFault::PageFault { va, level: 0 });
                            }
                            AccessType::Execute if !e.is_executable() => {
                                return Err(MemFault::PageFault { va, level: 0 });
                            }
                            _ => {}
                        }
                        self.hits += 1;
                        // Update PLRU.
                        // (Simplified: reuse CacheSet PLRU logic by trait or copy.)
                        let pa_base = e.ppn << page_size.bytes().trailing_zeros();
                        let offset  = va & page_size.offset_mask();
                        return Ok(pa_base | offset);
                    }
                }
            }
        }
        self.misses += 1;
        Err(MemFault::PageFault { va, level: 0 })
    }

    /// Insert a new TLB entry (called after a successful page walk).
    pub fn insert(&mut self, entry: TlbEntry) {
        let vpn = entry.vpn;
        let set_idx = self.set_index(vpn);
        // Find an empty slot or evict via PLRU.
        let way = self.sets[set_idx].iter().position(|e| e.is_none())
            .unwrap_or_else(|| plru_victim(self.plru[set_idx], self.config.assoc as usize));
        self.sets[set_idx][way] = Some(entry);
        self.plru[set_idx] = plru_touch(self.plru[set_idx], way, self.config.assoc as usize);
    }

    // -- SFENCE.VMA variants (Q35) --

    /// Flush all entries (SFENCE.VMA x0, x0 — global flush).
    pub fn flush_all(&mut self) {
        for set in &mut self.sets {
            for entry in set.iter_mut() {
                *entry = None;
            }
        }
    }

    /// Flush all entries for a specific ASID (SFENCE.VMA x0, rs2 — ASID flush).
    /// Global entries (G bit set) are NOT flushed.
    pub fn flush_asid(&mut self, asid: u16) {
        for set in &mut self.sets {
            for entry in set.iter_mut() {
                if let Some(e) = entry {
                    if e.asid == asid && !e.global {
                        *entry = None;
                    }
                }
            }
        }
    }

    /// Flush all entries covering virtual address `va` (SFENCE.VMA rs1, x0 — VA flush).
    pub fn flush_va(&mut self, va: u64) {
        for &page_size in &self.config.page_sizes {
            let vpn = va >> page_size.bytes().trailing_zeros();
            let set_idx = self.set_index(vpn);
            for entry in &mut self.sets[set_idx] {
                if let Some(e) = entry {
                    if e.vpn == vpn && e.size == page_size {
                        *entry = None;
                    }
                }
            }
        }
    }

    /// Flush entries for a specific ASID + VA (SFENCE.VMA rs1, rs2 — ASID+VA flush).
    /// Global entries are NOT flushed.
    pub fn flush_asid_va(&mut self, asid: u16, va: u64) {
        for &page_size in &self.config.page_sizes {
            let vpn = va >> page_size.bytes().trailing_zeros();
            let set_idx = self.set_index(vpn);
            for entry in &mut self.sets[set_idx] {
                if let Some(e) = entry {
                    if e.vpn == vpn && e.asid == asid && !e.global && e.size == page_size {
                        *entry = None;
                    }
                }
            }
        }
    }
}

/// Standalone PLRU helpers (shared with CacheSet).
fn plru_victim(bits: u64, n: usize) -> usize { todo!() }
fn plru_touch(bits: u64, way: usize, n: usize) -> u64 { todo!() }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessType {
    Read,
    Write,
    Execute,
}
```

---

## 4. Page Table Walker

### 4.1 Design (Q36)

The page table walker is a **function**, not a hardware component. It is called on TLB miss, uses `FunctionalMem` to read PTEs (bypassing cache/TLB side effects, matching hardware behavior where PTW uses a separate TLB and fill buffer). The function returns a `TlbEntry` on success, or a `MemFault::PageFault` on failure.

### 4.2 RISC-V Sv39 Walker

Sv39: 3-level page table, 39-bit virtual address space, 4KB base pages, 2MB and 1GB huge pages (Q37).

```rust
/// RISC-V Sv39 page table walk.
///
/// `satp_ppn`: PPN of the root page table (from `satp` CSR bits [43:0]).
/// `va`:       Virtual address to translate (39 bits used: [38:0]).
/// `asid`:     ASID from `satp` CSR bits [59:44].
/// `access`:   Read, Write, or Execute (determines PTE permission check).
/// `mem`:      Functional memory access (reads PTEs without cache side effects).
///
/// Returns `TlbEntry` on success. Returns `MemFault::PageFault { va, level }`
/// where `level` is the page table level at which translation failed (0=L1, 1=L2, 2=L3).
pub fn sv39_walk(
    satp_ppn: u64,
    va: u64,
    asid: u16,
    access: AccessType,
    mem: &dyn FunctionalMem,
) -> Result<TlbEntry, MemFault> {
    // Sv39 VPN decomposition:
    //   va[38:30] = VPN[2] (9 bits, level 2 index)
    //   va[29:21] = VPN[1] (9 bits, level 1 index)
    //   va[20:12] = VPN[0] (9 bits, level 0 index)
    //   va[11:0]  = page offset
    let vpn = [
        (va >> 12) & 0x1FF,  // VPN[0]
        (va >> 21) & 0x1FF,  // VPN[1]
        (va >> 30) & 0x1FF,  // VPN[2]
    ];

    let mut pt_ppn = satp_ppn;

    for level in (0..3usize).rev() {
        // Physical address of PTE = pt_ppn * 4096 + vpn[level] * 8.
        let pte_addr = (pt_ppn << 12) + (vpn[level] << 3);
        let pte = mem.read_u64(pte_addr)
            .map_err(|_| MemFault::PageFault { va, level: level as u8 })?;

        // PTE fields: V[0], R[1], W[2], X[3], U[4], G[5], A[6], D[7], RSW[9:8], PPN[53:10]
        let v = pte & 1 != 0;
        let r = (pte >> 1) & 1 != 0;
        let w = (pte >> 2) & 1 != 0;
        let x = (pte >> 3) & 1 != 0;

        if !v || (!r && w) {
            // Invalid PTE or reserved encoding (W=1, R=0).
            return Err(MemFault::PageFault { va, level: level as u8 });
        }

        if r || x {
            // Leaf PTE: this is the page (or huge page).
            // Check permissions.
            match access {
                AccessType::Read    if !r => return Err(MemFault::PageFault { va, level: level as u8 }),
                AccessType::Write   if !w => return Err(MemFault::PageFault { va, level: level as u8 }),
                AccessType::Execute if !x => return Err(MemFault::PageFault { va, level: level as u8 }),
                _ => {}
            }

            let leaf_ppn = (pte >> 10) & 0x0FFF_FFFF_FFFF;

            // Q37: Huge page detection.
            // At level 2 (Sv39 gigapage), lower VPN fields of PPN must be zero.
            // At level 1 (megapage), VPN[0] bits of PPN must be zero.
            // If not: misaligned superpage — fault.
            if level == 2 {
                if leaf_ppn & 0x3_FFFF != 0 {
                    return Err(MemFault::PageFault { va, level: 2 });
                }
                let vpn_full = va >> 30; // gigapage VPN
                return Ok(TlbEntry {
                    vpn: vpn_full,
                    ppn: leaf_ppn >> 18,
                    flags: pte & 0xFF,
                    asid,
                    size: PageSize::Page1G,
                    global: (pte >> 5) & 1 != 0,
                });
            } else if level == 1 {
                if leaf_ppn & 0x1FF != 0 {
                    return Err(MemFault::PageFault { va, level: 1 });
                }
                let vpn_full = va >> 21; // megapage VPN
                return Ok(TlbEntry {
                    vpn: vpn_full,
                    ppn: leaf_ppn >> 9,
                    flags: pte & 0xFF,
                    asid,
                    size: PageSize::Page2M,
                    global: (pte >> 5) & 1 != 0,
                });
            } else {
                // 4KB leaf.
                return Ok(TlbEntry {
                    vpn: va >> 12,
                    ppn: leaf_ppn,
                    flags: pte & 0xFF,
                    asid,
                    size: PageSize::Page4K,
                    global: (pte >> 5) & 1 != 0,
                });
            }
        }

        // Non-leaf: pointer PTE — descend to next level.
        pt_ppn = (pte >> 10) & 0x0FFF_FFFF_FFFF;
    }

    // Exhausted all levels without a leaf — fault.
    Err(MemFault::PageFault { va, level: 0 })
}
```

### 4.3 AArch64 4KB 4-Level Page Walker

AArch64 with 4KB granule: L0→L1→L2→L3 (48-bit VA), each level indexes 512 entries.

```rust
/// AArch64 4-level page table walk (4KB granule, 48-bit VA).
///
/// `ttbr_pa`: Physical address of the Level-0 translation table (from TTBR0/TTBR1).
/// `va`:      Virtual address (48 bits used).
/// `access`:  Read, Write, or Execute.
/// `mem`:     Functional memory access for PTE reads.
///
/// Supports block descriptors at L1 (1GB) and L2 (2MB) — huge pages (Q37).
pub fn aarch64_4k_walk(
    ttbr_pa: u64,
    va: u64,
    access: AccessType,
    mem: &dyn FunctionalMem,
) -> Result<TlbEntry, MemFault> {
    // 4KB granule indices (9 bits each level):
    //   L0: va[47:39], L1: va[38:30], L2: va[29:21], L3: va[20:12]
    let indices = [
        (va >> 39) & 0x1FF, // L0
        (va >> 30) & 0x1FF, // L1
        (va >> 21) & 0x1FF, // L2
        (va >> 12) & 0x1FF, // L3
    ];

    let mut table_pa = ttbr_pa;

    for level in 0..4usize {
        let desc_addr = table_pa + (indices[level] << 3);
        let desc = mem.read_u64(desc_addr)
            .map_err(|_| MemFault::PageFault { va, level: level as u8 })?;

        // Bits [1:0]: 0b00 = invalid, 0b01 = block (L1/L2 only), 0b11 = table/page.
        let valid = desc & 1 != 0;
        if !valid {
            return Err(MemFault::PageFault { va, level: level as u8 });
        }

        let is_table_or_page = (desc >> 1) & 1 != 0;

        if !is_table_or_page {
            // Block descriptor at L1 or L2.
            // L1 block = 1GB, L2 block = 2MB.
            if level == 0 {
                return Err(MemFault::PageFault { va, level: 0 }); // invalid at L0
            }
            let (page_size, vpn_shift) = if level == 1 {
                (PageSize::Page1G, 30u32)
            } else {
                (PageSize::Page2M, 21u32)
            };
            check_aarch64_perms(desc, access, va, level as u8)?;
            let oa_bits = output_address_bits(desc, vpn_shift);
            return Ok(TlbEntry {
                vpn: va >> vpn_shift,
                ppn: oa_bits >> vpn_shift,
                flags: aarch64_flags(desc),
                asid: 0, // ASID from TTBR — caller sets after return
                size: page_size,
                global: (desc >> 11) & 1 == 0, // nG=0 ⟹ global
            });
        }

        if level == 3 {
            // Page descriptor.
            check_aarch64_perms(desc, access, va, 3)?;
            let ppn = (desc >> 12) & 0x0000_FFFF_FFFF;
            return Ok(TlbEntry {
                vpn: va >> 12,
                ppn,
                flags: aarch64_flags(desc),
                asid: 0,
                size: PageSize::Page4K,
                global: (desc >> 11) & 1 == 0,
            });
        }

        // Table descriptor — extract next-level table PA.
        // Next table address = desc[47:12] << 12.
        table_pa = desc & 0x0000_FFFF_FFFF_F000;
    }

    Err(MemFault::PageFault { va, level: 3 })
}

fn check_aarch64_perms(desc: u64, access: AccessType, va: u64, level: u8)
    -> Result<(), MemFault>
{
    // AP[2:1] bits: AP[2]=0 → R/W, AP[2]=1 → RO.
    // XN bit: execute-never.
    let ap2 = (desc >> 7) & 1;
    let xn  = (desc >> 54) & 1;
    match access {
        AccessType::Write   if ap2 != 0 => Err(MemFault::PageFault { va, level }),
        AccessType::Execute if xn  != 0 => Err(MemFault::PageFault { va, level }),
        _ => Ok(()),
    }
}

fn output_address_bits(desc: u64, _vpn_shift: u32) -> u64 {
    desc & 0x0000_FFFF_FFFF_F000
}

fn aarch64_flags(desc: u64) -> u64 {
    // Encode AP, XN, UXN, AF, nG into the flags u64 for TlbEntry.
    (desc >> 6) & 0xFF
}
```

### 4.4 `FunctionalMem` Trait

Used by page table walkers to read PTEs without cache/TLB side effects.

```rust
/// Read-only view of physical memory for use by the page table walker.
/// Implemented by `MemoryMap` (delegating to its `read_functional` method).
pub trait FunctionalMem {
    /// Read 8 bytes at physical address `pa`.
    fn read_u64(&self, pa: u64) -> Result<u64, MemFault>;
}
```

---

## 5. Module Layout

```
helm-memory/src/
├── cache/
│   ├── mod.rs       — pub use
│   ├── config.rs    — CacheConfig
│   ├── model.rs     — CacheModel, CacheSet, CacheLine, CacheLookupResult, CacheStats
│   └── mshr.rs      — MshrFile
└── tlb/
    ├── mod.rs       — pub use, AccessType, FunctionalMem trait
    ├── config.rs    — TlbConfig, PageSize
    ├── entry.rs     — TlbEntry
    ├── model.rs     — TlbModel (translate, insert, flush_*)
    ├── sv39.rs      — sv39_walk (RISC-V Sv39/Sv48)
    └── aarch64.rs   — aarch64_4k_walk
```

---

## Design Decisions from Q&A

### Design Decision: Pseudo-LRU (PLRU) via binary tournament tree (Q30)

Cache replacement uses PLRU (as implemented in `CacheSet::plru_bits` and `touch()`/`plru_victim()` above). For an N-way set, `N-1` bits stored as a `u64` bitmask represent a tournament tree. Each access calls `touch(way)` which flips bits along the path from root to the accessed way's leaf. Victim selection follows the bits from root to leaf. Maximum supported associativity is 64 ways. No shipping RISC-V or AArch64 SoC uses true LRU for L1 caches due to hardware cost — PLRU matches real hardware behavior.

### Design Decision: Write-back by default with configurable per-level override (Q31)

Write-back is the default cache policy (`CacheConfig::write_back: bool`, defaulting to `true`). `CacheLine::dirty` is set on write hits. On eviction, if `dirty == true`, the evicted line's data is written to the next cache level. Write-allocate policy is assumed for write-back caches: a write miss allocates a new line. The `write_back` config flag allows a future write-through L1I model.

### Design Decision: MSHRs modeled per cache level with capacity enforcement (Q34)

MSHRs are modeled per cache level (as `CacheConfig::mshrs: u32` and `CacheLookupResult::MshrFull`). Default capacities: 8 for L1D, 16 for L2, 32 for LLC, matching typical SiFive U74 and Cortex-A72 configurations. A miss that finds the MSHR file full returns `CacheLookupResult::MshrFull { addr }`, signaling the pipeline to stall. MSHR merging (a second miss to the same cache line while an MSHR is already allocated) is handled by `MshrFile::is_pending()`. MSHR capacity is the dominant factor in memory-level parallelism modeling — unlimited MSHRs would produce systematically optimistic IPC figures for memory-bound workloads.

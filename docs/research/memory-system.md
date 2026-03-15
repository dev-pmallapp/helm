# Memory System Design

## helm-ng — Rust-Core Simulator Memory Architecture

---

## Table of Contents

1. [Memory System Architecture Overview](#1-memory-system-architecture-overview)
2. [MemoryRegion Type System](#2-memoryregion-type-system)
3. [FlatView Computation](#3-flatview-computation)
4. [Three Access Modes](#4-three-access-modes)
5. [Cache Model](#5-cache-model)
6. [TLB Design](#6-tlb-design)
7. [Virtual Memory (RISC-V Sv39)](#7-virtual-memory-risc-v-sv39)
8. [MMIO Dispatch](#8-mmio-dispatch)
9. [Endianness](#9-endianness)
10. [MemFault Error Types](#10-memfault-error-types)
11. [Testing the Memory System](#11-testing-the-memory-system)

---

## 1. Memory System Architecture Overview

### Core Abstraction: The MemoryRegion Tree

helm-ng models a machine's physical address space as a tree of `MemoryRegion` nodes. This is the same structural approach used by QEMU's `MemoryRegion` API: the tree captures the *intent* of the hardware layout — which ROM lives at which base address, which PCIe BAR window aliases into which backing memory, which address ranges are reserved guard pages — without committing to a flat representation.

The tree structure allows:

- **Hierarchical composition**: a PCIe root complex is a Container whose children are individual function BARs; each BAR may itself be a Container of MMIO sub-ranges.
- **Alias regions**: a mirrored ROM region points at the same backing data without copying it.
- **Dynamic reconfiguration**: adding or removing a subregion (e.g., plugging a virtio device) is a local tree mutation; the FlatView is recomputed once afterward.

### FlatView — The Resolved Address Map

The tree is not directly used for address lookups. Instead, `FlatView` is a derived, sorted, non-overlapping list of `FlatRange` entries covering every byte of the address space that has a defined mapping. It is the *compiled* form of the tree.

Whenever the tree changes, `FlatView::recompute` walks the tree, applies priority rules, and rebuilds the range list. Address resolution during simulation is then an O(log n) binary search over this list.

### MemoryMap — The Owning Type

`MemoryMap` owns both the root `MemoryRegion` tree and the current `FlatView`. It exposes the three access-mode traits to the rest of the simulator and is the single source of truth for what physical address maps to what.

```
MemoryMap
├── root: MemoryRegion (Container)
│   ├── 0x0000_0000 RAM (512 MB)
│   ├── 0x2000_0000 ROM (boot flash, 8 MB)
│   ├── 0x4000_0000 MMIO (UART)
│   ├── 0x8000_0000 Container (PCIe window)
│   │   ├── 0x8000_0000 MMIO (NVMe BAR0)
│   │   └── 0x8001_0000 Alias → RAM+offset
│   └── 0xffff_0000 Reserved (guard)
└── flat_view: FlatView (sorted, non-overlapping)
```

### Three Access Modes

The simulator exposes three distinct memory access modes. Each serves a different caller with different latency and correctness requirements:

| Mode | Synchrony | Timing Accuracy | Primary Callers |
|------|-----------|-----------------|-----------------|
| **Functional** | Synchronous | None (instant) | GDB, ELF loader, checkpoint |
| **Atomic** | Synchronous | Estimated latency | FE mode emulation, debugger |
| **Timing** | Asynchronous | Cycle-accurate | Interval and Accurate modes |

**Why three modes?**

A GDB `memory-read` request must complete immediately and must never stall — it is purely a state inspection. A boot ELF loader needs to write large byte ranges without disturbing cache state. Neither cares about timing.

The cycle-accurate timing mode models back-pressure, queue depth, MSHR occupancy, and actual interconnect latency. It is fundamentally async: a hart issues a load, the memory system may NACK it (queue full), and the hart must retry. Mixing synchronous callers into this path would deadlock the event loop.

Atomic mode sits in between: it runs synchronously so it can be used in fast-forward emulation, but it returns an *estimated* latency so the simulator can maintain a plausible cycle count even without full timing fidelity.

**Invariant: Timing and Atomic modes cannot coexist.** When the simulator transitions from fast-forward (Atomic) to accurate (Timing) mode, all in-flight atomic operations must drain before the first timing request is issued. The `MemoryMap` enforces this with a mode lock.

---

## 2. MemoryRegion Type System

### Definition

```rust
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

/// A node in the physical address space tree.
///
/// Variants are not stored with a base address — the base is tracked by the
/// parent Container so that subregions can be relocated without cloning.
pub enum MemoryRegion {
    /// General-purpose read/write DRAM.
    /// `data` is the backing store; length defines the region size.
    Ram { data: Vec<u8> },

    /// Read-only memory backed by a static byte slice.
    /// Writes fault with `MemFault::ReadOnly`.
    /// Typically used for boot flash, BIOS ROMs, device firmware images.
    Rom { data: &'static [u8] },

    /// Memory-mapped I/O region dispatched to a Device handler.
    /// The `size` field defines the byte extent for FlatView computation;
    /// the handler owns the register state.
    Mmio { handler: Box<dyn Device>, size: u64 },

    /// Transparent alias into another region at a given byte `offset`.
    /// Reads and writes are forwarded to `target` starting at `offset`.
    /// Used for mirror regions (e.g., a ROM image visible at two addresses).
    Alias {
        target: Arc<RwLock<MemoryRegion>>,
        offset: u64,
        size:   u64,
    },

    /// A container of subregions, each with its own base address.
    /// Key: subregion base (relative to the Container's own base).
    /// Value: (size, MemoryRegion).
    ///
    /// Containers do not own backing memory themselves; all accesses
    /// recurse into matching children. Used for PCIe BAR windows,
    /// SoC peripheral clusters, and the root address space node.
    Container {
        subregions: BTreeMap<u64, (u64, MemoryRegion)>,
    },

    /// Explicitly unmapped range. Any access (read or write) produces
    /// `MemFault::UnmappedAddress`. Used for guard pages, IOMMU holes,
    /// and address ranges that must not be silently ignored.
    Reserved { size: u64 },
}
```

### Variant Use Cases

| Variant | Real Use Case |
|---------|---------------|
| `Ram` | System DRAM — the main 512 MB or 4 GB heap of a simulated machine |
| `Rom` | Boot flash — first instructions executed at reset vector (e.g., `0x8000_0000` for RISC-V) |
| `Mmio` | UART, GIC, PCIe config space — each has a `Device` handler that interprets register offsets |
| `Alias` | ROM mirror — low 64 KB of ROM also mapped at `0xffff_0000` for reset vector; single backing data |
| `Container` | PCIe BAR window — a Container at `0x4000_0000` holds NVMe BAR0 at `+0x0000`, MSI-X table at `+0x1000`, etc. |
| `Reserved` | Guard pages flanking the stack, IOMMU hole between `0xfee0_0000`–`0xfeff_ffff` (x2APIC) |

### The Device Trait

MMIO regions delegate reads and writes to a `Device` implementor. The trait is defined here for context; full device documentation lives in the device model spec.

```rust
pub trait Device: Send + Sync {
    /// Read `size` bytes from register offset `offset` within this device's BAR.
    fn read(&self, offset: u64, size: usize) -> u64;

    /// Write `val` (`size` bytes) to register offset `offset`.
    fn write(&mut self, offset: u64, size: usize, val: u64);

    /// Return the byte size of the device's MMIO footprint.
    fn mmio_size(&self) -> u64;
}
```

---

## 3. FlatView Computation

### Purpose

`FlatView` is the resolved, collision-free map of the address space. It is a `Vec<FlatRange>` sorted by `base`, with no gaps between ranges and no overlaps. The `FlatRange` type is deliberately lightweight — it carries only what the dispatch path needs.

```rust
/// A single contiguous, resolved address range in the flat address map.
#[derive(Debug, Clone)]
pub struct FlatRange {
    pub base: u64,
    pub size: u64,
    pub region_type: RegionType,
}

/// The resolved type of a FlatRange — sufficient for dispatch.
#[derive(Debug, Clone)]
pub enum RegionType {
    Ram,
    Rom,
    /// Index into `MemoryMap::handlers` vec for O(1) dispatch.
    Mmio(usize),
    Reserved,
}

/// Sorted, non-overlapping, exhaustive view of the physical address space.
pub struct FlatView {
    ranges: Vec<FlatRange>,  // invariant: sorted by base, non-overlapping
}

impl FlatView {
    /// O(log n) binary search for the range containing `addr`.
    pub fn lookup(&self, addr: u64) -> Option<&FlatRange> {
        let idx = self.ranges.partition_point(|r| r.base + r.size <= addr);
        let range = self.ranges.get(idx)?;
        if addr >= range.base && addr < range.base + range.size {
            Some(range)
        } else {
            None
        }
    }

    /// Insert a range, splitting or truncating existing ranges as needed.
    /// Higher-priority callers invoke this first; lower-priority callers
    /// add ranges that fill remaining gaps.
    pub fn add_region(&mut self, base: u64, size: u64, region: FlatRange) {
        // Implementation removes any overlapping existing ranges (they have
        // lower priority), splits partially-overlapping ranges at boundaries,
        // then inserts the new range and re-sorts.
        todo!("split + insert + re-sort")
    }

    /// Rebuild the entire FlatView from the MemoryRegion tree.
    /// Called whenever the tree structure changes.
    pub fn recompute(&mut self, root: &MemoryRegion) {
        self.ranges.clear();
        self.flatten_region(root, 0, u64::MAX);
        self.ranges.sort_by_key(|r| r.base);
        // post-condition: no overlaps, sorted by base
    }

    fn flatten_region(&mut self, region: &MemoryRegion, base: u64, limit: u64) {
        match region {
            MemoryRegion::Ram { data } => {
                let size = (data.len() as u64).min(limit - base);
                self.ranges.push(FlatRange { base, size, region_type: RegionType::Ram });
            }
            MemoryRegion::Rom { data } => {
                let size = (data.len() as u64).min(limit - base);
                self.ranges.push(FlatRange { base, size, region_type: RegionType::Rom });
            }
            MemoryRegion::Mmio { handler, size } => {
                let size = (*size).min(limit - base);
                let handler_id = 0; // resolved during recompute via handler registry
                self.ranges.push(FlatRange { base, size, region_type: RegionType::Mmio(handler_id) });
            }
            MemoryRegion::Alias { target, offset, size } => {
                let size = (*size).min(limit - base);
                let target = target.read().unwrap();
                // Re-flatten target at this base with offset applied.
                self.flatten_region(&target, base, base + size);
            }
            MemoryRegion::Container { subregions } => {
                for (&sub_base, (sub_size, sub_region)) in subregions {
                    let abs_base = base + sub_base;
                    let abs_limit = (abs_base + sub_size).min(limit);
                    if abs_base < abs_limit {
                        self.flatten_region(sub_region, abs_base, abs_limit);
                    }
                }
            }
            MemoryRegion::Reserved { size } => {
                let size = (*size).min(limit - base);
                self.ranges.push(FlatRange { base, size, region_type: RegionType::Reserved });
            }
        }
    }
}
```

### Priority Rules

When two regions overlap in the address space, **the region higher in the tree wins** (parent over child, or earlier-inserted sibling over later-inserted sibling). This mirrors QEMU's priority model.

Concretely: `add_region` removes any existing `FlatRange` bytes covered by the new range before inserting. `recompute` processes the tree depth-first so that parent mappings are added first; Container children that overlap are clipped to the gap left by higher-priority siblings.

Practical consequence: a Container can establish a default `Reserved` range spanning its full extent, then add children that fill specific subranges. Any uncovered sub-address falls back to the Reserved entry.

### Recompute Trigger

`FlatView::recompute` is called after any of:

- Adding a subregion to a Container.
- Removing a subregion from a Container.
- Replacing an Alias target.
- Resizing a RAM or ROM region (unusual but permitted during setup).

It is **not** called on data writes. The tree structure is stable during simulation; recomputes happen during machine configuration, device hotplug, and reset.

### Address Lookup: O(log n)

`FlatView::lookup` uses `partition_point` (binary search) on the sorted `ranges` vec. With a typical machine having 10–200 flat ranges, lookup is 4–8 comparisons. This is called on every simulated memory access, so the constant factor matters.

---

## 4. Three Access Modes

### Atomic Mode (Fast-Forward, Default for FE Mode)

Atomic mode is the default for functional emulation. Every operation is synchronous: the caller issues a read or write and immediately receives a result plus an *estimated* latency in cycles. No event loop, no callbacks, no retry.

The estimated latency is computed from a simplified latency table (cache hit → 4 cycles, LLC miss → 200 cycles, MMIO → 10 cycles) rather than full cache hierarchy simulation. This is accurate enough for cycle-count bookkeeping in fast-forward mode.

```rust
/// Synchronous memory access returning an estimated cycle count.
pub trait AtomicMem {
    /// Read `size` bytes from physical address `addr`.
    /// Returns `(value, estimated_cycles)` on success.
    /// Returns `MemFault` if the address is unmapped or access is denied.
    fn read_atomic(&self, addr: u64, size: usize) -> Result<(u64, u64), MemFault>;

    /// Write `val` (`size` bytes) to physical address `addr`.
    /// Returns `estimated_cycles` on success.
    fn write_atomic(&mut self, addr: u64, size: usize, val: u64) -> Result<u64, MemFault>;
}
```

**Users of Atomic mode:**
- Functional emulation core (FE mode hart step loop).
- Debugger read/write when not in a timing simulation.
- ELF loader (bulk writes via repeated `write_atomic` or `FunctionalMem::load_bytes`).

**Invariant with Timing mode:** Atomic and Timing mode cannot be simultaneously active on the same `MemoryMap`. If a hart is in Timing mode, its atomic calls are blocked. Transition requires draining all in-flight timing requests first.

---

### Functional Mode (Debug, State Inspection)

Functional mode is for callers that need direct memory access without any timing or side-effect semantics. It never fails on valid addresses (panics on unmapped access rather than returning an error — callers must validate addresses before calling). Reads do not update cache state, LRU bits, or any performance counters.

```rust
/// Side-effect-free, instantaneous memory access for debug and state tools.
pub trait FunctionalMem {
    /// Read `size` bytes from physical address `addr`.
    /// Panics if `addr` is unmapped — callers must range-check first.
    fn read_functional(&self, addr: u64, size: usize) -> u64;

    /// Write `val` (`size` bytes) to physical address `addr`.
    /// Panics if `addr` is unmapped or read-only.
    fn write_functional(&mut self, addr: u64, size: usize, val: u64);

    /// Bulk write `data` bytes starting at `addr`.
    /// Used by the ELF loader and checkpoint restore.
    fn load_bytes(&mut self, addr: u64, data: &[u8]);
}
```

**Users of Functional mode:**
- GDB remote protocol handler (`memory-read`, `memory-write` packets).
- Checkpoint save: serializes raw RAM contents to disk.
- Checkpoint restore: calls `load_bytes` to repopulate RAM.
- ELF loader: writes program segments at load addresses.
- Watchpoint evaluation: reads a byte range to check if the value changed.

**Key property:** Functional reads on RAM bypass the cache model entirely. They read from the backing `Vec<u8>` directly. This means a GDB read of a dirty cache line returns the *last committed write to RAM*, not the in-cache value. This is acceptable: GDB is a debug tool and the discrepancy is documented. A future enhancement can add a "cache-coherent functional read" that flushes the relevant cache line first.

---

### Timing Mode (Accurate Simulation)

Timing mode is used when the simulator runs in Interval or Accurate timing mode. Memory operations are asynchronous: the hart issues a `MemRequest` and the memory system responds later via callback. The memory system may NACK a request (queue full, MSHR full) and the hart must retry.

```rust
/// Tag identifying a memory operation in flight.
pub type ReqId = u64;

/// A memory operation submitted in timing mode.
pub struct MemRequest {
    pub id:   ReqId,   // caller-assigned, unique per hart per in-flight set
    pub addr: u64,
    pub size: usize,
    pub op:   MemOp,
}

pub enum MemOp {
    Read,
    Write(u64),   // value to write
}

/// The response to a timing-mode memory request.
pub struct MemResponse {
    pub id:     ReqId,
    pub data:   u64,    // read data (0 for writes)
    pub cycles: u64,    // actual modeled latency from request to response
}

/// Asynchronous, flow-controlled memory access for cycle-accurate simulation.
pub trait TimingMem {
    /// Submit a memory request.
    /// Returns `Ok(())` if accepted; `Err(MemFault::QueueFull)` if the
    /// memory system cannot accept it now — caller must retry later.
    fn request_timing(&mut self, req: MemRequest) -> Result<(), MemFault>;

    /// Called by the memory system when a previously submitted request completes.
    fn on_response(&mut self, resp: MemResponse);

    /// Called by the memory system when a previously submitted request is NACKed.
    /// The caller should resubmit `id` at the next opportunity.
    fn on_retry(&mut self, id: ReqId);
}
```

**Mode transition enforcement:**

```rust
pub enum AccessMode {
    Functional,
    Atomic,
    Timing { in_flight: HashSet<ReqId> },
}

impl MemoryMap {
    pub fn switch_to_atomic(&mut self) -> Result<(), SimError> {
        if let AccessMode::Timing { in_flight } = &self.mode {
            if !in_flight.is_empty() {
                return Err(SimError::TimingRequestsInFlight(in_flight.len()));
            }
        }
        self.mode = AccessMode::Atomic;
        Ok(())
    }
}
```

---

## 5. Cache Model

### Set-Associative Cache

The cache model is a parameterized, software-modeled set-associative cache. It is used by Atomic mode (for estimated latency) and Timing mode (for full hit/miss/writeback simulation). Functional mode bypasses it entirely.

```rust
pub struct CacheConfig {
    pub sets:       usize,     // number of cache sets (power of two)
    pub ways:       usize,     // associativity
    pub line_bytes: usize,     // cache line size in bytes (power of two, typically 64)
    pub latency_hit_cycles:  u64,
    pub latency_miss_cycles: u64,
}

pub struct CacheModel {
    pub config: CacheConfig,
    sets:  Vec<CacheSet>,
    stats: CacheStats,
}

pub struct CacheSet {
    ways:      Vec<CacheLine>,
    lru_order: VecDeque<u8>,   // indices into `ways`, front = MRU, back = LRU
}

pub struct CacheLine {
    pub tag:   u64,
    pub valid: bool,
    pub dirty: bool,
    pub data:  Vec<u8>,   // length == CacheConfig::line_bytes
}

pub struct CacheStats {
    pub hits:       PerfCounter,
    pub misses:     PerfCounter,
    pub evictions:  PerfCounter,
    pub writebacks: PerfCounter,
}
```

### Address Decomposition

For a cache with `S` sets and `L` bytes per line:

```
line_offset_bits = log2(L)   // low bits — byte offset within line
set_index_bits   = log2(S)   // middle bits — which set
tag_bits         = 64 - set_index_bits - line_offset_bits  // high bits — tag
```

```rust
impl CacheModel {
    fn decompose(&self, addr: u64) -> (u64, usize, usize) {
        let lo = self.config.line_bytes.trailing_zeros() as u64;
        let ls = self.config.sets.trailing_zeros() as u64;
        let offset = (addr & (self.config.line_bytes as u64 - 1)) as usize;
        let set    = ((addr >> lo) & (self.config.sets as u64 - 1)) as usize;
        let tag    = addr >> (lo + ls);
        (tag, set, offset)
    }
}
```

### Cache Hierarchy Lookup

```rust
pub struct CacheLookupResult {
    pub hit:       bool,
    pub level_hit: Option<u8>,   // L1, L2, L3 (None = miss to DRAM)
    pub latency:   u64,          // cycles
    pub data:      Option<u64>,  // present on read hit
}

impl CacheModel {
    /// Perform a cache lookup for `addr`. If `is_write`, mark the line dirty on hit.
    /// On miss, does NOT fill — caller is responsible for issuing a fill after
    /// fetching the line from the next level.
    pub fn lookup(&mut self, addr: u64, is_write: bool) -> CacheLookupResult {
        let (tag, set_idx, _offset) = self.decompose(addr);
        let set = &mut self.sets[set_idx];

        for (i, way) in set.ways.iter_mut().enumerate() {
            if way.valid && way.tag == tag {
                // Hit — promote to MRU
                set.lru_order.retain(|&x| x as usize != i);
                set.lru_order.push_front(i as u8);
                if is_write { way.dirty = true; }
                self.stats.hits.increment();
                return CacheLookupResult {
                    hit: true,
                    level_hit: Some(1),
                    latency: self.config.latency_hit_cycles,
                    data: None, // caller reads from line.data[offset..]
                };
            }
        }

        self.stats.misses.increment();
        CacheLookupResult {
            hit: false,
            level_hit: None,
            latency: self.config.latency_miss_cycles,
            data: None,
        }
    }

    /// Fill a cache line at `addr` with `data` (length must equal line_bytes).
    /// Evicts the LRU way; if that way is dirty, its data is returned for writeback.
    pub fn fill(&mut self, addr: u64, data: &[u8]) {
        let (tag, set_idx, _) = self.decompose(addr);
        let set = &mut self.sets[set_idx];

        // Find an invalid way first; otherwise evict LRU
        let victim_idx = set.ways.iter().position(|w| !w.valid)
            .unwrap_or_else(|| *set.lru_order.back().unwrap() as usize);

        let victim = &mut set.ways[victim_idx];
        if victim.valid && victim.dirty {
            self.stats.writebacks.increment();
            // In a real implementation, the dirty data is sent to the next level.
        }
        if victim.valid { self.stats.evictions.increment(); }

        victim.tag   = tag;
        victim.valid = true;
        victim.dirty = false;
        victim.data.copy_from_slice(data);

        set.lru_order.retain(|&x| x as usize != victim_idx);
        set.lru_order.push_front(victim_idx as u8);
    }

    /// Evict the line containing `addr`. Returns dirty data if writeback needed.
    pub fn evict(&mut self, addr: u64) -> Option<Vec<u8>> {
        let (tag, set_idx, _) = self.decompose(addr);
        let set = &mut self.sets[set_idx];
        for way in set.ways.iter_mut() {
            if way.valid && way.tag == tag {
                way.valid = false;
                let dirty_data = if way.dirty { Some(way.data.clone()) } else { None };
                way.dirty = false;
                self.stats.evictions.increment();
                return dirty_data;
            }
        }
        None
    }

    /// Invalidate the line containing `addr` without writeback.
    pub fn invalidate(&mut self, addr: u64) {
        let (tag, set_idx, _) = self.decompose(addr);
        for way in self.sets[set_idx].ways.iter_mut() {
            if way.valid && way.tag == tag {
                way.valid = false;
                way.dirty = false;
                return;
            }
        }
    }

    /// Writeback all dirty lines. Called on cache flush (e.g., DMA coherence).
    pub fn flush(&mut self) {
        for set in self.sets.iter_mut() {
            for way in set.ways.iter_mut() {
                if way.valid && way.dirty {
                    self.stats.writebacks.increment();
                    // Send writeback to next level.
                    way.dirty = false;
                }
            }
        }
    }
}
```

### MSHR (Miss Status Holding Registers)

MSHRs prevent issuing duplicate fetch requests for the same cache line when multiple loads miss on the same address before the first response arrives. Each MSHR entry tracks one outstanding miss and the list of request IDs waiting for it.

```rust
pub struct Mshr {
    entries:  Vec<MshrEntry>,
    capacity: u8,  // typically 8–16
}

pub struct MshrEntry {
    pub addr:    u64,         // cache-line-aligned address
    pub state:   MshrState,
    pub waiters: Vec<ReqId>,  // request IDs blocked on this miss
}

pub enum MshrState {
    Pending,   // fetch issued, waiting for data
    Filling,   // data arrived, filling cache line
    Done,      // line filled, waiters being drained
}

impl Mshr {
    /// Check if `addr` already has an outstanding miss entry.
    pub fn find(&self, addr: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.addr == addr)
    }

    /// Allocate a new MSHR entry for `addr`. Returns `None` if full (caller must stall).
    pub fn allocate(&mut self, addr: u64, waiter: ReqId) -> Option<usize> {
        if self.entries.len() >= self.capacity as usize {
            return None;
        }
        let idx = self.entries.len();
        self.entries.push(MshrEntry {
            addr,
            state: MshrState::Pending,
            waiters: vec![waiter],
        });
        Some(idx)
    }

    /// Add `waiter` to an existing MSHR entry (coalesce).
    pub fn coalesce(&mut self, idx: usize, waiter: ReqId) {
        self.entries[idx].waiters.push(waiter);
    }

    /// Free the MSHR entry at `idx` after the line is filled and waiters drained.
    pub fn free(&mut self, idx: usize) {
        self.entries.swap_remove(idx);
    }
}
```

---

## 6. TLB Design

The TLB (Translation Lookaside Buffer) caches virtual-to-physical page translations to avoid walking the page table on every memory access.

```rust
pub struct TlbModel {
    entries:  Vec<TlbEntry>,
    capacity: usize,
    lru:      VecDeque<usize>,  // indices into `entries`; front = MRU
}

pub struct TlbEntry {
    pub vpn:   u64,       // virtual page number (right-shifted by page_size_bits)
    pub ppn:   u64,       // physical page number
    pub flags: u8,        // RISC-V PTE flags: R W X U G A D
    pub asid:  u16,       // address space ID (0 = global / ASID-unaware)
    pub valid: bool,
    pub size:  PageSize,
}

pub enum PageSize {
    Page4K,    // standard 4 KB page, Sv39/Sv48
    Page2M,    // 2 MB megapage (one level of page table skipped)
    Page1G,    // 1 GB gigapage (two levels skipped)
}

pub enum AccessType {
    Load,
    Store,
    Fetch,  // instruction fetch
}

impl TlbModel {
    /// Look up a translation for `va` under `asid`.
    /// Returns the matching entry if present and valid.
    pub fn lookup(&self, va: u64, asid: u16) -> Option<&TlbEntry> {
        let vpn = va >> 12;
        self.entries.iter().find(|e| {
            e.valid
                && (e.asid == asid || e.flags & PTE_GLOBAL != 0)
                && e.vpn == (vpn & vpn_mask_for(e.size))
        })
    }

    /// Insert a translation. Evicts LRU if at capacity.
    pub fn insert(&mut self, entry: TlbEntry) {
        if self.entries.len() >= self.capacity {
            let victim = self.lru.pop_back().unwrap();
            self.entries[victim] = entry;
            self.lru.push_front(victim);
        } else {
            let idx = self.entries.len();
            self.entries.push(entry);
            self.lru.push_front(idx);
        }
    }

    /// Invalidate all entries matching `asid` (SFENCE.VMA with rs2).
    pub fn invalidate_asid(&mut self, asid: u16) {
        for e in self.entries.iter_mut() {
            if e.asid == asid && e.flags & PTE_GLOBAL == 0 {
                e.valid = false;
            }
        }
    }

    /// Invalidate all entries (SFENCE.VMA with rs1=x0, rs2=x0).
    pub fn invalidate_all(&mut self) {
        for e in self.entries.iter_mut() {
            e.valid = false;
        }
    }

    /// Translate `va` to a physical address for `access` type.
    /// Checks PTE permission flags. On TLB miss, returns `PageFault`
    /// and the caller must run a page table walk.
    pub fn translate(&self, va: u64, asid: u16, access: AccessType) -> Result<u64, PageFault> {
        let entry = self.lookup(va, asid).ok_or(PageFault::TlbMiss { va })?;
        check_pte_permissions(entry, &access)?;
        let page_offset = va & 0xfff;
        Ok((entry.ppn << 12) | page_offset)
    }
}

const PTE_GLOBAL: u8 = 0x20;

fn vpn_mask_for(size: &PageSize) -> u64 {
    match size {
        PageSize::Page4K => u64::MAX,
        PageSize::Page2M => !0x1ff,       // mask off vpn0
        PageSize::Page1G => !0x3_ffff,    // mask off vpn0 + vpn1
    }
}

fn check_pte_permissions(entry: &TlbEntry, access: &AccessType) -> Result<(), PageFault> {
    let r = entry.flags & 0x02 != 0;
    let w = entry.flags & 0x04 != 0;
    let x = entry.flags & 0x08 != 0;
    match access {
        AccessType::Load  if !r => Err(PageFault::ReadPermission  { va: entry.vpn << 12 }),
        AccessType::Store if !w => Err(PageFault::WritePermission { va: entry.vpn << 12 }),
        AccessType::Fetch if !x => Err(PageFault::ExecPermission  { va: entry.vpn << 12 }),
        _ => Ok(()),
    }
}
```

---

## 7. Virtual Memory (RISC-V Sv39)

### Sv39 Overview

Sv39 uses a three-level page table with 39-bit virtual addresses (512 GB VA space). Each level uses 9 VPN bits; each page table entry (PTE) is 8 bytes.

```
VA[63:39] must be sign extension of VA[38]
VA[38:30] → VPN[2] → L2 page table index
VA[29:21] → VPN[1] → L1 page table index
VA[20:12] → VPN[0] → L0 page table index
VA[11:0]  → page offset
```

### Page Table Walk

```rust
pub fn sv39_walk(
    satp:   u64,           // CSR satp: MODE[63:60] | ASID[59:44] | PPN[43:0]
    va:     u64,
    access: AccessType,
    mem:    &dyn FunctionalMem,
) -> Result<u64, PageFault> {
    // Extract VPN components
    let vpn2 = (va >> 30) & 0x1ff;
    let vpn1 = (va >> 21) & 0x1ff;
    let vpn0 = (va >> 12) & 0x1ff;
    let page_offset = va & 0xfff;

    // Root physical page number from satp[43:0]
    let root_ppn = satp & 0x0fff_ffff_ffff;

    // Level 2 (root) PTE
    let pte2_addr = (root_ppn << 12) | (vpn2 << 3);
    let pte2 = mem.read_functional(pte2_addr, 8);
    if pte2 & PTE_V == 0 { return Err(PageFault::InvalidPte { va, pte_addr: pte2_addr }); }
    if is_leaf(pte2) {
        // 1 GB gigapage
        check_leaf_permissions(pte2, &access, va)?;
        let ppn2 = (pte2 >> 28) & 0x3ff_ffff;  // PTE.PPN[2]
        let pa = (ppn2 << 30) | ((va >> 0) & 0x3fff_ffff);
        return Ok(pa);
    }

    // Level 1 PTE
    let l1_ppn = (pte2 >> 10) & 0xfff_ffff_ffff;
    let pte1_addr = (l1_ppn << 12) | (vpn1 << 3);
    let pte1 = mem.read_functional(pte1_addr, 8);
    if pte1 & PTE_V == 0 { return Err(PageFault::InvalidPte { va, pte_addr: pte1_addr }); }
    if is_leaf(pte1) {
        // 2 MB megapage
        check_leaf_permissions(pte1, &access, va)?;
        let ppn2 = (pte1 >> 28) & 0x3ff_ffff;
        let ppn1 = (pte1 >> 19) & 0x1ff;
        let pa = (ppn2 << 30) | (ppn1 << 21) | (va & 0x1f_ffff);
        return Ok(pa);
    }

    // Level 0 PTE — must be a leaf
    let l0_ppn = (pte1 >> 10) & 0xfff_ffff_ffff;
    let pte0_addr = (l0_ppn << 12) | (vpn0 << 3);
    let pte0 = mem.read_functional(pte0_addr, 8);
    if pte0 & PTE_V == 0 { return Err(PageFault::InvalidPte { va, pte_addr: pte0_addr }); }
    if !is_leaf(pte0) { return Err(PageFault::NonLeafPte { va, pte_addr: pte0_addr }); }

    check_leaf_permissions(pte0, &access, va)?;

    // 4 KB standard page
    let ppn = (pte0 >> 10) & 0xfff_ffff_ffff;
    Ok((ppn << 12) | page_offset)
}

const PTE_V: u64 = 1 << 0;  // Valid
const PTE_R: u64 = 1 << 1;  // Readable
const PTE_W: u64 = 1 << 2;  // Writable
const PTE_X: u64 = 1 << 3;  // Executable
const PTE_U: u64 = 1 << 4;  // User-accessible
const PTE_A: u64 = 1 << 6;  // Accessed
const PTE_D: u64 = 1 << 7;  // Dirty

fn is_leaf(pte: u64) -> bool {
    pte & (PTE_R | PTE_W | PTE_X) != 0
}

fn check_leaf_permissions(pte: u64, access: &AccessType, va: u64) -> Result<(), PageFault> {
    match access {
        AccessType::Load  if pte & PTE_R == 0 => Err(PageFault::ReadPermission  { va }),
        AccessType::Store if pte & PTE_W == 0 => Err(PageFault::WritePermission { va }),
        AccessType::Fetch if pte & PTE_X == 0 => Err(PageFault::ExecPermission  { va }),
        _ => Ok(()),
    }
}
```

### Sv48 Extension

Sv48 adds a fourth level (VPN[3] at VA[47:39]) and extends the VA space to 256 TB. The walk is identical to Sv39 with one additional level prepended:

```rust
pub fn sv48_walk(satp: u64, va: u64, access: AccessType, mem: &dyn FunctionalMem)
    -> Result<u64, PageFault>
{
    let vpn3 = (va >> 39) & 0x1ff;
    let root_ppn = satp & 0x0fff_ffff_ffff;

    let pte3_addr = (root_ppn << 12) | (vpn3 << 3);
    let pte3 = mem.read_functional(pte3_addr, 8);
    if pte3 & PTE_V == 0 { return Err(PageFault::InvalidPte { va, pte_addr: pte3_addr }); }
    if is_leaf(pte3) { return Err(PageFault::NonLeafPte { va, pte_addr: pte3_addr }); }

    // Construct a synthetic satp pointing to the L2 table, then reuse Sv39 walk
    let l2_ppn = (pte3 >> 10) & 0xfff_ffff_ffff;
    let synthetic_satp = l2_ppn;  // mode bits irrelevant, just the PPN
    sv39_walk(synthetic_satp, va, access, mem)
}
```

### AArch64 4KB Page Walk (Comparison)

AArch64 with 4KB granule and 48-bit VA (TCR_EL1.T0SZ=16) uses a four-level walk (L0–L3). The structure is analogous to Sv48 but uses different PTE layouts and descriptor formats.

```rust
pub fn aarch64_walk_4k(
    ttbr: u64,       // TTBR0_EL1 or TTBR1_EL1 — contains base address of L0 table
    va:   u64,
    access: AccessType,
    mem:  &dyn FunctionalMem,
) -> Result<u64, PageFault> {
    // 48-bit VA, 4KB granule, 4 levels
    // Each level index is 9 bits; page offset is 12 bits
    let l0_idx = (va >> 39) & 0x1ff;
    let l1_idx = (va >> 30) & 0x1ff;
    let l2_idx = (va >> 21) & 0x1ff;
    let l3_idx = (va >> 12) & 0x1ff;
    let page_offset = va & 0xfff;

    let table_base = ttbr & 0xffff_ffff_f000;  // PA of L0 table

    macro_rules! read_desc {
        ($base:expr, $idx:expr) => {{
            let addr = $base + ($idx << 3);
            mem.read_functional(addr, 8)
        }};
    }

    macro_rules! next_table {
        ($desc:expr) => { ($desc & 0x0000_ffff_ffff_f000) };
    }

    let d0 = read_desc!(table_base, l0_idx);
    if d0 & 0x3 != 0x3 { return Err(PageFault::InvalidPte { va, pte_addr: table_base + (l0_idx << 3) }); }

    let d1 = read_desc!(next_table!(d0), l1_idx);
    if d1 & 0x3 == 0x1 {
        // 1GB block descriptor
        let pa_base = d1 & 0x0000_fffc_0000_0000;
        return Ok(pa_base | (va & 0x3fff_ffff));
    }

    let d2 = read_desc!(next_table!(d1), l2_idx);
    if d2 & 0x3 == 0x1 {
        // 2MB block descriptor
        let pa_base = d2 & 0x0000_ffff_ffe0_0000;
        return Ok(pa_base | (va & 0x1f_ffff));
    }

    let d3 = read_desc!(next_table!(d2), l3_idx);
    if d3 & 0x3 != 0x3 { return Err(PageFault::InvalidPte { va, pte_addr: next_table!(d2) + (l3_idx << 3) }); }

    // 4KB page
    let pa_base = d3 & 0x0000_ffff_ffff_f000;
    Ok(pa_base | page_offset)
}
```

---

## 8. MMIO Dispatch

### Dispatch Path

When a simulated load or store resolves to an MMIO region in the FlatView, `MemoryMap` dispatches the access to the appropriate `Device` handler. The `handler_id` stored in `RegionType::Mmio` is an index into `MemoryMap::handlers`, giving O(1) handler lookup with no hash overhead.

```rust
impl MemoryMap {
    /// Read `size` bytes from physical address `addr`.
    fn dispatch_mmio_read(&self, addr: u64, size: usize) -> Result<u64, MemFault> {
        let flat = self.flat_view.lookup(addr)
            .ok_or(MemFault::UnmappedAddress(addr))?;

        match flat.region_type {
            RegionType::Mmio(handler_id) => {
                let handler = &self.handlers[handler_id];
                let offset  = addr - flat.base;
                Ok(handler.read(offset, size))
            }
            RegionType::Ram => {
                // Direct backing store read — extract bytes from region's Vec<u8>
                let ram = self.ram_for_flat(flat)?;
                let offset = (addr - flat.base) as usize;
                Ok(read_le_u64(&ram[offset..offset + size], size))
            }
            RegionType::Rom => {
                let rom = self.rom_for_flat(flat)?;
                let offset = (addr - flat.base) as usize;
                Ok(read_le_u64(&rom[offset..offset + size], size))
            }
            RegionType::Reserved => Err(MemFault::UnmappedAddress(addr)),
        }
    }

    fn dispatch_mmio_write(&mut self, addr: u64, size: usize, val: u64) -> Result<(), MemFault> {
        let flat = self.flat_view.lookup(addr)
            .ok_or(MemFault::UnmappedAddress(addr))?
            .clone();  // clone to release the borrow on flat_view

        match flat.region_type {
            RegionType::Mmio(handler_id) => {
                let handler = &mut self.handlers[handler_id];
                let offset  = addr - flat.base;
                handler.write(offset, size, val);
                Ok(())
            }
            RegionType::Ram => {
                let ram = self.ram_mut_for_flat(&flat)?;
                let offset = (addr - flat.base) as usize;
                write_le_u64(&mut ram[offset..offset + size], size, val);
                Ok(())
            }
            RegionType::Rom => Err(MemFault::ReadOnly { addr }),
            RegionType::Reserved => Err(MemFault::UnmappedAddress(addr)),
        }
    }
}
```

### Handler Registry

Handlers are stored in `Vec<Box<dyn Device>>`. The index is stable as long as the FlatView is not recomputed. On recompute, handler IDs are reassigned based on the current tree ordering. This means the `handler_id` in a `FlatRange` is valid only until the next `recompute` call — callers must never cache a `FlatRange` across a structural tree change.

```rust
pub struct MemoryMap {
    root:      MemoryRegion,
    flat_view: FlatView,
    handlers:  Vec<Box<dyn Device>>,   // index == RegionType::Mmio(id)
    mode:      AccessMode,
}
```

---

## 9. Endianness

### RISC-V

RISC-V is **little-endian only** (the base ISA specification does not define a big-endian mode). All helm-ng RISC-V hart implementations read and write memory as little-endian unconditionally. No endianness flag is maintained per hart.

### AArch64

AArch64 supports configurable endianness:
- **EL1 data endianness**: controlled by `SCTLR_EL1.EE` (0 = LE, 1 = BE).
- **EL0 data endianness**: controlled by `CPSR.E` (or `SPSR_EL1.E` on exception entry).
- **Instruction fetch**: always little-endian (the A64 instruction set is fixed LE).

### helm-ng Endianness Handling

Each hart maintains a `data_endian: Endian` field:

```rust
pub enum Endian { Little, Big }

pub struct HartState {
    pub data_endian: Endian,
    // ...
}
```

Load and store instructions call endian-aware byte-swap helpers:

```rust
pub fn read_le_u64(bytes: &[u8], size: usize) -> u64 {
    let mut buf = [0u8; 8];
    buf[..size].copy_from_slice(&bytes[..size]);
    u64::from_le_bytes(buf)
}

pub fn read_be_u64(bytes: &[u8], size: usize) -> u64 {
    let mut buf = [0u8; 8];
    buf[8 - size..].copy_from_slice(&bytes[..size]);
    u64::from_be_bytes(buf)
}

pub fn read_endian(bytes: &[u8], size: usize, endian: Endian) -> u64 {
    match endian {
        Endian::Little => read_le_u64(bytes, size),
        Endian::Big    => read_be_u64(bytes, size),
    }
}
```

The backing `Vec<u8>` in RAM regions always stores bytes in *physical* (native) order — the endian swap is applied at the hart-level load/store instruction, not inside the memory system. MMIO device registers handle their own endianness internally.

---

## 10. MemFault Error Types

`MemFault` is the unified memory error type returned by all non-functional access paths. It is designed to carry enough context for the fault handler to raise the correct architectural exception (RISC-V `mcause`, AArch64 ESR_ELx).

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemFault {
    /// Physical address is not mapped by any region (no FlatRange covers it).
    UnmappedAddress(u64),

    /// Physical address is mapped but the access type is disallowed at the
    /// physical level (e.g., execute from a non-executable RAM region).
    AccessFault { addr: u64, access: AccessType },

    /// Virtual address translation failed. `pte_addr` is the physical address
    /// of the PTE that was invalid or lacked permissions.
    PageFault { addr: u64, va: u64, pte_addr: u64 },

    /// Address is not aligned to the required alignment for this access size.
    /// RISC-V: generates a load/store address misaligned exception unless
    ///   the Zicclsm extension is supported (software-emulated misaligned).
    Misaligned { addr: u64, size: usize, required_align: usize },

    /// Write attempted to a ROM region.
    ReadOnly { addr: u64 },

    /// PTE permission check failed (read/write/execute permission denied).
    PermissionDenied { addr: u64, required: AccessType, actual: u8 },

    /// Timing mode request queue or MSHR is full. The caller must retry.
    QueueFull,
}

impl MemFault {
    /// Map this fault to a RISC-V mcause value.
    pub fn to_riscv_cause(&self, access: &AccessType) -> u64 {
        match (self, access) {
            (MemFault::UnmappedAddress(_) | MemFault::AccessFault { .. }, AccessType::Fetch)
                => 1,   // Instruction access fault
            (MemFault::UnmappedAddress(_) | MemFault::AccessFault { .. }, AccessType::Load)
                => 5,   // Load access fault
            (MemFault::UnmappedAddress(_) | MemFault::AccessFault { .. }, AccessType::Store)
                => 7,   // Store/AMO access fault
            (MemFault::PageFault { .. } | MemFault::PermissionDenied { .. }, AccessType::Fetch)
                => 12,  // Instruction page fault
            (MemFault::PageFault { .. } | MemFault::PermissionDenied { .. }, AccessType::Load)
                => 13,  // Load page fault
            (MemFault::PageFault { .. } | MemFault::PermissionDenied { .. }, AccessType::Store)
                => 15,  // Store/AMO page fault
            (MemFault::Misaligned { .. }, AccessType::Load)
                => 4,   // Load address misaligned
            (MemFault::Misaligned { .. }, AccessType::Store)
                => 6,   // Store/AMO address misaligned
            (MemFault::Misaligned { .. }, AccessType::Fetch)
                => 0,   // Instruction address misaligned
            _ => 0,
        }
    }
}
```

---

## 11. Testing the Memory System

### Unit Tests: FlatView Computation

```rust
#[cfg(test)]
mod flatview_tests {
    use super::*;

    fn make_ram(size: usize) -> MemoryRegion {
        MemoryRegion::Ram { data: vec![0u8; size] }
    }

    #[test]
    fn test_single_ram_region() {
        let root = MemoryRegion::Container {
            subregions: {
                let mut m = BTreeMap::new();
                m.insert(0x0, (0x1000, make_ram(0x1000)));
                m
            },
        };
        let mut fv = FlatView { ranges: vec![] };
        fv.recompute(&root);
        assert_eq!(fv.ranges.len(), 1);
        assert_eq!(fv.ranges[0].base, 0x0);
        assert_eq!(fv.ranges[0].size, 0x1000);
    }

    #[test]
    fn test_overlapping_regions_parent_wins() {
        // Container has a RAM child at 0–0x2000 and a Reserved child at 0x1000–0x2000.
        // The Reserved should win for 0x1000–0x2000 (child overlay higher priority).
        // (Adjust according to actual priority semantics.)
        let mut subregions = BTreeMap::new();
        subregions.insert(0x0,    (0x2000, make_ram(0x2000)));
        subregions.insert(0x1000, (0x1000, MemoryRegion::Reserved { size: 0x1000 }));
        let root = MemoryRegion::Container { subregions };
        let mut fv = FlatView { ranges: vec![] };
        fv.recompute(&root);

        // Address 0x500 → RAM
        let r = fv.lookup(0x500).expect("should map");
        assert!(matches!(r.region_type, RegionType::Ram));

        // Address 0x1800 → Reserved (child overlay)
        let r = fv.lookup(0x1800).expect("should map");
        assert!(matches!(r.region_type, RegionType::Reserved));
    }

    #[test]
    fn test_gap_is_unmapped() {
        let mut subregions = BTreeMap::new();
        subregions.insert(0x0,    (0x1000, make_ram(0x1000)));
        subregions.insert(0x2000, (0x1000, make_ram(0x1000)));
        let root = MemoryRegion::Container { subregions };
        let mut fv = FlatView { ranges: vec![] };
        fv.recompute(&root);
        // Gap at 0x1000–0x2000 should return None
        assert!(fv.lookup(0x1500).is_none());
    }

    #[test]
    fn test_lookup_boundary() {
        let mut subregions = BTreeMap::new();
        subregions.insert(0x0, (0x1000, make_ram(0x1000)));
        let root = MemoryRegion::Container { subregions };
        let mut fv = FlatView { ranges: vec![] };
        fv.recompute(&root);
        assert!(fv.lookup(0x0).is_some());
        assert!(fv.lookup(0xfff).is_some());
        assert!(fv.lookup(0x1000).is_none());  // exclusive upper bound
    }
}
```

### Unit Tests: Cache Set-Associative Lookup

```rust
#[cfg(test)]
mod cache_tests {
    use super::*;

    fn make_cache(sets: usize, ways: usize) -> CacheModel {
        CacheModel::new(CacheConfig {
            sets,
            ways,
            line_bytes: 64,
            latency_hit_cycles: 4,
            latency_miss_cycles: 200,
        })
    }

    #[test]
    fn test_cold_miss() {
        let mut cache = make_cache(4, 2);
        let result = cache.lookup(0x1000, false);
        assert!(!result.hit);
        assert_eq!(cache.stats.misses.value(), 1);
    }

    #[test]
    fn test_fill_then_hit() {
        let mut cache = make_cache(4, 2);
        let data = vec![0xABu8; 64];
        cache.fill(0x1000, &data);
        let result = cache.lookup(0x1000, false);
        assert!(result.hit);
        assert_eq!(cache.stats.hits.value(), 1);
    }

    #[test]
    fn test_lru_eviction() {
        // 1-set, 2-way cache: fill A, fill B, access A (promotes A), fill C → B evicted
        let mut cache = make_cache(1, 2);
        cache.fill(0x000, &vec![1u8; 64]);   // way 0: tag A
        cache.fill(0x040, &vec![2u8; 64]);   // way 1: tag B
        // Access A to make it MRU
        cache.lookup(0x000, false);
        // Fill C → must evict LRU = B
        cache.fill(0x080, &vec![3u8; 64]);
        // A should still hit, B should miss
        assert!(cache.lookup(0x000, false).hit);
        assert!(!cache.lookup(0x040, false).hit);
    }

    #[test]
    fn test_dirty_eviction_writeback() {
        let mut cache = make_cache(1, 1);
        cache.fill(0x000, &vec![0u8; 64]);
        cache.lookup(0x000, true);             // mark dirty via write
        assert_eq!(cache.sets[0].ways[0].dirty, true);
        cache.fill(0x040, &vec![0u8; 64]);     // evict → writeback counted
        assert_eq!(cache.stats.writebacks.value(), 1);
    }

    #[test]
    fn test_invalidate_clears_line() {
        let mut cache = make_cache(4, 2);
        cache.fill(0x1000, &vec![0u8; 64]);
        assert!(cache.lookup(0x1000, false).hit);
        cache.invalidate(0x1000);
        assert!(!cache.lookup(0x1000, false).hit);
    }
}
```

### Unit Tests: TLB Lookup and Page Table Walk

```rust
#[cfg(test)]
mod tlb_tests {
    use super::*;

    #[test]
    fn test_tlb_miss_on_empty() {
        let tlb = TlbModel { entries: vec![], capacity: 16, lru: VecDeque::new() };
        assert!(tlb.lookup(0x1000, 0).is_none());
    }

    #[test]
    fn test_tlb_insert_and_hit() {
        let mut tlb = TlbModel { entries: vec![], capacity: 16, lru: VecDeque::new() };
        tlb.insert(TlbEntry {
            vpn: 0x1,   // VA 0x1000
            ppn: 0xABC,
            flags: PTE_R | PTE_W | PTE_V,
            asid: 1,
            valid: true,
            size: PageSize::Page4K,
        });
        let entry = tlb.lookup(0x1000, 1).expect("should hit");
        assert_eq!(entry.ppn, 0xABC);
    }

    #[test]
    fn test_tlb_asid_isolation() {
        let mut tlb = TlbModel { entries: vec![], capacity: 16, lru: VecDeque::new() };
        tlb.insert(TlbEntry {
            vpn: 0x1, ppn: 0x111, flags: PTE_R | PTE_V, asid: 1,
            valid: true, size: PageSize::Page4K,
        });
        // Different ASID should not see the entry (and it's not global)
        assert!(tlb.lookup(0x1000, 2).is_none());
    }

    #[test]
    fn test_tlb_invalidate_asid() {
        let mut tlb = TlbModel { entries: vec![], capacity: 16, lru: VecDeque::new() };
        tlb.insert(TlbEntry {
            vpn: 0x1, ppn: 0x111, flags: PTE_R | PTE_V, asid: 1,
            valid: true, size: PageSize::Page4K,
        });
        tlb.invalidate_asid(1);
        assert!(tlb.lookup(0x1000, 1).is_none());
    }
}

#[cfg(test)]
mod sv39_tests {
    use super::*;

    struct MockMem {
        pages: std::collections::HashMap<u64, u64>,
    }
    impl FunctionalMem for MockMem {
        fn read_functional(&self, addr: u64, _size: usize) -> u64 {
            *self.pages.get(&addr).unwrap_or(&0)
        }
        fn write_functional(&mut self, addr: u64, _size: usize, val: u64) {
            self.pages.insert(addr, val);
        }
        fn load_bytes(&mut self, _addr: u64, _data: &[u8]) { todo!() }
    }

    #[test]
    fn test_sv39_basic_4k_walk() {
        // Build a minimal page table:
        // root PPN = 0x1 (at PA 0x1000)
        // L2 PTE for VPN[2]=0: points to L1 at PPN 0x2
        // L1 PTE for VPN[1]=0: points to L0 at PPN 0x3
        // L0 PTE for VPN[0]=1: leaf, PPN=0xABC, R|W|V
        let mut mem = MockMem { pages: std::collections::HashMap::new() };

        // PTE format: PPN[53:10] | flags[7:0]
        // L2[0]: next table at PPN 0x2 → PA 0x2000
        mem.pages.insert(0x1000 + 0 * 8, (0x2 << 10) | PTE_V);
        // L1[0]: next table at PPN 0x3 → PA 0x3000
        mem.pages.insert(0x2000 + 0 * 8, (0x3 << 10) | PTE_V);
        // L0[1]: leaf, PPN=0xABC, RW
        mem.pages.insert(0x3000 + 1 * 8, (0xABC << 10) | PTE_V | PTE_R | PTE_W);

        // satp: PPN = 0x1 (root at 0x1000)
        let satp = 0x1u64;
        // VA: VPN[2]=0, VPN[1]=0, VPN[0]=1, offset=0x42
        let va = (0 << 30) | (0 << 21) | (1 << 12) | 0x42u64;

        let pa = sv39_walk(satp, va, AccessType::Load, &mem).expect("walk should succeed");
        assert_eq!(pa, (0xABC << 12) | 0x42);
    }
}
```

### Property-Based Tests (proptest)

```rust
#[cfg(test)]
mod proptest_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Random (addr, size) pairs never panic on lookup — they return Some or None cleanly.
        #[test]
        fn flatview_lookup_never_panics(addr in 0u64..=u64::MAX) {
            let mut fv = FlatView { ranges: vec![] };
            // Populate with a known range
            fv.ranges.push(FlatRange { base: 0x1000, size: 0x1000, region_type: RegionType::Ram });
            let _result = fv.lookup(addr);  // must not panic
        }

        /// Write then read roundtrip equality for RAM regions.
        #[test]
        fn write_read_roundtrip(
            offset in 0usize..0x100,
            val in any::<u64>(),
            size in 1usize..=8usize,
        ) {
            let size = size.next_power_of_two().min(8);
            let mut data = vec![0u8; 0x200];
            let mask = if size < 8 { (1u64 << (size * 8)) - 1 } else { u64::MAX };
            let val = val & mask;
            write_le_u64(&mut data[offset..offset + size], size, val);
            let read_back = read_le_u64(&data[offset..offset + size], size);
            prop_assert_eq!(val, read_back);
        }
    }
}
```

### MMIO Dispatch Test with Mock Device

```rust
#[cfg(test)]
mod mmio_dispatch_tests {
    use super::*;

    struct MockUart {
        tx_data: u8,
        rx_data: u8,
    }

    impl Device for MockUart {
        fn read(&self, offset: u64, _size: usize) -> u64 {
            match offset {
                0x00 => self.rx_data as u64,  // RX FIFO
                0x04 => 0x20,                  // status: TX empty
                _ => 0,
            }
        }

        fn write(&mut self, offset: u64, _size: usize, val: u64) {
            if offset == 0x00 { self.tx_data = val as u8; }
        }

        fn mmio_size(&self) -> u64 { 0x1000 }
    }

    #[test]
    fn test_mmio_read_dispatch() {
        let uart = Box::new(MockUart { tx_data: 0, rx_data: 0x41 /* 'A' */ });
        let mut map = MemoryMap::new();
        map.add_mmio_device(0x1000_0000, uart);

        let (val, _cycles) = map.read_atomic(0x1000_0000, 1).expect("mmio read");
        assert_eq!(val, 0x41);  // RX data
    }

    #[test]
    fn test_mmio_write_dispatch() {
        let uart = Box::new(MockUart { tx_data: 0, rx_data: 0 });
        let mut map = MemoryMap::new();
        map.add_mmio_device(0x1000_0000, uart);

        map.write_atomic(0x1000_0000, 1, 0x42).expect("mmio write");
        // Verify by reading back the tx_data via a direct device reference
        // (in practice, the device exposes a test accessor or the test reads a status reg)
    }

    #[test]
    fn test_reserved_region_faults() {
        let map = MemoryMap::new_with_reserved(0xdead_0000, 0x1000);
        let err = map.read_atomic(0xdead_0000, 4).expect_err("should fault");
        assert!(matches!(err, MemFault::UnmappedAddress(0xdead_0000)));
    }
}
```

---

## Appendix: Struct and Trait Index

| Item | Section | Purpose |
|------|---------|---------|
| `MemoryRegion` | §2 | Tree node: Ram, Rom, Mmio, Alias, Container, Reserved |
| `Device` | §2 | MMIO handler trait |
| `FlatRange` | §3 | Single resolved address range |
| `FlatView` | §3 | Sorted, non-overlapping address map |
| `AtomicMem` | §4 | Synchronous access with estimated latency |
| `FunctionalMem` | §4 | Side-effect-free debug access |
| `TimingMem` | §4 | Async, flow-controlled cycle-accurate access |
| `MemRequest` / `MemResponse` | §4 | Timing mode message types |
| `CacheModel` | §5 | Set-associative cache with LRU |
| `CacheSet` / `CacheLine` | §5 | Cache storage |
| `CacheStats` | §5 | Hit/miss/eviction/writeback counters |
| `Mshr` / `MshrEntry` | §5 | Miss coalescing registers |
| `TlbModel` / `TlbEntry` | §6 | TLB with ASID support |
| `PageSize` | §6 | 4KB / 2MB / 1GB page granules |
| `sv39_walk` | §7 | RISC-V Sv39 three-level page table walk |
| `sv48_walk` | §7 | RISC-V Sv48 four-level extension |
| `aarch64_walk_4k` | §7 | AArch64 four-level 4KB walk |
| `MemoryMap` | §8 | Owner of tree + FlatView + handlers |
| `MemFault` | §10 | Unified memory error type |

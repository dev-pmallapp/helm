# helm-memory — LLD: MemoryRegion and MemoryMap

> **Status:** Draft — Phase 1 target
> **Covers:** `MemoryRegion` enum, `MemoryMap` struct, public API, access modes, `MemFault`

---

## 1. `MemoryRegion` Enum

The fundamental unit of address space modeling. Every range in a physical address space is exactly one variant.

```rust
/// A node in the physical address space tree.
///
/// Regions are composed into a tree via `Container`. The tree is flattened into
/// a `FlatView` for O(log n) access-time lookup. Region nodes themselves are
/// never walked during normal simulation — only during `FlatView` recomputation.
pub enum MemoryRegion {
    /// Mutable backing store. Read/write by byte offset into `data`.
    /// Owned by `MemoryMap`; no external aliasing into this Vec outside of
    /// `Alias` regions pointing at the same `Arc`.
    Ram {
        data: Vec<u8>,
        /// Size is implicit: `data.len()`. Stored separately for zero-size guards.
        size: u64,
    },

    /// Immutable backing store. Write → `MemFault::ReadOnly`.
    /// Same layout as Ram; enforced at the `MemoryMap::write` layer.
    Rom {
        data: Vec<u8>,
        size: u64,
    },

    /// Dispatches reads/writes to a device handler.
    /// `Box<dyn Device>` is justified: MMIO is a cold path (device I/O, not
    /// per-instruction), so vtable cost is negligible.
    ///
    /// Q27: `MemoryMap` owns the `Box<dyn Device>` directly. Device lifecycle
    /// (init/elaborate/startup) is the caller's responsibility before handing
    /// ownership to `MemoryMap`.
    Mmio {
        handler: Box<dyn Device>,
        size: u64,
    },

    /// Transparent view into another region at a fixed byte offset.
    ///
    /// Access to `[base, base+size)` of this alias translates to:
    ///   `target[target_offset + (addr - alias_base), ...]`
    ///
    /// Q28: The alias itself is a `Arc<MemoryRegion>` so multiple aliases can
    /// point at the same backing region without copying. The `FlatView`
    /// flattener resolves aliases recursively during recomputation.
    Alias {
        target: Arc<MemoryRegion>,
        /// Byte offset into `target` where this alias begins.
        target_offset: u64,
        size: u64,
    },

    /// Groups subregions. Provides no backing store itself.
    /// Used for: SoC bus segments, PCIe root complexes, platform memory maps.
    ///
    /// Each subregion is placed at `(base_offset, region)` relative to the
    /// Container's own base address. Subregions may overlap; the last-added
    /// wins (Q25, QEMU semantics). The `FlatView` flattener applies this
    /// priority when merging overlapping subregion ranges.
    Container {
        /// `(offset_within_container, region)` pairs, in insertion order.
        /// Insertion order determines priority: last element wins on overlap.
        subregions: Vec<(u64, MemoryRegion)>,
        size: u64,
    },

    /// Placeholder for address ranges that must not be accessed.
    /// Any read or write → `MemFault::AccessFault`.
    /// Used for: guard pages, DRAM holes, platform firmware reserved ranges.
    Reserved {
        size: u64,
    },
}

impl MemoryRegion {
    /// Returns the size of this region in bytes.
    pub fn size(&self) -> u64 {
        match self {
            Self::Ram { size, .. }       => *size,
            Self::Rom { size, .. }       => *size,
            Self::Mmio { size, .. }      => *size,
            Self::Alias { size, .. }     => *size,
            Self::Container { size, .. } => *size,
            Self::Reserved { size }      => *size,
        }
    }

    /// Returns the region type tag for use in `FlatRange`.
    pub fn region_type(&self) -> RegionType {
        match self {
            Self::Ram { .. }       => RegionType::Ram,
            Self::Rom { .. }       => RegionType::Rom,
            Self::Mmio { .. }      => RegionType::Mmio,
            Self::Alias { .. }     => RegionType::Alias,
            Self::Container { .. } => RegionType::Container,
            Self::Reserved { .. }  => RegionType::Reserved,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionType {
    Ram, Rom, Mmio, Alias, Container, Reserved,
}
```

---

## 2. `MemoryMap` Struct

`MemoryMap` is the top-level address space object. It owns the `MemoryRegion` tree, the cached `FlatView`, and the current access mode state.

```rust
/// The physical address space of one hart (or a shared system bus).
///
/// Invariants:
/// - `mode` is `Atomic` or `Functional` unless `timing_enabled` is set.
/// - `timing_enabled` and any outstanding `AtomicMem` use are mutually exclusive.
/// - `flat_view` is always valid (non-dirty) at the point of any lookup.
pub struct MemoryMap {
    /// The root region. Typically a `Container` at address 0.
    root: MemoryRegion,

    /// Cached resolved view. Rebuilt lazily when `dirty` is true.
    flat_view: FlatView,

    /// Set to true whenever `root` is mutated (add/remove subregion).
    dirty: bool,

    /// Current access mode. Timing and Atomic are mutually exclusive.
    mode: AccessMode,

    /// In-flight timing requests. Non-empty ⟹ mode must be Timing.
    pending_timing: Vec<TimingRequest>,

    /// Listeners notified after each FlatView recomputation.
    listeners: Vec<Box<dyn MemoryListener>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    /// Synchronous, returns (data, latency). Used in fast-forward / SE mode.
    Atomic,
    /// Asynchronous, events-driven. Used in Virtual/Accurate timing mode.
    Timing,
    /// Synchronous, no side effects. Always available.
    Functional,
}
```

---

## 3. Public API

### 3.1 Region Management

```rust
impl MemoryMap {
    /// Create a new address space with the given root region.
    /// Typically `root` is `MemoryRegion::Container` covering the full address range.
    pub fn new(root: MemoryRegion) -> Self;

    /// Add a subregion to a `Container` at `path` within the tree.
    ///
    /// `path` identifies the target `Container` by a sequence of subregion indices.
    /// An empty path means the root. Panics if the target is not a `Container`.
    ///
    /// Q25: If the new region overlaps existing subregions, it wins (last-added priority).
    /// Q26: Sets `dirty = true`; FlatView is not recomputed until next lookup.
    /// Q29: Supported. In-flight Timing requests are not affected by the mutation;
    ///       callers are responsible for draining pending_timing before structural changes
    ///       that would invalidate outstanding requests.
    pub fn add_region(&mut self, path: &[usize], offset: u64, region: MemoryRegion);

    /// Remove a subregion at `path` within the tree.
    ///
    /// Q29: Same invalidation protocol as `add_region`.
    pub fn remove_region(&mut self, path: &[usize]) -> Option<MemoryRegion>;

    /// Returns the FlatView (recomputing if dirty).
    pub fn flat_view(&mut self) -> &FlatView;
}
```

### 3.2 Scalar Read/Write

```rust
impl MemoryMap {
    /// Atomic read: synchronous, returns (value, estimated_latency_cycles).
    /// Width: 1, 2, 4, or 8 bytes. Returns `MemFault::BusError` on misalignment
    /// (configurable; some platforms allow unaligned).
    pub fn read_atomic(&mut self, addr: u64, width: usize) -> Result<(u64, u64), MemFault>;

    /// Atomic write: synchronous, returns estimated_latency_cycles.
    pub fn write_atomic(&mut self, addr: u64, width: usize, val: u64) -> Result<u64, MemFault>;

    /// Functional read: no side effects, no cache fill, no TLB update.
    /// Always succeeds unless the address is `Reserved` or completely unmapped.
    pub fn read_functional(&self, addr: u64, width: usize) -> Result<u64, MemFault>;

    /// Functional write: no side effects. Writes directly to backing store.
    pub fn write_functional(&mut self, addr: u64, width: usize, val: u64) -> Result<(), MemFault>;
}
```

### 3.3 Bulk Byte Read/Write

```rust
impl MemoryMap {
    /// Copy `len` bytes from `addr` into `buf` using Functional mode.
    /// Used by the binary loader and GDB memory commands.
    /// Fails with `MemFault::AccessFault` if any byte falls in a `Reserved` region.
    pub fn read_bytes(&self, addr: u64, buf: &mut [u8]) -> Result<(), MemFault>;

    /// Copy `buf` bytes to `addr` using Functional mode.
    pub fn write_bytes(&mut self, addr: u64, buf: &[u8]) -> Result<(), MemFault>;
}
```

### 3.4 Timing Mode API

```rust
impl MemoryMap {
    /// Switch to Timing mode. Panics if there are outstanding Atomic operations
    /// (checked only in debug builds; release builds return Err).
    pub fn enable_timing(&mut self) -> Result<(), MemFault>;

    /// Switch back to Atomic mode. Drains `pending_timing` first.
    /// Blocks (spin) until all pending timing requests complete.
    pub fn disable_timing(&mut self);

    /// Issue a Timing read request.
    ///
    /// Returns a `RequestId`. The result is delivered by calling `complete_timing`
    /// in the simulation loop. Q8: The CPU owns in-flight request tracking via
    /// the `RequestId`; `MemoryMap` stores the in-progress state in `pending_timing`.
    pub fn request_timing_read(
        &mut self,
        addr: u64,
        width: usize,
        callback: Box<dyn FnOnce(Result<u64, MemFault>) + Send>,
    ) -> Result<RequestId, MemFault>;

    /// Issue a Timing write request.
    pub fn request_timing_write(
        &mut self,
        addr: u64,
        width: usize,
        val: u64,
        callback: Box<dyn FnOnce(Result<(), MemFault>) + Send>,
    ) -> Result<RequestId, MemFault>;

    /// Called by the event loop to deliver a completed timing response.
    /// Invokes the stored callback and removes the request from `pending_timing`.
    pub fn complete_timing(&mut self, id: RequestId);

    /// Number of in-flight timing requests.
    pub fn pending_count(&self) -> usize;
}
```

---

## 4. Atomic vs. Functional Distinction

Both Atomic and Functional are synchronous, but they differ in side effects:

| Property | Atomic | Functional |
|----------|--------|------------|
| Cache fill on hit/miss | Yes | No |
| TLB update on walk | Yes | No |
| Latency returned | Yes (estimated cycles) | No (always 0) |
| MMIO device callback | Yes | Yes (necessary for GDB register reads) |
| Can be used during Timing mode | No | Yes |
| Modifies MemoryMap state | Yes (cache, TLB) | No (only backing store for writes) |

Functional mode is used by:
- GDB RSP stub: read/write registers and memory mid-simulation without disturbing cache state.
- Binary loader: initialize RAM before `startup()`.
- Page table walker: read PTEs without triggering TLB fills (Q36).

---

## 5. MMIO Dispatch

When a lookup resolves to a `RegionType::Mmio` `FlatRange`, the access is dispatched to the `Device` handler stored in the `MemoryRegion::Mmio` node.

### Offset Calculation

```
Physical address:   addr
FlatRange base:     range.base
Device offset:      offset = addr - range.base

Device::read(offset, width)   → u64
Device::write(offset, width, val)
```

The device receives only the byte offset within its mapped region (Q27). It has no knowledge of its base address. This matches real hardware: the device IP block sees only internal register offsets.

### MMIO in Functional Mode

Functional mode still dispatches to `Device::read` / `Device::write`. This is necessary for GDB to read device registers. However, any device state side effects (e.g., clearing a status register on read) occur. Functional mode prevents *cache and TLB* side effects, not device side effects.

If a device must distinguish functional from atomic access (e.g., to suppress FIFO drain on debugger read), the `Device` trait may expose an optional `read_functional` method; the default implementation falls through to `read`.

---

## 6. MemFault Enum

Returned by all access functions. Variants map to ISA exception causes.

```rust
/// A memory access failure.
///
/// Conversion to ISA exception codes is done in `helm-arch`:
///   - RISC-V: `mcause` value (see RISC-V Privileged Spec §3.1.15)
///   - AArch64: `ESR_EL1.EC` + `ISS` field (AArch64 Architecture Reference Manual §D17)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemFault {
    /// Address not mapped to any region. RISC-V: load/store access fault (cause 5/7).
    /// AArch64: Translation fault (EC=0b100100 + level).
    AccessFault { addr: u64 },

    /// Address mapped to a `Reserved` region. Same ISA encoding as `AccessFault`.
    ReservationFault { addr: u64 },

    /// Write to a `Rom` region. RISC-V: store access fault (cause 7).
    /// AArch64: Permission fault (EC=0b100101).
    ReadOnly { addr: u64 },

    /// Virtual-to-physical translation failed (page not present, invalid PTE).
    /// RISC-V: load/store page fault (cause 13/15). AArch64: Translation fault.
    PageFault { va: u64, level: u8 },

    /// Misaligned access (when platform enforces alignment).
    /// RISC-V: load/store address misaligned (cause 4/6). AArch64: Alignment fault.
    AlignmentFault { addr: u64, width: usize },

    /// Timing/Atomic mode conflict. Not an ISA exception; a simulator invariant
    /// violation. The caller should panic in debug mode.
    ModeMismatch { current: AccessMode, requested: AccessMode },
}
```

### Conversion Helpers

```rust
impl MemFault {
    /// RISC-V mcause value for this fault.
    /// Returns `None` for `ModeMismatch` (not an ISA-level fault).
    pub fn riscv_mcause(&self, is_store: bool) -> Option<u64>;

    /// AArch64 ESR_EL1 encoding for this fault.
    pub fn aarch64_esr(&self, is_store: bool) -> Option<u32>;
}
```

---

## 7. Dynamic Region Add/Remove Protocol

Supporting PCIe BAR remapping and hotplug (Q29) requires a safe mutation protocol:

```
1. Caller signals intent to modify the address space.
2. MemoryMap::pending_count() must return 0 (no in-flight Timing requests).
   If non-zero, caller must call disable_timing() first to drain them.
3. Caller calls add_region() or remove_region().
4. MemoryMap sets dirty = true.
5. FlatView is rebuilt lazily on next lookup.
6. MemoryListeners are notified after rebuild (step 5), so caches can
   invalidate tags covering remapped physical addresses.
```

In practice, PCIe BAR reprogramming happens during device initialization (cold path), not during simulation hot loops. The protocol above is sufficient for Phase 1.

---

## 8. MemoryListener

Used to notify subsystems (primarily `CacheModel`) when the physical address space changes.

```rust
/// Called after FlatView is recomputed following an `add_region` / `remove_region`.
pub trait MemoryListener: Send {
    /// Invoked with the old and new FlatViews.
    /// The cache should invalidate any lines whose physical address now maps
    /// to a different region type (e.g., was RAM, now MMIO after PCIe remap).
    fn on_region_change(&mut self, old: &FlatView, new: &FlatView);
}
```

`CacheModel` registers a `MemoryListener` during `elaborate()` to invalidate cache lines covering remapped addresses.

---

## 9. Module Layout

```
helm-memory/src/
├── lib.rs             — pub use re-exports
├── region.rs          — MemoryRegion enum, RegionType
├── map.rs             — MemoryMap, AccessMode, RequestId, TimingRequest
├── fault.rs           — MemFault, ISA conversion helpers
├── flat_view.rs       — FlatRange, FlatView, MemoryListener (see LLD-flat-view.md)
├── cache/
│   ├── mod.rs
│   ├── model.rs       — CacheModel, CacheSet, CacheLine (see LLD-cache-tlb.md)
│   ├── config.rs      — CacheConfig
│   └── mshr.rs        — MshrFile
└── tlb/
    ├── mod.rs
    ├── model.rs       — TlbModel, TlbEntry, TlbConfig (see LLD-cache-tlb.md)
    ├── sv39.rs        — RISC-V Sv39/Sv48 page table walker
    └── aarch64.rs     — AArch64 4KB 4-level page table walker
```

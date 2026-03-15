# helm-memory — LLD: FlatView

> **Status:** Draft — Phase 1 target
> **Covers:** `FlatRange`, `FlatView`, recompute algorithm, binary search, alias expansion, MemoryListener

---

## 1. Overview

`FlatView` is the resolved, non-overlapping representation of a physical address space. Where the `MemoryRegion` tree captures *structural intent* (a Container with subregions, aliases into other regions), `FlatView` captures *access-time truth*: for any given physical address, exactly one `FlatRange` applies.

The invariant is:

> At any point of any memory access, `FlatView` is consistent with the current `MemoryRegion` tree (the `dirty` flag is false).

---

## 2. `FlatRange` Struct

```rust
/// One contiguous, non-overlapping physical address range in the resolved view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatRange {
    /// Inclusive start of the physical address range.
    pub base: u64,
    /// Size in bytes. `base + size` is the exclusive end.
    pub size: u64,
    /// What kind of region backs this range.
    pub kind: RegionType,
    /// Offset within the backing region where this range begins.
    /// For Ram/Rom: index into `data` Vec.
    /// For Mmio: byte offset passed to `Device::read/write`.
    /// For Alias: resolved to the target's backing offset during flatten.
    pub region_offset: u64,
    /// Stable index back into the `MemoryMap`'s region storage, used to
    /// retrieve the actual `MemoryRegion` node for dispatch.
    pub region_id: RegionId,
}

/// Opaque identifier for a region node within the `MemoryMap` tree.
/// Assigned during tree construction; stable until the node is removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegionId(u32);
```

---

## 3. `FlatView` Struct

```rust
/// The resolved physical address map: a sorted list of non-overlapping ranges.
///
/// Invariants:
/// - Ranges are sorted by `base` in ascending order.
/// - No two ranges overlap.
/// - Every byte not covered by any range is an implicit `Reserved` hole.
pub struct FlatView {
    /// Sorted, non-overlapping ranges covering the mapped address space.
    ranges: Vec<FlatRange>,
    /// True if the backing `MemoryRegion` tree has changed since last recompute.
    dirty: bool,
}

impl FlatView {
    pub fn new() -> Self {
        FlatView { ranges: Vec::new(), dirty: true }
    }

    /// Mark as needing recomputation.
    pub(crate) fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// True if the view needs to be recomputed before use.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Recompute from `root`, replacing the current `ranges`.
    /// Notifies all `listeners` after successful recompute.
    pub(crate) fn recompute(
        &mut self,
        root: &MemoryRegion,
        root_base: u64,
        listeners: &mut [Box<dyn MemoryListener>],
    ) {
        let old = std::mem::take(&mut self.ranges);
        let mut builder = FlatViewBuilder::new();
        builder.flatten(root, root_base, 0, root.size());
        self.ranges = builder.finish();
        self.dirty = false;
        let new_view = FlatView { ranges: self.ranges.clone(), dirty: false };
        let old_view = FlatView { ranges: old, dirty: false };
        for listener in listeners.iter_mut() {
            listener.on_region_change(&old_view, &new_view);
        }
    }

    /// Binary-search lookup: find the `FlatRange` covering `addr`.
    ///
    /// Returns `None` if `addr` falls in an unmapped hole.
    /// O(log n) where n = number of mapped ranges.
    pub fn lookup(&self, addr: u64) -> Option<&FlatRange> {
        debug_assert!(!self.dirty, "lookup called on dirty FlatView");
        // Find the last range whose base ≤ addr.
        let idx = self.ranges.partition_point(|r| r.base <= addr);
        if idx == 0 {
            return None;
        }
        let range = &self.ranges[idx - 1];
        if addr < range.base + range.size {
            Some(range)
        } else {
            None // addr falls in a gap between ranges
        }
    }

    /// Returns all ranges (for iteration, testing, MemoryListener diffing).
    pub fn ranges(&self) -> &[FlatRange] {
        &self.ranges
    }
}
```

---

## 4. Recompute Algorithm

`FlatViewBuilder` does a recursive depth-first traversal of the `MemoryRegion` tree, building a list of `FlatRange` candidates, then resolving overlaps according to priority rules.

### 4.1 Traversal

```rust
struct FlatViewBuilder {
    /// Collected ranges before deduplication, in tree-traversal order.
    candidates: Vec<FlatRangeCandidate>,
    /// Monotonically increasing priority; higher = wins on overlap.
    priority: u32,
}

struct FlatRangeCandidate {
    base:          u64,
    size:          u64,
    kind:          RegionType,
    region_offset: u64,
    region_id:     RegionId,
    priority:      u32,
}

impl FlatViewBuilder {
    fn flatten(
        &mut self,
        region: &MemoryRegion,
        // Absolute physical address of this region's base.
        phys_base: u64,
        // Byte offset within the region's own backing store (for alias chains).
        backing_offset: u64,
        // Byte length to map.
        size: u64,
    ) {
        match region {
            MemoryRegion::Ram { .. } | MemoryRegion::Rom { .. } => {
                self.emit(phys_base, size, region.region_type(), backing_offset, region.id());
            }

            MemoryRegion::Mmio { .. } => {
                self.emit(phys_base, size, RegionType::Mmio, backing_offset, region.id());
            }

            MemoryRegion::Reserved { .. } => {
                self.emit(phys_base, size, RegionType::Reserved, 0, region.id());
            }

            // Q28: Alias — resolve the target offset chain.
            // An alias `[base, base+size)` maps to `target[target_offset, target_offset+size)`.
            // If the target is itself an alias, recurse until a leaf is found.
            MemoryRegion::Alias { target, target_offset, size: alias_size } => {
                let effective_size = (*alias_size).min(size);
                self.flatten(
                    target,
                    phys_base,
                    backing_offset + target_offset,
                    effective_size,
                );
            }

            // Container: recurse into subregions, in insertion order.
            // Each subregion gets its own priority level; later subregions
            // have higher priority (Q25: last added wins).
            MemoryRegion::Container { subregions, size: container_size } => {
                let container_end = phys_base + (*container_size).min(size);
                for (sub_offset, sub_region) in subregions.iter() {
                    let sub_base = phys_base + sub_offset;
                    // Clamp to the container's physical range.
                    if sub_base >= container_end {
                        continue;
                    }
                    let sub_size = sub_region.size().min(container_end - sub_base);
                    self.priority += 1;
                    self.flatten(sub_region, sub_base, 0, sub_size);
                }
            }
        }
    }

    fn emit(&mut self, base: u64, size: u64, kind: RegionType,
            region_offset: u64, region_id: RegionId) {
        self.candidates.push(FlatRangeCandidate {
            base, size, kind, region_offset, region_id,
            priority: self.priority,
        });
    }
}
```

### 4.2 Overlap Resolution

After collecting all candidates, `finish()` produces a sorted, non-overlapping `Vec<FlatRange>`.

```rust
impl FlatViewBuilder {
    /// Produce the final sorted, non-overlapping FlatRange list.
    ///
    /// Algorithm:
    ///   1. Sort candidates by (base ASC, priority DESC) — within the same
    ///      byte, the highest-priority (last-added) candidate wins.
    ///   2. Sweep left to right; clip lower-priority candidates that overlap
    ///      a higher-priority one already committed.
    ///   3. Emit non-zero-size surviving ranges, sorted by base.
    fn finish(self) -> Vec<FlatRange> {
        let mut candidates = self.candidates;
        // Sort: by base ascending; ties broken by priority descending.
        candidates.sort_unstable_by(|a, b| {
            a.base.cmp(&b.base).then(b.priority.cmp(&a.priority))
        });

        let mut output: Vec<FlatRange> = Vec::with_capacity(candidates.len());

        for c in candidates {
            let c_end = c.base + c.size;

            // Find how much of this candidate is already covered by higher-priority
            // ranges already in `output` (they have higher priority, so they win).
            // `output` is sorted by base; scan backwards from the end.
            let mut remaining_base = c.base;

            for existing in output.iter().rev() {
                let ex_end = existing.base + existing.size;
                if existing.base >= c_end {
                    continue; // existing is entirely after c
                }
                if ex_end <= remaining_base {
                    break; // existing is entirely before remaining portion of c
                }
                // existing and c overlap; existing wins (it was added with higher priority
                // OR it appeared earlier in the sorted order with same priority).
                // Clip c from remaining_base to the overlap start (if any gap before).
                if existing.base > remaining_base {
                    // Emit the gap [remaining_base, existing.base) from c.
                    let gap_size = existing.base - remaining_base;
                    output.push(FlatRange {
                        base: remaining_base,
                        size: gap_size,
                        kind: c.kind,
                        region_offset: c.region_offset + (remaining_base - c.base),
                        region_id: c.region_id,
                    });
                }
                // Skip past the existing range.
                remaining_base = ex_end.max(remaining_base);
            }

            // Emit any remaining tail of c.
            if remaining_base < c_end {
                output.push(FlatRange {
                    base: remaining_base,
                    size: c_end - remaining_base,
                    kind: c.kind,
                    region_offset: c.region_offset + (remaining_base - c.base),
                    region_id: c.region_id,
                });
            }
        }

        // Final sort (insertions above may be out of order after clipping).
        output.sort_unstable_by_key(|r| r.base);
        output
    }
}
```

### 4.3 Priority Rules Summary

| Scenario | Winner |
|----------|--------|
| Two subregions at the same offset in a Container | Last added (higher `priority` value) |
| Container subregion vs. sibling Container | The one added later |
| Alias expansion overlapping a RAM region | Determined by insertion order in the parent Container |
| Reserved vs. anything | Reserved wins only if added after (same priority rule) |

This matches QEMU FlatView semantics (Q25).

---

## 5. Alias Expansion

Aliases are expanded during `flatten()` by recursing into the target region with an adjusted `backing_offset`. The alias itself does not appear in the final `FlatView` — only its resolved backing type does.

Example:

```
Root (Container, 0x0–0xFFFF_FFFF)
├── RAM at 0x0000_0000, size 64MB
├── ROM at 0x2000_0000, size 1MB
└── Alias at 0x8000_0000, target=ROM, target_offset=0, size=1MB
```

Flat result:

```
[0x0000_0000, 64MB)    → Ram,  region_offset=0
[0x2000_0000, 1MB)     → Rom,  region_offset=0
[0x8000_0000, 1MB)     → Rom,  region_offset=0   ← alias resolved
```

Both `0x2000_0000` and `0x8000_0000` point to the same ROM backing store (same `region_id`), at `region_offset=0`.

---

## 6. MemoryListener

```rust
/// Receives a notification after each FlatView recomputation.
/// Registered on `MemoryMap` during `elaborate()`.
pub trait MemoryListener: Send {
    /// Called with the old and new views.
    ///
    /// Implementors should diff the views and invalidate any cached state
    /// (e.g., cache lines, icache blocks) covering addresses that changed
    /// region type or backing store.
    fn on_region_change(&mut self, old: &FlatView, new: &FlatView);
}
```

### CacheInvalidatingListener

The primary implementor. Registered by `CacheModel` during `elaborate()`.

```rust
pub struct CacheInvalidatingListener {
    cache: Arc<Mutex<CacheModel>>,
}

impl MemoryListener for CacheInvalidatingListener {
    fn on_region_change(&mut self, old: &FlatView, new: &FlatView) {
        // Find physical addresses that changed region_id between old and new.
        // For each such address (aligned to cache line size), invalidate the
        // corresponding cache set/way by clearing the valid bit.
        let changed = diff_flat_views(old, new);
        let mut cache = self.cache.lock().unwrap();
        for addr in changed {
            cache.invalidate_line(addr);
        }
    }
}

/// Returns the set of physical addresses (cache-line aligned) that
/// differ between `old` and `new`.
fn diff_flat_views(old: &FlatView, new: &FlatView) -> Vec<u64> {
    // Walk both sorted lists simultaneously (merge-join).
    // Collect any range where the region_id differs.
    todo!()
}
```

---

## 7. Lookup Performance

- `FlatView::lookup` uses `partition_point` (standard library binary search): O(log n).
- For a typical platform (≤ 64 mapped regions), `n ≤ 64`, so lookup is ≤ 6 comparisons.
- The `FlatView` is a `Vec<FlatRange>` so it is cache-friendly for sequential access patterns (e.g., instruction fetch streaming).
- The `MemoryMap` ensures `dirty = false` before returning from `flat_view()`, so callers always see a valid view.

---

## 8. Example: Typical RISC-V Virt Platform

After `recompute()`, a typical RISC-V virt board produces approximately:

```
base=0x0000_0000  size=0x0200_0000  kind=Mmio   (CLINT)
base=0x0200_0000  size=0x0400_0000  kind=Reserved
base=0x0C00_0000  size=0x0400_0000  kind=Mmio   (PLIC)
base=0x1000_0000  size=0x0000_1000  kind=Mmio   (UART 16550)
base=0x2000_0000  size=0x0200_0000  kind=Rom    (boot ROM)
base=0x8000_0000  size=0x8000_0000  kind=Ram    (DRAM, 2GB)
```

Lookup for address `0x8000_1234`: `partition_point` finds the RAM range, returns `region_offset=0x1234`.

---

## Design Decisions from Q&A

### Design Decision: Last-added-wins for overlapping subregions (Q25)

When a `Container` region has two or more children whose address ranges overlap, the last-added child takes precedence at overlapping addresses. `FlatView` recomputation iterates `Container::children` in reverse insertion order; earlier children are shadowed by later ones. Documentation must warn that `add_region` ordering is significant when overlaps are intentional. An explicit priority field may be added in Phase 1 if needed.

### Design Decision: Lazy FlatView recomputation (Q26)

`FlatView` is rebuilt on the next lookup call when `dirty` is set (as implemented). `add_region()` and `remove_region()` set `dirty = true` and fire no callbacks immediately. `MemoryListener::region_add` / `region_del` are fired synchronously inside `ensure_flat_view()`. In practice, all regions are added during configuration/elaboration before any simulation access — lazy evaluation means the rebuild cost is paid exactly once, on the first memory access.

### Design Decision: MemoryMap owns Box<dyn Device> (Q27)

`MemoryMap` owns `Box<dyn Device>` directly inside the `MemoryRegion::Mmio` variant. MMIO dispatch is on the access critical path — eliminating a registry hash-lookup per access is the primary justification. The `Device` trait (or a trimmed `MmioHandler` trait) must be defined in `helm-core` so that `helm-memory` can reference it without depending on `helm-devices`. `World` may hold a separate reference (`Arc`) to each device for Python inspection and lifecycle management.

### Design Decision: Alias resolution at FlatView build time (Q28)

`MemoryRegion::Alias { target, offset, size }` variants are chased at FlatView-build time until a non-alias target is found. The resulting `FlatRange` points directly at the concrete backing region with an adjusted offset. At access time there is no alias indirection. `FlatRange::region_offset` carries the resolved `base_offset` so that `read_at(flat_range, local_offset)` computes `backing_store_offset = flat_range.region_offset + local_offset` without re-examining the alias chain. Alias-of-alias depth is bounded (max 8 levels) to detect configuration cycles.

### Design Decision: Dynamic add/remove with pending_timing_count check (Q29)

Dynamic `add_region` / `remove_region` is supported during simulation. Callers are responsible for draining all in-flight `Timing` requests before mutating the map. This is enforced in debug builds by a runtime check (panic if `pending_timing_count > 0`); in release builds it returns `Err(MemFault::ModeMismatch)`. `MemoryMap` maintains a `pending_timing_count: AtomicU32` that `Timing` requests increment/decrement.

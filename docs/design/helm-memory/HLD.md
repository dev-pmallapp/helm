# helm-memory — High-Level Design

> **Status:** Draft — Phase 1 target
> **Crate:** `helm-memory`
> **Depends on:** `helm-core` only
> **Provides to:** `helm-engine`, `helm-arch`, `helm-timing`, `helm-devices`

---

## 1. Purpose

`helm-memory` implements the unified memory system for helm-ng. It answers three questions for every simulated memory access:

1. **Where does this physical address map?** — ROM, RAM, MMIO, reserved, or aliased region?
2. **What access semantics apply?** — Atomic (fast-forward), Functional (debugger/loader), or Timing (event-driven simulation)?
3. **What is the microarchitectural cost?** — TLB hit/miss, cache hit/miss, MSHR stall?

The crate provides address space modeling, cache and TLB simulation, and virtual memory translation. It does **not** contain ISA-specific instruction decode, device business logic, or the event queue.

---

## 2. Scope

### In scope

| Concern | Component |
|---------|-----------|
| Physical address space layout | `MemoryRegion` tree + `FlatView` |
| RAM, ROM, MMIO dispatch, alias regions | `MemoryRegion` enum |
| Three memory access modes | `MemInterface` trait + `MemoryMap` impl |
| Set-associative cache simulation | `CacheModel` |
| Per-hart TLB simulation | `TlbModel` |
| Virtual-to-physical address translation | Page table walker functions |
| Memory fault modeling | `MemFault` enum |

### Out of scope

| Concern | Where it lives |
|---------|---------------|
| MMIO device business logic (what a UART does) | `helm-devices` |
| ISA-specific instruction decode | `helm-arch` |
| Discrete event scheduling | `helm-event` |
| Pipeline timing models (OoO window, branch predictor) | `helm-timing` |
| Linux virtual memory syscalls (`mmap`/`brk`) | `helm-engine/se` |
| Cache coherence protocols (MESI, MOESI) | Deferred — Phase 3 |

---

## 3. Three Access Modes

Inspired by Gem5's port model. Every `MemoryMap` access is one of three modes:

```
Atomic      — Synchronous. Returns (data, estimated_latency).
              Used for: fast-forward execution, SE mode functional runs.
              Properties: no callbacks, no queued events, returns immediately.

Functional  — Synchronous, no side effects.
              Used for: GDB read/write, binary loader, page table walker.
              Properties: skips cache fill, skips TLB update, pure read/write of backing store.

Timing      — Asynchronous. Issues a request; result delivered via callback.
              Used for: Virtual and Accurate timing simulation.
              Properties: fires events on cache miss, TLB miss, bus contention.
```

### Mode Invariant

**Timing and Atomic cannot coexist simultaneously.** Switching from Timing to Atomic (or vice versa) requires draining all in-flight Timing requests first. Violation is a runtime panic in debug builds and an `Err(MemFault::ModeMismatch)` in release builds.

Functional mode can be used at any time — it bypasses the Timing/Atomic state entirely. This allows the GDB stub to inspect memory mid-simulation without disturbing in-flight requests.

```
┌──────────────┐     ┌──────────────┐
│    Timing    │◄───►│   Atomic     │   MUTUALLY EXCLUSIVE
└──────────────┘     └──────────────┘
       ▲                    ▲
       │  both allow        │
       ▼                    ▼
┌────────────────────────────────────┐
│          Functional                │  ALWAYS ALLOWED (no side effects)
└────────────────────────────────────┘
```

---

## 4. MemoryRegion Tree

The physical address space is modeled as a tree of `MemoryRegion` nodes. This is the same structural approach as QEMU's `MemoryRegion` API.

```
MemoryRegion (enum)
├── Ram          — mutable byte store, read/write by offset
├── Rom          — immutable byte store, write raises MemFault::ReadOnly
├── Mmio         — dispatches to Box<dyn Device> handler
├── Alias        — transparent view into another region at an offset
├── Container    — groups subregions (PCIe root, SoC bus, etc.)
└── Reserved     — address space placeholder; any access → MemFault::AccessFault
```

The tree captures *intent*: which ROM is at which base, which PCIe BAR aliases into which backing memory, which guard pages are reserved. The tree is never walked at access time; instead, it is flattened into a `FlatView` for O(log n) lookup.

### Why a Tree?

- **Hierarchical composition**: a PCIe root complex is a `Container` whose children are BAR `Mmio` regions.
- **Alias regions**: mirror ROM without copying bytes.
- **Dynamic reconfiguration**: PCIe hotplug adds/removes a subregion. Only the `FlatView` must be recomputed; the tree mutation is local.
- **Priority semantics**: when two subregions overlap in a `Container`, the last-added wins (Q25), matching QEMU's behavior.

---

## 5. FlatView — Resolved Address Map

`FlatView` is a sorted `Vec<FlatRange>` of non-overlapping physical address ranges. It is the *resolved* representation: the tree is flattened once, and all access-time lookups use the flat list.

- **Recomputed lazily** (Q26): a `dirty` flag is set on every `add_region` / `remove_region`. The flat view is rebuilt on the next lookup if dirty.
- **Lookup**: binary search with `partition_point` → O(log n).
- **MemoryListener callbacks**: fired after recomputation to invalidate cache tags that mapped into remapped regions.

---

## 6. Cache Model

Set-associative cache simulator with pseudo-LRU replacement (PLRU, Q30) and write-back policy (Q31).

```
CacheModel
├── CacheConfig   — size_kb, assoc, line_size, hit_latency, mshrs, write_back
├── Vec<CacheSet> — indexed by (addr >> line_bits) % num_sets
│   ├── Vec<CacheLine>   — tag, valid, dirty, data
│   └── plru_bits: u64   — PLRU tree state, O(1) update
└── MshrFile      — tracks outstanding misses, capacity-limited (Q34)
```

Key decisions:

| Question | Decision |
|----------|----------|
| Q30 | Pseudo-LRU (PLRU) via tree bits. O(1) update, good approximation of true LRU. |
| Q31 | Write-back. Dirty lines evicted to next level; no write-through overhead. |
| Q32 | Cache state persists between Interval timing intervals (warmup effects modeled). |
| Q33 | Shared LLC under `Arc<Mutex<CacheModel>>` for multi-hart (simple; no MESI in Phase 1). |
| Q34 | MSHRs modeled per cache level. `MshrFile` has fixed capacity; excess misses block. |

---

## 7. TLB and Page Table Walker

Per-hart TLB with ASID-aware invalidation and huge page support.

```
TlbModel
├── TlbConfig    — entries, assoc, page_sizes (4KB/2MB/1GB)
├── Vec<TlbSet>  — indexed by vpn % num_sets
│   └── Vec<TlbEntry>  — vpn, ppn, flags, asid, size (PageSize enum)
└── translate(va, asid, access) -> Result<u64, PageFault>
```

Key decisions:

| Question | Decision |
|----------|----------|
| Q35 | All four SFENCE.VMA variants implemented: global, ASID-only, VA-only, ASID+VA. |
| Q36 | Page table walker is a function (`sv39_walk`, `aarch64_walk`), not a hardware component. It calls `FunctionalMem` to read PTEs. |
| Q37 | Huge pages stored as `PageSize::Mega` / `PageSize::Giga` TLB entries; translation offset computed from the appropriate VPN level. |

---

## 8. Dependencies

```
helm-core   — MemInterface trait, MemFault, ExecContext
    ▲
helm-memory — MemoryRegion, MemoryMap, FlatView, CacheModel, TlbModel
    ▲
helm-engine, helm-arch, helm-timing, helm-devices
```

`helm-memory` depends only on `helm-core`. It does not depend on `helm-event`, `helm-arch`, or `helm-devices`. Device business logic is injected as `Box<dyn Device>` at the `MemoryMap` layer, using the `Device` trait defined in `helm-core`.

---

## 9. Key Design Decisions Summary

| ID | Question | Decision |
|----|----------|----------|
| Q25 | Overlapping subregion priority | Last added wins (QEMU semantics) |
| Q26 | FlatView computation | Lazy (dirty flag, recomputed on first lookup after change) |
| Q27 | MMIO handler ownership | `MemoryMap` owns `Box<dyn Device>` directly |
| Q28 | Alias offset calculation | `read(base + off)` → `read_from(target_base + alias.offset + off)` |
| Q29 | Dynamic add/remove | Supported; invalidates FlatView, callers drain Timing before mutation |
| Q30 | Cache replacement | Pseudo-LRU (PLRU) |
| Q31 | Cache write policy | Write-back |
| Q32 | Cache state across intervals | Persists |
| Q33 | Shared LLC | `Arc<Mutex<CacheModel>>` |
| Q34 | MSHR modeling | Per-level, fixed capacity, blocks on overflow |
| Q35 | SFENCE.VMA variants | All four in Phase 0 |
| Q36 | Page table walker | Function calling FunctionalMem |
| Q37 | Huge pages | Supported; `PageSize` enum in `TlbEntry` |

---

## 10. Related Documents

- `LLD-memory-region.md` — `MemoryRegion` enum, `MemoryMap` struct, public API, MMIO dispatch, `MemFault`
- `LLD-flat-view.md` — `FlatRange`, `FlatView`, recompute algorithm, binary search, MemoryListener
- `LLD-cache-tlb.md` — `CacheModel`, `TlbModel`, MSHR, page table walker, RISC-V Sv39 / AArch64
- `TEST.md` — unit tests, property tests, timing invariant tests

# Memory Model

How guest memory works in helm-ng — from virtual addresses down to
simulated DRAM and MMIO.

## Architecture

helm-ng has two memory subsystems serving different execution modes:

- **FlatMem** — sparse RAM with page-table fast path, used in SE mode
- **HelmAddressSpace** — FlatMem + device address map + MMIO dispatch,
  used in FS mode

Both live in the `helm-memory` crate. The CPU accesses memory through
the `MemInterface` trait defined in `helm-core`.

## FlatMem

`FlatMem` is a sparse, demand-paged memory backend ported from the
reference implementation's `address_space.rs`. It uses a flat page
table (`Vec<*mut u8>`) with one entry per 4K page for O(1) host-pointer
lookup.

```text
Guest Address
     │
     ▼
┌──────────────┐
│ Page Table   │  Vec<*mut u8>, indexed by GPA >> 12
│ [page_idx]   │──► host pointer (or null if unmapped)
└──────────────┘
     │
     ▼
┌──────────────┐
│ Host Memory  │  mmap'd regions, copy_nonoverlapping on fast path
└──────────────┘
```

Key properties:
- Page-aligned regions get direct `unsafe copy_nonoverlapping` reads
  and writes — zero allocation, zero bounds checking per access
- Pages are demand-mapped for SE mode scattered ELF segments
- No `HashMap` in the access path

## HelmAddressSpace

`HelmAddressSpace` in `helm-memory/src/address_space.rs` combines
FlatMem with an `AddressMap` for MMIO routing:

```text
Guest Physical Address
     │
     ▼
┌──────────────────────┐
│  AddressMap lookup    │  sorted Vec<(base, size, DeviceId)>
│  (binary search)     │
└────┬────────────┬────┘
     │ RAM        │ MMIO
     ▼            ▼
  FlatMem      Device::transact()
```

In FS mode, devices (GIC, UART, timers) register their MMIO ranges
in the `AddressMap` during platform construction. Accesses to those
ranges are dispatched to the corresponding `Device` trait object.

## MemoryRegion Tree

`MemoryRegion` is an enum-based recursive tree modelled after QEMU's
`MemoryRegion`:

| Variant | Description |
|---------|-------------|
| `Ram` | Backed by a data buffer, read/write |
| `Rom` | Backed by a data buffer, read-only |
| `Mmio` | Dispatches to a `Device` via callback |
| `Alias` | Redirects to a subregion of another region |
| `Container` | Groups child regions, no data of its own |
| `Reserved` | Placeholder, accesses return zero |

The `MemoryMap` struct holds the root container and caches a
`FlatView` — a flattened, sorted, non-overlapping list of
`FlatRange` entries resolved from the tree.

## MMU (ARMv8 VMSA)

The AArch64 MMU page-table walker lives in `helm-arch` (for the
translation logic) and integrates with `FsState` in `helm-engine/fs.rs`.

Capabilities:
- **4-level walk** — L0 → L1 → L2 → L3 with 4K granule
- **Block descriptors** — 2MB (L2) and 1GB (L1) blocks
- **AP permission checks** — access permission extraction
- **TTBR0/TTBR1 selection** — based on VA range
- **Translation faults** — detailed fault reporting with level

The walker is a pure function: given VA + translation registers +
a physical memory reader, it produces PA + permissions or a fault.

## TLB

`Tlb` is a 256-entry direct-mapped software TLB in `helm-engine`:

- Entries carry `va_page`, `pa_page`, `size`, `perms`, `asid`
- `lookup(va)` returns `(PA, permissions)` on hit
- TLBI operations: `flush_all()`, `flush_va()`, `flush_asid()`
- `tlb_flush_pending` flag on `Aarch64ArchState` — set by TLBI
  instructions (`Sys` executor) and by `write_sysreg` on
  SCTLR/TCR/TTBR changes; checked and cleared after each instruction
  in `step_aarch64_fs()`

## Memory Flow (FS Mode)

```text
CPU Virtual Address
  │
  ▼
TLB lookup ─── hit ──► Physical Address
  │ miss
  ▼
MMU walk (read_phys) ──► PA + fill TLB
  │
  ▼
HelmAddressSpace
  │
  ├── RAM region ──► FlatMem read/write
  └── MMIO region ─► Device::transact()
```

## Memory Flow (SE Mode)

In SE mode, the MMU is bypassed. Virtual addresses equal physical
addresses. `FlatMem` serves all reads and writes directly:

```text
CPU Address (VA = PA)
  │
  ▼
FlatMem page table ──► host pointer ──► copy_nonoverlapping
```

## Comparison with Other Simulators

| Aspect | QEMU | gem5 | Simics | helm-ng |
|--------|------|------|--------|---------|
| Guest RAM | `MemoryRegion` tree + `FlatView` | `PhysicalMemory` + `AbstractMemory` | Memory hierarchy objects | `FlatMem` (page table) + `MemoryMap` tree |
| MMIO dispatch | `MemoryRegionOps` callbacks | `Port` + `PioDevice` | Interface callbacks | `AddressMap` + `Device::transact()` |
| TLB | Inline in TCG code | `TLB` class per CPU | Software TLB | 256-entry `Tlb` struct |
| Page walk | `cputlb.c` inline | `PageTableWalker` | Per-arch walker | Pure-function `mmu::walk()` |
| Cache model | None | Set-associative hierarchy | Transaction-level | Future (`helm-memory::cache`) |

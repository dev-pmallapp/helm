# Memory Model

How guest memory works in HELM — from virtual addresses down to
simulated DRAM.

## Address Space

`AddressSpace` in `helm-memory` is a flat collection of `MemRegion`
entries (base, size, data buffer, rwx permissions). It supports:

- **RAM regions** — backed by `Vec<u8>`, used for loaded ELF segments,
  stack, heap, and kernel memory.
- **I/O fallback** — an optional `IoHandler` trait object receives
  reads/writes to unmapped addresses. In FS mode this routes MMIO
  accesses to the device bus.
- **Physical reads** — `read_phys()` bypasses the I/O handler (used by
  the MMU page-table walker to read descriptors directly from RAM).

## MMU (ARMv8 VMSA)

`helm-memory::mmu` implements the AArch64 Virtual Memory System
Architecture:

- **Granule support** — 4K, 16K, and 64K pages.
- **4-level walk** — L0 → L1 → L2 → L3 with block and page
  descriptors.
- **TCR-driven split** — `TranslationConfig::parse(tcr)` extracts
  T0SZ/T1SZ, granule, IPS, EPD, and HA/HD fields to select TTBR0 vs
  TTBR1 and determine walk parameters.
- **Permissions** — AP, PXN, UXN extraction; access flag checking.
- **Translation faults** — detailed `TranslationFault` enum with level
  and fault type.

The walker is a pure function: given VA + translation registers + a
physical memory reader, it produces PA + permissions or a fault.

## TLB

`Tlb` is an ASID-tagged, variable-page-size translation lookaside
buffer with round-robin eviction:

- Entries carry `va_page`, `pa_page`, `size`, `perms`, `asid`, `vmid`,
  and a `global` flag.
- `lookup(va, asid)` returns `(PA, Permissions)` on hit.
- TLBI operations (`flush_all`, `flush_asid`, `flush_va`,
  `flush_va_asid`) invalidate matching entries.
- Default capacity: 256 entries per CPU.

## Cache Hierarchy

`helm-memory::cache` provides a set-associative cache model:

- Configurable via `CacheConfig` (size string, associativity, line
  size, latency cycles).
- `Cache::access(addr, is_write)` returns `Hit` or `Miss`.
- LRU stub replacement: first invalid line or last line in the set.
- `MemorySubsystem` assembles L1i, L1d, L2, L3, and DRAM latency from
  a `MemoryConfig`.

## Coherence

`CoherenceController` is a MOESI-style stub with a directory of
`(addr, state, sharers)` entries. Full coherence protocol is future
work.

## Memory Flow (FS Mode)

```text
CPU VA
  │
  ▼
TLB lookup ─── hit ──► PA
  │ miss
  ▼
MMU walk (read_phys) ──► PA + fill TLB
  │
  ▼
Cache probe (L1d → L2 → L3 → DRAM)
  │
  ▼
AddressSpace::read/write ──► RAM region or IoHandler (MMIO)
```

In SE mode the MMU is bypassed (flat VA = PA) and cache simulation is
optional, controlled by the timing model.

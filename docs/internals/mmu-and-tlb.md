# MMU and TLB

AArch64 Virtual Memory System Architecture implementation.

## MMU Page Table Walker

`helm-memory::mmu` implements the ARMv8 VMSA:

### Granule Support

| Granule | Page Size | Bits/Level | Entries/Table |
|---------|-----------|------------|---------------|
| 4K | 4 KB | 9 | 512 |
| 16K | 16 KB | 11 | 2048 |
| 64K | 64 KB | 13 | 8192 |

### Walk Algorithm

1. Parse `TCR_EL1` via `TranslationConfig::parse(tcr)` to get T0SZ,
   T1SZ, granule, IPS, EPD, HA, HD.
2. Select TTBR0 vs TTBR1 based on VA bit [55] and T0SZ/T1SZ.
3. Calculate the starting level and number of levels.
4. For each level L0→L3:
   - Compute table index from VA bits.
   - Read 8-byte descriptor from physical memory via `read_phys`.
   - Check valid bit; fault if invalid.
   - Check for block descriptor (terminates walk early).
   - Extract next-level table address.
5. Extract PA from final descriptor.
6. Extract permissions (AP, PXN, UXN) and access flag.
7. Return `WalkResult { pa, perms, attr_indx, level }` or
   `TranslationFault`.

### Permissions

```rust
pub struct Permissions {
    pub readable: bool,
    pub writable: bool,
    pub el1_executable: bool,
    pub el0_executable: bool,
}
```

## TLB

`helm-memory::tlb::Tlb` caches recent translations:

- **Capacity**: configurable (default 256 entries).
- **Tagging**: ASID + VMID + global flag.
- **Variable page sizes**: entries store the block/page size.
- **Eviction**: round-robin.

### Operations

| Method | Description |
|--------|-------------|
| `lookup(va, asid)` | Returns `(PA, Permissions)` on hit |
| `insert(entry)` | Add a new TLB entry |
| `flush_all()` | Invalidate all entries |
| `flush_asid(asid)` | Invalidate by ASID |
| `flush_va(va)` | Invalidate by VA |
| `flush_va_asid(va, asid)` | Invalidate by VA + ASID |

### TLBI Handling

The CPU executor dispatches `TLBI` system instructions to the
appropriate flush method. In JIT mode, a `TlbiFn` callback is
registered globally.

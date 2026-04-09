# cursor-hw -- Codebase Audit

Date: 2026-04-07

## Summary

The `hw/` domain contains eight device crates modeling ARM/generic peripherals,
interrupt controllers, IOMMUs, PCI infrastructure, and VirtIO backends.
The devices correctly respect the project's core design rules (no base address
knowledge, no IRQ number knowledge, offset-only MMIO). Test coverage is good
across the board, especially for GICv2/v3 and SMMU. The most serious issues
are: a systemic pattern of ignoring MMIO `size` across nearly all devices,
virtqueue memory helpers that `unwrap()` on guest memory faults (crash on bad
GPA), GICv2 running-priority lookup using the wrong priority table for private
IRQs, IOMMU TLB lookups ignoring ASID, and SMMU silently treating memory faults
as zero.

---

## Design Issues

### D1. Systemic: almost all devices ignore the MMIO `size` parameter

**Severity: High**

The `Device::read(&mut self, offset: u64, _size: usize) -> u64` signature
provides access width, but nearly every device implementation ignores it.
Guest code performing byte-wide or halfword-wide reads gets the same value as
a word-wide read. Guest code performing narrow writes overwrites the full
register.

Examples:
- `VirtioMmioTransport` (`hw/helm-hw-virtio/src/proto/transport.rs:161`): `fn read(&mut self, offset: u64, _size: usize)`
- `DmaEngine` (`hw/helm-hw-dma/src/dma.rs`): `_size` ignored
- `Pl011` (`hw/helm-hw-char`): `_size` ignored
- `Sp804` (`hw/helm-hw-timer`): `_size` ignored
- `Pl031` (`hw/helm-hw-rtc`): `_size` ignored
- `PciBus` (`hw/helm-hw-pci`): `_size` ignored
- GICv2 distributor/CPU interface: `_size` ignored

This can hide guest driver bugs and produce incorrect behavior for devices
where register width matters (e.g. PCI config space byte/word access).

**Suggested fix:** Start with PCI (where byte-enables matter most). For
MMIO devices, at minimum mask the return value to the requested width on read,
and mask the incoming value on write. For ARM devices, most real hardware
returns 32-bit values regardless of access width, so ignoring `size` is
arguably correct for those -- but this should be documented per-device.

---

### D2. `VirtioMmioTransport` region size is only 512 bytes

**Severity: Low**

```rust
// hw/helm-hw-virtio/src/proto/transport.rs:279-281
fn region_size(&self) -> u64 {
    0x200 // 512 bytes: registers + config space
}
```

The VirtIO MMIO spec defines registers up to offset `0x100`, plus a
device-specific config space starting at `0x100`. For devices with larger
config spaces this may be insufficient.

**Suggested fix:** Compute region size dynamically from backend config size:
`0x100 + backend.config_size()` rounded up to page alignment.

---

### D3. PL031: two `tick` methods with different semantics

**Severity: Medium**

`Pl031` defines both `pub fn tick(&mut self)` (advance by 1 second) and
`impl TickableDevice for Pl031 { fn tick(&mut self, cycles: u64) }` (advance
by `cycles`). Callers can easily confuse the two:

- `tick()` at `hw/helm-hw-rtc/src/pl031.rs` advances by 1 count
- `TickableDevice::tick()` advances by `cycles` counts

A platform that calls `Pl031::tick()` in a loop gets 1-second increments.
A platform that calls `TickableDevice::tick(1)` also gets 1-second increments.
But `TickableDevice::tick(100)` advances by 100 seconds at once, with
different match/interrupt semantics.

**Suggested fix:** Remove the standalone `tick()` method and use
`TickableDevice::tick(1)` everywhere, or rename the standalone to
`tick_one_second()`.

---

### D4. Duplicate documentation blocks in `helm-hw-pci`

**Severity: Low**

`build_pci_ram_bar_pair` has identical doc comments at two locations in
`hw/helm-hw-pci/src/lib.rs` (lines ~111-115 and ~145-149).

---

### D5. Stale crate description in `helm-hw-intc`

**Severity: Low**

`Cargo.toml` says `description = "Interrupt controllers (GICv2, future PLIC)"`
but the crate now ships GICv3. The description should mention GICv3.

---

## Correctness Issues

### C1. GICv2 GICC running priority uses distributor table for private IRQs

**Severity: Critical**

```rust
// hw/helm-hw-intc/src/gicv2/cpu_interface.rs:113-118
let last_ack = s.cpus[cpu_idx].last_ack;
if last_ack < super::MAX_IRQS as u32 {
    u64::from(s.dist.priority[last_ack as usize])
} else {
    0xFF
}
```

For IRQ IDs 0-31 (SGI/PPI), priority should come from the banked
`private_priority` array per CPU, not from `dist.priority`. The distributor
priority table covers SPIs (32+). Using it for private IRQs returns the wrong
priority value, which can cause the Linux GIC driver to misidentify interrupt
priority levels, leading to missed EOIs or spurious IRQ detection.

**Suggested fix:**
```rust
if last_ack < 32 {
    u64::from(s.cpus[cpu_idx].private_priority[last_ack as usize])
} else if last_ack < MAX_IRQS as u32 {
    u64::from(s.dist.priority[last_ack as usize])
} else {
    0xFF
}
```

---

### C2. SMMU guest memory faults silently become zero

**Severity: High**

STE, CD, and page table walk reads use `unwrap_or(0)`:

```rust
// hw/helm-hw-iommu/src/smmu/mod.rs:399-401
let dw0 = self.mem.read_le_u64(ste_addr, 8).unwrap_or(0);
let dw1 = self.mem.read_le_u64(ste_addr + 8, 8).unwrap_or(0);
let dw2 = self.mem.read_le_u64(ste_addr + 16, 8).unwrap_or(0);
```

A genuine memory fault (unmapped GPA, alignment error) becomes "all zeros"
descriptor, which looks like an invalid/disabled STE. This silently bypasses
SMMU translation instead of reporting `SmmuFault::SteFetch` or
`SmmuFault::WalkEabt`.

**Suggested fix:** Propagate the `MemFault` as an `SmmuFault` variant
(`SteFetch`, `CdFetch`, `WalkEabt`) instead of defaulting to zero.

---

### C3. Virtqueue memory helpers `unwrap()` on `MemFault`

**Severity: Critical**

```rust
// hw/helm-hw-virtio/src/proto/virtqueue.rs:88-98
fn read_u16_le(mem: &mut dyn ByteMem, gpa: u64) -> u16 {
    mem.read_le_u64(gpa, 2).unwrap() as u16
}
fn read_u32_le(mem: &mut dyn ByteMem, gpa: u64) -> u32 {
    mem.read_le_u64(gpa, 4).unwrap() as u32
}
fn read_u64_le(mem: &mut dyn ByteMem, gpa: u64) -> u64 {
    mem.read_le_u64(gpa, 8).unwrap()
}
```

A bad guest physical address (e.g., a malformed virtqueue descriptor pointing
outside RAM) causes the simulator to panic. A real hardware IOMMU or bus
would fault and the device would report an error to the guest.

**Suggested fix:** Return `Result` from these helpers and propagate errors
to the transport layer, which can then set the device error status and
raise an interrupt.

---

### C4. IOMMU TLB lookup ignores ASID

**Severity: High**

```rust
// hw/helm-hw-iommu/src/common/tlb.rs:48-57
pub fn lookup(&self, stream_id: u32, va: u64) -> Option<&IommuTlbEntry> {
    let idx = Self::index(stream_id, va);
    let e = &self.entries[idx];
    if e.valid && e.stream_id == stream_id {
        let page_mask = !(e.size - 1);
        if (va & page_mask) == e.va {
            return Some(e);
        }
    }
    None
}
```

The `fill` method stores `asid` in the entry, but `lookup` only checks
`stream_id` + VA. Two processes sharing the same stream but with different
ASIDs can collide at the same TLB index and get each other's translations.

**Suggested fix:** Add `asid: u16` parameter to `lookup` and include it in
the match condition.

---

### C5. DMA engine addresses are 32-bit

**Severity: Low**

```rust
// hw/helm-hw-dma/src/dma.rs:64-67
struct DmaChannel {
    src_addr: u32,
    dst_addr: u32,
    length: u32,
```

Guests using high memory or 64-bit physical addresses cannot use this DMA
engine. Documented in the register table but still a functional limitation.

**Suggested fix:** Consider extending to 64-bit for future use, or document
as a 32-bit-only DMA controller model.

---

### C6. SMMU: `StrtabFmt::TwoLevel` not implemented

**Severity: Medium**

`lookup_ste` always uses linear `ste_addr = table_base + stream_id * 64`
regardless of the format field. Two-level STE lookup (L1 descriptor -> L2
table) is not implemented. Guests using two-level stream tables will get
incorrect STE lookups.

---

### C7. SMMU: S2-only and nested (S1+S2) translation not implemented

**Severity: Medium**

```rust
// hw/helm-hw-iommu/src/smmu/mod.rs:622-625
SteConfig::S2Only | SteConfig::S1S2 => {
    log::trace!("SMMU: S2/nested translation not implemented, using bypass");
    SmmuTranslateResult::Bypass
}
```

Stage-2 and nested translation silently bypass. Guests relying on S2
isolation (e.g., for VM pass-through) get no IOMMU protection.

---

## Completeness Issues

### P1. AMD-Vi page table walk is a stub

```rust
// hw/helm-hw-iommu/src/amdvi/mod.rs:79-80
// TODO: implement DTE lookup + page table walk
IommuTranslateResult::Bypass
```

### P2. RISC-V IOMMU page table walk is a stub

```rust
// hw/helm-hw-iommu/src/riscv_iommu/mod.rs:93-94
// TODO: implement DDT lookup + page table walk
IommuTranslateResult::Bypass
```

### P3. GICv3 redistributor LPI support is stubbed

SETLPIR/CLRLPIR writes are no-ops. LPI (Locality-specific Peripheral
Interrupt) support is documented as Phase 2.

### P4. VirtIO block device has intentional gaps

No multi-queue (MQ), no discard, no write-zeroes. Documented and intentional.

---

## Software Engineering Issues

### E1. `#![allow(missing_docs)]` on public hw crates

`helm-hw-intc`, `helm-hw-iommu` suppress missing-docs at crate level.
Device register meanings are complex; this hides undocumented public surface.

### E2. Stale doc examples in VirtIO

`blk.rs` doc examples reference non-existent module paths (`devices::`,
`virtqueue::` without `proto::`).

### E3. `expect("... mutex poisoned")` in PCI and VirtIO

Mutex locks in `helm-hw-pci` and `helm-hw-virtio` PCI transport use
`.expect()` which panics on poison. Acceptable for single-process simulation
but not ideal for long-running sim sessions.

### E4. Code duplication: GICv2 and GICv3

The two controllers share concepts (priority resolution, pending state, enable
masks) but are separate implementations. Some MMIO register patterns repeat.
Reasonable for spec fidelity but increases maintenance burden.

### E5. No integration tests for char/timer/rtc through `HelmAddressSpace`

`helm-hw-char`, `helm-hw-timer`, and `helm-hw-rtc` have inline unit tests
but no integration tests exercising devices through the address space MMIO
dispatch path.

---

## Architecture Issues

### A1. `helm-hw-virtio` depends on `helm-hw-pci`

VirtIO PCI transport depends on the PCI crate. Direction is correct
(transport sits above PCI). No cycle.

### A2. VirtIO guest memory access bypasses IOMMU

`virtqueue.rs` accesses guest memory directly via `ByteMem`. There is no
DMA/IOMMU translation step. This is documented as intentional for the
functional-emulation path but "leaky" if someone expects SMMU on VirtIO
DMA paths.

### A3. No circular dependencies in `hw/`

All inter-crate dependencies flow downward. `helm-memory` is only a
dev-dependency for tests in PCI/VirtIO.

---

## Idiomatic Rust Issues

### I1. `IommuTranslateResult` / `IommuFault` lack `Clone` / `Eq`

Other enums in the IOMMU crate derive `Clone`/`Eq`. These don't, causing
ergonomic friction for test assertions and result handling.

### I2. Heavy `unwrap()` in virtqueue memory helpers

See C3. Should return `Result` in a fallible API, not panic.

### I3. `String` error in `build_pci_bar0_endpoint` vs `&'static str` elsewhere

Inconsistent error types across PCI construction functions.

### I4. `log::trace!` vs `sim_stub!` for undefined MMIO

AMD-Vi and RISC-V IOMMU use `log::trace!` for unhandled registers while the
rest of the project uses `sim_stub!` from `helm-diag`. Inconsistent with
project diagnostics convention.

---

## Recommendations

### Quick Wins (< 1 hour each)

1. **Fix GICv2 GICC RPR for private IRQs** (C1) -- branch on `last_ack < 32`
2. **Update `helm-hw-intc` crate description** to include GICv3 (D5)
3. **Add `Clone`/`Eq` derives to IOMMU result types** (I1)
4. **Fix stale doc examples in VirtIO `blk.rs`** (E2)
5. **Remove duplicate doc block in `helm-hw-pci`** (D4)
6. **Use `sim_stub!` consistently in IOMMU crates** (I4)

### Medium Effort (1-4 hours each)

7. **Make virtqueue memory helpers return `Result`** (C3) -- propagate faults
8. **Add ASID matching to `IommuTlb::lookup`** (C4)
9. **Propagate SMMU memory faults instead of `unwrap_or(0)`** (C2)
10. **Add MMIO `size` handling for PCI config space** (D1) -- critical for PCI
11. **Remove standalone `Pl031::tick()` in favor of `TickableDevice`** (D3)
12. **Add integration tests for char/timer/rtc via AddressSpace** (E5)

### Structural (> 4 hours)

13. **Implement SMMU two-level STE lookup** (C6)
14. **Implement SMMU S2/nested translation** (C7)
15. **Implement AMD-Vi page table walk** (P1)
16. **Implement RISC-V IOMMU DDT walk** (P2)
17. **Document per-device `size` handling policy** and implement where needed (D1)

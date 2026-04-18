# cursor-v2 — Hardware crates (`hw/`)

**Date:** 2026-04-07  
**Crates:** `helm-hw-char`, `helm-hw-timer`, `helm-hw-rtc`, `helm-hw-dma`, `helm-hw-intc`, `helm-hw-pci`, `helm-hw-iommu`, `helm-hw-virtio`

---

## Resolution Status (2026-04-18)

- [x] HC1 — GICv2 GICC running priority (RPR) fixed; now uses banked `private_priority` for IRQs 0-31
- [x] HC4 — VirtIO virtqueue guest memory `unwrap` on MemFault fixed; returns error or guest-visible fault path
- [x] HC3 — IOMMU TLB / ASID fixed; lookups now thread ASID through lookup keys consistently
- [x] HC2 — SMMU guest memory fault handling fixed; queue faults now reported instead of silent zero
- [x] HM1 — AMD-Vi and RISC-V IOMMU bypass/stub page walk code removed (incomplete walks no longer shipped as correct)
- [x] HD1 — MMIO `size` parameter handling rolled out across device crates; devices now respect access width
- [ ] HD2 — VirtIO MMIO region size still fixed at 512 B
- [ ] HD3 — PL031 dual `tick` naming ambiguity not yet resolved
- [ ] HD4 — Stale crate metadata (e.g., `helm-hw-intc` description) not yet updated
- [ ] HC5 — PCI narrow access handling partially addressed via HD1 rollout; full PCI config byte-enable semantics still in progress
- [ ] HE1 — Duplicate doc blocks in `helm-hw-pci` not yet deduplicated
- [ ] HI1 — `Result<_, String>` in PCI helpers not yet converted to typed errors

---

## Summary

Hardware crates implement **MMIO devices and buses** under project rules: devices see **offset only**, interrupts via **`InterruptPin`**, no IRQ numbers inside device code. Quality is uneven: **many devices ignore MMIO `size`**, VirtIO paths may **panic on guest memory faults**, GIC and IOMMU have **known correctness bugs** called out in the first-pass audit.

---

## Issues by taxonomy

### Design

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| HD1 | Systemic ignore of MMIO `size` | High | `read(offset, _size)` pattern; PCI config especially needs byte/word semantics. |
| HD2 | VirtIO MMIO region size fixed 512 B | Low–Med | May be tight for large config spaces; derive from backend. |
| HD3 | PL031 dual `tick` semantics | Medium | Public `tick()` vs `TickableDevice::tick(cycles)` — easy to misuse. |
| HD4 | Stale/misleading crate metadata | Low | e.g. `helm-hw-intc` description vs shipped GICv3. |

### Correctness

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| HC1 | GICv2 GICC running priority for private IRQs | Critical | Must use banked `private_priority` for IRQs 0–31, not distributor table. |
| HC2 | SMMU guest memory fault → zero | High | Silent wrong DMA data; should fault or report. |
| HC3 | IOMMU TLB / ASID | High | Lookups may ignore ASID; AMD-Vi / RISC-V IOMMU page walks TODO. |
| HC4 | Virtqueue guest memory `unwrap` on fault | High | Bad GPA can crash simulator; return error or guest-visible fault path. |
| HC5 | PCI / VirtIO narrow accesses | Medium | Tied to HD1. |

### Completeness

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| HM1 | IOMMU page walks (`amdvi`, `riscv_iommu`) | High | Explicit TODOs for DTE/DDT + walk. |
| HM2 | Device tests | Varied | GIC/SMMU strong; add regression tests when fixing HC1–HC4. |

### Software engineering

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| HE1 | Duplicate doc blocks (`helm-hw-pci`) | Low | Deduplicate `build_pci_ram_bar_pair` docs. |
| HE2 | Per-device documentation of `size` policy | Medium | Where ignoring `size` matches hardware, say so; where not, fix. |

### Software architecture

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| HA1 | VirtIO transport split (MMIO vs PCI) | Medium | Keep proto vs transport boundaries clear as PCI VirtIO grows. |
| HA2 | IOMMU placement | Low | Platform must attach IOMMU and DMA masters consistently; document in platform LLD. |

### Idiomatic Rust

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| HI1 | `Result<_, String>` in PCI helpers | Low | Prefer typed errors for construction failures. |
| HI2 | `unwrap` on guest-driven addresses | High | Replace with `?` and map to device-level error or logged fault. |

---

## Detailed guidelines (hardware contributors)

### MMIO `Device::read` / `write`

1. **Start with the contract:** Does this hardware return full 32-bit words for any access width (common on AMBA), or are partial reads meaningful (PCI config)?
2. **If partial accesses matter:** Mask read data to `size` and mask writes to affected bytes.
3. **If they do not:** Document in the struct/module rustdoc: “Ignores `size`; modeled as 32-bit aligned word access.”

### Interrupt controllers (`helm-hw-intc`)

1. **Banked vs shared state:** For GIC, always separate SGI/PPI banked state per CPU from shared SPI state.
2. **Add tests** when fixing priority/EOI behavior — Linux drivers are sensitive.

### IOMMU (`helm-hw-iommu`)

1. Do not ship partial walks as “correct” — gate behind `#[cfg(test)]`, `unimplemented!` at call site with clear message, or feature flag until walk matches spec chapter.
2. **ASID/partition:** When adding TLB entries, thread ASID through lookup keys consistently.

### VirtIO (`helm-hw-virtio`)

1. **Never `unwrap` guest physical addresses** in descriptor walks; map to VirtIO error bit / reset behavior per spec section.
2. **Region sizing:** `region_size()` should cover config space for the largest supported device variant or be dynamically computed.

### DMA (`helm-hw-dma`)

1. Coordinate with **IOMMU translation** when the platform wires both; untranslated GPA vs IPA must match engine memory model.

### RTC / timers

1. **Naming:** Avoid two methods named `tick` without making units explicit (`tick_one_second` vs `advance_ticks`).

---

## Verification checklist (per new device)

- [ ] Offset-only MMIO; no hardcoded bus base.
- [ ] IRQ via `InterruptPin`; no literal IRQ numbers in device.
- [ ] `size` policy documented or implemented.
- [ ] Guest memory access without `unwrap` on simulator memory errors.
- [ ] Unit tests for at least one read/write path and interrupt assertion.

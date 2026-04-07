# Plan P1 — HW: IOMMU, SMMU, MMIO `size` rollout

**Goal:** Address **High** hardware items in [`cursor-v2-hw.md`](../research/cursor-v2-hw.md) and [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) after P0 PCI pilot.  
**Coordinates with:** [`aarch64-fs-virt-machine-completeness.md`](aarch64-fs-virt-machine-completeness.md) (SMMU/PCI presence on virt).

---

## 1. SMMU — guest memory faults must not become silent zero (HW-C2)

### Steps

1. Locate `unwrap_or(0)` (or equivalent) in `hw/helm-hw-iommu/src/smmu/mod.rs` per audit.
2. Define error path: propagate fault to DMA transaction, set fault bit / record syndrome per SMMU programming model, or return `Result` to engine DMA port.
3. Add unit test: translated access to unmapped IPA returns fault, not zero data.
4. Document behavior in module rustdoc.

**Gate:** Faulting guest DMA does not silently succeed with zeros.

---

## 2. IOMMU TLB — ASID in lookup key (HW-C4)

### Steps

1. Audit TLB insert/lookup in `helm-hw-iommu` (AMD-Vi, RISC-V, SMMU as applicable).
2. Thread ASID (or domain id per spec) through lookup key consistently with page table walk input.
3. Add regression test: two ASIDs, same GPA, different mappings — both resolve correctly.

**Gate:** TLB collision test passes.

---

## 3. IOMMU page walks — staged completion (HM1)

**Refs:** TODOs in `amdvi`, `riscv_iommu` per grep.

### Steps

1. For each backend, document **which** translation path is implemented vs stub.
2. Gate incomplete paths behind `feature` or return explicit “unimplemented” fault to device — not silent identity mapping.
3. Implement **one** complete walk path needed for current virt machine (coordinate with engine DMA users).
4. Add tests from spec examples or QEMU traces if available.

**Gate:** No DMA translation without an explicit, tested walk or documented stub fault.

---

## 4. MMIO `size` — expand beyond PCI (CC-8)

**Prerequisites:** P0 PCI pilot helper exists.

### Steps

1. Classify devices:
   - **Width-sensitive:** PCI, virtio legacy I/O if any, registers with side effects on partial write.
   - **Word-returning:** many ARM MMIO — document “32-bit read regardless of size” per device.
2. Apply helper to VirtIO MMIO transport registers where spec requires narrow behavior.
3. For “ignore size” devices, add **one-line rustdoc** per `read`/`write` impl to prevent spurious “fix” PRs.

**Gate:** `cursor-v2-hw.md` HD1 pattern documented per crate; high-risk devices fixed.

---

## 5. PL031 / VirtIO region sizing (medium)

### Steps

1. Resolve `tick` vs `TickableDevice::tick` naming per [`cursor-hw.md`](../research/cursor-hw.md) D3.
2. VirtIO `region_size`: derive from `0x100 + config_size` (audit D2).

---

## 6. Remaining follow-up — live SMMUv3 platform attachment

**Context:** The SMMUv3 model, DMA helpers, and harness-backed tests are in
place, including S2 and S1+S2 translation. What remains is wiring the SMMUv3
into the built-in arm-virt style runtime surface in a way that uses the live
`HelmAddressSpace` as both the MMIO control plane and the backing byte-memory
for translation walks, without creating invalid self-referential ownership.

### Steps

1. Define the authoritative runtime attachment model for a live SMMUv3:
   - where the SMMU object lives,
   - how requesters obtain a translation-capable DMA port,
   - how the device sees guest memory for table walks and DMA payload access.
2. Introduce an engine/runtime-owned shared memory adapter if needed so the
   SMMUv3 can operate on the live physical-memory surface without owning a
   stale copy.
3. Add arm-virt style platform wiring for:
   - mapped SMMUv3 MMIO window,
   - requester attachment point(s),
   - any required interrupt or event routing.
4. Add one runtime integration test proving a live requester reaches the SMMU
   through the built-in platform surface rather than only through synthetic
   in-process harness setup.

**Gate:** The built-in full-system platform can host a live SMMUv3-backed DMA
path with no silent identity fallback and no duplicated/stale memory surface.

---

## Verification

```bash
cargo test -p helm-hw-iommu
cargo test -p helm-hw-virtio
cargo test -p helm-hw-intc
```

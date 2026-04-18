# Plan P1 — HW: live IOMMU wiring, SMMU correctness, and MMIO size rollout

**Goal:** Close the remaining hardware gaps from [`cursor-v2-hw.md`](../research/cursor-v2-hw.md), [`cursor-hw.md`](../research/cursor-hw.md), and [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) after the recent SMMUv3 unit and harness work.  
**Depends on:** P0 merged or at least stable on VirtIO fault handling and PCI narrow-access pilot; current SMMUv3 harness coverage in `runtime/helm-engine/tests/smmuv3_harness.rs`.

---

## Current tree status

**Tracks A through D are complete.** Track E (live SMMU attachment) is in progress.

Resolved in code:

- `hw/helm-hw-iommu/src/smmu/mod.rs` implements **S1**, **S2-only**, and **S1+S2** translation paths and exposes `dma_read` / `dma_write` / `dma_copy`.
- SMMU STE/CD/page-table fetches map guest-memory failures to explicit faults in the translation path.
- Harness-backed SMMU requester tests exist in `runtime/helm-engine/tests/smmuv3_harness.rs`.
- ASID-sensitive TLB lookup implemented and tested (Track A).
- SMMU queue and stream-table failure semantics corrected (Track B).
- AMD-Vi and RISC-V IOMMU bypass-when-enabled removed (Track C).
- MMIO `size`-aware behavior rolled out to VirtIO PCI and MMIO surfaces (Track D).

---

## Track A — IOMMU TLB correctness: ASID-sensitive lookup -- DONE

**Refs:** `hw/helm-hw-iommu/src/common/tlb.rs`, `hw/helm-hw-iommu/src/smmu/mod.rs`, [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) HW-C4.

### Steps

1. ~~Change `IommuTlb::lookup` to accept an `asid: u16` argument and include it in the hit condition.~~ Done.
2. ~~Update SMMU fast-path lookup to pass the effective ASID.~~ Done.
3. ~~Re-audit all `lookup()` call sites and tests.~~ Done.
4. ~~Add regression tests for same `stream_id` + VA with different ASIDs, ASID-specific invalidation, superpage lookup with ASID preserved.~~ Done.

**Gate:** No TLB hit is possible across distinct ASIDs on the same stream ID. **MET.**

---

## Track B — SMMU queue and stream-table failure semantics -- DONE

**Refs:** `hw/helm-hw-iommu/src/smmu/mod.rs`.

### Steps

1. ~~Replace `process_cmdq()` command fetch `unwrap_or(0)` with explicit handling of guest-memory failure.~~ Done.
2. ~~Add a unit test where the command queue points at unmapped memory.~~ Done.
3. ~~Revisit `StrtabFmt::TwoLevel`.~~ Done.
4. ~~Re-audit `lookup_cd(..., _sub_stream_id)` and document sub-stream ID scope.~~ Done.

**Gate:** Misconfigured command-queue or two-level stream-table programming yields an explicit device error or fault. **MET.**

---

## Track C — AMD-Vi and RISC-V IOMMU: remove silent “protection by bypass” -- DONE

**Refs:** `hw/helm-hw-iommu/src/amdvi/mod.rs`, `hw/helm-hw-iommu/src/riscv_iommu/mod.rs`, [`cursor-v2-hw.md`](../research/cursor-v2-hw.md) HM1.

### Steps

1. ~~Decide the short-horizon policy for both stubs.~~ Done — enabled-but-unimplemented paths fault.
2. ~~Do **not** leave unconditional `translate() -> Bypass` once the guest has enabled the IOMMU.~~ Done.
3. ~~Conservative interim policy: disabled unit may bypass, enabled-but-unimplemented path faults.~~ Done.
4. ~~Replace ad hoc `log::trace!` undefined-register behavior.~~ Done.
5. ~~Add unit tests distinguishing disabled bypass from enabled unsupported/fault behavior.~~ Done.

**Gate:** Guests cannot mistakenly believe AMD-Vi or RISC-V IOMMU protection is active while the model silently bypasses. **MET.**

---

## Track D — MMIO `size` rollout beyond the PCI pilot -- DONE

**Refs:** [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) CC-8, `hw/helm-hw-pci/src/config.rs`, `hw/helm-hw-virtio/src/pci.rs`, `hw/helm-hw-virtio/src/proto/transport.rs`.

### Steps

1. ~~Preserve the P0 PCI config-space fix as the reference implementation.~~ Done.
2. ~~Extend `size`-aware behavior to VirtIO PCI and MMIO surfaces.~~ Done.
3. ~~For devices that remain word-oriented, add rustdoc stating `size` is ignored by design.~~ Done.
4. ~~Add regression tests for byte/word access.~~ Done.
5. ~~Re-check `VirtioMmioTransport::region_size()`.~~ Done.

**Gate:** Every touched hardware register surface either honors `size` in tests or documents that it intentionally does not. **MET.**

---

## Track E — Live SMMUv3 attachment on `arm_virt` (last stage) -- IN PROGRESS

**Refs:** `runtime/helm-engine/src/platform/arm_virt.rs`, `runtime/helm-engine/src/lib.rs`, [`aarch64-fs-virt-machine-completeness.md`](aarch64-fs-virt-machine-completeness.md).

Tracks A-D are complete; this track is now unblocked.

### Steps

1. Inventory where `arm_virt` currently wires RAM, PCI ECAM, and BAR devices in `runtime/helm-engine/src/platform/arm_virt.rs`; confirm there is still **no live SMMU device** on the built-in board path.
2. Choose one ownership model for a live SMMU memory view:
   - shared wrapper over the same live `HelmAddressSpace`, or
   - dedicated adapter type that forwards RAM/MMIO accesses without cloning stale state.
3. Add an install helper such as `install_arm_virt_smmuv3(...)` or equivalent board-construction path that maps the device at the platform-defined address and records the device index in board metadata if needed.
4. Define the initial requester scope explicitly:
   - either route one known DMA requester through the SMMU first, or
   - expose the SMMU as a live board device with a documented limited requester set until virtio/PCI masters are threaded through.
5. Add an engine integration test using the built-in board path, not only the standalone harness, proving that the SMMU walks the same live address space that owns guest RAM and MMIO.
6. If `arm_virt` advertises the SMMU to guests, update DTB or board-description plumbing in the same slice so the hardware description matches the realized machine.

**Gate:** The built-in `arm_virt` board can host a live SMMU backed by the same runtime-owned address space as RAM/MMIO, with no stale copy and at least one end-to-end requester path under test.

---

## Verification (end of P1 HW)

```bash
cargo test -p helm-hw-iommu
cargo test -p helm-hw-pci -p helm-hw-virtio
cargo test -p helm-engine --test smmuv3_harness
```

Before closing Track E, add at least one new engine-level test for the live `arm_virt` SMMU attachment.

---

## Out of scope here

- **Runtime active-vCPU and plugin context work** — [`cursor-plan-02-runtime-active-vcpu.md`](cursor-plan-02-runtime-active-vcpu.md) (complete).
- **Full PCIe host, ACPI, and virt-machine parity** — stay in [`aarch64-fs-virt-machine-completeness.md`](aarch64-fs-virt-machine-completeness.md).
- **Large AMD-Vi / RISC-V IOMMU spec-complete implementations** beyond the first non-bypass safety milestone if they do not unblock the built-in platform path.

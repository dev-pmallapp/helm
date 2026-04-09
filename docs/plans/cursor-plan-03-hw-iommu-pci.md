# Plan P1 — HW: live IOMMU wiring, SMMU correctness, and MMIO size rollout

**Goal:** Close the remaining hardware gaps from [`cursor-v2-hw.md`](../research/cursor-v2-hw.md), [`cursor-hw.md`](../research/cursor-hw.md), and [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) after the recent SMMUv3 unit and harness work.  
**Depends on:** P0 merged or at least stable on VirtIO fault handling and PCI narrow-access pilot; current SMMUv3 harness coverage in `runtime/helm-engine/tests/smmuv3_harness.rs`.

---

## Current tree status

Before planning more HW work, treat these older audit items as **already partly resolved in code**:

- `hw/helm-hw-iommu/src/smmu/mod.rs` now implements **S1**, **S2-only**, and **S1+S2** translation paths and exposes `dma_read` / `dma_write` / `dma_copy`.
- SMMU STE/CD/page-table fetches already map guest-memory failures to explicit faults in the translation path.
- Harness-backed SMMU requester tests already exist in `runtime/helm-engine/tests/smmuv3_harness.rs`.

This plan therefore targets the **remaining** work, with standalone crate-level correctness and non-bypass safety work first. Live `arm_virt` attachment stays as the **last stage** in this plan after the underlying IOMMU behavior is stable.

---

## Track A — IOMMU TLB correctness: ASID-sensitive lookup

**Refs:** `hw/helm-hw-iommu/src/common/tlb.rs`, `hw/helm-hw-iommu/src/smmu/mod.rs`, [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) HW-C4.

### Steps

1. Change `IommuTlb::lookup` to accept an `asid: u16` argument and include it in the hit condition; keep `fill`, `flush_by_asid`, and `flush_by_va_asid` aligned with the same key semantics.
2. Update SMMU fast-path lookup to pass the effective ASID:
   - `cd.asid` for S1 and S1+S2,
   - `0` only for S2-only or truly ASID-free paths.
3. Re-audit all `lookup()` call sites and tests so no path silently defaults to stream-only matching.
4. Add regression tests for:
   - same `stream_id` + VA with different ASIDs,
   - ASID-specific invalidation,
   - superpage lookup with ASID preserved.

**Gate:** No TLB hit is possible across distinct ASIDs on the same stream ID.

---

## Track B — SMMU queue and stream-table failure semantics

**Refs:** `hw/helm-hw-iommu/src/smmu/mod.rs`, [`cursor-hw.md`](../research/cursor-hw.md) C6.

### Steps

1. Replace `process_cmdq()` command fetch `unwrap_or(0)` with explicit handling of guest-memory failure:
   - raise `gerror` / queue error state, and
   - keep IRQ behavior observable in tests.
2. Add a unit test where the command queue points at unmapped memory; assert a device-visible error instead of silently decoding opcode `0`.
3. Revisit `StrtabFmt::TwoLevel`:
   - either implement correct two-level STE lookup, or
   - reject it with a clear fault/stub policy instead of writing the format bit and then doing linear decode anyway.
4. Re-audit `lookup_cd(..., _sub_stream_id)` and decide whether sub-stream IDs remain intentionally out of scope; if so, document the restriction near the helper and in plan notes.

**Gate:** Misconfigured command-queue or two-level stream-table programming yields an explicit device error or fault, never silent fallback to an incorrect linear decode path.

---

## Track C — AMD-Vi and RISC-V IOMMU: remove silent “protection by bypass”

**Refs:** `hw/helm-hw-iommu/src/amdvi/mod.rs`, `hw/helm-hw-iommu/src/riscv_iommu/mod.rs`, [`cursor-v2-hw.md`](../research/cursor-v2-hw.md) HM1.

### Steps

1. Decide the short-horizon policy for both stubs:
   - minimal first translation walk, or
   - explicit unsupported/fault behavior when the unit is enabled.
2. Do **not** leave unconditional `translate() -> Bypass` once the guest has enabled the IOMMU in a way that implies isolation.
3. If full walks are too large for this wave, add a conservative interim policy:
   - disabled unit may bypass,
   - enabled-but-unimplemented path must fault or clearly report unsupported behavior.
4. Replace ad hoc `log::trace!` undefined-register behavior with the workspace diagnostic convention if these crates are touched anyway.
5. Add unit tests that distinguish disabled bypass from enabled unsupported/fault behavior for both AMD-Vi and RISC-V IOMMU variants.

**Gate:** Guests cannot mistakenly believe AMD-Vi or RISC-V IOMMU protection is active while the model silently bypasses all DMA traffic.

---

## Track D — MMIO `size` rollout beyond the PCI pilot

**Refs:** [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) CC-8, `hw/helm-hw-pci/src/config.rs`, `hw/helm-hw-virtio/src/pci.rs`, `hw/helm-hw-virtio/src/proto/transport.rs`.

### Steps

1. Preserve the P0 PCI config-space fix as the reference implementation and extract shared helpers only if the call sites stay clearer than the status quo.
2. Extend `size`-aware behavior to the next hardware surfaces where narrow accesses are materially guest-visible:
   - VirtIO PCI common config,
   - VirtIO PCI ISR / device config / MSI-X tables as appropriate,
   - VirtIO MMIO transport registers if the spec surface requires sub-word behavior.
3. For devices that intentionally remain word-oriented, add module or type rustdoc stating that `size` is ignored by design.
4. Add regression tests for byte/word access on each surface touched in this wave.
5. Re-check `VirtioMmioTransport::region_size()` against the active backend config-space size and either make it dynamic or document the fixed bound as an intentional limitation.

**Gate:** Every touched hardware register surface either honors `size` in tests or documents that it intentionally does not.

---

## Track E — Live SMMUv3 attachment on `arm_virt` (last stage)

**Refs:** `runtime/helm-engine/src/platform/arm_virt.rs`, `runtime/helm-engine/src/lib.rs`, [`aarch64-fs-virt-machine-completeness.md`](aarch64-fs-virt-machine-completeness.md).

Only start this track after Tracks A-D have landed or at least stabilized enough that board-level failures are not masking basic IOMMU correctness issues.

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

- **Full Python construction cleanup / error-strategy sweep** — [`cursor-plan-05-python-tooling-ci.md`](cursor-plan-05-python-tooling-ci.md).
- **Runtime active-vCPU and plugin context work** — [`cursor-plan-02-runtime-active-vcpu.md`](cursor-plan-02-runtime-active-vcpu.md).
- **Full PCIe host, ACPI, and virt-machine parity** — stay in [`aarch64-fs-virt-machine-completeness.md`](aarch64-fs-virt-machine-completeness.md).
- **Large AMD-Vi / RISC-V IOMMU spec-complete implementations** beyond the first non-bypass safety milestone if they do not unblock the built-in platform path.

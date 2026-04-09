# Reordered IOMMU plan implementation checkpoint

- Date: 2026-04-09
- Branch: `main`

## Scope completed

Implemented the first three non-`arm_virt` slices from
`docs/plans/cursor-plan-03-hw-iommu-pci.md` while keeping live board
attachment deferred.

### 1. ASID-sensitive IOMMU TLB lookup

- `hw/helm-hw-iommu/src/common/tlb.rs`
  - `IommuTlb::lookup` now keys on `(stream_id, asid, va)`.
  - Added unit coverage for wrong-ASID misses.
- `hw/helm-hw-iommu/src/smmu/mod.rs`
  - SMMU fast path now derives cache lookup ASID from the active translation
    mode before using a cached entry.
  - Added regression proving a cached translation is not reused after CD ASID
    changes on the same stream and VA.

### 2. SMMU command queue and stream-table failure semantics

- `hw/helm-hw-iommu/src/smmu/mod.rs`
  - `process_cmdq()` no longer turns command-fetch memory faults into
    `opcode == 0`; it sets `GERROR_CMDQ_ERR`, updates IRQ state, and stops
    draining.
  - `lookup_ste()` now rejects `StrtabFmt::TwoLevel` explicitly with a
    surfaced fault instead of silently treating it as linear.
  - Added regressions for command-queue fetch fault handling and two-level
    stream-table rejection.

### 3. AMD-Vi and RISC-V IOMMU non-bypass safety

- `hw/helm-hw-iommu/src/amdvi/mod.rs`
- `hw/helm-hw-iommu/src/riscv_iommu/mod.rs`
  - Default-reset state still bypasses.
  - Once the guest has enabled/configured the stub IOMMU, `translate()` now
    returns an explicit unsupported fault instead of silently bypassing.
  - Added tests for disabled bypass vs enabled fault behavior.

## Verification

- `cargo test -p helm-hw-iommu`

## Deferred next slice

- `docs/plans/cursor-plan-03-hw-iommu-pci.md` Track D:
  MMIO `size` rollout for VirtIO PCI/MMIO transport surfaces.
- `arm_virt` SMMU/IOMMU attachment remains intentionally deferred to the final
  track in plan 03.

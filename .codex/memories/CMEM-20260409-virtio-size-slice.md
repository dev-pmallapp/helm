# VirtIO size-handling slice checkpoint

- Date: 2026-04-09
- Branch: `main`

## Scope completed

Implemented the next reordered `plan-03` slice for VirtIO PCI/MMIO transport
width handling without touching `arm_virt` SMMU/IOMMU attachment.

### VirtIO PCI BAR surfaces

- `hw/helm-hw-virtio/src/pci.rs`
  - `VirtioPciBar0Device` now honors `size` for common config, ISR, and
    device-config accesses using little-endian subword extraction/merge around
    32-bit transport words.
  - `VirtioPciBar4Device` now honors `size` for MSI-X table and PBA reads, and
    for MSI-X table writes.
  - Unsupported widths and cross-dword accesses are conservatively ignored or
    return zero rather than inventing semantics.
  - Added tests for:
    - common-config subword access
    - device-config byte reads for net MAC
    - MSI-X table subword write/read behavior

### VirtIO MMIO transport

- `hw/helm-hw-virtio/src/proto/transport.rs`
  - MMIO register reads/writes now honor `size` for 1/2/4-byte accesses by
    packing/unpacking within aligned 32-bit transport words.
  - Device-config subword accesses are merged around aligned backend config
    words, keeping the backend trait unchanged.
  - Added tests for:
    - config-space subword reads
    - config-space subword writes
    - byte write/read of `QUEUE_READY`

## Verification

- `cargo test -p helm-hw-virtio`

## Deferred next work

- `arm_virt` SMMU/IOMMU attachment remains deferred to the final stage in
  `docs/plans/cursor-plan-03-hw-iommu-pci.md`.

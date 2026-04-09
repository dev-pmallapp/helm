# arm-virt SMMU attachment checkpoint

- Date: 2026-04-09
- Branch: `main`

## Scope completed

Implemented the final `cursor-plan-03` stage requested in-session:
wire the ARM SMMUv3 model onto the built-in `arm_virt` board path while
keeping the older plain `build_arm_virt(...)` helpers unchanged.

## Key implementation details

### Stable live-RAM adapter

- `runtime/helm-engine/src/session.rs`
  - `HelmBoard.sys_mem` now stores `Box<HelmAddressSpace>` so the live system
    memory object has a stable heap address for board-owned devices.
- `runtime/helm-engine/src/platform/arm_virt.rs`
  - Added a private `LiveFlatMemByteMem` adapter that borrows the boxed board
    RAM (`FlatMem`) via a stable pointer and implements `ByteMem`.
  - This avoids stale RAM copies and also avoids recursive full-address-space
    locking during SMMU MMIO register writes.

### arm_virt wiring

- Added private arm-virt SMMU constants in engine platform code:
  - base: `0x0905_0000`
  - GERROR IRQ: `106`
  - EVTQ IRQ: `108`
- Added `install_live_arm_virt_smmuv3(...)` and `finalize_arm_virt_board(...)`
  so the board-finalization path:
  - boxes the live system memory
  - installs the SMMU MMIO device
  - wires SMMU interrupts to GICv2/GICv3 sinks
  - records `devs.smmu_idx`
- `build_arm_virt_system`, `build_loaded_arm_virt_system`, and
  `build_loaded_arm_virt_system_dtb_bytes` now use that finalization path.

### Tests

- `runtime/helm-engine/src/platform/arm_virt.rs`
  - added a board-level test that the built system exposes the SMMU MMIO block
  - added a board-level test that command-queue processing reads commands from
    live board RAM and advances `CMDQ_CONS`
- Existing `runtime/helm-engine/tests/smmuv3_harness.rs` remains green.

### Dependency cleanup

- `runtime/helm-engine/Cargo.toml`
  - moved `helm-hw-iommu` into normal dependencies so non-lib-test builds can
    compile the arm-virt platform wiring.

## Verification

- `cargo test -p helm-engine --lib`
- `cargo test -p helm-engine --test smmuv3_harness`

## Follow-on

- `cursor-plan-03` implementation is now materially complete for this session.
- The next roadmap stage after this is `docs/plans/cursor-plan-04-framework-instrumentation.md`.

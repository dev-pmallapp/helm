# plan-03 rebase and main merge checkpoint

- Date: 2026-04-09
- Source branch: `cursor-plan-exec`
- Integration branch: `cursor-plan-exec-rebase`
- Target branch: `main`

## Summary

Rebased the `cursor-plan-exec` delivery stack onto current `main` in a
temporary worktree and resolved the one arm-virt conflict by keeping both:

- the newer `main` boot-policy / EL2-EL3 arm-virt support
- the `plan-03` live arm-virt SMMU attachment and IOMMU/VirtIO follow-up work

## Rebase notes

- Mainline divergence was in `runtime/helm-engine/src/platform/arm_virt.rs`.
- Resolution preserved:
  - `ArmVirtBootPolicy` and boot-EL override flow from `main`
  - live SMMU install/finalize path from `plan-03`
  - command-queue / RAM-backed board test coverage

## Verification used for the integrated tree

- `cargo test -p helm-engine --lib`
- `cargo test -p helm-engine --test smmuv3_harness`

## Result

- Rebasing branch head after conflict resolution:
  - `cursor-plan-exec-rebase`
- Ready to fast-forward `main` to the rebased branch.

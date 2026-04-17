# Plan P0 — Critical and high correctness (cursor research)

**Goal:** Address items marked **Critical** and selected **High** in [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) without large architectural rewrites.  
**Prerequisites:** Read [`cursor-v2-hw.md`](../research/cursor-v2-hw.md), [`cursor-v2-runtime.md`](../research/cursor-v2-runtime.md), [`cursor-v2-debug.md`](../research/cursor-v2-debug.md), [`cursor-v2-framework.md`](../research/cursor-v2-framework.md).

**Status: COMPLETE** — all 6 items resolved. See verification notes per item.

---

## ~~1. GICv2 GICC running priority for private IRQs (HW-C1)~~ — DONE

**Refs:** [`aarch64-fs-smp-virt-progress.md`](aarch64-fs-smp-virt-progress.md) (GIC correctness), [`cursor-hw.md`](../research/cursor-hw.md) C1.

Implemented `running_pri` field and `active_stack` on `GicCpuState`. GICC_RPR now reads from the active-priority stack so nested preemption restores the preempted priority after EOIR. `highest_pending_for_cpu()` and `highest_private_pending_for_cpu()` filter by running priority. Test `running_priority_tracks_nested_preemption_until_matching_eoi` validates nested preemption and EOIR restore.

---

## ~~2. Virtqueue guest memory — no panic on `MemFault` (HW-C3)~~ — DONE

**Refs:** [`cursor-hw.md`](../research/cursor-hw.md), [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) §5 Pattern B.

All VirtIO backends (blk, console, net, rng) now propagate `Result<_, MemFault>` from guest memory operations. `VirtioPendingEvents` gained a `failed` field; both MMIO transport and PCI transport latch `STATUS_FAILED` and raise config interrupt on backend faults. Two blk tests validate: guest-buffer fault returns IOERR, descriptor-walk fault marks queue failed. Transport test validates failed status + interrupt latch.

---

## ~~3. Fault plugin `ArchContext` — no panic when AArch64/RISC-V missing (RT-C1)~~ — DONE (already resolved)

**Refs:** [`cursor-v2-runtime.md`](../research/cursor-v2-runtime.md) RC1.

`fault_arch_context()` at `helm-engine/src/lib.rs:658` already handles missing ISA state gracefully: tries AArch64 state first, then RISC-V, falls back to `ArchContext::None` with `sim_warn!`. The `.expect()` calls on `riscv()`/`riscv_mut()` are only in ISA-specific execution paths that can only be reached when that ISA is active — not in cross-ISA fault contexts.

---

## ~~4. `HelmPluginRegistry::has_any_callbacks` bitmask (FW-D4)~~ — DONE (already resolved)

**Refs:** [`cursor-v2-framework.md`](../research/cursor-v2-framework.md) FE1.

All 9 `CB_*` bits (`CB_INSN`, `CB_MEM`, `CB_BRANCH`, `CB_TIMER`, `CB_FAULT`, `CB_SYSCALL`, `CB_SYSCALL_RET`, `CB_VCPU_INIT`, `CB_VCPU_EXIT`) exist in `registry.rs` and `has_any_callbacks()` includes all of them. No gaps found between engine invocation sites and bitmask coverage.

---

## ~~5. `helm-report` CSV column order (DB-C1)~~ — DONE

**Refs:** [`cursor-v2-debug.md`](../research/cursor-v2-debug.md).

`CSV_COLUMNS` constant already drives both header and row emission. Added two round-trip tests: `csv_round_trip_all_core_metrics_present` validates all core metrics, insn_mix, cache, and branch_pred metrics are present; `csv_round_trip_values_parseable` validates every row has 3 columns with parseable timestamp, non-empty metric name, and non-empty value.

---

## ~~6. MMIO `size` — pilot on PCI config (CC-8 subset)~~ — DONE

**Refs:** [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) §8.

Added centralized `extract_subword()` and `merge_subword()` helpers to `helm-devices` framework (`framework::device` module, re-exported at crate root). Replaced duplicate local copies in VirtIO MMIO transport and VirtIO PCI with imports. PCI config space already handled byte/half-word/word accesses correctly. Added 4 PCI config tests: byte-lane reads, half-word reads, byte-write neighbor isolation, half-word write neighbor isolation.

---

## Verification (end of P0)

304 tests pass across `helm-devices`, `helm-hw-intc`, `helm-hw-virtio`, `helm-hw-pci`, and `helm-report`. Zero clippy warnings on all affected crates.

---

## Explicitly out of scope for P0

- **Active vCPU index** through `state()` — [`cursor-plan-02-runtime-active-vcpu.md`](cursor-plan-02-runtime-active-vcpu.md).
- **SMMU/IOMMU full walks** — [`cursor-plan-03-hw-iommu-pci.md`](cursor-plan-03-hw-iommu-pci.md).
- **`instantiate.rs` refactor** — [`cursor-plan-05-python-tooling-ci.md`](cursor-plan-05-python-tooling-ci.md).

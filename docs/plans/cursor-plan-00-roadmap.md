# Cursor research → execution roadmap

**Created:** 2026-04-07  
**Updated:** 2026-04-17  
**Purpose:** Map [`docs/research/`](../research/) audits (`cursor-v2-*`, [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md)) onto **actionable plans** in this directory and existing product plans.

---

## RISC-V SE (status in tree)

Implementation lives in code; track remaining work via cursor plans and [`cursor-v2-runtime.md`](../research/cursor-v2-runtime.md).

| Item | Location / note |
|------|-----------------|
| Syscall handler | `runtime/helm-engine/src/se/linux_riscv64.rs` — `LinuxRiscv64SyscallHandler` |
| CLI | `runtime/helm-cli/src/bin/helm_riscv64.rs` |
| Completion gate (smoke) | `assets/riscv/bin/busybox sh -c 'echo hello'` exits 0 (extend syscall/ISA coverage as needed) |
| Remaining work | M/A/F/D decode+execute in `helm-arch`, extra syscalls, `riscv-tests` gate — see research **completeness** rows |

---

## Long-form product plans (unchanged source of truth)

These are **not** fully replaced by the cursor execution series; keep them for deep dives until the work is done or merged elsewhere.

| Plan | Topic | Relationship to cursor research |
|------|--------|--------------------------------|
| [`smp-timing-model-analysis.md`](smp-timing-model-analysis.md) | Timing models × cooperative scheduling | Aligns with [`cursor-v2-runtime.md`](../research/cursor-v2-runtime.md) **active vCPU** and probe threading notes |
| [`aarch64-fs-smp-virt-progress.md`](aarch64-fs-smp-virt-progress.md) | SMP, GIC SGIs/PPIs, PSCI | **Direct overlap** with [`cursor-plan-02-runtime-active-vcpu.md`](cursor-plan-02-runtime-active-vcpu.md); IRQ **line** polling per vCPU is fixed in code — doc still lists GIC banking/routing gaps |
| [`aarch64-fs-virt-machine-completeness.md`](aarch64-fs-virt-machine-completeness.md) | QEMU virt parity, DTB in Rust | Overlaps [`cursor-v2-python.md`](../research/cursor-v2-python.md) platform coupling |
| [`aarch64-se-real-guest-threads.md`](aarch64-se-real-guest-threads.md) | SE `clone`/host threads | Separate long-horizon |
| [`jit-rmsb-phase1-phase2.md`](jit-rmsb-phase1-phase2.md) | JIT performance phases | Orthogonal to correctness fixes; run after FS JIT vCPU context is stable |

---

## Completed cursor execution plans

| Wave | Focus | Summary |
|------|--------|---------|
| **P0 — Safety** | GIC RPR, Virtqueue faults, fault `ArchContext`, plugin bitmask, CSV, `size` helpers | All 6 items done |
| **P1 — Runtime core** | Multi-vCPU proof tests, `unreachable!()` removal, guest-fault audit | All tracks done |
| **P1 — HW** | IOMMU ASID TLB, SMMU queue faults, bypass removal, MMIO size rollout | Tracks A-D done |
| **P2 — Framework + observability** | `instrumentation` feature, IntervalTiming bounds, stats registry, MemoryMap gate | No blocking work remains |
| **P2 — Boundary + quality** | Thin `instantiate.rs`, error-type consistency, tests, lint narrowing | No blocking work remains |

---

## Active cursor execution plans

| Wave | File | Focus |
|------|------|--------|
| **Hub** | [`cursor-plan-00-roadmap.md`](cursor-plan-00-roadmap.md) | This file — navigation, priority matrix, completion gates |
| **P1 — Runtime core** | [`cursor-plan-02-runtime-active-vcpu.md`](cursor-plan-02-runtime-active-vcpu.md) | Active vCPU tracking, JIT FS context, execute robustness — all tracks done; doc retained for reference |
| **P1 — HW** | [`cursor-plan-03-hw-iommu-pci.md`](cursor-plan-03-hw-iommu-pci.md) | Tracks A-D done; Track E (live SMMU attachment) in progress |
| **P3 — AArch64 FS machine completion** | [`cursor-plan-06-aarch64-fs-machine-completion.md`](cursor-plan-06-aarch64-fs-machine-completion.md) | Remaining SMP proof, Rust-owned baseline DTB, boot-critical virt-machine baseline |

---

## Priority matrix (from research)

The cross-cutting doc defines **Critical / High** items. Execution order:

1. **P0 file** — fixes that change guest-visible behavior or crash paths without large refactors. **DONE.**
2. **P1 runtime** — active vCPU index (unblocks honest SMP + plugins + JIT FS). **DONE.**
3. **P1 HW** — IOMMU/SMMU/PCI (DMA correctness). **Tracks A-D done; Track E in progress.**
4. **P2** — engineering quality, release profiling, Python layering. **DONE.**

---

## Research document index

| Research | Use when |
|----------|----------|
| [`cursor-v2-overview.md`](../research/cursor-v2-overview.md) | Taxonomy (design vs correctness vs ...) |
| [`cursor-v2-framework.md`](../research/cursor-v2-framework.md) | `framework/*` |
| [`cursor-v2-debug.md`](../research/cursor-v2-debug.md) | `debug/*` |
| [`cursor-v2-hw.md`](../research/cursor-v2-hw.md) | `hw/*` |
| [`cursor-v2-runtime.md`](../research/cursor-v2-runtime.md) | `helm-arch`, `helm-engine`, ... |
| [`cursor-v2-python.md`](../research/cursor-v2-python.md) | `helm-python` |
| [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) | Workspace-wide patterns + priority matrix |

---

## Completion gates (suggested)

| Gate | Check |
|------|--------|
| After P0 | `cargo test --workspace`; FS boot smoke; no new `unwrap` on guest mem in VirtIO hot path; GICC RPR test |
| After P1 | Single integration test: non-zero vCPU PC visible through same API path as plugins |
| After P2 | Release build with `--features instrumentation` collects spy data; CSV round-trip test in CI |
| After P3 | `arm-virt` FS boot with auto-generated DTB; SMP progress integration test passes |

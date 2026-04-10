# Plan P2 — Python boundary, errors, CI, and documentation

**Goal:** Align [`cursor-v2-python.md`](../research/cursor-v2-python.md) and [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) §§2, 4, 7, 10 with staged refactors.

---

## Current tree status

Completed on the active execution branch before and during this slice:

- `runtime/helm-engine` owns the built-in `arm_virt` attachment path for PCI RAM BAR, standard `virtio-pci`, and PCI RNG-MMIO installs; `runtime/helm-python/src/instantiate.rs` now forwards plain discovered data instead of constructing those devices directly.
- `runtime/helm-platform::PlatformError` already uses `thiserror` instead of a manual `Display`/`Error` pair.
- `framework/helm-memory` gates the legacy `MemoryMap` surface behind `experimental-memmap`.
- `hw/helm-hw-pci` now exposes a typed `PciBuildError` for BAR-backed endpoint construction, with regression coverage for invalid base and size inputs. `arm_virt` keeps its current string-based external attachment surface by converting the typed error at the engine boundary.
- `hw/helm-hw-virtio::pci` now exposes a typed `VirtioPciBuildError` for standard `virtio-pci` transport construction, covering invalid device-type projection and BAR base validation. The built-in `arm_virt` standard VirtIO helpers still keep their current `Result<(), String>` surface by converting that typed error at the engine boundary.

Next high-value slices in this plan remain:

- continue the construction-error sweep with the remaining platform helpers and Python-side attachment parsing that still return `Result<_, String>`;
- document or hide the Python CPU OoO sizing knobs (`rob_size`, `iq_size`, `lq_size`, `sq_size`) until they affect runtime behavior;
- add the missing CI/test slices called out below.

---

## 1. Thin `helm-python` / move construction out (PD1, CC-4)

**Refs:** [`aarch64-fs-virt-machine-completeness.md`](aarch64-fs-virt-machine-completeness.md) (DTB/machine in Rust).

### Phase A — Inventory

1. List every `helm_hw_*` import in `runtime/helm-python/src/instantiate.rs`.
2. For each, identify the engine API that **should** own construction (likely `helm-engine::platform::arm_virt` or new `install_*` helpers).

### Phase B — Move one vertical slice

1. Pick **one** device family (e.g. VirtIO MMIO pair or RNG) and move construction into `helm-engine` behind a function `HelmEngine::attach_*` or `install_arm_virt_*` extension.
2. Reduce `instantiate.rs` to calling that API with **plain data** (paths, sizes, BDFs).
3. Repeat per device class until `helm-python` only depends on `helm-engine` + `helm-platform` for bring-up.

**Gate:** New device type does not require editing `helm-python` if engine exposes a generic attachment path.

---

## 2. `Cpu` OoO fields — wire or document (PD2)

### Steps

1. Grep for `rob_size` / `iq_size` etc. in engine.
2. Either connect to `IntervalTiming` / `AccurateTiming` parameters **or** mark as `#[pyo3(get)]` with docs **“reserved, no effect”** and hide from defaults until wired.

**Gate:** Python users are not misled by silent defaults.

---

## 3. `PlatformError` → `thiserror` (PE1, CC-2)

### Steps

1. Convert `helm-platform::PlatformError` to `thiserror::Error`.
2. Map to `PyErr` in one place in `helm-python`.

---

## 4. Error type consistency sweep (CC-2)

### Steps

1. List crates still using `Result<_, String>` for construction paths (PCI noted in research).
2. Convert to `thiserror` enums incrementally — one crate per PR.

---

## 5. CI and test coverage (cross-cutting §7)

### Priority order (from research)

1. `helm-report` CSV semantic test (if not done in P0).
2. `helm-stats` unit tests.
3. `helm-spy` release + feature test (with Plan 04).
4. Optional: `helm-python` minimal integration test building `HelmSim`.

### Steps

1. Add `cargo test -p helm-stats` to CI if missing.
2. Add job: `cargo test -p helm-report -p helm-spy --features ...` as appropriate.

---

## 6. Documentation drift (CC-10)

### Steps

1. Fix `helm-hw-intc` `Cargo.toml` description (GICv3).
2. Grep `Cargo.toml` in `hw/` for stale “future” wording.
3. Align `ARCHITECTURE.md` or `CLAUDE.md` phase table with maintainers (single PR).

---

## 7. Built-in platform defaults (PD3)

**Refs:** [`cursor-v2-python.md`](../research/cursor-v2-python.md).

### Steps

1. Thread vCPU count and GIC version from frozen config into `install_arm_virt_board` (exact API TBD).
2. Ensure Python examples can override `--smp` without patching Rust.

---

## Dependencies on other plans

| Dependency | Plan |
|------------|------|
| Active vCPU | [`cursor-plan-02-runtime-active-vcpu.md`](cursor-plan-02-runtime-active-vcpu.md) — Python must use same accessor |
| Instrumentation | [`cursor-plan-04-framework-instrumentation.md`](cursor-plan-04-framework-instrumentation.md) |

---

## Out of scope

- **Full QEMU virt parity** — remain in [`aarch64-fs-virt-machine-completeness.md`](aarch64-fs-virt-machine-completeness.md).
- **RISC-V SE completion** — [`cursor-plan-00-roadmap.md`](cursor-plan-00-roadmap.md) § RISC-V SE; implementation in `linux_riscv64.rs` / `helm_riscv64`.

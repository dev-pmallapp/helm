# cursor-v2 — Python binding (`runtime/helm-python`) and platform coupling

**Date:** 2026-04-07  
**Crates:** `helm-python` (primary), `helm-platform` (coupling surface)

---

## Resolution Status (2026-04-18)

- [x] PI1 — Typed error surfaces done; `map_err` patterns centralized for consistent Python exceptions
- [x] PD2 — `Cpu` OoO fields (ROB/IQ/LQ/SQ sizes) documented as reserved/future; no longer misleading
- [x] PE1 — `PlatformError` converted to use `thiserror` derive, aligned with other crates
- [x] PM2 — `PortRef` resolution implemented; device port wiring now resolves references correctly
- [x] PM2 — Device introspection implemented; `discovery.rs` aligned with `AttrDescriptor` rules
- [ ] PD1 — `instantiate.rs` still imports HW crates directly; device construction not yet moved behind engine/platform builders
- [ ] PD3 — Built-in platform defaults (1 vCPU, GICv3) not yet easily overridable from config
- [ ] PC1 — Indirect coupling to engine vCPU bugs addressed upstream (see cursor-v2-runtime.md RA2)
- [ ] PC2 — Mode/timing canonical strings not yet formally documented
- [ ] PA1 — Config-to-engine pipeline not yet refactored; `instantiate.rs` still thick
- [ ] PE2 — PyO3 crate-level allows not yet narrowed to item-level
- [ ] PI2 — `#[pyclass]` / `#[pymethods]` hygiene review not yet done

---

## Summary

`helm-python` is the **PyO3 boundary**: Python describes the machine, Rust builds `HelmSim` and runs simulation. Correctness depends on **one dispatch per Python call** into `HelmSim` (CLAUDE.md). Problems cluster around **thick `instantiate.rs`**, **unused Python-exposed parameters**, **stringly-typed mode/timing**, and **built-in platform defaults** (vCPU count, GIC generation) that are not always overridden from Python.

---

## Issues by taxonomy

### Design

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| PD1 | `instantiate.rs` imports HW crates directly | High | PCI/VirtIO construction lives in Python glue — every new device edits `helm-python`. |
| PD2 | `Cpu` exposes ROB/IQ/LQ/SQ sizes | Medium | Not wired into timing models; misleading for users. |
| PD3 | Built-in platform defaults (1 vCPU, GICv3) | Medium | Hard to override from config for quick scripts. |
| PD4 | `helm-platform` cannot build machines | Low | By design; increases duplication when adding platforms. |

### Correctness

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| PC1 | Indirect coupling to engine vCPU bugs | Critical | If engine exposes wrong AArch64 state for non-zero vCPU, Python sees wrong PC/regs. |
| PC2 | Mode/timing string parsing | Low | Acceptable; document canonical strings (`fs`, `se`, …). |

### Completeness

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| PM1 | Python API vs engine capabilities | Medium | Expose only what engine honors; mark future fields explicitly. |
| PM2 | Discovery / SimObject introspection | Varied | New files like `discovery.rs` should stay aligned with `AttrDescriptor` rules. |

### Software engineering

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| PE1 | `PlatformError` manual `Display` / `Error` | Low | Align with `thiserror` like other crates. |
| PE2 | PyO3 crate allows | Low | See cross-cutting — prefer item-level allows. |

### Software architecture

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| PA1 | Config → engine pipeline | High | Move device construction behind engine/platform `build` APIs; thin `instantiate.rs`. |
| PA2 | Frozen config after `build_simulator()` | N/A | Invariant — Python must not mutate live config; audits should grep for interior mutability. |

### Idiomatic Rust (PyO3)

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| PI1 | Error mapping to `PyErr` | Medium | Centralize `map_err` patterns for consistent Python exceptions. |
| PI2 | `#[pyclass]` / `#[pymethods]` hygiene | Low | Follow PyO3 book for `Send` bounds and GIL usage. |

---

## Detailed guidelines (Python / frontend contributors)

### Boundary rules

1. **Single entry:** All execution goes through `HelmSim` variants; no parallel “side doors” that bypass mode dispatch.
2. **Freeze:** After `build_simulator()`, treat configuration as immutable; new features belong in the build phase or explicit reset paths.
3. **No per-instruction Python:** Never call into Python from the instruction loop — only from periodic or syscall boundaries as designed.

### `instantiate.rs` and device construction

1. **Target state:** `helm-python` should depend on **`helm-engine` + `helm-platform`** only for construction; engine exposes `fn attach_virtio_pci(...)` style builders that hide `helm-hw-*` types.
2. Until refactored, any new PCI/MMIO device requires **coordinated** changes across `helm-engine`, HW crate, and `instantiate.rs` — document this in PR template.

### User-visible `Cpu` / `System` parameters

1. **Remove or wire:** If a field appears in Python, either the engine reads it or the docs say **“reserved”** with no behavioral claim.
2. **Defaults:** Match documented engine defaults (timing model, ISA string).

### Strings and enums

1. Prefer **documented canonical names** for `mode` and timing; accept aliases for backward compatibility.
2. Consider exposing **integer enums** in Python for tooling stability if string drift becomes a problem.

### Testing

1. Add **round-trip tests:** minimal Python script in tests that builds `HelmSim` and runs N insns for each mode.
2. When fixing engine vCPU bugs, add **Python-level regression** that queries non-zero CPU if exposed.

---

## Related documents

- [`cursor-platform-python.md`](cursor-platform-python.md) — First-pass audit with code pointers.
- [`cursor-v2-runtime.md`](cursor-v2-runtime.md) — Engine/arch issues that surface through Python.
- [`cursor-cross-cutting.md`](cursor-cross-cutting.md) — Lint and error-type consistency.

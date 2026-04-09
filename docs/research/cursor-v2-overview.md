# cursor-v2 — Research overview and issue taxonomy

**Date:** 2026-04-07  
**Scope:** `debug/`, `framework/`, `hw/`, `runtime/helm-python`, and other `runtime/` crates (see per-domain files).  
**Prerequisite:** Read [`CLAUDE.md`](../../CLAUDE.md) and [`AGENTS.md`](../../AGENTS.md) for project invariants.

## Purpose of the v2 series

The `cursor-v2-*.md` documents complement the first-pass audits (`cursor-framework.md`, `cursor-debug.md`, etc.) and [`cursor-cross-cutting.md`](cursor-cross-cutting.md). They:

1. Use a **consistent taxonomy** so issues can be triaged and tracked uniformly.
2. End each domain document with **contributor guidelines** — rules of thumb that prevent recurring problems.
3. Call out **completeness** and **software-engineering** gaps explicitly (often missing from “bug-only” reviews).

## Reading order

| Order | Document | Contents |
|-------|----------|----------|
| 1 | This file | Taxonomy, conventions, links |
| 2 | [`cursor-v2-framework.md`](cursor-v2-framework.md) | `framework/*` crates |
| 3 | [`cursor-v2-debug.md`](cursor-v2-debug.md) | `debug/helm-spy`, `debug/helm-report` |
| 4 | [`cursor-v2-hw.md`](cursor-v2-hw.md) | `hw/*` device crates |
| 5 | [`cursor-v2-runtime.md`](cursor-v2-runtime.md) | `helm-arch`, `helm-engine`, `helm-debug`, `helm-platform`, `helm-cli` |
| 6 | [`cursor-v2-python.md`](cursor-v2-python.md) | `helm-python` and PyO3 boundary |

For workspace-wide patterns (lint suppression, error types, memory surfaces), keep [`cursor-cross-cutting.md`](cursor-cross-cutting.md) as the canonical cross-domain reference; v2 domain files only summarize what matters locally.

---

## Issue taxonomy (definitions)

Use these labels when filing issues or reviewing PRs.

### Design

**Meaning:** Intended structure, boundaries, or invariants are wrong, unclear, or fight the project’s stated rules (e.g. CLAUDE.md “Critical Design Rules”).

**Examples:** Wrong crate dependency direction; duplicate APIs with different semantics; mixing experimental and production memory surfaces without a feature gate.

**Not the same as:** A simple bug in an otherwise sound design (that is **Correctness**).

### Correctness

**Meaning:** Behavior disagrees with the architecture manual, ABI, POSIX expectations, or documented simulator contract for the chosen mode (SE vs FS).

**Examples:** Wrong GIC priority table lookup; fault path constructing the wrong `ArchContext`; MMIO ignoring access width where the guest relies on byte/halfword behavior.

### Completeness

**Meaning:** Stubbed, gated-off, or explicitly TODO’d behavior; missing tests for a claimed feature; API surface that suggests behavior that is not implemented.

**Examples:** IOMMU page walk TODOs; `MemoryMap` alias resolution TODO; Python `Cpu` fields that nothing reads; RISC-V syscall set incomplete vs plan.

**Not the same as:** A deliberate minimal MVP with clear docs (that is acceptable **Design** if documented).

### Software engineering

**Meaning:** Maintainability, testability, observability, CI hygiene, and change safety — without necessarily being a “bug” today.

**Examples:** Crate-level `#![allow(dead_code)]` hiding rot; duplicate wiring entry points; release builds silently disabling instrumentation; CSV column/header mismatch in reporting.

### Software architecture

**Meaning:** Larger-scale structure: layering, coupling, boundaries between “Python describes / Rust simulates”, placement of platform bring-up, fan-out of dependencies, evolution toward SMP or multi-ISA.

**Examples:** `helm-python` importing every HW crate for PCI/VirtIO construction; `HelmEngine` monolith; thread-local probe context vs future SMP.

### Idiomatic Rust (and API style)

**Meaning:** Non-idiomatic patterns that increase bug risk or confuse Rust API consumers: manual `Error` impls where `thiserror` is used elsewhere; `Result<_, String>`; inconsistent naming; missing or misleading `must_use` / type safety.

---

## Severity guidance

Use **Critical / High / Medium / Low** consistently:

| Level | When to use |
|-------|-------------|
| **Critical** | Wrong guest-visible state for common workloads; data loss; UB; security-relevant mistake; silent wrong CPU in multi-vCPU paths |
| **High** | Wrong behavior for plausible guest code; silent no-op in release; systemic pattern (e.g. all devices ignore `size`) |
| **Medium** | Bounded incorrectness, perf traps, API confusion |
| **Low** | Docs, naming, duplicate comments, long-term tick overflow |

---

## Global guidelines (all crates)

These repeat CLAUDE.md in actionable form:

1. **Monomorphize timing only** — `HelmEngine<T: TimingModel>` is the only generic simulation parameter; ISA and mode use enums.
2. **One dispatch per Python entry** — not per instruction.
3. **No dark state** — persistent fields must be registered for checkpoint/introspection where the object model applies.
4. **Devices:** no base address; no IRQ number in device code — platform owns routing.
5. **No hot-loop dynamic lookup** — resolve `Arc` and indices in `elaborate()`.
6. **Determinism** — no wall clock in the execution hot path unless explicitly documented otherwise.
7. **Prefer `sim_*!` macros** for simulator diagnostics (see AGENTS.md); separate guest serial from sim trace.

---

## Relationship to older `cursor-*.md` files

| Older file | v2 counterpart |
|------------|----------------|
| `cursor-framework.md` | `cursor-v2-framework.md` |
| `cursor-debug.md` | `cursor-v2-debug.md` |
| `cursor-hw.md` | `cursor-v2-hw.md` |
| `cursor-runtime.md` + platform/cli notes | `cursor-v2-runtime.md` |
| `cursor-platform-python.md` | `cursor-v2-python.md` |
| `cursor-cross-cutting.md` | Stays canonical for cross-cutting; overview above references it |

When a finding appears in both places, treat **v2** as the taxonomy-aligned index; treat **first-pass cursor-\*.md** as the detailed code-pointer narrative.

---

## Execution plans (rectify issues / enhance codebase)

Step-by-step plans derived from this research and existing product plans live under [`docs/plans/`](../plans/):

- **[`cursor-plan-00-roadmap.md`](../plans/cursor-plan-00-roadmap.md)** — hub linking P0–P2 waves and `docs/plans/*.md`
- **[`cursor-plan-01-p0-correctness.md`](../plans/cursor-plan-01-p0-correctness.md)** through **[`cursor-plan-05-python-tooling-ci.md`](../plans/cursor-plan-05-python-tooling-ci.md)** — ordered remediation tracks

# cursor-v2 — Debug crates (`debug/`)

**Date:** 2026-04-07  
**Crates:** `helm-spy`, `helm-report`

---

## Summary

`helm-spy` collects analysis state and session wiring; `helm-report` formats and sinks output (JSON, CSV, async). Dependency direction is correct: `helm-report` → `helm-spy`, no cycle.

**Top risks:** `cfg(debug_assertions)` gating probe subscription (release builds silently collect nothing); CSV/header consistency; swallowed errors in async/report paths; duplication between `HelmSpy::subscribe` and `ProbePluginBridge::wire`.

---

## Issues by taxonomy

### Design

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| DD1 | Debug-only probe wiring | High | Release binaries may build with no subscription path; use a Cargo feature or runtime flag. |
| DD2 | Two wiring entry points (`HelmSpy` vs `ProbePluginBridge`) | Medium | Different trigger coverage; consolidate or delegate. |
| DD3 | Fat `ReportFormatter` trait | Low | Incremental vs session formatters could split for clarity. |
| DD4 | `QuantumObserver` (if still present) undocumented integration | Low | Either wire or hide from public API until integrated. |

### Correctness

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| DC1 | CSV columns vs row order | High | Must match header; add round-trip tests for every formatter variant. |
| DC2 | Async sink error handling | Medium | Swallowed errors lose diagnostics; log or surface. |

### Completeness

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| DM1 | Release profiling story | Medium | Document how to enable spy in release (feature flags). |
| DM2 | Formatter coverage | Low | Property tests for empty sessions, large traces, unicode paths. |

### Software engineering

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| DE1 | Test matrix debug vs release | High | CI should include at least one release build test that asserts instrumentation hooks when feature enabled. |
| DE2 | Duplicated wiring logic | Medium | Single source of truth for probe attachment. |

### Software architecture

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| DA1 | Spy vs engine responsibilities | Low | Spy should not own execution policy; engine owns stepping, spy observes. |

### Idiomatic Rust

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| DI1 | `unsafe_code` allow at crate level (`helm-spy`) | Low | Narrow to modules that need it (`trace_ring` etc.). |

---

## Detailed guidelines (debug contributors)

### `helm-spy`

1. **Never rely on `debug_assertions` alone** for user-visible profiling. If the feature must exist in release, gate with `feature = "spy"` (or similar) and document in crate README / AGENTS.md.
2. **Single wiring API:** Add new probe hooks in one function; have bridge call that function.
3. **Tests:** Include a test that enables the same code path as production profiling builds.

### `helm-report`

1. **CSV/TSV:** Generate header from the same ordered field list used for rows; add a unit test that parses output back.
2. **Errors:** Prefer `thiserror` or structured errors for sink failures; avoid empty `catch` patterns in async code without logging.
3. **Incremental delivery:** If a formatter implements only `format_session`, document that incremental methods are no-ops or panic — or split traits.

### Cross-layer

1. Align with **probe → spy → report** pipeline described in `helm-plugin` crate docs; avoid a third parallel observation channel without an architectural note.

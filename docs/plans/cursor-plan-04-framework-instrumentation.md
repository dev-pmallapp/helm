# Plan P2 — Framework: instrumentation, timing bounds, stats, memory API clarity

**Goal:** Address **Medium** framework and cross-cutting items from [`cursor-v2-framework.md`](../research/cursor-v2-framework.md) and [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) §§1, 3, 9.

---

## Current tree status

Completed on the active execution branch:

- `instrumentation` feature gating on the probe/spy wiring path
- unified probe wiring in `debug/helm-spy`
- `IntervalTiming` register-index guards
- full-width register-field mask handling
- `StatsRegistry` histogram registration/export
- `MemoryMap` feature gating
- `EventQueue::post_after` overflow hardening
- current-doc cleanup for `SubscriptionId` explicit unsubscribe semantics in user-facing architecture/API docs
- CI release coverage for instrumentation via `.github/workflows/ci.yml`
- narrowed root-level lint suppressions in `helm-engine` and `helm-python`, plus follow-up lint cleanups needed to keep workspace clippy green

Still left in this plan:

- any additional lint-suppression narrowing you want to pursue beyond the current slice
- broader documentation cleanup in historical design docs that still describe older event-bus handle designs

---

## 1. Replace `cfg(debug_assertions)` with a Cargo feature (CC-9)

**Refs:** [`cursor-v2-debug.md`](../research/cursor-v2-debug.md) DD1.

### Steps

1. Add workspace feature `instrumentation` (or `spy`) on `helm-spy` / `helm-probe` consumers as needed.
2. Replace `#[cfg(debug_assertions)]` on `HelmSpy::subscribe` and `ProbePluginBridge::wire` with `#[cfg(feature = "instrumentation")]` **or** compile always with runtime no-op when disabled.
3. Default: enable feature in dev profile via `[profile.dev]` package features **or** document `cargo build --features instrumentation` for release profiling.
4. Add CI job: **release** test with `--features instrumentation` that asserts probe wiring runs.

**Gate:** Release binary can collect spy data when feature enabled.

---

## 2. Unify probe wiring entry points

**Refs:** [`cursor-v2-debug.md`](../research/cursor-v2-debug.md) DD2.

### Steps

1. Choose canonical API: `HelmSpy::subscribe` or thin wrapper.
2. Implement `ProbePluginBridge::wire` as `delegate` to the same internal function used by `subscribe` (including triggers).
3. Delete duplicate logic; update call sites.

**Gate:** One code path; triggers work from both entry points.

---

## 3. `IntervalTiming` register index safety (FW-C1)

### Steps

1. In `helm-timing`, guard `dst_reg` / `src_reg` before indexing fixed arrays — `debug_assert!` plus production-safe clamp or skip with `sim_warn!` once.
2. Document maximum register id contract for `TimingInsnInfo`.
3. Add unit test with out-of-range metadata in **test-only** path.

**Gate:** No panic on malformed metadata in release.

---

## 4. `FieldDesc::mask` full-width fields (FC2)

### Steps

1. Fix `register_bank!` / `FieldDesc::mask` for 64-bit width using safe shift pattern (`u64::MAX >> (64 - width) << lsb` or special case `width == 64`).
2. Add const-eval test for `msb=63, lsb=0`.

---

## 5. `StatsRegistry` histogram integration (FW-D6)

### Steps

1. Extend `StatsRegistry` with optional `HashMap` of histograms mirroring counter API.
2. Wire `dump_json` / iteration if counters already dump.
3. Unit tests for register + sample + dump.

---

## 6. `MemoryMap` experimental gate (FD1)

### Steps

1. Add feature `experimental-memmap` on `helm-memory` and gate `MemoryMap` exports **or** prominent `module` split.
2. Update `docs/` references to use `HelmAddressSpace` for production paths.

---

## 7. `EventQueue` tick overflow (FC3)

### Steps

1. Use `checked_add` in `post_after`; on overflow, panic with clear message or saturate — document choice.

---

## 8. `SubscriptionId` Drop (event bus)

### Steps

1. Remove stale RAII claims from current docs and keep `SubscriptionId` as an explicit unsubscribe token (see [`cursor-v2-framework.md`](../research/cursor-v2-framework.md) FM4).

---

## 9. Lint narrowing (cross-cutting §1)

### Steps

1. Remove `dead_code` from `helm-engine` crate root — fix or delete dead items.
2. Progressively narrow `clippy::pedantic` allows to file or item level.

**Gate:** Documented in [`cursor-plan-00-roadmap.md`](cursor-plan-00-roadmap.md) completion gates.

---

## Out of scope

- **`helm-decode` test expansion** — small follow-up PR or tie to decode format work.
- **AccurateTiming** full pipeline — [`smp-timing-model-analysis.md`](smp-timing-model-analysis.md) Phase 3.

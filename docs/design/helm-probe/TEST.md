# TEST: helm-probe

> **Status**: Reflects the 17 tests that actually exist and pass in the current codebase.
> All tests live in `framework/helm-probe/tests/`.
> Tests for chain/filter, TraceSink, ProbePluginBridge, and sim_trace level filtering
> are **not yet written** — they are Phase 2/3 work.

---

## Run command

```bash
cargo test -p helm-probe
```

Expected: 17 tests pass (dev profile). Several tests are gated `#[cfg(debug_assertions)]`
and do not run in release.

---

## Test files

```
framework/helm-probe/tests/
├── probe_basic.rs     — Probe<T> struct and bundle tests (12 tests)
├── macro_tests.rs     — probe!() macro tests (4 tests)
└── probe_release.rs   — ZST size and BranchEvent construction tests (2 tests, both profiles)
```

---

## `probe_basic.rs` — Probe struct and bundle tests

### Always run (both profiles)

| Test | What it verifies |
|---|---|
| `new_probe_has_no_listeners` | `Probe::new()` starts with `has_listeners() == false` |
| `default_equals_new` | `Probe::default()` behaves the same as `Probe::new()` |
| `notify_no_listeners_no_panic` | `notify()` with no subscribers does not panic |
| `probe_is_send_sync` | `Probe<u64>` and `Probe<CpuStepEvent>` satisfy `Send + Sync` bounds (compile-time check) |
| `cpu_probes_default` | `CpuProbes::default()` creates all five fields with no listeners |
| `gic_probes_default` | `GicProbes::default()` creates all three fields with no listeners |

### Debug-only (`#[cfg(debug_assertions)]`)

| Test | What it verifies |
|---|---|
| `debug_only::subscribe_enables_has_listeners` | After `subscribe()`, `has_listeners()` returns `true` |
| `debug_only::listener_count_increments` | `listener_count()` returns 0 then 1 then 2 as listeners are added |
| `debug_only::notify_delivers_to_subscriber` | `notify(&42)` and `notify(&99)` both land in the subscriber's collection in order |
| `debug_only::multiple_listeners_all_receive` | Two subscribers both receive the same event |
| `debug_only::listeners_fire_in_order` | Three subscribers fire in subscription order: [1, 2, 3] |

---

## `macro_tests.rs` — `probe!()` macro tests

### Always run (both profiles)

| Test | What it verifies |
|---|---|
| `macro_skips_eval_no_listeners` | The event expression inside `probe!()` is not evaluated when no listeners are attached. Sets `evaluated = true` inside the expression and asserts it stays `false`. |

### Debug-only (`#[cfg(debug_assertions)]`)

| Test | What it verifies |
|---|---|
| `debug_only::macro_delivers_when_subscribed` | `probe!(p, 77u32)` delivers value 77 to a subscriber |
| `debug_only::macro_delivers_struct_event` | `probe!(p, CpuStepEvent { pc: 0x4000_0000, ... })` delivers the correct `pc` field to a `CpuStepEvent` subscriber |
| `debug_only::macro_evaluates_once_per_call` | The expression block is evaluated exactly once per `probe!()` invocation — counter increments once per call, not once per listener |

---

## `probe_release.rs` — Size and event construction tests

These tests run in both dev and release profiles.

| Test | What it verifies |
|---|---|
| `probe_zst_in_release` | In release (`#[cfg(not(debug_assertions))]`): `size_of::<Probe<u64>>() == 0` and `size_of::<CpuProbes>() == 0`. In dev: just checks the types compile and construct. |
| `cpu_probes_default_and_branch_event` | `CpuProbes::default()` constructs without panic; `BranchEvent { pc, target, taken, kind: BranchKind::Call }` constructs with correct field values. |

---

## Test count summary

| File | Always-run | Debug-only | Total |
|---|---|---|---|
| `probe_basic.rs` | 6 | 5 | 11 |
| `macro_tests.rs` | 1 | 3 | 4 |
| `probe_release.rs` | 2 | 0 | 2 |
| **Total** | **9** | **8** | **17** |

In a release build (`cargo test -p helm-probe --release`), 9 tests run. In dev, all 17 run.

---

## What is NOT tested (planned for Phase 2/3)

The following items are designed in TEST.md (old version) but not yet implemented:

| Planned test area | Phase |
|---|---|
| Chain/filter: `FilteredCb<T>`, stock filters, `Chain<T>` | 3 |
| TraceSink: buffer/null/stderr/simtrace variants | 3 |
| ProbePluginBridge: probe → InsnInfo enrichment | 2 |
| sim_trace level ordering and level filtering | 2 |
| Engine integration: pre_step fires per instruction, post_step PC values | 2 |
| Release ASM verification: zero probe instructions in hot loop | 2 |
| ISA regression: 663+ ISA tests still pass after probe wiring | ongoing |

The ISA regression is verified separately by:
```bash
cargo test -p helm-arch --lib
```

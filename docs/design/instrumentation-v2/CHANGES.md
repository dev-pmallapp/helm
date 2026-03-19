# Instrumentation-v2 — Required Implementation Changes per Crate

> This document tracks every code change needed to implement the Instrumentation-v2
> redesign described in [PLAN.md](PLAN.md). Each section covers one crate. Status is
> updated as work is completed.
>
> **Legend:** ☐ pending · ✓ done · ⚠ breaking change · 🗑 delete

---

## New Crates to Create

### `framework/helm-diag/` ✅ IMPLEMENTED (50 tests)

**Why**: Extract `helm-debug::sim_trace` to break the `helm-arch → helm-debug` layer violation.

| Change | Detail |
|---|---|
| ✓ `Cargo.toml` | `name = "helm-diag"`, zero deps |
| ✓ `src/entry.rs` | `DiagEntry`, `DiagLevel` (Info/Warn/Stub/Error — Branch removed) |
| ✓ `src/sink.rs` | `DiagMonitor`, `DiagSink` (background drain thread, URI backend) |
| ✓ `src/lib.rs` (emit, install_monitor()`, `update_sim_ctx()`, thread-locals |
| ✓ `src/macros.rs` | `sim_stub!`, `sim_warn!`, `sim_info!` (unchanged semantics, new import path) |
| ☐ Root `Cargo.toml` | Add `helm-diag = { path = "framework/helm-diag" }` to workspace deps |

**Verification**: `cargo build -p helm-diag` passes; `helm-arch` test build passes without `helm-debug` dep.

---

### `framework/helm-spy/` ✅ IMPLEMENTED (74 tests)

**Why**: Replace `helm-plugin` with a composable analysis primitive system, decoupled from delivery.

| Change | Detail |
|---|---|
| ✓ `Cargo.toml` | `name = "helm-spy"`, deps: `helm-probe`, `helm-core` |
| ✓ `src/primitives/counter.rs` | `Counter`, `PerVcpuCounter` |
| ✓ `src/primitives/indexed_counter.rs` | `IndexedCounter` (fixed label array of `AtomicU64`) |
| ✓ `src/primitives/histogram.rs` | `Histogram`, `IntervalHistogram` |
| ✓ `src/primitives/heatmap.rs` | `HeatMap` (per-PC DashMap or sharded HashMap) |
| ✓ `src/primitives/ringbuf.rs` | `RingBuffer<T>`, `EventStream<T>` |
| ✓ `src/primitives/trace_ring.rs` | `TraceRing<T, const N: usize>` (SPSC lock-free), `BranchRecord` |
| ✓ `src/primitives/correl.rs` | `CorrelHist2D` |
| ✓ `src/trigger.rs` | `Trigger`, `TriggerKind` |
| ✓ `src/window.rs` | `Window`, `Windowed<T>` |
| ✓ `src/quantum.rs` | `QuantumObserver` trait |
| ✓ `src/events.rs` | Move `InsnInfo`, `BranchInfo`, `MemInfo`, `InsnClass`, `BranchKind`, `ArchContext` etc. from `helm-plugin::runtime::info` |
| ✓ `src/analysis/insn_mix.rs` | `InsnMix` (replaces HowVec plugin) |
| ✓ `src/analysis/cache.rs` | `CacheModel` (replaces CacheSim plugin) |
| ✓ `src/analysis/branch_pred.rs` | `BranchPredictor` (BiModal, GShare) |
| ☐ Phase 3 `src/analysis/simpoint.rs` | `SimPoint` (Phase 3) |
| ☐ Phase 3 `src/analysis/power.rs` | `PowerModel` (Phase 3) |
| ☐ Phase 3 `src/analysis/diff.rs` | `DiffAnalysis` (Phase 3) |
| ☐ `src/bridge.rs` (pending probe wiring) | `ProbePluginBridge` (moved from planned helm-plugin location) |
| ✓ `src/session.rs` | `SpySession`, `SpySnapshot` |
| ☐ Root `Cargo.toml` | Add workspace dep (excluded for now) |

---

### `framework/helm-report/` ✅ IMPLEMENTED standalone (62 tests; SpySessionSnapshot not yet wired to helm-spy dep)

**Why**: Delivery layer — separate from collection. Replaces `sim_trace::Backend` for analysis output.

| Change | Detail |
|---|---|
| ☐ `Cargo.toml` | `name = "helm-report"`, deps: `helm-spy` |
| ☐ `src/sink/mod.rs` | `Sink` trait |
| ☐ `src/sink/file.rs` | `FileSink`, `AsyncFileSink` (async drain thread) |
| ☐ `src/sink/stderr.rs` | `StderrSink` |
| ☐ `src/sink/tcp.rs` | `TcpSink` |
| ☐ `src/sink/null.rs` | `NullSink` |
| ☐ `src/sink/binary.rs` | `BinaryTraceSink<T>` + `TraceHeader` |
| ☐ `src/sink/python.rs` | `PythonSink` (Arc<Mutex<Vec<String>>> for PyO3) |
| ☐ `src/sink/uri.rs` | `sink_from_uri(uri) -> Box<dyn Sink>` |
| ☐ `src/format/text.rs` | `TextFormatter` (human-readable atexit-style) |
| ☐ `src/format/json.rs` | `JsonFormatter` |
| ☐ `src/format/csv.rs` | `CsvFormatter` |
| ☐ `src/format/gemstats.rs` | `GemstatsFormatter` (gem5 stats.txt compat) |
| ☐ `src/report.rs` | `Report` struct + `deliver()` |
| ☐ `src/schedule.rs` | `ReportSchedule`, `ReportTrigger` |
| ☐ `src/snapshot.rs` | `SpySpySnapshot` (immutable capture for formatting) |
| ☐ Root `Cargo.toml` | Add workspace dep |

---

## Existing Crates — Changes Required

### `framework/helm-probe/` ⚠

| Change | Priority | Detail |
|---|---|---|
| ☐ Add `BranchEvent` to `src/events.rs` | **P1** | `{ pc, target, taken, kind: BranchKind }` — `BranchKind` re-exported from `helm-spy::events` or defined here |
| ☐ Add `branch: Probe<BranchEvent>` to `CpuProbes` | **P1** | In `helm-engine/src/lib.rs` |
| ☐ Add `MmioEvent` to `src/events.rs` | P2 | `{ addr, size, val, is_write }` — for SystemMem dispatch wiring |
| ☐ Update `src/lib.rs` re-exports | P1 | Export `BranchEvent`, `MmioEvent` |

No doc changes needed beyond updating HLD to note BranchEvent.

---

### `framework/helm-plugin/` 🗑 ⚠

**Status**: Deprecated and scheduled for removal. Being replaced by `helm-spy`.

| Change | Priority | Detail |
|---|---|---|
| ☐ Move `runtime/info.rs` types to `helm-spy::events` | **P1** | `InsnInfo`, `BranchInfo`, `MemInfo`, `SyscallInfo`, `FaultInfo`, `InsnClass`, `BranchKind`, `ArchContext`, `FaultKind` |
| ☐ Move `runtime/scoreboard.rs` to `helm-spy::primitives` | **P1** | `Scoreboard<T>` used by `PerVcpuCounter` |
| 🗑 Delete `runtime/registry.rs` | P1 | `PluginRegistry` replaced by `SpySession::subscribe()` |
| 🗑 Delete `runtime/callback.rs` | P1 | All `Box<dyn Fn(...)>` callback type aliases |
| 🗑 Delete `api/plugin.rs` | P1 | `HelmPlugin` trait and `PluginArgs` |
| 🗑 Delete entire `builtins/` directory | P2 | All 11 built-in plugins replaced by `helm-spy` primitives and models |
| ☐ Remove `helm-plugin` dep from `helm-engine/Cargo.toml` | P1 | Replace with `helm-spy` |
| ☐ Remove `helm-plugin` dep from `helm-python/Cargo.toml` | P1 | Python API now uses `helm-spy` + `helm-report` |
| ☐ Remove from workspace members | P2 | After all deps migrated |

**Plugin migration mapping:**
| Old plugin | New mechanism |
|---|---|
| `insn-count` | `SpySession.insn_count: Counter` |
| `howvec` | `SpySession.insn_mix: InsnMix` (uses `IndexedCounter`) |
| `hotblocks` | `SpySession.hot_pcs: HeatMap` |
| `branch-trace` | `SpySession.branch_heatmap: HeatMap` + `BranchPredictor` |
| `cache-sim` | `SpySession.cache_l1d: CacheModel` |
| `execlog` | `SpySession.exec_stream: EventStream<InsnInfo>` |
| `mem-trace` | `SpySession.mem_stream: EventStream<MemInfo>` |
| `syscall-trace` | `SpySession.syscall_stream: EventStream<SyscallInfo>` |
| `fault-detect` | `SpySession.fault_history: RingBuffer<CpuFaultEvent>` |
| `stub-tracer` | `SpySession.insn_mix` (is_stub flag on InsnInfo) |
| `watchpoint` | `helm-debug::WatchpointEngine` |

---

### `runtime/helm-debug/` ⚠

| Change | Priority | Detail |
|---|---|---|
| 🗑 Delete `src/sim_trace.rs` | **P1** | Moved to `helm-diag`; remove entirely from helm-debug |
| 🗑 Delete `src/lib.rs::TraceLogger` | **P1** | Was a stub; never implemented |
| ☐ Add dep on `helm-diag` | P1 | For `DiagSink::open(uri)` at startup; `DiagMonitor` install |
| ☐ Add `src/watchpoint.rs` | P2 | `WatchpointEngine` — subscribes to `Probe<MemAccessEvent>` |
| ☐ Add `src/breakpoint.rs` | P2 | `BreakpointEngine` — subscribes to `Probe<CpuStepEvent>` (pre_step) |
| ☐ Add `src/inspect.rs` | P3 | `InspectionAPI` — dump arch state, memory range on demand |
| ☐ Update `Cargo.toml` | P1 | Remove dependency on `helm-core` for sim_trace (now in helm-diag); add `helm-diag`, `helm-probe` |
| ☐ `src/lib.rs` | P1 | Remove `pub mod sim_trace`; add `pub mod watchpoint`, `pub mod breakpoint` |

---

### `runtime/helm-engine/` ⚠

| Change | Priority | Detail |
|---|---|---|
| ☐ `Cargo.toml` | P1 | Add `helm-probe`, `helm-diag`, `helm-spy`, `helm-report`; remove `helm-plugin` |
| ☐ `src/lib.rs` | P1 | Add `pub probes: CpuProbes` to `HelmEngine<T>`; remove `pub plugins: PluginRegistry` |
| ☐ `src/lib.rs` | P1 | Add `CpuProbes` struct with `pre_step`, `post_step`, `fault`, `mem`, `branch` fields |
| ☐ `src/lib.rs` | P1 | Remove `InstrumentedMem` (or adapt to emit `Probe<MemAccessEvent>` instead of recording to `PluginRegistry`) |
| ☐ `src/lib.rs` | P1 | Remove all `self.plugins.fire_*()` calls; replace with `probe!(...)` calls |
| ☐ `src/lib.rs` | P1 | Remove `add_plugin()` method |
| ☐ `src/lib.rs` | P1 | Add `observe() -> SpySession` builder method |
| ☐ `src/lib.rs` | P1 | Add `quantum_end()` call at `run()` return — notifies SpySession |
| ☐ `src/fs.rs` | P1 | Add `probes: &CpuProbes` param; insert `probe!(probes.pre_step, ...)`, `probe!(probes.branch, ...)`, etc. |
| ☐ `src/se/mod.rs` | P1 | Wire SE step loop with same probe calls |
| ☐ `classify_aarch64_opcode()` | P2 | Must remain in helm-engine (used by ProbePluginBridge for InsnInfo enrichment) |
| ☐ Remove `HelmSim::add_plugin()` | P1 | ⚠ Breaking Python API change |
| ☐ Add `HelmSim::observe()` | P1 | Returns `SpySession` configured for this engine |
| ☐ `HelmSim::cpu_probes_mut()` | P1 | External subscription access |

---

### `runtime/helm-arch/` ⚠

| Change | Priority | Detail |
|---|---|---|
| ☐ `Cargo.toml` | **P1** | Remove `helm-debug` dep; add `helm-diag` dep |
| ☐ All `use helm_debug::sim_trace` | P1 | Change to `use helm_diag` — `sim_stub!`, `sim_warn!` import path update (~50 call sites in execute/) |
| ☐ `src/aarch64/execute/branch.rs` | **P1** | Delete `sim_branch!(...)` calls; add `probe!(probes.branch, BranchEvent{...})` — requires `probes` param added to execute functions |
| ☐ Execute function signatures | P2 | Add `probes: &CpuProbes` parameter to `aarch64_execute()` or use thread-local approach |

**Note on execute signatures**: Adding `probes` to `aarch64_execute()` is a non-trivial API change. Two options:
1. Thread-local `CURRENT_PROBES: RefCell<Option<*mut CpuProbes>>` — set by engine before each step, read by executor (unsafe but simple)
2. Pass `probes: Option<&mut CpuProbes>` — explicit but changes every execute signature

Recommended: option 1 for Phase 1 (faster to ship), option 2 for Phase 2 refactor.

---

### `hw/helm-hw-intc/` ⚠

| Change | Priority | Detail |
|---|---|---|
| ☐ `Cargo.toml` | P2 | Add optional `helm-diag` dep (for `sim_warn!` in GIC stubs) |
| ☐ `Cargo.toml` | P2 | Add optional `helm-probe` dep (feature = "probe") for `GicProbes` |
| ☐ `src/gicv2/mod.rs` | P2 | Add `GicProbes` struct under `#[cfg(feature = "probe")]` |
| ☐ `src/gicv2/distributor.rs` | P2 | Wire `probe!(state.probes.irq_asserted, ...)` on IRQ assert |
| ☐ `src/gicv2/cpu_interface.rs` | P2 | Wire `probe!(state.probes.eoi, ...)` on EOI |

---

### `hw/helm-hw-char/`, `hw/helm-hw-timer/`, `hw/helm-hw-rtc/` ⚠

| Change | Priority | Detail |
|---|---|---|
| ☐ All `use helm_debug::sim_trace::*` | P1 | Change to `use helm_diag::*` (~5-10 call sites each) |
| ☐ `Cargo.toml` | P1 | Add `helm-diag` dep; remove `helm-debug` dep if only used for sim_trace |

---

### `runtime/helm-python/` ⚠

| Change | Priority | Detail |
|---|---|---|
| ☐ `src/lib.rs` | **P1** | ⚠ Breaking: remove `add_plugin(name, args)` PyO3 method |
| ☐ `src/lib.rs` | **P1** | ⚠ Breaking: remove `set_sim_trace(uri)` PyO3 method |
| ☐ `src/lib.rs` | P1 | Add `observe() -> PySpySession` — returns a Python-visible session object |
| ☐ `src/lib.rs` | P1 | Add `PySpySession` pyclass with `track_insns()`, `track_branches()`, `track_memory(l1d_size, ...)`, `report(sink, format)` |
| ☐ `src/lib.rs` | P1 | Add `breakpoint(pc, action)`, `watchpoint(addr, size, kind)` Python methods |
| ☐ `src/lib.rs` | P2 | Add `PySpySession.insn_count.value()`, `.insn_mix.table()`, `.hot_pcs.top(n)`, `.cache_l1d.hit_rate()` etc. as PyO3-exposed query methods |
| ☐ `Cargo.toml` | P1 | Remove `helm-plugin`; add `helm-spy`, `helm-report` |
| ☐ Python examples in `examples/debug/` | P1 | Update all `.py` files to new API (add_plugin → observe().track_*()) |

---

### Root `Cargo.toml` ⚠

| Change | Priority | Detail |
|---|---|---|
| ☐ Add `helm-diag` workspace dep | **P1** | `helm-diag = { path = "framework/helm-diag" }` |
| ☐ Add `helm-spy` workspace dep | P1 | `helm-spy = { path = "framework/helm-spy" }` |
| ☐ Add `helm-report` workspace dep | P1 | `helm-report = { path = "framework/helm-report" }` |
| ☐ Add `framework/helm-diag` to `members` | P1 | (auto-included if using `framework/*` glob) |
| ☐ Verify `framework/*` glob picks up new crates | P1 | Should work automatically |

---

## Implementation Order (Phases)

```
Phase 1 — Zero-breakage foundation
  1. Create helm-diag (move sim_trace; update 50+ import sites)
  2. Implement helm-probe fully (add BranchEvent, wire FS + SE loops)
  3. Delete sim_branch! from helm-arch/execute/branch.rs
  ── cargo build --workspace must pass ──

Phase 2 — Analysis primitives (helm-spy)
  4. Create helm-spy primitives (Counter through TraceRing)
  5. Create SpySession + ProbePluginBridge
  6. Create helm-report (Sink trait, formatters, Report)
  7. Migrate helm-engine: add probes, remove plugins, add observe()
  ── cargo test -p helm-arch --lib (663 tests) must pass ──

Phase 3 — Python API + plugin removal
  8. Migrate helm-python: remove add_plugin(), add observe() PyO3 bindings
  9. Remove helm-plugin crate (after all dependents migrated)
  10. Update Python examples in examples/debug/
  ── Full integration test: boot Linux ──

Phase 4 — Advanced analysis models
  11. CacheModel, BranchPredictor in helm-spy
  12. WatchpointEngine, BreakpointEngine in helm-debug
  13. BinaryTraceSink + typed trace files in helm-report

Phase 5 — Architectural exploration tools
  14. SimPoint BBV computation
  15. DiffAnalysis (compare two SpySessions)
  16. GemstatsFormatter
  17. PowerModel
```

---

## File Deletion List

When Phase 3 is complete, delete:

```
framework/helm-plugin/src/api/plugin.rs
framework/helm-plugin/src/api/mod.rs
framework/helm-plugin/src/runtime/registry.rs
framework/helm-plugin/src/runtime/callback.rs
framework/helm-plugin/src/runtime/info.rs          (moved to helm-spy)
framework/helm-plugin/src/runtime/scoreboard.rs    (moved to helm-spy)
framework/helm-plugin/src/builtins/               (entire directory)
framework/helm-plugin/src/lib.rs
framework/helm-plugin/Cargo.toml
framework/helm-plugin/                            (entire crate)
runtime/helm-debug/src/sim_trace.rs               (moved to helm-diag)
runtime/helm-debug/src/lib.rs → TraceLogger entry
```

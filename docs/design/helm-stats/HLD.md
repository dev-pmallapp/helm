# helm-stats — High-Level Design

> **Crate:** `helm-stats`
> **Phase:** Phase 1
> **Dependencies:** `helm-core` (for `System` path conventions)

---

## Overview

`helm-stats` provides a lock-free, hierarchically-namespaced statistics system for helm-ng simulations. It is modeled after Gem5's Stats system but adapted for Rust's concurrency model: counters use `AtomicU64` for multi-hart safety, formulas are lazy expression trees evaluated at dump time, and all statistics are accessible by a dot-path that mirrors the component hierarchy.

### Goals

- **Zero allocation on the hot path.** Incrementing a counter must be a single `fetch_add` on an `AtomicU64`.
- **Multi-hart safe.** All counter operations are lock-free; no mutex is held during a simulation step.
- **Hierarchical namespace.** Path `system.cpu0.icache.hits` is unambiguous and maps to the component tree.
- **Lazy formula evaluation.** Derived metrics (hit rates, CPI, bandwidth) are computed at dump time, not during simulation.
- **Simple dump formats.** JSON (for tooling) and a human-readable table (for the terminal).

### Non-Goals (Phase 0)

- Per-interval time-series dumps are deferred. Phase 0 produces final values only.
- No live histogram plotting or streaming output.

---

## Component Diagram

```
┌──────────────────────────────────────────────────────────┐
│                    StatsRegistry                          │
│  dot-path namespace → Arc<PerfCounter | PerfHistogram>   │
│                                                          │
│  perf_counter("system.cpu0.icache.hits", "...")          │
│  perf_histogram("system.cpu0.icache.latency", "...", []) │
│  perf_formula("system.cpu0.icache.hit_rate", expr)       │
│                                                          │
│  dump_json(path)   →  {"system.cpu0.icache.hits": 42}   │
│  print_table()     →  tabular output to stdout           │
└──────────────────────────────────────────────────────────┘
           │
           ├─── Arc<PerfCounter>    (AtomicU64, lock-free)
           ├─── Arc<PerfHistogram>  (per-bucket AtomicU64)
           └─── PerfFormula         (lazy expression tree)
```

---

## Subsystems

### PerfCounter

A single 64-bit lock-free counter. Components hold an `Arc<PerfCounter>` obtained from the `StatsRegistry` during `elaborate()`. Incrementing the counter is a single `fetch_add(1, Ordering::Relaxed)`.

### PerfHistogram

A fixed-bucket histogram where each bucket is an `AtomicU64`. Bucket edges are set at construction and do not change. `record(val)` binary-searches the edge array to find the bucket, then increments it atomically.

### PerfFormula

A lazy expression tree that references counters by name. Evaluation is deferred to dump time via `eval(&registry) -> f64`. Formulas support: counter references, arithmetic (`+`, `-`, `*`, `/`), and literal constants. Division by zero yields `f64::NAN`.

### StatsRegistry

The global container. Owned by `System` (one registry per simulation). Components call `registry.perf_counter(path, desc)` during `elaborate()` to register and retrieve their `Arc<PerfCounter>`. The registry enforces path uniqueness. At simulation end, `dump_json()` or `print_table()` is called from Python.

---

## Namespace Convention

Paths mirror the component hierarchy defined in `System::register()`:

```
system.cpu0.icache.hits
system.cpu0.icache.misses
system.cpu0.icache.hit_rate        ← formula: hits / (hits + misses)
system.cpu0.dcache.hits
system.cpu0.cycles
system.cpu0.insns_retired
system.cpu0.cpi                    ← formula: cycles / insns_retired
system.dram.reads
system.dram.writes
system.dram.bandwidth_gbps         ← formula: (reads+writes)*8 / elapsed_ns
```

Dot segments must be lowercase identifiers matching the component name segment in the System tree.

---

## Integration with SimObject Lifecycle

```rust
impl SimObject for L1Cache {
    fn elaborate(&mut self, system: &mut System) {
        let reg = system.stats_registry_mut();
        self.hits   = reg.perf_counter("system.cpu0.icache.hits",   "L1I cache hits");
        self.misses = reg.perf_counter("system.cpu0.icache.misses", "L1I cache misses");
        // Formula is registered by the registry automatically or explicitly:
        reg.perf_formula(
            "system.cpu0.icache.hit_rate",
            "L1I cache hit rate",
            PerfFormula::div(
                PerfFormula::counter("system.cpu0.icache.hits"),
                PerfFormula::add(
                    PerfFormula::counter("system.cpu0.icache.hits"),
                    PerfFormula::counter("system.cpu0.icache.misses"),
                ),
            ),
        );
    }
}
```

During `run()`, on a cache hit:

```rust
fn handle_hit(&self) {
    self.hits.inc();  // fetch_add(1, Relaxed) — zero allocation, no lock
}
```

---

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Counter backing type | `AtomicU64` | Lock-free, correct across harts without synchronization |
| Counter ordering | `Relaxed` for `inc()`, `SeqCst` for `get()` at dump | `Relaxed` is sufficient for independent counters; snapshot consistency at dump uses `SeqCst` |
| Formula evaluation | Lazy (at dump time) | Avoids per-instruction arithmetic; derived metrics are cold-path |
| Formula representation | Expression tree (enum, heap-allocated) | Simple, correct, easy to compose |
| Histogram bucket search | Binary search on edge array | `O(log b)` where b is bucket count; acceptable on cold path |
| Dump formats | JSON + terminal table | JSON for tooling; table for interactive use |
| Registry ownership | `System` owns one `StatsRegistry` | Single source of truth; no global state |
| Path validation | At `perf_counter()` call time | Catches mis-named paths at elaboration, not at dump |

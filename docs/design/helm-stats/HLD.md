# helm-stats — High-Level Design

> **Crate:** `helm-stats`
> **Phase:** Phase 1
> **Dependencies:** `helm-core` (for `System` path conventions)
>
> **Status (apr 2026):** Counters, histograms, and a `StatsRegistry` with
> dot-path keys are implemented. `JitPerfStats` lives here as a plain
> struct shared with `framework/helm-jit` (`Arc<AtomicU64>` is *not* used
> on that path today). `PerfFormula` is specified below but **not yet
> implemented**. The hot-path call sites are *not* feature-gated -- a
> perf build still pays for every `fetch_add` and (in `JitPerfStats`)
> every `BTreeMap::entry().or_insert()`.
>
> This revision aligns the crate with the trait+feature model already
> used by `helm-probe`: the public types are *interfaces* whose backing
> storage and method bodies disappear when the `stats` Cargo feature is
> disabled. The hot path call sites collapse to ZSTs and inlined empty
> functions in a perf build.

---

## Overview

`helm-stats` provides a lock-free, hierarchically-namespaced statistics
system for helm-ng simulations. It is modeled after gem5's Stats system
(see `docs/research/gem5-stats-helm-adaptation.md`) but adapted for
Rust's concurrency model and helm's perf-vs-instrumented build duality:

- Counters use `AtomicU64` for multi-hart safety.
- Formulas are lazy expression trees evaluated at dump time.
- All statistics are accessible by a dot-path that mirrors the
  component hierarchy.
- Every public type is dual-implemented behind the `stats` Cargo
  feature: an `AtomicU64`/`Vec<AtomicU64>` implementation when the
  feature is on, a zero-sized no-op implementation when it is off.
  Hot-path call sites compile to nothing in the perf build.

### Goals

- **Zero allocation on the hot path.** Incrementing a counter must be a single `fetch_add` on an `AtomicU64`.
- **Multi-hart safe.** All counter operations are lock-free; no mutex is held during a simulation step.
- **Hierarchical namespace.** Path `system.cpu0.icache.hits` is unambiguous and maps to the component tree.
- **Lazy formula evaluation.** Derived metrics (hit rates, CPI, bandwidth) are computed at dump time, not during simulation.
- **Simple dump formats.** JSON (for tooling) and a human-readable table (for the terminal).
- **Feature-gated cost.** With `stats` disabled the hot path is the
  same code as if the call sites were `#[cfg]`-removed. Verified by an
  integration test asserting `size_of::<PerfCounter>() == 0` and a
  `cargo asm` check that `PerfCounter::inc()` lowers to `ret`.

### Non-Goals (Phase 0)

- Per-interval time-series dumps are deferred. Phase 0 produces final values only.
- No live histogram plotting or streaming output.

---

## Cargo Features

| Feature        | Default | Effect |
|----------------|---------|--------|
| `stats`        | on      | Enables `AtomicU64` storage in `PerfCounter`, `Vec<AtomicU64>` in `PerfHistogram`, the `BTreeMap` index in `StatsRegistry`, and the JSON/table dump methods. Without it, all of these become ZSTs and the methods become inlined empty bodies. |
| `formulas`     | on      | Enables `PerfFormula` (expression tree + lazy eval at dump time). Implies `stats`. Without it, `PerfFormula` is absent (compile error if referenced). |
| `gem5-compat`  | off     | Enables an extra dump path that emits gem5-shaped column-aligned text directly from the registry (parallel to `helm-report::HelmstatsFormatter`, used when `helm-report` is not in the build). Implies `stats`. |
| `serde`        | on      | Enables the `serde_json` JSON dump in `StatsRegistry`. Without it the crate has no `serde_json` link. |

The default helm-cli build uses `stats,formulas,serde`. A perf binary
uses `--no-default-features` (or `--features=core`) and contains no
stats storage at all.

---

## Interfaces (the dual-impl pattern)

Every public type is shaped as:

```rust
#[cfg(feature = "stats")]
pub struct PerfCounter { value: Arc<AtomicU64>, /* ... */ }

#[cfg(not(feature = "stats"))]
#[derive(Clone, Copy, Default)]
pub struct PerfCounter; // ZST

impl PerfCounter {
    #[inline(always)]
    pub fn inc(&self) {
        #[cfg(feature = "stats")]
        self.value.fetch_add(1, Ordering::Relaxed);
    }
    // get(), add(), reset() follow the same shape; without `stats`
    // get() returns 0 and reset() does nothing.
}
```

The same shape applies to `PerfHistogram`, `LabelCounter`,
`StatsRegistry`, and `PerfFormula`. Callers always use the same API; the
feature flag chooses whether the calls have any runtime effect.

`Arc<PerfCounter>` (the historical handle the LLD specifies) is also
supported -- when `stats` is off, `Arc<PerfCounter>` is `Arc<()>`-shaped
and can typically be reduced to no allocation by the caller (or held in
a `'static`-lifetime registry slot if cloning is unwanted).

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
           │     └─ ZST when `stats` feature is off
           ├─── Arc<PerfHistogram>  (per-bucket AtomicU64)
           │     └─ ZST when `stats` feature is off
           └─── PerfFormula         (lazy expression tree; `formulas` feature)
```

---

## Subsystems

### PerfCounter

A single 64-bit lock-free counter. Components hold an `Arc<PerfCounter>`
obtained from the `StatsRegistry` during `elaborate()`. With the `stats`
feature, incrementing the counter is a single `fetch_add(1, Relaxed)`.
Without the feature, `PerfCounter` is a unit struct, `inc()` is an
inlined empty function, and `get()` returns 0 -- no atomic instruction
is emitted at the call site.

### PerfHistogram

A fixed-bucket histogram where each bucket is an `AtomicU64`. Bucket
edges are set at construction and do not change. `record(val)`
binary-searches the edge array to find the bucket, then increments it
atomically. Without `stats`, `PerfHistogram` is a ZST, `record(_)` is a
no-op, and `counts()` returns an empty `Vec`.

### LabelCounter

gem5's `Vector` / `SparseHistogram` analogue for label-keyed sparse
counts (JIT reject reasons, unsupported opcodes, syscall numbers).
Backed by `DashMap<&'static str, AtomicU64>` when `stats` is on; ZST
otherwise. Keys are `&'static str` to avoid the per-event allocation
that `BTreeMap<String, u64>` performs in today's `JitPerfStats`. This
is the type the JIT runtime should hold instead of the current
`BTreeMap<String, u64>` fields.

### PerfFormula

A lazy expression tree that references counters by name. Evaluation is
deferred to dump time via `eval(&registry) -> f64`. Formulas support
counter references, arithmetic (`+`, `-`, `*`, `/`), and literal
constants. Division by zero yields `f64::NAN`. Behind the `formulas`
feature; not present in the perf build.

### StatsRegistry

The global container. Owned by `System` (one registry per simulation).
Components call `registry.counter(path, desc)` during `elaborate()` to
register and retrieve their `Arc<PerfCounter>`. The registry enforces
path uniqueness. At simulation end, `dump_json()` or `print_table()` is
called from Python.

Without `stats`, `StatsRegistry` is a unit struct: `counter()` returns
the ZST `PerfCounter` immediately (no map insert, no allocation), and
`dump_json()` returns `"{}"`. Components that do `registry.counter(...)`
during elaboration pay nothing.

### JitPerfStats

gem5 has no analogue -- this is a helm-specific aggregate of JIT
counters consumed by `framework/helm-jit/src/runtime.rs` and
`runtime/helm-engine/src/jit.rs`. Today it is a plain struct of `u64`
and `BTreeMap<String,u64>`, taken as `&mut JitPerfStats` on every
compile/exec event. The migration target is:

```rust
pub struct JitPerfStats {
    pub blocks_compiled:        PerfCounter,
    pub compiled_guest_insns:   PerfCounter,
    pub trace_cache_hits:       PerfCounter,
    pub trace_cache_misses:     PerfCounter,
    pub fallback_count:         PerfCounter,
    pub fallback_insns:         PerfCounter,
    pub unsupported_opcodes:    LabelCounter,
    pub reject_reasons:         LabelCounter,
    /* ... */
}
```

- All fields become feature-gated handles. `&mut JitPerfStats` is no
  longer required (counters are interior-mutable, internally `Arc`'d),
  which simplifies the `runtime.rs` borrow tangle.
- With `--no-default-features`, the struct itself becomes a ZST.
- `helm-jit` declares `helm-stats = { workspace = true, default-features = false, optional = true }`
  and a `stats` feature that simply re-exports `helm-stats/stats`.

### StatsProducer + StatsScope (QOM-style registration)

Helm's object model already gives every component a name and a place
in the System tree (Python `SimObject`, Rust `helm-core`). The stats
system uses that tree as the single source of truth for the dot-path
namespace -- gem5-style `system.cpu0.icache.hits` -- without any
component spelling its own absolute path.

- `StatsProducer` is a trait that any tree node may implement: CPUs,
  memory regions / address spaces, caches, MMUs/TLBs, buses
  (`helm-devices::Bus`), individual devices (UART, RTC, GIC, VirtIO,
  PCI), the JIT runtime, the engine, and the scheduler. The trait has
  one method, `register_stats(&self, scope: &mut StatsScope)`.
- `StatsScope` is a path-prefixed view onto the global `StatsRegistry`.
  Every `counter()`/`histogram()`/`label_counter()`/`formula()` call on
  the scope is silently prefixed with the owning object's canonical
  path. Nested objects get a child scope via `scope.child("icache")`.
- During elaboration the engine walks the tree, computes
  `system.<parent>.<...>.<this>`, and calls `register_stats` on every
  node. Renaming or reparenting an object moves all of its stats with
  it -- no string surgery at call sites.
- The trait and `StatsScope` follow the same dual-impl rule as
  `PerfCounter`: with `--no-default-features` they are no-op shells,
  every method is `#[inline(always)]` empty, and devices' counter
  fields are ZSTs.

The same elaboration walk produces `m5out/config.{ini,json}` (gem5's
"what was actually simulated" record) by visiting each node's
`AttrRegistry`. Stats and config are emitted from one pass; they
cannot drift.

See `docs/research/gem5-stats-helm-adaptation.md` § 3.7 for the full
rationale, the per-layer table of which objects publish what, and
the slice plan.

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
system.cpu0.jit.blocks_compiled
system.cpu0.jit.trace_cache_hits
system.cpu0.jit.reject_reasons.<reason>
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
| Feature gate `stats` | Compile-out hot-path storage and bodies | Mirrors `helm-probe::Probe<T>::has_listeners()` -- single Cargo flag flips the whole crate between "live counters" and "ZST no-op" |
| Label-keyed sparse counts (`LabelCounter`) | `DashMap<&'static str, AtomicU64>`, `&'static str` keys | Avoids the `String` alloc per event that today's `JitPerfStats::unsupported_opcodes` performs |
| `JitPerfStats` field types | `PerfCounter` / `LabelCounter`, not `u64` / `BTreeMap` | Hot-path becomes uniform; perf build elides everything; removes `&mut JitPerfStats` plumbing in `helm-jit/runtime.rs` |
| `&dyn StatsRegistry` for dump consumers | `helm-report` formatters take a registry trait, not a snapshot struct | Keeps the snapshot path (`HelmSpySnapshot`) for analysis models; raw counters dump via the registry directly, gem5-style |

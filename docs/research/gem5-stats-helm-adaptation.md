# gem5 Stats System -- Adaptation to helm-ng

> **Source:** https://www.gem5.org/documentation/learning_gem5/part1/gem5_stats/
> and https://www.gem5.org/documentation/general_docs/statistics/
>
> **Scope:** map gem5's stats output / Stats classes onto helm-ng's existing
> instrumentation stack (`helm-stats`, `helm-spy`, `helm-report`, `helm-probe`)
> and define a feature-gated, trait-based interface so that the perf-build
> hot path stays identical to a stats-disabled build (the call sites compile
> to nothing).

---

## 1. What gem5 Provides

After every gem5 run, three artifacts land in `m5out/`:

| Artifact | Purpose |
|----------|---------|
| `config.ini`  | every SimObject in the simulation, every parameter (set or default), in a flat INI-style dump. The "what was actually simulated" record. |
| `config.json` | same data as `config.ini`, JSON-encoded for tooling. |
| `stats.txt`   | one or more `---------- Begin Simulation Statistics ----------` blocks, each containing every registered statistic in the format `<dot.path> <value> # <description> (<unit>)`. |

`stats.txt` blocks are emitted at simulation end and at every explicit dump
(checkpoint, `m5dumpstats`, end-of-window). Each statistic carries a
hierarchical name rooted at `system.` and a one-line description with unit.

### gem5 stat types (`src/base/stats/`)

| Type | Purpose |
|------|---------|
| `Scalar`               | single counter (`++`, `+=`). |
| `Average`              | running average over cycles, only updated on change. |
| `Vector`               | fixed-size `Vec<Counter>`, indexed numerically; per-thread / per-port stats. |
| `Vector2d`             | 2-D matrix, named in both axes. |
| `AverageVector`        | vector of `Average`. |
| `Distribution`         | fixed-bucket linear histogram with `min, max, bkt`. |
| `VectorDistribution`   | vector of `Distribution`. |
| `Histogram`            | dynamic-extent histogram (buckets grow). |
| `SparseHistogram`      | `HashMap<u64, u64>` for natural-number samples (e.g. PC heatmaps). |
| `StandardDeviation`, `AverageDeviation` | running stddev. |
| `Formula`              | lazy expression tree over other stats; evaluated at dump time. |

Initialization is done in each SimObject's `regStats()` method via a fluent
builder: `name(...).desc(...).flags(pdf | nozero | nonan | total | cdf).precision(p).prereq(other_stat)`.
`Formula` references other stats by reference / name; constants are wrapped
in `constant(...)`.

### Hot-path discipline in gem5

gem5 stats are not lock-free. Counter increments are plain integer adds on a
SimObject-owned field; thread safety is provided by gem5's event-driven
single-event-queue execution model. Helm runs vCPUs across host threads
(JIT, FS-mode quantum), so a 1:1 port is unsafe -- helm uses `AtomicU64`.

---

## 2. Where helm-ng Already Stands

| gem5 concept                    | helm-ng analogue                         | Status |
|---------------------------------|------------------------------------------|--------|
| `Scalar`                        | `helm_stats::PerfCounter` (`Arc<AtomicU64>`) | Implemented |
| `Vector`                        | `helm_spy::primitives::IndexedCounter`, `PerVcpuCounter` | Implemented (in spy crate) |
| `Distribution` / `Histogram`    | `helm_stats::PerfHistogram`, `helm_spy::primitives::Histogram`, `IntervalHistogram` | Implemented |
| `SparseHistogram`               | `helm_spy::primitives::HeatMap` (`DashMap<u64,u64>`) | Implemented |
| `Formula`                       | `helm_stats::PerfFormula` -- specified in LLD, **not yet implemented** | Gap |
| `regStats()`                    | `StatsRegistry::counter()` / `histogram()` -- called wherever counters are constructed | Implemented; not lifecycle-anchored |
| `stats.txt`                     | `helm_report::format::HelmstatsFormatter` (gem5 column-aligned) | Implemented |
| `config.ini` / `config.json`    | none -- helm has no SimObject-tree dump file | **Gap** |
| Per-component dot-path namespace| `StatsRegistry` keys are dot-paths, but no validation, no prefix conventions | Partial |
| End-of-sim dump trigger         | `Report::deliver()` + `ReportSchedule` triggers | Implemented |
| Lazy formula eval               | spec'd, not built | Gap |

The helm-ng stats stack also has surface gem5 does not:

- `helm_probe::Probe<T>` -- typed probe points that already collapse to ZSTs
  without the `instrumentation` feature (the model we want to extend
  consistently to counters and registries).
- `JitPerfStats` -- a plain struct in `helm-stats` accessed as `&mut self.jit_stats`
  on every JIT block / trace event in `framework/helm-jit/src/runtime.rs`.
  This is a hot-path concern: it is *not* feature-gated, so a perf build
  still performs the integer adds and `BTreeMap::entry().or_insert()`
  for `unsupported_opcodes` and `reject_reasons`.
- Recent commits have been wiring more JIT counters through this path
  (`a23df49 feat(stats): integrate JIT reject reasons with helm-stats`),
  which makes the lack of a no-op build mode more painful.

---

## 3. Adaptation Decisions

### 3.1 The interface shape (gem5-style, helm-typed)

Adopt gem5's stat-type taxonomy as helm traits. Concrete types are picked
by the build configuration:

```text
trait Counter   { fn inc(&self); fn add(&self, n: u64); fn get(&self) -> u64; }
trait Histogram { fn record(&self, val: u64); fn counts(&self) -> Vec<u64>; }
trait Vector    { fn inc(&self, idx: usize); fn value(&self, idx: usize) -> u64; }
trait Formula   { fn eval(&self, reg: &dyn StatsRegistry) -> f64; }
trait Registry  { fn counter(&mut self, path: &str, desc: &str, unit: Unit) -> Arc<dyn Counter>; ... }
```

Each crate exports concrete *type aliases* (`PerfCounter`, `PerfHistogram`,
`StatsRegistry`) that resolve to either the real implementation or a
zero-sized no-op implementation depending on Cargo features.

### 3.2 Feature-gating model

**Goal:** with stats disabled, every `counter.inc()` call site compiles to
nothing, the registry holds no allocations, and there is no measurable cost
vs. removing the call entirely.

**Default posture: release builds carry no stats.** Stats are
observability/dev-loop tooling, not a runtime user-facing capability.
Cargo features are therefore **default-off** so a plain
`cargo build --release` produces a binary with zero stats storage and
zero stats-call overhead. Stats turn on for dev/profiling builds via
explicit feature selection or via aggregate "profile" features that
the workspace exposes (see § 3.2.1).

| Crate         | Feature      | What the feature unlocks |
|---------------|--------------|--------------------------|
| `helm-probe`  | `instrumentation` (existing) | `Probe<T>::subscribe` and listener vector. Without it: ZST. |
| `helm-probe`  | `probe-full` (existing)      | Richer event payloads. |
| `helm-stats`  | `stats` (NEW; **off by default**)             | Backing `AtomicU64` + `StatsRegistry` storage. Without it: counters are ZSTs, `inc()` is `#[inline(always)] {}`, registry is a unit struct. |
| `helm-stats`  | `formulas` (NEW; off by default; implies `stats`) | `PerfFormula` expression tree + lazy eval at dump time. |
| `helm-spy`    | `collection` (NEW; off by default)            | `Counter`/`Histogram`/`HeatMap` keep their `AtomicU64`/`DashMap` storage. Without it: ZSTs, `inc()` no-op. |
| `helm-spy`    | `instrumentation` (existing; off by default)  | enables Probe wiring; depends on `helm-probe/instrumentation` and (now) `collection`. |
| `helm-spy`    | `analysis-models` (NEW; off by default; implies `collection`) | `CacheModel`, `BranchPredictor`, `InsnMix`. |
| `helm-report` | `report` (NEW; off by default)                | Sink trait + impls + formatters. Without it: `Report::deliver` is `#[inline] fn _ {}` and the crate has no `serde_json`/`bytemuck` link. |
| `helm-report` | `helmstats` (NEW; off by default; implies `report`) | `HelmstatsFormatter` + `config.ini`/`stats.txt` writers. (Feature is named after helm; the gem5-shaped formatter file remains `src/format/helmstats.rs`.) |

### 3.2.1 Aggregate profile features

Individual feature toggles are clumsy for normal dev use. The workspace
exposes two aggregate features on `helm-cli` (the binary crate) that
turn on the right combination across the dependency graph:

| Aggregate feature | Forwarded to | Intended profile |
|-------------------|--------------|------------------|
| `dev-instrumentation` | `helm-stats/stats,formulas`, `helm-spy/collection,instrumentation,analysis-models`, `helm-probe/instrumentation`, `helm-report/report,helmstats` | `cargo build` (debug), `cargo test`, `cargo bench` baseline |
| `profiling`           | same as `dev-instrumentation` plus `helm-report/binary-trace` | `cargo build --release --features=profiling`; ship to perf-engineering builds |

Workflow:

- `cargo build --release` -- ships to users; no stats, no probes, no
  formatters. Identical to `#[cfg]`-ing every counter call out.
- `cargo build` (debug) -- enables `dev-instrumentation` via the dev
  profile's `[features]` selection (see Cargo's `[profile.dev]` /
  per-profile features pattern, or via `required-features` on the dev
  binary target). Stats are live for the inner-loop dev experience.
- `cargo build --release --features=profiling` -- the explicit
  perf-engineering build; release optimisations *plus* live stats and
  the gem5-style `m5out/` dump.
- `cargo test --workspace` -- enables `dev-instrumentation` so unit
  tests can assert on counter values.
- `cargo bench --workspace` -- runs both ways: with `profiling` to
  measure stats overhead, and without it to measure the elision
  guarantee (sizes 0, asm = single `ret`).

The `dev-instrumentation` and `profiling` aggregate features live on
`helm-cli` (and on `runtime/helm-engine` so the engine can be tested
stand-alone). Crates lower in the DAG declare only the leaf features.

### 3.2.2 Why default-off

- A `cargo build --release` consumer should never need to know stats
  exist. The storage, the dump path, and the formatter linkage must
  all be absent.
- Probe/Spy/Report subscribers in current code accept `&mut` borrows
  on hot paths (e.g. `JitPerfStats`). The default-off model forces
  every interface to be *callable* in both modes, which is the
  property we want anyway.
- It avoids the trap where "default features" silently re-enable
  stats in any downstream crate that depends on `helm-stats` for an
  unrelated reason. Workspace dependency declarations use
  `default-features = false` for `helm-stats`, `helm-spy`, and
  `helm-report`; opt-in is explicit at the binary crate.

### 3.3 Concrete pattern (Counter)

```rust
// framework/helm-stats/src/counter.rs

#[cfg(feature = "stats")]
#[derive(Clone)]
pub struct PerfCounter(Arc<AtomicU64>);

#[cfg(not(feature = "stats"))]
#[derive(Clone, Copy, Default)]
pub struct PerfCounter; // ZST

impl PerfCounter {
    #[inline(always)]
    pub fn inc(&self) {
        #[cfg(feature = "stats")]
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn add(&self, _n: u64) {
        #[cfg(feature = "stats")]
        self.0.fetch_add(_n, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn get(&self) -> u64 {
        #[cfg(feature = "stats")]
        { self.0.load(Ordering::Relaxed) }
        #[cfg(not(feature = "stats"))]
        { 0 }
    }
}
```

The same pattern applies to `PerfHistogram` (no `Vec<AtomicU64>` allocated
without the feature; `record(_)` is empty), `StatsRegistry` (unit struct;
`counter()` returns the ZST), and `PerfFormula` (the entire enum is gated
behind the `formulas` feature).

### 3.4 `JitPerfStats` -- the live hot-path problem

Today `framework/helm-jit/src/runtime.rs` calls something like:

```rust
stats.blocks_compiled = stats.blocks_compiled.saturating_add(1);
stats.unsupported_opcodes.entry(name.to_string()).and_modify(|c| *c += 1).or_insert(1);
```

from inside the trace/block compile path. Even with stats "off" today, the
adds still execute and `BTreeMap::entry` allocates on first sight of a
given opcode.

Adapted shape:

```rust
pub struct JitPerfStats {
    pub blocks_compiled:        PerfCounter,
    pub compiled_guest_insns:   PerfCounter,
    pub trace_cache_hits:       PerfCounter,
    pub trace_cache_misses:     PerfCounter,
    pub fallback_count:         PerfCounter,
    pub fallback_insns:         PerfCounter,
    pub unsupported_opcodes:    LabelCounter, // helm-stats feature: behind `stats`
    pub reject_reasons:         LabelCounter,
    // ...
}
```

`LabelCounter` is the gem5-`SparseHistogram`/`Vector`-with-string-labels
analogue: a `DashMap<&'static str, AtomicU64>` when `stats` is on, ZST
otherwise. The `&'static str` requirement avoids the allocation that
`BTreeMap<String, u64>` performs today; reject-reason and opcode names are
already string literals at the JIT call sites.

`stats` callers in JIT then become:

```rust
self.jit_stats.blocks_compiled.inc();
self.jit_stats.unsupported_opcodes.bump("ldp_unsupported_size");
```

With `stats` off the calls vanish; with it on the cost is identical to
today's path (a single `fetch_add(Relaxed)`) and crucially does *not*
require `&mut JitPerfStats`, freeing the JIT runtime from the borrow that
forces all the threading in `runtime.rs`.

### 3.5 `config.ini` / `config.json` for helm

gem5 dumps the SimObject tree with every parameter so users can verify
"what was simulated". Helm has the same need but no equivalent file.

`HelmSystem`/`SimObject` already carry a name and parameter set. Add an
`emit_config()` method on `helm_engine::HelmSim` that walks the SimObject
tree and writes:

- `m5out/config.ini` -- INI-style, gem5-compatible enough that the gem5
  visualisation scripts in `gem5-resources` work unchanged.
- `m5out/config.json` -- same data, JSON-encoded.

This belongs in `helm-engine` (it knows the system tree) but must be gated
behind the `report` feature so it is absent from a perf build.

### 3.6 Output schema -- gem5 stats.txt parity

Helm already has `HelmstatsFormatter`, but it is hand-rolled per metric out
of `HelmSpySnapshot`. The adapted design routes everything through the
registry:

```text
m5out/
  config.ini
  config.json
  stats.txt        # one block per Report::deliver() call
```

Rules to match gem5:

- Every block opens with `---------- Begin Simulation Statistics ----------`
  and closes with `---------- End Simulation Statistics ----------`.
- One stat per line: `<dot.path><pad to col 40><value><pad to col 60># <desc> (<unit>)`.
- Histograms expand to `<path>::<bucket_label>` lines plus aggregated
  `<path>::total`, `<path>::min_value`, `<path>::max_value`,
  `<path>::mean`, `<path>::stdev` per the gem5 convention.
- Vectors expand to `<path>::<subname>` (or `<path>::<idx>`) lines plus
  `<path>::total` if the `total` flag is set.
- Formulas evaluated lazily at dump time; appear after their inputs in
  output order (sorted dot-path keeps this natural).
- Standard pseudo-stats at the top of every block: `simSeconds`, `simTicks`,
  `simFreq`, `simInsts`, `simOps`, `hostSeconds`, `hostTickRate`,
  `hostMemory`, `hostInstRate`, `hostOpRate`. Helm fills these from
  `HelmEngine` + `helm-timing`.

---

## 3.7 QOM-style: every object owns its stats

gem5's hierarchical stat names work because every SimObject has a
canonical path in the system tree (`system.cpu0.icache`). QEMU
generalises this further with QOM: *every* object -- CPU, memory
region, device, bus -- is a typed object with a canonical path
(`/machine/peripheral/uart0`, `/machine/unattached/device[3]/bus`),
and arbitrary properties / introspection hang off that path.

Helm already has the seed of this: `runtime/helm-python/src/simobject.rs`
gives every Python-attached component a name and an `IndexMap` of
children, and `framework/helm-core/src/attr.rs` (`AttrRegistry`)
attaches typed attributes per object. What is missing is a uniform
Rust-side trait that lets *any* object -- CPU, RAM, MemorySpace, GIC,
UART, bus controller -- register stats without knowing about the
global `StatsRegistry`.

The adaptation:

```rust
/// Implemented by anything in the system tree that may publish stats.
/// CPU, memory, devices, buses, caches, MMUs, JIT runtime, ...
pub trait StatsProducer {
    /// Called once during elaboration with a scope already prefixed
    /// to this object's canonical dot-path.
    fn register_stats(&self, scope: &mut StatsScope<'_>);
}

/// A path-prefixed view onto the global StatsRegistry.
/// All `counter()` / `histogram()` / `formula()` calls are silently
/// prefixed with the owning object's canonical path.
pub struct StatsScope<'a> {
    registry: &'a mut StatsRegistry,
    prefix:   String,        // e.g. "system.cpu0.icache"
}

impl<'a> StatsScope<'a> {
    pub fn counter(&mut self, leaf: &str, desc: &str) -> PerfCounter { /* ... */ }
    pub fn histogram(&mut self, leaf: &str, desc: &str, edges: Vec<u64>) -> PerfHistogram;
    pub fn label_counter(&mut self, leaf: &str, desc: &str) -> LabelCounter;
    pub fn formula(&mut self, leaf: &str, desc: &str, expr: PerfFormula);
    pub fn child(&mut self, segment: &str) -> StatsScope<'_>;  // nested objects
}
```

During `System::instantiate()`, the elaboration walker visits every
object in the tree, computes its canonical path, and calls
`register_stats()` on it. The CPU registers
`commit.insns_retired` / `cycles` / `branch.taken` etc., the L1
ICache registers `hits`/`misses`, the GIC registers `interrupts.sgi`
/ `interrupts.ppi` / `interrupts.spi`, the UART registers
`tx_bytes`/`rx_bytes`, the JIT runtime registers its
`blocks_compiled`/`reject_reasons`. None of them ever see the global
`StatsRegistry` -- they only see their own scoped slice.

### 3.7.1 What the trait covers

Every helm object that has runtime activity:

| Layer       | Examples                                              | Stats it would publish |
|-------------|-------------------------------------------------------|------------------------|
| CPU         | `RiscvArchState`, `Aarch64ArchState`, JIT runtime     | `cycles`, `insns_retired`, `committed_ops`, `branch.{taken,not_taken,mispredict}`, `jit.blocks_compiled`, `jit.reject_reasons` |
| Memory      | `FlatMem`, `HelmAddressSpace`, `MemoryMap` regions    | `loads`, `stores`, `bytes_read`, `bytes_written`, per-region `accesses` |
| Cache       | `CacheModel`, future ICache/DCache/L2                 | `hits`, `misses`, `writebacks`, `prefetches`, `latency_hist` |
| MMU/TLB     | per-stage TLB, page-walker                            | `tlb_hits`, `tlb_misses`, `stage1_walks`, `stage2_walks`, `walk_latency_hist` |
| Bus         | AMBA, I2C, SPI controllers in `helm-devices`          | `transactions`, `arb_cycles`, `idle_cycles`, `error_count` |
| Device      | PL011, PL031, SP804, GICv2/v3, VirtIO, PCI ECAM       | per-device `read_count`, `write_count`, `irq_raised`, `irq_acked`, `bytes_in`, `bytes_out` |
| Interconnect| Future XBar / NoC                                     | `cycles_busy`, `port[*].requests`, `congestion_events` |
| Engine      | `HelmEngine`, `HelmSim`, scheduler, `EventQueue`      | `events_serviced`, `quanta_run`, `host_seconds`, `instructions_per_quantum_hist` |

The same `StatsProducer` trait covers all of them; the canonical-path
prefix differentiates the namespace. The output is exactly the gem5
`system.cpu0.icache.hits` shape, derived for free from the system
tree.

### 3.7.2 How it interacts with the feature gates

`StatsProducer` and `StatsScope` are declared in `helm-stats` and
follow the same dual-impl rule (§ 3.3):

```rust
#[cfg(feature = "stats")]
pub trait StatsProducer { fn register_stats(&self, scope: &mut StatsScope<'_>); }

#[cfg(not(feature = "stats"))]
pub trait StatsProducer { fn register_stats(&self, _: &mut StatsScope<'_>) {} }
```

`StatsScope` itself is a unit struct without `stats`, all of its
`counter()` / `histogram()` methods are `#[inline(always)]` empty
bodies returning ZSTs. Every object's `register_stats` lowers to a
trivial empty function in the release build -- the elaboration walk
still happens (it has to, for wiring), but the stats side of it
disappears.

Because every counter handle returned from `StatsScope` is a
`PerfCounter` (ZST in release builds) the *fields on each device
struct* are also ZSTs in release builds. A PL011 with twelve stats
fields adds zero bytes to its struct in the perf binary. Verified by
the same `feature_gate_off` size_of test pattern listed in § 5.

### 3.7.3 What this changes vs. the original LLD

The current `helm-stats/LLD-stats.md` shows components calling
`system.stats_registry_mut().counter("system.cpu0.icache.hits", ...)`
with the full path baked in at the call site. With QOM-style scopes,
the call becomes:

```rust
impl StatsProducer for L1Icache {
    fn register_stats(&self, scope: &mut StatsScope<'_>) {
        self.hits   = scope.counter("hits",   "L1I cache hits");
        self.misses = scope.counter("misses", "L1I cache misses");
        scope.formula("hit_rate", "L1I hit rate",
            PerfFormula::div(
                PerfFormula::counter_rel("hits"),
                PerfFormula::add(
                    PerfFormula::counter_rel("hits"),
                    PerfFormula::counter_rel("misses"),
                ),
            ),
        );
    }
}
```

- The cache code never spells `"system.cpu0.icache"`. The path is
  derived from the object tree -- if you reparent or rename the
  cache, all stats automatically follow.
- `PerfFormula::counter_rel(leaf)` resolves against the current
  scope, so formulas survive renames too. (Absolute
  `PerfFormula::counter("system.cpu0.icache.hits")` remains
  available for cross-tree references.)
- `register_stats()` runs once per object during elaboration. After
  that, the hot path is exactly as before: `self.hits.inc()`.

### 3.7.4 Slice ordering

This goes between Slice S4 (helm-report feature gate) and Slice S5
(PerfFormula + dump pipeline) in § 4:

- **Slice S4.5: StatsProducer trait + scope walker**
  - Add `StatsProducer` trait + `StatsScope` to `helm-stats`
    (dual-impl, same feature gate as the rest of the crate).
  - Implement the elaboration walker in `runtime/helm-engine`:
    visits the SimObject tree, builds the canonical path, calls
    `register_stats()` on every node that implements `StatsProducer`.
  - Make the existing helm-stats consumers (CPU, memory, JIT,
    devices) implement `StatsProducer`. Migrate their counter
    construction from manual paths to scope-relative leaves.
  - QOM bonus: emit `m5out/config.{ini,json}` from the same walker
    by collecting every object's `AttrRegistry` -- one pass produces
    both the gem5 config dump and the stats registration.

The walker pattern means `m5out/config.ini` (gem5's "what was
actually simulated" record) and the stats namespace are derived from
the same source of truth: the object tree. They cannot drift.

### 3.7.5 Open question

Should `StatsProducer::register_stats` take `&mut self` so devices
can stash counter handles in their own fields, or should counters
stay solely in the registry and be looked up by path on the cold
path? The former is what gem5 does and what JIT already needs (it
holds counters as fields for hot-path access). Default to `&mut self`
for symmetry; cold-path-only consumers can ignore the returned
handles and look them up later via `registry.get_counter(path)`.

## 4. Adaptation Plan -- Slice Order

1. **Slice S1: feature gates and ZST no-op skeleton**
   - Add `stats` feature to `framework/helm-stats/Cargo.toml`; default-off.
   - Convert `PerfCounter`, `PerfHistogram`, `StatsRegistry` to the
     `cfg(feature = "stats")` two-impl pattern from Section 3.3.
   - Add `LabelCounter` (DashMap-backed under-feature, ZST without).
   - Verify `cargo build -p helm-stats --no-default-features` produces a
     crate whose public types compile to ZSTs (size_of_val == 0).

2. **Slice S2: helm-jit migration**
   - Replace fields in `JitPerfStats` with `PerfCounter`/`LabelCounter`.
   - Drop `&mut JitPerfStats` arguments throughout
     `framework/helm-jit/src/runtime.rs`, `trace/compiler.rs`,
     `trace/exit.rs`, and `runtime/helm-engine/src/jit.rs`.
     Counters are `Clone` (Arc internally), no exclusive borrow needed.
   - Confirm `cargo bench` / sim harness shows no regression with
     `--features=stats` and the call sites disappear without it.

3. **Slice S3: helm-spy primitives feature-gated**
   - Add `collection` feature to `debug/helm-spy/Cargo.toml`.
     Make `instrumentation` imply `collection`.
   - Apply ZST-when-off pattern to `Counter`, `PerVcpuCounter`,
     `IndexedCounter`, `Histogram`, `IntervalHistogram`, `HeatMap`,
     `RingBuffer`, `EventStream`, `TraceRing`, `CorrelHist2D`.
   - Add `analysis-models` feature; gate `analysis/cache.rs`,
     `branch_pred.rs`, `insn_mix.rs`, `power.rs`, `simpoint.rs`.

4. **Slice S4: helm-report feature-gated**
   - Add `report` feature; with it off, `Report::deliver()`,
     `Sink`, formatters become `pub` re-exports of empty no-op types
     (or the items vanish behind `cfg`, with empty stub modules).
   - Add `helmstats` sub-feature; under it, expose `emit_config_ini()`
     and `emit_stats_txt()` writers that consume `&dyn StatsRegistry`.

5. **Slice S5: PerfFormula and dump pipeline**
   - Implement `PerfFormula` enum + `eval(&StatsRegistry) -> f64` in
     `helm-stats` behind `formulas` feature.
   - Wire `helm-report::HelmstatsFormatter` to consume `&dyn StatsRegistry`
     directly rather than `HelmSpySnapshot` (snapshot becomes one of many
     inputs).
   - Add `m5out/{config.ini,config.json,stats.txt}` emission to
     `HelmSim::dump_stats()`.

6. **Slice S6: Python surface**
   - Expose `sim.dump_stats(path="m5out")` via `helm-python`.
   - Surface `PerfCounter`/`PerfHistogram` as opaque Python handles for
     test assertions; lazy `formula(name)` lookup returns f64.

Each slice is a self-contained PR with its own tests; slices S1-S3 must
land before any benchmark-visible perf claim about the stats system.

---

## 5. Cost / Performance Targets

| Build                                 | `counter.inc()` cost | Registry footprint | Notes |
|---------------------------------------|----------------------|--------------------|-------|
| default (`stats,collection,report,instrumentation`) | 1x `fetch_add(Relaxed)` (~1 ns x86) | grows with registered metrics | matches today |
| `--no-default-features` (perf)        | 0 instructions (call elided)         | 0 bytes            | identical to deleting the call site |
| `--features stats` only               | 1x `fetch_add(Relaxed)`              | counters allocated, no probes/sinks | useful for perf-counter-only collection |
| `--features instrumentation` only     | 0 (counters are ZST), probes ZST too | 0                  | enables compile-time wiring scaffolding without runtime cost |

The "perf elided" guarantee is verified by:

1. `cargo build -p helm-stats --no-default-features` then
   `cargo asm --no-default-features helm_stats::PerfCounter::inc` -- expect
   `ret` only.
2. A `tests/feature_gate.rs` integration test that asserts
   `std::mem::size_of::<PerfCounter>() == 0` and
   `std::mem::size_of::<StatsRegistry>() == 0` without the feature.

---

## 6. Open Questions

1. **Per-vCPU sharding for `PerfCounter`?** gem5 sidesteps this with its
   single event queue. Helm runs JIT compile/exec on potentially different
   threads from the FS-mode vCPU. A single `AtomicU64` is correct but
   contended on hot counters. Sharding to `[AtomicU64; N_CPUS]` with a
   `total()` aggregator is a future-Slice optimisation; not part of S1-S6.
2. **Hierarchical reset semantics.** gem5 supports per-SimObject
   `regStats()/resetStats()`. The current `StatsRegistry::reset_all()` is
   coarse. Adding `reset_prefix("system.cpu0.")` is cheap on a `BTreeMap`
   and should land in S5.
3. **Live streaming.** gem5 supports an HDF5 binary stats output; helm has
   `BinaryTraceSink` already. Whether to wire stats-as-streaming-tuples is
   deferred until S5 ships.

---

## 7. References

- gem5 stats output tutorial: https://www.gem5.org/documentation/learning_gem5/part1/gem5_stats/
- gem5 stats package overview: https://www.gem5.org/documentation/general_docs/statistics/
- gem5 stats API reference: https://www.gem5.org/documentation/general_docs/statistics/api
- helm: `docs/design/helm-stats/HLD.md`, `docs/design/helm-stats/LLD-stats.md`
- helm: `docs/design/helm-spy/HLD.md`, `docs/design/helm-spy/LLD-primitives.md`
- helm: `docs/design/helm-report/HLD.md`
- helm: `framework/helm-stats/src/lib.rs`
- helm: `framework/helm-probe/src/probe.rs` (the ZST-when-off pattern we mirror)
- helm: `debug/helm-report/src/format/helmstats.rs`

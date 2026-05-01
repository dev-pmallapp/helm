# helm-stats — LLD: Statistics Implementation

> **Module:** `helm-stats`
> **Types:** `PerfCounter`, `PerfHistogram`, `PerfFormula`, `StatsRegistry`
>
> **Feature surface:** `stats` (default, gates `AtomicU64` storage),
> `formulas` (default, gates `PerfFormula`), `gem5-compat` (off, gates
> the in-crate gem5-style emitter), `serde` (default, gates JSON dump).
> See `docs/design/helm-stats/HLD.md` for the rationale.

---

## Table of Contents

1. [Feature gates and the dual-impl pattern](#0-feature-gates-and-the-dual-impl-pattern)
2. [PerfCounter](#1-perfcounter)
3. [PerfHistogram](#2-perfhistogram)
4. [LabelCounter](#2b-labelcounter)
5. [PerfFormula](#3-perfformula)
6. [StatsRegistry](#4-statsregistry)
7. [Dot-Path Resolution](#5-dot-path-resolution)
8. [Dump Formats](#6-dump-formats)
9. [Module Structure](#7-module-structure)
10. [JitPerfStats migration](#8-jitperfstats-migration)

---

## 0. Feature gates and the dual-impl pattern

Every public type in `helm-stats` has two implementations:

- **Live impl** (compiled when `--features=stats` is on, the default):
  carries the real `AtomicU64` / `Vec<AtomicU64>` / `BTreeMap` storage,
  and the methods perform the documented work.
- **No-op impl** (compiled when `--no-default-features` and `stats` is
  off): the type is a unit struct (ZST), and every method is
  `#[inline(always)]` with an empty body or a constant return value.

The pattern in source:

```rust
// counter.rs
#[cfg(feature = "stats")]
pub use live::PerfCounter;
#[cfg(not(feature = "stats"))]
pub use noop::PerfCounter;

#[cfg(feature = "stats")]
mod live {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Clone)]
    pub struct PerfCounter {
        pub(crate) name: Arc<str>,
        pub(crate) desc: Arc<str>,
        value: Arc<AtomicU64>,
    }

    impl PerfCounter {
        pub(crate) fn new(name: Arc<str>, desc: Arc<str>) -> Self {
            Self { name, desc, value: Arc::new(AtomicU64::new(0)) }
        }
        #[inline(always)] pub fn inc(&self)        { self.value.fetch_add(1, Ordering::Relaxed); }
        #[inline(always)] pub fn add(&self, n: u64){ self.value.fetch_add(n, Ordering::Relaxed); }
        #[inline]         pub fn get(&self) -> u64 { self.value.load(Ordering::SeqCst) }
        pub fn reset(&self)                        { self.value.store(0, Ordering::SeqCst); }
        pub fn name(&self) -> &str { &self.name }
        pub fn desc(&self) -> &str { &self.desc }
    }
}

#[cfg(not(feature = "stats"))]
mod noop {
    #[derive(Clone, Copy, Default)]
    pub struct PerfCounter;
    impl PerfCounter {
        #[inline(always)] pub fn inc(&self)         {}
        #[inline(always)] pub fn add(&self, _n: u64) {}
        #[inline(always)] pub fn get(&self) -> u64  { 0 }
        #[inline(always)] pub fn reset(&self)        {}
        #[inline(always)] pub fn name(&self) -> &str { "" }
        #[inline(always)] pub fn desc(&self) -> &str { "" }
    }
}
```

The same pattern applies to `PerfHistogram`, `LabelCounter`,
`PerfFormula`, and `StatsRegistry`.

### Verification of "no cost when off"

`tests/feature_gate_off.rs` (gated `#![cfg(not(feature = "stats"))]`):

```rust
use helm_stats::{PerfCounter, PerfHistogram, StatsRegistry};

#[test]
fn types_are_zst_when_disabled() {
    assert_eq!(std::mem::size_of::<PerfCounter>(),  0);
    assert_eq!(std::mem::size_of::<PerfHistogram>(), 0);
    assert_eq!(std::mem::size_of::<StatsRegistry>(), 0);
}

#[test]
fn ops_compile_to_nothing() {
    let c = PerfCounter::default();
    for _ in 0..1_000_000 { c.inc(); }
    assert_eq!(c.get(), 0);
}
```

`cargo build -p helm-stats --no-default-features` plus
`cargo asm --no-default-features helm_stats::PerfCounter::inc` is
expected to produce a single `ret` instruction (verified manually in CI
via the `tools/check_zero_cost.sh` helper).

---

## 1. PerfCounter

### Definition (live impl)

```rust
use std::sync::atomic::{AtomicU64, Ordering};

/// A single lock-free 64-bit performance counter.
///
/// Designed to be held as `Arc<PerfCounter>` by SimObject components.
/// All operations are safe to call concurrently from multiple hart threads.
///
/// When the `stats` feature is **off**, this is a unit ZST and all methods
/// are inlined empty bodies. See § 0.
pub struct PerfCounter {
    /// Human-readable dot-path name (e.g. "system.cpu0.icache.hits").
    pub name: String,
    /// Human-readable description for dump output.
    pub desc: String,
    /// The underlying atomic counter value.
    value: AtomicU64,
}

impl PerfCounter {
    /// Create a new counter with initial value 0.
    pub fn new(name: impl Into<String>, desc: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            desc: desc.into(),
            value: AtomicU64::new(0),
        }
    }

    /// Increment the counter by 1. Lock-free. Safe to call on the hot path.
    ///
    /// Uses `Relaxed` ordering — sufficient for independent event counting
    /// where no ordering relationship with other atomics is required.
    #[inline(always)]
    pub fn inc(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the counter by `n`. Lock-free.
    #[inline(always)]
    pub fn inc_by(&self, n: u64) {
        self.value.fetch_add(n, Ordering::Relaxed);
    }

    /// Read the current counter value.
    ///
    /// Uses `SeqCst` ordering at dump time to ensure a coherent snapshot
    /// across all counters on all harts. On the hot path, prefer not calling
    /// `get()` — use `inc()` only.
    pub fn get(&self) -> u64 {
        self.value.load(Ordering::SeqCst)
    }

    /// Reset the counter to 0. Should only be called when simulation is paused.
    pub fn reset(&self) {
        self.value.store(0, Ordering::SeqCst);
    }
}
```

### Usage in a Component

```rust
pub struct L1Cache {
    name: String,
    hits:   Arc<PerfCounter>,
    misses: Arc<PerfCounter>,
}

impl SimObject for L1Cache {
    fn elaborate(&mut self, system: &mut System) {
        let reg = system.stats_registry_mut();
        self.hits   = reg.perf_counter("system.cpu0.icache.hits",   "L1 instruction cache hits");
        self.misses = reg.perf_counter("system.cpu0.icache.misses", "L1 instruction cache misses");
    }
}

impl L1Cache {
    fn lookup(&self, addr: u64) -> Option<CacheLine> {
        if let Some(line) = self.lines.get(addr) {
            self.hits.inc();    // single fetch_add — no allocation, no lock
            Some(line)
        } else {
            self.misses.inc();
            None
        }
    }
}
```

---

## 2. PerfHistogram

### Definition (live impl)

```rust
/// A fixed-bucket histogram where each bucket is a lock-free `AtomicU64`.
///
/// Bucket `i` counts values `v` where `edges[i-1] <= v < edges[i]`.
/// When the `stats` feature is off, this is a unit ZST; `record(_)` is a
/// no-op and `counts()` returns an empty `Vec`.
/// An implicit underflow bucket (below `edges[0]`) and overflow bucket
/// (above `edges[last]`) are included.
pub struct PerfHistogram {
    pub name: String,
    pub desc: String,
    /// Monotonically increasing bucket boundary values.
    /// Length N → N+1 buckets (N-1 inner + 1 underflow + 1 overflow).
    edges: Vec<u64>,
    /// Per-bucket atomic counters. Length = edges.len() + 1.
    buckets: Vec<AtomicU64>,
}

impl PerfHistogram {
    /// Construct a histogram from an ordered list of bucket edge values.
    /// `edges` must be strictly increasing and non-empty.
    ///
    /// Example: `edges = [10, 100, 1000]` creates buckets:
    ///   [0, 10)   [10, 100)   [100, 1000)   [1000, ∞)
    pub fn new(name: impl Into<String>, desc: impl Into<String>, edges: Vec<u64>) -> Self {
        assert!(!edges.is_empty(), "histogram must have at least one edge");
        assert!(
            edges.windows(2).all(|w| w[0] < w[1]),
            "histogram edges must be strictly increasing"
        );
        let bucket_count = edges.len() + 1;
        Self {
            name: name.into(),
            desc: desc.into(),
            edges,
            buckets: (0..bucket_count).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    /// Record a single observation `val`.
    /// Performs a binary search on `edges` then a single `fetch_add`. Lock-free.
    #[inline]
    pub fn record(&self, val: u64) {
        let bucket = self.edges.partition_point(|&edge| val >= edge);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
    }

    /// Return all bucket counts as a `Vec<u64>`.
    /// Ordered from underflow (index 0) to overflow (index last).
    pub fn counts(&self) -> Vec<u64> {
        self.buckets.iter().map(|b| b.load(Ordering::SeqCst)).collect()
    }

    /// Return the bucket edges (does not include the implicit underflow/overflow boundaries).
    pub fn edges(&self) -> &[u64] {
        &self.edges
    }

    /// Reset all buckets to 0.
    pub fn reset(&self) {
        for b in &self.buckets {
            b.store(0, Ordering::SeqCst);
        }
    }

    /// Compute the approximate mean from bucket midpoints.
    pub fn approx_mean(&self) -> f64 {
        let counts = self.counts();
        let total: u64 = counts.iter().sum();
        if total == 0 { return 0.0; }

        let mut weighted_sum = 0.0f64;
        // Underflow bucket: midpoint = edges[0] / 2
        weighted_sum += counts[0] as f64 * (self.edges[0] as f64 / 2.0);
        // Inner buckets
        for i in 0..self.edges.len().saturating_sub(1) {
            let mid = (self.edges[i] + self.edges[i + 1]) as f64 / 2.0;
            weighted_sum += counts[i + 1] as f64 * mid;
        }
        // Overflow bucket: midpoint = edges[last] * 1.5 (heuristic)
        let last_edge = *self.edges.last().unwrap() as f64;
        weighted_sum += counts[self.edges.len()] as f64 * (last_edge * 1.5);
        weighted_sum / total as f64
    }
}
```

---

## 3. PerfFormula

## 2b. LabelCounter

gem5 names this `Vector` (when keys are statically known at registration) or
`SparseHistogram` (when keys are sampled at runtime). In helm we collapse the
two for the JIT/syscall use case where keys are short `&'static str` literals
(reject reasons, opcode mnemonics, syscall names). This is the type that
replaces `JitPerfStats::unsupported_opcodes: BTreeMap<String,u64>` and
`JitPerfStats::reject_reasons: BTreeMap<String,u64>` in the migration plan.

### Definition (live impl, behind `feature = "stats"`)

```rust
use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct LabelCounter {
    pub(crate) name: Arc<str>,
    pub(crate) desc: Arc<str>,
    /// Static-string keys -- avoids the per-event allocation that a
    /// `BTreeMap<String, u64>` performs on each new key.
    slots: DashMap<&'static str, AtomicU64>,
}

impl LabelCounter {
    pub fn new(name: impl Into<Arc<str>>, desc: impl Into<Arc<str>>) -> Self {
        Self { name: name.into(), desc: desc.into(), slots: DashMap::new() }
    }

    /// Hot-path. Idempotent insert + atomic add.
    /// Cost: one DashMap shard lock + one `fetch_add(Relaxed)`.
    #[inline]
    pub fn bump(&self, key: &'static str) {
        self.slots
            .entry(key)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Cold-path. Returns `(label, count)` pairs sorted by descending count.
    pub fn snapshot(&self) -> Vec<(&'static str, u64)> {
        let mut out: Vec<_> = self.slots
            .iter()
            .map(|kv| (*kv.key(), kv.value().load(Ordering::SeqCst)))
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1));
        out
    }

    pub fn total(&self) -> u64 {
        self.slots.iter().map(|kv| kv.value().load(Ordering::Relaxed)).sum()
    }

    pub fn name(&self) -> &str { &self.name }
    pub fn desc(&self) -> &str { &self.desc }
}
```

### Definition (no-op impl, when `stats` is off)

```rust
#[derive(Clone, Copy, Default)]
pub struct LabelCounter;

impl LabelCounter {
    #[inline(always)] pub fn bump(&self, _key: &'static str) {}
    #[inline(always)] pub fn total(&self) -> u64 { 0 }
    #[inline(always)] pub fn snapshot(&self) -> Vec<(&'static str, u64)> { Vec::new() }
    #[inline(always)] pub fn name(&self) -> &str { "" }
    #[inline(always)] pub fn desc(&self) -> &str { "" }
}
```

### Usage in `JitPerfStats`

```rust
pub struct JitPerfStats {
    pub blocks_compiled:      PerfCounter,
    pub trace_cache_hits:     PerfCounter,
    pub trace_cache_misses:   PerfCounter,
    pub fallback_count:       PerfCounter,
    pub fallback_insns:       PerfCounter,
    pub unsupported_opcodes:  LabelCounter,
    pub reject_reasons:       LabelCounter,
    /* ... */
}
```

Hot-path call sites in `framework/helm-jit/src/runtime.rs` become:

```rust
stats.blocks_compiled.inc();
stats.unsupported_opcodes.bump("ldp_unsupported_size");
stats.reject_reasons.bump(JitRejectReason::TraceTooShort.label());
```

- No `&mut JitPerfStats` required: counters are interior-mutable.
- No `String::from` per event: keys are `&'static str`.
- With `stats` off: every line above is a typed no-op.

---

## 3. PerfFormula

> Behind `feature = "formulas"` (which implies `stats`). Without it,
> `PerfFormula` is not exported -- referencing it is a compile error.

### Definition

```rust
/// A lazy expression tree evaluated at dump time.
///
/// Formulas reference counters by dot-path name. The actual counter value
/// is not read until `eval()` is called. This means formulas are always
/// up-to-date with the final counter values at dump time.
///
/// # Examples
///
/// ```
/// // hit_rate = hits / (hits + misses)
/// let hit_rate = PerfFormula::div(
///     PerfFormula::counter("system.cpu0.icache.hits"),
///     PerfFormula::add(
///         PerfFormula::counter("system.cpu0.icache.hits"),
///         PerfFormula::counter("system.cpu0.icache.misses"),
///     ),
/// );
/// let value = hit_rate.eval(&registry);
/// ```
#[derive(Debug, Clone)]
pub enum PerfFormula {
    /// Reference a counter by dot-path name.
    Counter(String),
    /// A literal constant.
    Const(f64),
    /// Binary arithmetic operations.
    Add(Box<PerfFormula>, Box<PerfFormula>),
    Sub(Box<PerfFormula>, Box<PerfFormula>),
    Mul(Box<PerfFormula>, Box<PerfFormula>),
    Div(Box<PerfFormula>, Box<PerfFormula>),
}

impl PerfFormula {
    pub fn counter(path: impl Into<String>) -> Self {
        PerfFormula::Counter(path.into())
    }

    pub fn constant(val: f64) -> Self {
        PerfFormula::Const(val)
    }

    pub fn add(a: PerfFormula, b: PerfFormula) -> Self {
        PerfFormula::Add(Box::new(a), Box::new(b))
    }

    pub fn sub(a: PerfFormula, b: PerfFormula) -> Self {
        PerfFormula::Sub(Box::new(a), Box::new(b))
    }

    pub fn mul(a: PerfFormula, b: PerfFormula) -> Self {
        PerfFormula::Mul(Box::new(a), Box::new(b))
    }

    pub fn div(a: PerfFormula, b: PerfFormula) -> Self {
        PerfFormula::Div(Box::new(a), Box::new(b))
    }

    /// Evaluate the formula against the given registry.
    ///
    /// Returns `f64::NAN` if any referenced counter is not found
    /// or if a division by zero occurs.
    pub fn eval(&self, registry: &StatsRegistry) -> f64 {
        match self {
            PerfFormula::Counter(path) => {
                registry.get_counter(path)
                    .map(|c| c.get() as f64)
                    .unwrap_or(f64::NAN)
            }
            PerfFormula::Const(v) => *v,
            PerfFormula::Add(a, b) => a.eval(registry) + b.eval(registry),
            PerfFormula::Sub(a, b) => a.eval(registry) - b.eval(registry),
            PerfFormula::Mul(a, b) => a.eval(registry) * b.eval(registry),
            PerfFormula::Div(a, b) => {
                let divisor = b.eval(registry);
                if divisor == 0.0 { f64::NAN } else { a.eval(registry) / divisor }
            }
        }
    }
}
```

### Compound Formula Example

```rust
// CPI = cycles / instructions_retired
let cpi = PerfFormula::div(
    PerfFormula::counter("system.cpu0.cycles"),
    PerfFormula::counter("system.cpu0.insns_retired"),
);

// IPC = 1 / CPI
let ipc = PerfFormula::div(
    PerfFormula::constant(1.0),
    cpi.clone(),
);

// L1 miss rate = misses / (hits + misses)
let miss_rate = PerfFormula::div(
    PerfFormula::counter("system.cpu0.icache.misses"),
    PerfFormula::add(
        PerfFormula::counter("system.cpu0.icache.hits"),
        PerfFormula::counter("system.cpu0.icache.misses"),
    ),
);
```

---

## 4. StatsRegistry

### Definition

```rust
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

/// The central statistics registry. Owned by `System`.
///
/// All counters and histograms are registered here during `elaborate()`.
/// Formulas are registered here for inclusion in dump output.
pub struct StatsRegistry {
    /// Sorted by path for deterministic dump output.
    counters:    BTreeMap<String, Arc<PerfCounter>>,
    histograms:  BTreeMap<String, Arc<PerfHistogram>>,
    formulas:    BTreeMap<String, (String, PerfFormula)>,  // path → (desc, formula)
}

impl StatsRegistry {
    pub fn new() -> Self {
        Self {
            counters:   BTreeMap::new(),
            histograms: BTreeMap::new(),
            formulas:   BTreeMap::new(),
        }
    }

    /// Register a counter at `path` with `desc` and return a shared handle.
    ///
    /// If a counter is already registered at `path`, returns the existing handle.
    /// If `path` is occupied by a histogram or formula, panics.
    ///
    /// Called during `elaborate()` only. Not valid after `startup()`.
    pub fn perf_counter(&mut self, path: &str, desc: &str) -> Arc<PerfCounter> {
        if let Some(existing) = self.counters.get(path) {
            return Arc::clone(existing);
        }
        assert!(
            !self.histograms.contains_key(path) && !self.formulas.contains_key(path),
            "path '{path}' already registered as a different stat type"
        );
        let counter = Arc::new(PerfCounter::new(path, desc));
        self.counters.insert(path.to_string(), Arc::clone(&counter));
        counter
    }

    /// Register a histogram.
    pub fn perf_histogram(
        &mut self,
        path: &str,
        desc: &str,
        edges: Vec<u64>,
    ) -> Arc<PerfHistogram> {
        assert!(
            !self.counters.contains_key(path) && !self.formulas.contains_key(path),
            "path '{path}' already registered as a different stat type"
        );
        let hist = Arc::new(PerfHistogram::new(path, desc, edges));
        self.histograms.insert(path.to_string(), Arc::clone(&hist));
        hist
    }

    /// Register a formula. Formulas are evaluated lazily at dump time.
    pub fn perf_formula(&mut self, path: &str, desc: &str, formula: PerfFormula) {
        assert!(
            !self.counters.contains_key(path) && !self.histograms.contains_key(path),
            "path '{path}' already registered as a different stat type"
        );
        self.formulas.insert(path.to_string(), (desc.to_string(), formula));
    }

    /// Look up a counter by path. Returns `None` if not found.
    pub fn get_counter(&self, path: &str) -> Option<&Arc<PerfCounter>> {
        self.counters.get(path)
    }

    /// Look up a histogram by path. Returns `None` if not found.
    pub fn get_histogram(&self, path: &str) -> Option<&Arc<PerfHistogram>> {
        self.histograms.get(path)
    }

    /// Reset all counters and histograms to zero. Formulas are stateless.
    pub fn reset_all(&self) {
        for c in self.counters.values() { c.reset(); }
        for h in self.histograms.values() { h.reset(); }
    }

    /// Dump all stats to a JSON file.
    pub fn dump_json(&self, path: &Path) -> io::Result<()>;

    /// Print a human-readable table to stdout.
    pub fn print_table(&self);
}
```

---

## 4b. StatsProducer + StatsScope (QOM-style)

`StatsRegistry` is the global container. `StatsScope` is a path-prefixed
*view* onto it that any tree node can hand around. `StatsProducer` is
the trait every helm object implements when it has stats to publish:
CPU, memory regions / `HelmAddressSpace`, caches, MMUs/TLBs,
`helm-devices::Bus` controllers, individual devices (PL011, PL031,
SP804, GICv2/v3, PCI ECAM, VirtIO), the JIT runtime, the engine,
and the scheduler.

> **Status (Slice S4.5, landed):** the trait + scope dual-impl and a
> standalone walker function (`helm_engine::stats_walker::walk_and_register`)
> shipped in this slice. SimObject-tree integration through
> `HelmSim::instantiate()` (the `walk(node, scope)` recursion in
> § 4b.4) is deferred until the engine exposes a child-node iterator
> on the session tree. Until then, callers pass a flat
> `(path, &producer)` list to the standalone walker; canonical paths
> are explicit at the call site rather than derived from the tree.

### 4b.0 Implemented surface (S4.5)

The shipped `StatsProducer` trait takes `&self` rather than `&mut self`.
Counter handles are `Clone` (cheap Arc bumps when `stats` is on, ZST
copies otherwise), so producers either own the handles directly or
stash them through interior mutability (`Cell<Option<PerfCounter>>`,
`OnceCell`, etc.). This keeps the walker free of borrow-checker
gymnastics when the same producer participates in multiple sub-trees
via shared `Arc`/`Rc` ownership.

The dot-path concatenation rule is:

- non-empty prefix + leaf `L` produces `prefix.L`
- empty prefix (root scope) + leaf `L` produces `L` (no leading `.`)
- `scope.child(seg)` follows the same rule against the current prefix.

Tiny usage example, end to end:

```rust
use helm_stats::{StatsProducer, StatsRegistry, StatsScope};
use helm_engine::stats_walker::walk_and_register;

struct L1Icache;
impl StatsProducer for L1Icache {
    fn register_stats(&self, scope: &mut StatsScope<'_>) {
        let hits   = scope.counter("hits",   "L1I cache hits");
        let misses = scope.counter("misses", "L1I cache misses");
        // Stash via interior mutability (Cell/OnceCell) for hot-path access.
        let _ = (hits, misses);
    }
}

let mut registry = StatsRegistry::new();
let icache = L1Icache;
walk_and_register([("system.cpu0.icache", &icache)], &mut registry);
// The registry now contains "system.cpu0.icache.hits" and ".misses".
```

### 4b.1 Trait

```rust
/// Implemented by anything in the helm system tree that may publish stats.
/// Called once during elaboration with a scope already prefixed to this
/// object's canonical dot-path.
#[cfg(feature = "stats")]
pub trait StatsProducer {
    fn register_stats(&mut self, scope: &mut StatsScope<'_>);
}

#[cfg(not(feature = "stats"))]
pub trait StatsProducer {
    #[inline(always)]
    fn register_stats(&mut self, _scope: &mut StatsScope<'_>) {}
}
```

`&mut self` because devices typically stash counter handles in their
own fields for hot-path access (the JIT pattern).

### 4b.2 StatsScope (live impl, behind `feature = "stats"`)

```rust
pub struct StatsScope<'a> {
    registry: &'a mut StatsRegistry,
    prefix:   String,   // canonical dot-path, e.g. "system.cpu0.icache"
}

impl<'a> StatsScope<'a> {
    pub fn counter(&mut self, leaf: &str, desc: &str) -> PerfCounter {
        let path = format!("{}.{}", self.prefix, leaf);
        self.registry.counter(&path, desc)
    }

    pub fn histogram(&mut self, leaf: &str, desc: &str, edges: Vec<u64>) -> PerfHistogram {
        let path = format!("{}.{}", self.prefix, leaf);
        self.registry.histogram(&path, desc, edges)
    }

    pub fn label_counter(&mut self, leaf: &str, desc: &str) -> LabelCounter {
        let path = format!("{}.{}", self.prefix, leaf);
        self.registry.label_counter(&path, desc)
    }

    #[cfg(feature = "formulas")]
    pub fn formula(&mut self, leaf: &str, desc: &str, expr: PerfFormula) {
        let path = format!("{}.{}", self.prefix, leaf);
        self.registry.formula(&path, desc, expr)
    }

    /// Open a child scope for a nested object.
    pub fn child(&mut self, segment: &str) -> StatsScope<'_> {
        let prefix = if self.prefix.is_empty() {
            segment.to_string()
        } else {
            format!("{}.{}", self.prefix, segment)
        };
        StatsScope { registry: &mut *self.registry, prefix }
    }

    pub fn prefix(&self) -> &str { &self.prefix }
}
```

### 4b.3 StatsScope (no-op impl, when `stats` is off)

```rust
#[derive(Default)]
pub struct StatsScope<'a> {
    _marker: core::marker::PhantomData<&'a ()>,
}

impl<'a> StatsScope<'a> {
    #[inline(always)] pub fn counter(&mut self, _leaf: &str, _desc: &str) -> PerfCounter { PerfCounter }
    #[inline(always)] pub fn histogram(&mut self, _leaf: &str, _desc: &str, _edges: Vec<u64>) -> PerfHistogram { PerfHistogram }
    #[inline(always)] pub fn label_counter(&mut self, _l: &str, _d: &str) -> LabelCounter { LabelCounter }
    #[inline(always)] pub fn child(&mut self, _segment: &str) -> StatsScope<'_> { StatsScope::default() }
    #[inline(always)] pub fn prefix(&self) -> &str { "" }
}
```

### 4b.4 Elaboration walker (engine side)

The walker lives in `runtime/helm-engine` and is invoked from
`HelmSim::instantiate()` (the Rust-side counterpart of Python's
`System.instantiate()`):

```rust
fn walk(node: &mut dyn HelmObject, mut scope: StatsScope<'_>) {
    if let Some(producer) = node.as_stats_producer_mut() {
        producer.register_stats(&mut scope);
    }
    for (name, child) in node.children_mut() {
        let child_scope = scope.child(name);
        walk(child, child_scope);
    }
}

let mut root_scope = StatsScope::root(&mut self.stats); // prefix = "system"
walk(&mut self.tree_root, root_scope);
```

`HelmObject::as_stats_producer_mut()` is `Option<&mut dyn StatsProducer>`.
Objects opt in by implementing `StatsProducer`; objects without stats
(pure config nodes) do not.

### 4b.5 Per-layer registration table

| Layer       | Implementor                                              | Sample stats |
|-------------|----------------------------------------------------------|--------------|
| CPU         | `RiscvArchState`, `Aarch64ArchState`                     | `cycles`, `insns_retired`, `committed_ops`, `branch.taken` |
| JIT         | `JitRuntime`, `TraceCache`                               | `blocks_compiled`, `trace_cache_hits`, `reject_reasons` (LabelCounter) |
| Memory      | `FlatMem`, `HelmAddressSpace`, `MemoryMap` regions       | `loads`, `stores`, `bytes_read`, `bytes_written` |
| Cache       | `CacheModel`, future ICache/DCache/L2                    | `hits`, `misses`, `writebacks`, `latency_hist` |
| MMU/TLB     | per-stage TLB, page-walker                               | `tlb_hits`, `tlb_misses`, `walk_latency_hist` |
| Bus         | AMBA, I2C, SPI controllers (`helm-devices::Bus`)         | `transactions`, `arb_cycles`, `error_count` |
| Device      | PL011, PL031, SP804, GICv2/v3, VirtIO MMIO, PCI ECAM     | `read_count`, `write_count`, `irq_raised`, `irq_acked`, `bytes_in`, `bytes_out` |
| Interconnect| Future XBar / NoC                                        | `cycles_busy`, `port[*].requests`, `congestion_events` |
| Engine      | `HelmEngine`, `HelmSim`, scheduler, `EventQueue`         | `events_serviced`, `quanta_run`, `host_seconds` |

### 4b.6 Example -- L1 ICache implementor

```rust
pub struct L1Icache {
    /* config fields */
    hits:   PerfCounter,
    misses: PerfCounter,
}

impl StatsProducer for L1Icache {
    fn register_stats(&mut self, scope: &mut StatsScope<'_>) {
        self.hits   = scope.counter("hits",   "L1 instruction cache hits");
        self.misses = scope.counter("misses", "L1 instruction cache misses");

        #[cfg(feature = "formulas")]
        scope.formula(
            "hit_rate",
            "L1I hit rate",
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

impl L1Icache {
    fn lookup(&self, addr: u64) -> Option<CacheLine> {
        if let Some(line) = self.lines.get(addr) {
            self.hits.inc();   // hot path: single fetch_add when stats on, nothing when off
            Some(line)
        } else {
            self.misses.inc();
            None
        }
    }
}
```

The cache code never spells `"system.cpu0.icache"`. The path comes
from the tree. `PerfFormula::counter_rel(leaf)` resolves against the
current scope, so formulas survive renames too. Absolute references
(`PerfFormula::counter("system.dram.reads")`) remain available for
cross-tree formulas.

### 4b.7 Tie-in with `m5out/config.{ini,json}`

The same elaboration walker can collect each node's
`AttrRegistry` into a flat config dump. One pass produces both:

- `<m5out>/config.ini` -- `[system.cpu0.icache] size=32KiB ...`
- `<m5out>/stats.txt`  -- `system.cpu0.icache.hits 12345 # ...`

They cannot drift: same tree, same canonical paths.

---

## 5. Dot-Path Resolution

Paths are stored in a `BTreeMap<String, ...>` keyed by the full dot-path string. Resolution is exact-match only — no wildcards, no prefix matching. The `BTreeMap` provides sorted iteration for deterministic dump output.

### Path Validation Rules

Applied at `perf_counter()` / `perf_histogram()` / `perf_formula()` call time:

1. Must not be empty.
2. Every segment (split on `.`) must be a non-empty lowercase ASCII identifier matching `[a-z0-9_]+`.
3. Must not conflict with an existing path of a different type.

```rust
fn validate_path(path: &str) {
    assert!(!path.is_empty(), "stat path must not be empty");
    for segment in path.split('.') {
        assert!(
            !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
            "invalid path segment '{segment}' in '{path}'"
        );
    }
}
```

---

## 6. Dump Formats

### JSON Output

```rust
impl StatsRegistry {
    pub fn dump_json(&self, path: &Path) -> io::Result<()> {
        use std::collections::BTreeMap;

        let mut out: BTreeMap<String, serde_json::Value> = BTreeMap::new();

        for (name, counter) in &self.counters {
            out.insert(name.clone(), serde_json::json!({
                "value": counter.get(),
                "desc":  counter.desc,
            }));
        }

        for (name, hist) in &self.histograms {
            out.insert(name.clone(), serde_json::json!({
                "edges":  hist.edges(),
                "counts": hist.counts(),
                "mean":   hist.approx_mean(),
                "desc":   hist.desc,
            }));
        }

        for (name, (desc, formula)) in &self.formulas {
            let value = formula.eval(self);
            out.insert(name.clone(), serde_json::json!({
                "value": value,
                "desc":  desc,
            }));
        }

        let file = std::fs::File::create(path)?;
        serde_json::to_writer_pretty(file, &out)?;
        Ok(())
    }
}
```

**Example output:**

```json
{
  "system.cpu0.cpi": {
    "value": 1.23,
    "desc": "Cycles per instruction"
  },
  "system.cpu0.cycles": {
    "value": 1230000000,
    "desc": "Total simulated cycles"
  },
  "system.cpu0.icache.hit_rate": {
    "value": 0.9876,
    "desc": "L1 instruction cache hit rate"
  },
  "system.cpu0.icache.hits": {
    "value": 987600000,
    "desc": "L1 instruction cache hits"
  },
  "system.cpu0.icache.latency": {
    "edges": [1, 4, 16, 64],
    "counts": [987600000, 11000000, 1200000, 180000, 20000],
    "mean": 1.08,
    "desc": "L1 instruction cache access latency (cycles)"
  },
  "system.cpu0.icache.misses": {
    "value": 12400000,
    "desc": "L1 instruction cache misses"
  },
  "system.cpu0.insns_retired": {
    "value": 1000000000,
    "desc": "Instructions retired"
  }
}
```

### Terminal Table

```rust
impl StatsRegistry {
    pub fn print_table(&self) {
        println!("{:<55} {:>20}  {}", "Statistic", "Value", "Description");
        println!("{}", "─".repeat(100));

        for (name, counter) in &self.counters {
            println!("{:<55} {:>20}  {}", name, counter.get(), counter.desc);
        }
        for (name, (desc, formula)) in &self.formulas {
            let val = formula.eval(self);
            println!("{:<55} {:>20.6}  {}", name, val, desc);
        }
        for (name, hist) in &self.histograms {
            println!("{:<55} {:>20.3}  {} (mean)", name, hist.approx_mean(), hist.desc);
            for (i, count) in hist.counts().iter().enumerate() {
                let label = if i == 0 {
                    format!("  [0, {})", hist.edges()[0])
                } else if i == hist.edges().len() {
                    format!("  [{}, ∞)", hist.edges()[i - 1])
                } else {
                    format!("  [{}, {})", hist.edges()[i - 1], hist.edges()[i])
                };
                println!("{:<55} {:>20}", label, count);
            }
        }
    }
}
```

**Example terminal output:**

```
Statistic                                               Value  Description
────────────────────────────────────────────────────────────────────────────────────────────────────
system.cpu0.cycles                              1230000000  Total simulated cycles
system.cpu0.cpi                                   1.230000  Cycles per instruction
system.cpu0.icache.hits                          987600000  L1 instruction cache hits
system.cpu0.icache.hit_rate                        0.987600  L1 instruction cache hit rate
system.cpu0.icache.latency                           1.080  L1 instruction cache access latency (cycles) (mean)
  [0, 1)                                               0
  [1, 4)                                       987600000
  [4, 16)                                       11000000
  [16, 64)                                       1200000
  [64, ∞)                                         200000
system.cpu0.icache.misses                        12400000  L1 instruction cache misses
system.cpu0.insns_retired                      1000000000  Instructions retired
```

---

## 7. Module Structure

```
helm-stats/
└── src/
    ├── lib.rs          # Public re-exports: PerfCounter, PerfHistogram, PerfFormula, StatsRegistry
    ├── counter.rs      # PerfCounter (live + noop modules, cfg-selected)
    ├── histogram.rs    # PerfHistogram (live + noop modules)
    ├── label.rs        # LabelCounter (live + noop modules)
    ├── formula.rs      # PerfFormula (compiled only with `formulas`)
    ├── registry.rs     # StatsRegistry (live + noop modules)
    ├── producer.rs     # StatsProducer trait + StatsScope (QOM-style scope walker hook)
    ├── jit.rs          # JitPerfStats: PerfCounter / LabelCounter aggregate
    └── path.rs         # validate_path, path segment rules
```

`Cargo.toml`:

```toml
[features]
default  = ["stats", "formulas", "serde"]
stats    = ["dep:dashmap"]            # LabelCounter needs DashMap
formulas = ["stats"]
gem5-compat = ["stats"]
serde    = ["dep:serde", "dep:serde_json"]

[dependencies]
dashmap     = { version = "5", optional = true }
serde       = { workspace = true, optional = true }
serde_json  = { workspace = true, optional = true }
```

The crate compiles cleanly under all four `cargo build -p helm-stats
--no-default-features --features=…` combinations of `stats`/`formulas`.
CI runs all four; perf builds use `cargo build --no-default-features`.

---

## 8. JitPerfStats migration

The current `framework/helm-stats/src/lib.rs` defines `JitPerfStats` as a
plain struct of `u64` and `BTreeMap<String, u64>`. It is consumed in
`framework/helm-jit/src/runtime.rs`, `framework/helm-jit/src/trace/{compiler,exit}.rs`,
and `runtime/helm-engine/src/jit.rs` -- always as `&mut JitPerfStats`.

The migration is staged so `helm-jit` keeps building at every step.

### Step 1 -- introduce `PerfCounter` / `LabelCounter` aggregates

New target shape (lives in `framework/helm-stats/src/jit.rs`):

```rust
#[derive(Default, Clone)]
pub struct JitPerfStats {
    pub block_cache_hits:        PerfCounter,
    pub block_cache_misses:      PerfCounter,
    pub blocks_compiled:         PerfCounter,
    pub compiled_guest_insns:    PerfCounter,
    pub blocks_executed:         PerfCounter,
    pub traces_compiled:         PerfCounter,
    pub trace_guest_insns:       PerfCounter,
    pub traces_executed:         PerfCounter,
    pub trace_cache_hits:        PerfCounter,
    pub trace_cache_misses:      PerfCounter,
    pub trace_guard_exits:       PerfCounter,
    pub trace_retired:           PerfCounter,
    pub fallback_count:          PerfCounter,
    pub fallback_insns:          PerfCounter,
    pub unsupported_block_starts: PerfCounter,
    pub unsupported_opcodes:     LabelCounter,
    pub reject_reasons:          LabelCounter,
    pub cache_promotions:        PerfCounter,
    pub cache_evictions:         PerfCounter,
    /// Cardinality, not a counter -- read at quantum end from the cache itself.
    pub cache_entries:           usize,
    pub trace_cache_entries:     usize,
}
```

- `PerfCounter` and `LabelCounter` are `Clone`; they hold `Arc` storage
  internally, so `JitPerfStats: Clone` is cheap and there is no need to
  thread `&mut JitPerfStats` through the JIT call graph.
- `cache_entries` / `trace_cache_entries` are read at quantum end; they
  remain `usize`.

### Step 2 -- replace mutating sites

| Today                                                              | After |
|--------------------------------------------------------------------|-------|
| `stats.blocks_compiled = stats.blocks_compiled.saturating_add(1);` | `stats.blocks_compiled.inc();` |
| `stats.compiled_guest_insns += n as u64;`                          | `stats.compiled_guest_insns.add(n as u64);` |
| `stats.unsupported_opcodes.entry(name.to_string()).and_modify(|c| *c += 1).or_insert(1);` | `stats.unsupported_opcodes.bump(name);` (caller provides a `&'static str`) |
| `stats.reject_reasons.entry(reason.label().to_string())...`        | `stats.reject_reasons.bump(reason.label());` |

### Step 3 -- drop `&mut JitPerfStats` in signatures

Functions in `framework/helm-jit/src/{runtime,trace/compiler,trace/exit}.rs`
currently take `stats: &mut JitPerfStats`. After Step 2 they take
`stats: &JitPerfStats` (or hold a clone), since every counter mutation
goes through interior mutability. This unsticks the borrow conflicts in
`runtime.rs` around the `dispatch_block` / `compile_trace` paths.

### Step 4 -- expose into `StatsRegistry`

During engine elaboration, register each `PerfCounter` field under the
canonical dot-path:

```rust
let prefix = format!("system.cpu{vcpu_idx}.jit");
reg.register_counter(&format!("{prefix}.blocks_compiled"),
                     "JIT blocks compiled",
                     jit_stats.blocks_compiled.clone());
/* ... */
reg.register_label_counter(&format!("{prefix}.reject_reasons"),
                           "JIT compile reject reasons",
                           jit_stats.reject_reasons.clone());
```

`register_counter` is a new `StatsRegistry` method that accepts an
already-constructed handle (vs. `counter()` which creates one). This
lets the JIT own the lifecycle of its counters and the registry simply
observe them for dump purposes.

### Step 5 -- delete legacy `Default` map allocations

After Step 2, `JitPerfStats::default()` no longer allocates the two
`BTreeMap`s; with `--no-default-features` it allocates nothing at all
(every field is a ZST). Verified by the `feature_gate_off.rs` test.

---

## Design Decisions from Q&A

### Design Decision: AtomicU64 Relaxed for inc(), SeqCst for get() (Q90)

`PerfCounter::inc()` uses `Ordering::Relaxed` (implemented above as `fetch_add(1, Ordering::Relaxed)`). `PerfCounter::get()` uses `Ordering::SeqCst` for consistent snapshot at dump time. Hot-path performance is non-negotiable — a single `fetch_add(1, Relaxed)` compiles to a single locked instruction on x86 and a `stlxr`/`ldadd` on ARM. Relaxed ordering is correct for independent counters: each counter only needs a consistent snapshot at dump time, not real-time cross-core visibility. The `SeqCst` barrier on `get()` ensures all prior `Relaxed` stores are visible before the value is read.

### Design Decision: Dot-path namespace, uniqueness enforced at registration (Q93)

Stats use a dot-path namespace mirroring the component hierarchy (e.g., `"system.cpu0.icache.hits"`). Each `SimObject` component receives its path prefix during `elaborate()` via the `WorldContext`. The `StatsRegistry` enforces uniqueness at registration time: duplicate paths of the **same type** return the existing handle (idempotent); duplicate paths of **different types** panic. Path construction is done once at `elaborate()` — no string formatting on the hot path. The dot-path convention supports prefix-based filtering (`registry.dump_prefix("system.cpu0")`).

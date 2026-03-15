# helm-stats — LLD: Statistics Implementation

> **Module:** `helm-stats`
> **Types:** `PerfCounter`, `PerfHistogram`, `PerfFormula`, `StatsRegistry`

---

## Table of Contents

1. [PerfCounter](#1-perfcounter)
2. [PerfHistogram](#2-perfhistogram)
3. [PerfFormula](#3-perfformula)
4. [StatsRegistry](#4-statsregistry)
5. [Dot-Path Resolution](#5-dot-path-resolution)
6. [Dump Formats](#6-dump-formats)
7. [Module Structure](#7-module-structure)

---

## 1. PerfCounter

### Definition

```rust
use std::sync::atomic::{AtomicU64, Ordering};

/// A single lock-free 64-bit performance counter.
///
/// Designed to be held as `Arc<PerfCounter>` by SimObject components.
/// All operations are safe to call concurrently from multiple hart threads.
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

### Definition

```rust
/// A fixed-bucket histogram where each bucket is a lock-free `AtomicU64`.
///
/// Bucket `i` counts values `v` where `edges[i-1] <= v < edges[i]`.
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
    ├── counter.rs      # PerfCounter implementation
    ├── histogram.rs    # PerfHistogram implementation
    ├── formula.rs      # PerfFormula expression tree + eval
    ├── registry.rs     # StatsRegistry: perf_counter, perf_histogram, perf_formula, dump_json, print_table
    └── path.rs         # validate_path, path segment rules
```

---

## Design Decisions from Q&A

### Design Decision: AtomicU64 Relaxed for inc(), SeqCst for get() (Q90)

`PerfCounter::inc()` uses `Ordering::Relaxed` (implemented above as `fetch_add(1, Ordering::Relaxed)`). `PerfCounter::get()` uses `Ordering::SeqCst` for consistent snapshot at dump time. Hot-path performance is non-negotiable — a single `fetch_add(1, Relaxed)` compiles to a single locked instruction on x86 and a `stlxr`/`ldadd` on ARM. Relaxed ordering is correct for independent counters: each counter only needs a consistent snapshot at dump time, not real-time cross-core visibility. The `SeqCst` barrier on `get()` ensures all prior `Relaxed` stores are visible before the value is read.

### Design Decision: Dot-path namespace, uniqueness enforced at registration (Q93)

Stats use a dot-path namespace mirroring the component hierarchy (e.g., `"system.cpu0.icache.hits"`). Each `SimObject` component receives its path prefix during `elaborate()` via the `WorldContext`. The `StatsRegistry` enforces uniqueness at registration time: duplicate paths of the **same type** return the existing handle (idempotent); duplicate paths of **different types** panic. Path construction is done once at `elaborate()` — no string formatting on the hot path. The dot-path convention supports prefix-based filtering (`registry.dump_prefix("system.cpu0")`).

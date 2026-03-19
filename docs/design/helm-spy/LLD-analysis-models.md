# helm-spy — LLD: Analysis Models

> **Document:** Low-Level Design — Analysis Models (`src/analysis/`)
> **Crate:** `debug/helm-spy`
> **See also:** [HLD.md](HLD.md), [LLD-primitives.md](LLD-primitives.md)

---

## 1. Module Map

```
src/analysis/
├── mod.rs          pub use InsnMix, CacheModel, BranchPredictor, PredictorKind
├── insn_mix.rs     InsnMix, INSN_CLASS_LABELS
├── cache.rs        CacheModel, CacheState (private)
└── branch_pred.rs  BranchPredictor, PredictorKind
```

Also in `src/session.rs`: `SpySession`, `SpySnapshot`.

---

## 2. InsnMix (`analysis/insn_mix.rs`)

### 2.1 Constant

```rust
pub const INSN_CLASS_LABELS: &[&str] = &[
    "IntAlu", "IntMul", "Branch", "Load", "Store",
    "FpAlu", "SimdAlu", "System", "Nop", "Atomic", "Unknown",
];
```

11 labels, matching `InsnClass` discriminants (0..=10).

### 2.2 `InsnMix`

```rust
pub struct InsnMix {
    counts: IndexedCounter,
}
```

Wraps an `IndexedCounter` dimensioned over `InsnClass`. The counter is named
`"insn_mix"` and constructed with `INSN_CLASS_LABELS`.

**Methods:**
- `new() -> Self`
- `record(&self, class: InsnClass)` — `counts.inc(class as usize)`; hot-path safe
- `table(&self) -> Vec<(&'static str, u64, f64)>` — delegates to `IndexedCounter::table()`
- `total(&self) -> u64` — total instructions recorded
- `value(&self, class: InsnClass) -> u64` — count for a specific class
- `fraction(&self, class: InsnClass) -> f64` — fraction of total for a specific class
- `reset(&self)` — zeros all buckets

`Default` is implemented as `Self::new()`.

---

## 3. CacheModel (`analysis/cache.rs`)

### 3.1 Internal state (private)

```rust
struct CacheState {
    sets: usize,
    ways: usize,
    line_size: usize,
    tags: Vec<Vec<u64>>,    // [set][way] — tag value; u64::MAX = invalid
    lru: Vec<Vec<u32>>,     // [set][way] — LRU counter (higher = more recently used)
    hits: u64,
    misses: u64,
    clock: u32,             // monotonic counter incremented on every access
}
```

### 3.2 `CacheModel`

```rust
pub struct CacheModel {
    name: String,
    state: Mutex<CacheState>,
}
```

LRU set-associative cache model. Uses `Mutex<CacheState>` — the lock is acceptable
because cache simulation is per-quantum (not truly hot-path), used via the analysis
session rather than inside per-instruction callbacks.

**Constructor:**
```rust
pub fn new(name: &str, size_bytes: usize, ways: usize, line_size: usize) -> Self
```

Asserts: `line_size.is_power_of_two()`, `ways > 0`.
Computes: `sets = size_bytes / (ways * line_size)`. Asserts `sets > 0`.
Initializes all tags to `u64::MAX` (invalid), all lru counters to 0.

**`access(&self, addr: u64)`:**
1. Compute `line_addr = addr / line_size`.
2. Compute `set_idx = line_addr % sets`, `tag = line_addr / sets`.
3. Increment clock.
4. Scan all ways in `set_idx` for matching tag: hit — update lru, return.
5. Miss: find way with minimum lru counter; evict; install new tag; update lru.

**Other methods:**
- `name(&self) -> &str`
- `hit_rate(&self) -> f64` — `hits / (hits + misses)`; returns 0.0 if no accesses
- `hits(&self) -> u64`
- `misses(&self) -> u64`
- `mpki(&self, insn_count: u64) -> f64` — `misses / insn_count * 1000`; returns 0.0 if insn_count == 0
- `reset(&self)` — clears all tags to `u64::MAX`, all lru to 0, zeros hits/misses/clock

---

## 4. BranchPredictor (`analysis/branch_pred.rs`)

### 4.1 `PredictorKind`

```rust
pub enum PredictorKind {
    BiModal { bits: u8 },                         // direct-mapped, indexed by PC bits
    GShare { hist_bits: u8, table_bits: u8 },     // XOR of GHR with PC bits
}
```

### 4.2 `BranchPredictor`

```rust
pub struct BranchPredictor {
    kind: PredictorKind,
    table: Vec<u8>,     // 2-bit saturating counters (0..=3)
    history: u64,       // global branch history register (GShare)
    predictions: u64,
    mispredictions: u64,
}
```

2-bit saturating counter table:
- 0 = strongly not taken
- 1 = weakly not taken (initial state for all entries)
- 2 = weakly taken
- 3 = strongly taken

Prediction: counter >= 2 → predict taken.

**Constructor:**
```rust
pub fn new(kind: PredictorKind) -> Self
```

Table size: `1 << bits` for BiModal, `1 << table_bits` for GShare.
All counters initialized to 1 (weakly not taken).

**Table index computation (`table_index(pc)` — private):**
- BiModal: `(pc >> 2) & ((1 << bits) - 1)` (skips 2 low bits for instruction alignment)
- GShare: `((pc >> 2) ^ (history & hist_mask)) & table_mask`

**`predict_and_update(&mut self, pc: u64, taken: bool)`:**
1. Compute table index.
2. Predict: `table[idx] >= 2`.
3. If prediction != taken: `mispredictions += 1`.
4. `predictions += 1`.
5. Update counter: if taken, `(counter + 1).min(3)`; else `counter.saturating_sub(1)`.
6. Update history: `history = (history << 1) | (taken as u64)`.

**Other methods:**
- `miss_rate(&self) -> f64` — `mispredictions / predictions`; returns 0.0 if no predictions
- `mpki(&self, insn_count: u64) -> f64` — `mispredictions / insn_count * 1000`; returns 0.0 if insn_count == 0
- `predictions(&self) -> u64`
- `mispredictions(&self) -> u64`
- `reset(&mut self)` — fills table with 1, zeros history/predictions/mispredictions

Note: `predict_and_update` takes `&mut self` (not `&self`) because it mutates the predictor
table. `BranchPredictor` is not lock-protected; callers must not share across threads without
external synchronization.

---

## 5. SpySession (`session.rs`)

### 5.1 `SpySession`

```rust
pub struct SpySession {
    pub insn_count: Counter,
    pub insn_mix: InsnMix,
    pub hot_pcs: HeatMap,
    pub branch_heatmap: HeatMap,
    pub cache_l1d: Option<CacheModel>,
    pub branch_pred: Option<BranchPredictor>,
    pub fault_history: RingBuffer<String>,
    pub triggers: Vec<Trigger>,
}
```

User-facing aggregator. All fields are public for direct inspection.
Optional fields (`cache_l1d`, `branch_pred`) are configured via builder methods.
`fault_history` is always present with capacity 128.
`triggers` is a `Vec<Trigger>` checked via `check_triggers()`.

**Constructors and builders:**
```rust
pub fn new() -> Self
pub fn with_cache_l1d(mut self, size_bytes: usize, ways: usize, line_size: usize) -> Self
pub fn with_branch_predictor(mut self, pred: BranchPredictor) -> Self
```

`Default` is implemented as `Self::new()`.

**Methods:**
- `add_trigger(&mut self, trigger: Trigger)` — push a trigger onto `self.triggers`
- `check_triggers(&self, pc: u64, insn_count: u64)` — calls `t.check(pc, insn_count)` for each trigger
- `snapshot(&self) -> SpySnapshot` — creates a point-in-time snapshot

**`snapshot()` implementation:**
```rust
SpySnapshot {
    insn_count:       self.insn_count.value(),
    insn_mix_table:   self.insn_mix.table(),
    hot_pcs_top20:    self.hot_pcs.top(20),
    cache_hit_rate:   self.cache_l1d.as_ref().map(|c| c.hit_rate()),
    branch_miss_rate: self.branch_pred.as_ref().map(|p| p.miss_rate()),
}
```

### 5.2 `SpySnapshot`

```rust
pub struct SpySnapshot {
    pub insn_count: u64,
    pub insn_mix_table: Vec<(&'static str, u64, f64)>,
    pub hot_pcs_top20: Vec<(u64, u64)>,
    pub cache_hit_rate: Option<f64>,
    pub branch_miss_rate: Option<f64>,
}
```

Point-in-time snapshot of session state for reporting or differential analysis.
All fields are public. Produced by `SpySession::snapshot()`.

---

## 6. What Is Not Yet Implemented

The following were described in earlier design documents but are not present in the codebase:

| Feature | Notes |
|---|---|
| `SimPoint` | Basic-block vector computation — Phase 3 |
| `PowerModel` | Per-class energy estimation — Phase 3 |
| `DiffAnalysis` | Session differential comparison — Phase 3 |
| `SpySession::subscribe()` | Probe wiring — requires `ProbePluginBridge` |
| `QuantumObserver` registration | `SpySession` does not hold observers; no `quantum_observers` field |
| `branch_count`, `branch_mix`, `syscall_log`, `branch_trace` fields | Not present in `SpySession`; only `branch_heatmap` and optional `branch_pred` |
| PyO3 `#[pyclass]` bindings | Not yet added to this crate |

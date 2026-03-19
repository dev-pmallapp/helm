# helm-spy — LLD: Analysis Models

> **Module:** `helm-spy` (`framework/helm-spy`)
> **Path:** `framework/helm-spy/src/analysis/`
> **Types:** `InsnMix`, `CacheModel`, `BranchPredictor`, `SimPoint`, `PowerModel`,
> `DiffAnalysis`, `IndexedCounter`, `IntervalHistogram`, `CorrelHist2D`,
> `TraceRing<T, N>`, `Trigger`, `Window`, `SpySession`, `SpySnapshot`
> **Supersedes:** `helm-plugin` (`HowVec`, `CacheSim`, `BranchTrace`, `InsnCount` plugins)
> **See also:** `docs/design/instrumentation-v2/PLAN.md` §5,
> `docs/design/helm-probe/LLD-probe-framework.md`

---

## Table of Contents

1. [IndexedCounter](#1-indexedcounter)
2. [InsnMix](#2-insnmix)
3. [CacheModel](#3-cachemodel)
4. [BranchPredictor](#4-branchpredictor)
5. [SimPoint — Basic Block Vector Computation](#5-simpoint--basic-block-vector-computation)
6. [PowerModel — Per-Class Energy Estimation](#6-powermodel--per-class-energy-estimation)
7. [DiffAnalysis — Session Differential](#7-diffanalysis--session-differential)
8. [IntervalHistogram](#8-intervalhistogram)
9. [CorrelHist2D](#9-correlhist2d)
10. [TraceRing](#10-tracering)
11. [Trigger System](#11-trigger-system)
12. [Window](#12-window)
13. [Per-vCPU Scoreboard Pattern](#13-per-vcpu-scoreboard-pattern)
14. [SpySession — Complete Definition](#14-observesession--complete-definition)
15. [subscribe() — Probe Wiring Method](#15-subscribe--probe-wiring-method)
16. [SpySnapshot — Differential Baseline](#16-sessionsnapshot--differential-baseline)
17. [PyO3 Exposure](#17-pyo3-exposure)
18. [Module Structure](#18-module-structure)
19. [Design Decisions](#19-design-decisions)

---

## 1. IndexedCounter

`IndexedCounter` is the fundamental dimension-keyed counter primitive. It replaces the
`HowVec` plugin from `helm-plugin` as a first-class, composable primitive.

### Purpose

Answer "per-class" questions — instruction class mix, branch kind distribution, SIMD
utilization — without bespoke plugins. The fixed-size array layout ensures the optimizer
can elide bounds checks when the index comes from a match on an exhaustive enum.

### Definition

```rust
// framework/helm-spy/src/primitives/indexed_counter.rs

/// Fixed-size counter array indexed by a dimension (e.g. InsnClass, BranchKind).
///
/// Lock-free: one AtomicU64 per bucket. Hot-loop cost is one array index +
/// `fetch_add(Relaxed)` ≈ 1–2 ns, no allocation, no locking.
pub struct IndexedCounter {
    name:    String,
    labels:  Vec<&'static str>,
    buckets: Vec<AtomicU64>,
}

impl IndexedCounter {
    pub fn new(name: impl Into<String>, labels: Vec<&'static str>) -> Self {
        let n = labels.len();
        Self {
            name: name.into(),
            labels,
            buckets: (0..n).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    /// Increment bucket at `idx` by 1.
    ///
    /// Bounds check is present in debug; elided in release when `idx` comes from
    /// a match on an exhaustive enum (`InsnClass as usize` etc.).
    #[inline(always)]
    pub fn inc(&self, idx: usize) {
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    /// Add `n` to bucket `idx`.
    #[inline(always)]
    pub fn add(&self, idx: usize, n: u64) {
        self.buckets[idx].fetch_add(n, Ordering::Relaxed);
    }

    /// Read a single bucket.
    pub fn get(&self, idx: usize) -> u64 {
        self.buckets[idx].load(Ordering::SeqCst)
    }

    /// Sum of all buckets.
    pub fn total(&self) -> u64 {
        self.buckets.iter().map(|b| b.load(Ordering::Relaxed)).sum()
    }

    /// Fraction of total in bucket `idx`. Returns 0.0 when total is 0.
    pub fn fraction(&self, idx: usize) -> f64 {
        let total = self.total();
        if total == 0 { return 0.0; }
        self.buckets[idx].load(Ordering::Relaxed) as f64 / total as f64
    }

    /// All buckets as `(label, count, fraction)`.
    pub fn table(&self) -> Vec<(&'static str, u64, f64)> {
        let total = self.total();
        self.labels.iter().zip(self.buckets.iter())
            .map(|(&label, b)| {
                let count = b.load(Ordering::SeqCst);
                let frac  = if total > 0 { count as f64 / total as f64 } else { 0.0 };
                (label, count, frac)
            })
            .collect()
    }

    /// Reset all buckets to 0.
    pub fn reset(&self) {
        for b in &self.buckets { b.store(0, Ordering::SeqCst); }
    }

    /// Merge another `IndexedCounter` into self (quantum_end aggregation).
    pub fn merge(&self, other: &IndexedCounter) {
        assert_eq!(self.buckets.len(), other.buckets.len());
        for (a, b) in self.buckets.iter().zip(other.buckets.iter()) {
            let v = b.load(Ordering::Relaxed);
            if v > 0 { a.fetch_add(v, Ordering::Relaxed); }
        }
    }
}
```

---

## 2. InsnMix

**File:** `src/analysis/insn_mix.rs`

Replaces the `HowVec` plugin. Counts retired instructions by `InsnClass` using an
inline `[AtomicU64; InsnClass::COUNT]` array. One `fetch_add(Relaxed)` per instruction;
no allocation, no locking.

### `InsnClass` (in `helm-engine`, re-exported from `helm-spy::events`)

```rust
// helm-engine/src/lib.rs — canonical location
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum InsnClass {
    IntAlu   = 0,   // add, sub, and, orr, eor, lsl, mov, cmp, cmn, tst
    IntMul   = 1,   // mul, mla, smull, umull, madd, msub
    IntDiv   = 2,   // sdiv, udiv
    Load     = 3,   // ldr, ldp, ldur, ldrb, ldrh, ldapr, ldxr, ldar
    Store    = 4,   // str, stp, stur, strb, strh, stlr, stxr, stlxr
    Branch   = 5,   // b, bl, blr, ret, cbz, cbnz, tbz, tbnz, b.cond
    FpScalar = 6,   // fadd, fsub, fmul, fdiv, fsqrt, fcvt, fmov, fcmp
    FpVector = 7,   // SIMD data-proc: add, mul, ext, dup, zip, tbl
    LdstSimd = 8,   // ld1, st1, ld2, ld3, ld4, ld1r, …
    Atomic   = 9,   // LSE: ldadd, cas, casp, swp, stadd, stlxr-pair
    Sysreg   = 10,  // mrs, msr, sys, sysl, at, dc, ic, tlbi
    Barrier  = 11,  // dsb, dmb, isb, esb, wfe, wfi, sev
    Other    = 12,  // nop, bti, hint, udf, and unclassified encodings
}

impl InsnClass {
    pub const COUNT: usize = 13;

    pub fn from_repr(idx: usize) -> Self {
        // SAFETY: repr(usize) with COUNT variants; caller bounds-checks idx.
        assert!(idx < Self::COUNT);
        unsafe { std::mem::transmute(idx) }
    }
}
```

### `InsnMix` Struct

```rust
// framework/helm-spy/src/analysis/insn_mix.rs

use std::sync::atomic::{AtomicU64, Ordering};
use helm_engine::InsnClass;
use helm_probe::{CpuProbes, CpuStepEvent};

/// Instruction-class mix counter.
///
/// Holds one `AtomicU64` per `InsnClass` in a fixed-size inline array.
/// Hot-loop cost: `classify_aarch64_opcode(raw)` (~2 ns) + `fetch_add(Relaxed)`.
pub struct InsnMix {
    name:   String,
    counts: [AtomicU64; InsnClass::COUNT],
}

impl InsnMix {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            counts: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    /// Record one instruction of the given class. Lock-free.
    #[inline(always)]
    pub fn record(&self, class: InsnClass) {
        self.counts[class as usize].fetch_add(1, Ordering::Relaxed);
    }

    /// Read the count for one class.
    pub fn count(&self, class: InsnClass) -> u64 {
        self.counts[class as usize].load(Ordering::SeqCst)
    }

    /// Total instructions across all classes.
    pub fn total(&self) -> u64 {
        self.counts.iter().map(|c| c.load(Ordering::Relaxed)).sum()
    }

    /// Fraction of total in `class`. Returns 0.0 when total is 0.
    pub fn fraction(&self, class: InsnClass) -> f64 {
        let total = self.total();
        if total == 0 { 0.0 } else {
            self.counts[class as usize].load(Ordering::Relaxed) as f64 / total as f64
        }
    }

    /// Full distribution as `(class, count, percentage)`, sorted descending by count.
    /// Only non-zero classes are included.
    pub fn table(&self) -> Vec<(InsnClass, u64, f64)> {
        let raw: [u64; InsnClass::COUNT] =
            std::array::from_fn(|i| self.counts[i].load(Ordering::SeqCst));
        let total: u64 = raw.iter().sum();
        let mut rows: Vec<(InsnClass, u64, f64)> = raw.iter().enumerate()
            .filter(|(_, &c)| c > 0)
            .map(|(i, &c)| {
                let pct = if total > 0 { c as f64 / total as f64 * 100.0 } else { 0.0 };
                (InsnClass::from_repr(i), c, pct)
            })
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1));
        rows
    }

    /// Merge another `InsnMix` into self (quantum_end aggregation).
    pub fn merge(&self, other: &InsnMix) {
        for i in 0..InsnClass::COUNT {
            let v = other.counts[i].load(Ordering::Relaxed);
            if v > 0 { self.counts[i].fetch_add(v, Ordering::Relaxed); }
        }
    }

    /// Subscribe to `probes.post_step`. Classifies each retired instruction
    /// and calls `record(class)`.
    #[cfg(debug_assertions)]
    pub fn subscribe_to_steps(&self, probes: &mut CpuProbes) {
        let ptr = self as *const InsnMix;
        probes.post_step.subscribe(move |ev: &CpuStepEvent| {
            let class = helm_engine::classify_aarch64_opcode(ev.raw);
            // SAFETY: `SpySession` outlives all probe subscriptions.
            unsafe { (*ptr).record(class); }
        });
    }

    /// Reset all class counts to zero.
    pub fn reset(&self) {
        for c in &self.counts { c.store(0, Ordering::SeqCst); }
    }
}
```

---

## 3. CacheModel

**File:** `src/analysis/cache.rs`

Replaces the `CacheSim` plugin. Simulates an LRU set-associative cache for architectural
exploration (not cycle-accurate). `access(&mut self, vaddr)` takes `&mut self` for LRU
updates — not directly thread-safe. Per-vCPU isolation via `Scoreboard<CacheModel>`
(see §13) provides the thread-safety contract.

### `CacheResult` Type

```rust
/// Result of a single cache access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheResult {
    /// Address was present in a valid cache line.
    Hit,
    /// Address not present; installed in an empty way.
    Miss,
    /// Address not present; an existing line was evicted. Contains the
    /// evicted line's tag-aligned address.
    Evict(u64),
}
```

### `CacheModel` Struct

```rust
/// LRU set-associative cache model.
///
/// Not thread-safe (`access()` takes `&mut self`). Use one per vCPU slot in a
/// `Scoreboard<CacheModel>`, or wrap in `Mutex<CacheModel>` for single-vCPU.
///
/// # Parameters
///
/// - `sets`: number of cache sets — must be a power of two (e.g. 64)
/// - `ways`: associativity (e.g. 8 for 8-way)
/// - `line_size`: bytes per cache line — must be a power of two (e.g. 64)
///
/// # Memory
///
/// 32-KiB L1D (sets=64, ways=8, line_size=64):
/// tags = 64×8×8 = 4096 B, lru = 64×8×1 = 512 B → 4.5 KiB total.
pub struct CacheModel {
    name:      String,
    pub sets:      usize,
    pub ways:      usize,
    pub line_size: usize,

    /// `tags[set][way]` — the tag stored in this slot.
    /// Tag = `vaddr & !(line_size - 1)`. `u64::MAX` → slot invalid (cold).
    tags: Vec<Vec<u64>>,

    /// `lru[set][way]` — LRU age counter (0 = LRU, ways-1 = MRU).
    lru: Vec<Vec<u8>>,

    hits:   AtomicU64,
    misses: AtomicU64,
    evicts: AtomicU64,
}

impl CacheModel {
    pub fn new(name: impl Into<String>, sets: usize, ways: usize, line_size: usize) -> Self {
        assert!(sets.is_power_of_two(),      "sets must be power of two");
        assert!(line_size.is_power_of_two(), "line_size must be power of two");
        assert!(ways >= 1 && ways <= 64,     "ways must be in [1, 64]");
        Self {
            name: name.into(),
            sets, ways, line_size,
            tags:   vec![vec![u64::MAX; ways]; sets],
            lru:    vec![vec![0u8;     ways]; sets],
            hits:   AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evicts: AtomicU64::new(0),
        }
    }

    /// Access the cache at `vaddr`. Returns `Hit`, `Miss`, or `Evict(evicted_tag)`.
    ///
    /// Updates LRU state. Called per data memory access in the per-vCPU slot.
    #[inline]
    pub fn access(&mut self, vaddr: u64) -> CacheResult {
        let line_tag = vaddr & !(self.line_size as u64 - 1);
        let set_idx  = ((vaddr / self.line_size as u64) as usize) & (self.sets - 1);

        let tags = &mut self.tags[set_idx];
        let lru  = &mut self.lru[set_idx];

        // 1. Check for hit.
        for way in 0..self.ways {
            if tags[way] == line_tag {
                let promoted = lru[way];
                for w in 0..self.ways {
                    if lru[w] > promoted { lru[w] -= 1; }
                }
                lru[way] = (self.ways - 1) as u8;
                self.hits.fetch_add(1, Ordering::Relaxed);
                return CacheResult::Hit;
            }
        }

        // 2. Miss — find a free way.
        if let Some(way) = tags.iter().position(|&t| t == u64::MAX) {
            tags[way] = line_tag;
            lru[way]  = (self.ways - 1) as u8;
            for w in 0..self.ways {
                if w != way && lru[w] > 0 { lru[w] -= 1; }
            }
            self.misses.fetch_add(1, Ordering::Relaxed);
            return CacheResult::Miss;
        }

        // 3. Evict the LRU way (age == 0).
        let victim = lru.iter().position(|&a| a == 0).unwrap_or(0);
        let evicted_tag = tags[victim];
        tags[victim] = line_tag;
        lru[victim]  = (self.ways - 1) as u8;
        for w in 0..self.ways {
            if w != victim { lru[w] -= 1; }
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        self.evicts.fetch_add(1, Ordering::Relaxed);
        CacheResult::Evict(evicted_tag)
    }

    /// Cache hit rate: `hits / (hits + misses)`. Returns `1.0` if no accesses.
    pub fn hit_rate(&self) -> f64 {
        let h = self.hits.load(Ordering::SeqCst);
        let m = self.misses.load(Ordering::SeqCst);
        if h + m == 0 { 1.0 } else { h as f64 / (h + m) as f64 }
    }

    /// Miss rate: `1.0 - hit_rate()`.
    pub fn miss_rate(&self) -> f64 { 1.0 - self.hit_rate() }

    /// Misses per kilo-instruction (MPKI). Returns 0.0 if `insn_count == 0`.
    pub fn mpki(&self, insn_count: u64) -> f64 {
        if insn_count == 0 { return 0.0; }
        let m = self.misses.load(Ordering::SeqCst);
        m as f64 / insn_count as f64 * 1000.0
    }

    pub fn hits(&self)   -> u64 { self.hits.load(Ordering::Relaxed) }
    pub fn misses(&self) -> u64 { self.misses.load(Ordering::Relaxed) }
    pub fn evicts(&self) -> u64 { self.evicts.load(Ordering::Relaxed) }

    /// Merge another `CacheModel`'s stats into self (quantum_end aggregation).
    /// Does NOT merge tag/LRU state — only accumulates hit/miss/evict counters.
    pub fn merge_stats(&mut self, other: &CacheModel) {
        self.hits.fetch_add(other.hits.load(Ordering::Relaxed), Ordering::Relaxed);
        self.misses.fetch_add(other.misses.load(Ordering::Relaxed), Ordering::Relaxed);
        self.evicts.fetch_add(other.evicts.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    /// Reset stats and flush all cache lines (cold start).
    pub fn reset(&mut self) {
        for row in &mut self.tags { row.fill(u64::MAX); }
        for row in &mut self.lru  { row.fill(0); }
        self.hits.store(0, Ordering::SeqCst);
        self.misses.store(0, Ordering::SeqCst);
        self.evicts.store(0, Ordering::SeqCst);
    }
}
```

### Thread-Safety Note

`CacheModel::access()` takes `&mut self` — LRU counters are not atomic. The caller must
provide exclusive access. The per-vCPU Scoreboard pattern (§13) gives each vCPU its own
`CacheModel` slot with no contention. At `quantum_end`, each vCPU slot's hit/miss
counters are aggregated into the summary model via `merge_stats()`.

For single-vCPU FS mode, wrapping in `Mutex<CacheModel>` and calling `try_lock()` inside
the `mem` probe callback is acceptable — branches fire roughly once per ~10 instructions,
so the Mutex is acquired infrequently.

---

## 4. BranchPredictor

**File:** `src/analysis/branch_pred.rs`

Simulates a software branch predictor. Subscribes to `Probe<BranchEvent>`. Per-vCPU via
`Scoreboard<BranchPredictor>` (§13), merged at `quantum_end`.

### `PredictorKind` Enum

```rust
/// Which predictor algorithm to simulate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictorKind {
    /// Bimodal: one 2-bit saturating counter per PC-indexed table entry.
    /// Table size = `1 << table_bits` entries. Typical: table_bits=12 (4096 entries).
    BiModal { table_bits: u8 },

    /// GShare: index = `(PC >> 2) XOR global_history`. Both `history_bits` and
    /// `table_bits` are configurable. Typical: history_bits=12, table_bits=12.
    GShare { history_bits: u8, table_bits: u8 },

    /// Two-Level (Yeh/Patt): per-PC history register → shared pattern history table.
    /// Placeholder for Phase 3. Panics if used.
    TwoLevel,

    /// Perfect predictor: never mispredicts. Used as an upper-bound baseline.
    Perfect,
}
```

### `BranchPredictor` Struct

```rust
use helm_probe::{BranchEvent, CpuProbes};

/// Software branch predictor model.
///
/// Not thread-safe (`predict()` takes `&mut self`). Use one per vCPU slot in a
/// `Scoreboard<BranchPredictor>`, or wrap in `Mutex` for single-vCPU.
pub struct BranchPredictor {
    name: String,
    pub kind: PredictorKind,

    /// 2-bit saturating counter table. Values in [0, 3]; >= 2 predicts taken.
    /// Initialized to 0b10 (weakly taken).
    table: Vec<u8>,

    /// Global history register (GShare). Width = `history_bits` bits.
    history: u64,

    pub predictions:    u64,
    pub mispredictions: u64,
}

impl BranchPredictor {
    pub fn new(name: impl Into<String>, kind: PredictorKind) -> Self {
        let table_bits = match kind {
            PredictorKind::BiModal { table_bits } => table_bits,
            PredictorKind::GShare  { table_bits, .. } => table_bits,
            _ => 0,
        };
        let table_size = if table_bits > 0 { 1usize << table_bits } else { 0 };
        Self {
            name: name.into(), kind,
            table: vec![0b10u8; table_size],   // weakly taken
            history: 0,
            predictions: 0,
            mispredictions: 0,
        }
    }

    pub fn bimodal_4k() -> Self { Self::new("bimodal", PredictorKind::BiModal { table_bits: 12 }) }
    pub fn gshare_4k()  -> Self { Self::new("gshare",  PredictorKind::GShare { history_bits: 12, table_bits: 12 }) }
    pub fn perfect()    -> Self { Self::new("perfect",  PredictorKind::Perfect) }

    /// Predict and update state for one branch.
    ///
    /// Returns `true` if prediction was correct, `false` on misprediction.
    /// Updates the 2-bit counter (and global history for GShare) before returning.
    /// Prediction is made from the **pre-update** counter value — matching hardware.
    pub fn predict(&mut self, pc: u64, taken: bool) -> bool {
        self.predictions += 1;

        let predicted_taken = match self.kind {
            PredictorKind::BiModal { table_bits } => {
                let idx = (pc >> 2) as usize & ((1usize << table_bits) - 1);
                let pred = self.table[idx] >= 2;
                self.update_2bit(idx, taken);
                pred
            }

            PredictorKind::GShare { history_bits, table_bits } => {
                let hmask = (1u64 << history_bits) - 1;
                let tmask = (1usize << table_bits) - 1;
                let idx = ((pc >> 2) ^ (self.history & hmask)) as usize & tmask;
                let pred = self.table[idx] >= 2;
                self.update_2bit(idx, taken);
                self.history = ((self.history << 1) | taken as u64) & hmask;
                pred
            }

            PredictorKind::Perfect => taken,

            PredictorKind::TwoLevel => unimplemented!("TwoLevel predictor not yet implemented"),
        };

        let correct = predicted_taken == taken;
        if !correct { self.mispredictions += 1; }
        correct
    }

    fn update_2bit(&mut self, idx: usize, taken: bool) {
        if taken {
            if self.table[idx] < 3 { self.table[idx] += 1; }
        } else {
            if self.table[idx] > 0 { self.table[idx] -= 1; }
        }
    }

    /// Misprediction rate. Returns 0.0 if no branches seen.
    pub fn miss_rate(&self) -> f64 {
        if self.predictions == 0 { 0.0 }
        else { self.mispredictions as f64 / self.predictions as f64 }
    }

    /// Mispredictions per kilo-instruction (MPKI).
    pub fn mpki(&self, insn_count: u64) -> f64 {
        if insn_count == 0 { 0.0 }
        else { self.mispredictions as f64 / insn_count as f64 * 1000.0 }
    }

    /// Subscribe to the `branch` probe. Wraps in `Arc<Mutex<Self>>`.
    ///
    /// The `Mutex` is locked per branch event (~5–10% of instruction rate).
    /// Acceptable in dev mode. In production (release), the probe is ZST — no cost.
    pub fn subscribe_to_branches(self: &Arc<Mutex<Self>>, probes: &mut CpuProbes) {
        let me = Arc::clone(self);
        probes.branch.subscribe(move |ev: &BranchEvent| {
            if let Ok(mut pred) = me.try_lock() {
                pred.predict(ev.pc, ev.taken);
            }
        });
    }

    /// Merge another predictor's statistics into self (quantum_end aggregation).
    /// Does NOT merge table state — only accumulates prediction/misprediction counts.
    pub fn merge_stats(&mut self, other: &BranchPredictor) {
        self.predictions    += other.predictions;
        self.mispredictions += other.mispredictions;
    }

    /// Reset prediction counters and reinitialize the counter table (cold start).
    pub fn reset(&mut self) {
        for c in &mut self.table { *c = 0b10; }
        self.history        = 0;
        self.predictions    = 0;
        self.mispredictions = 0;
    }
}
```

---

## 5. SimPoint — Basic Block Vector Computation

**File:** `src/analysis/simpoint.rs`
**Phase:** 3

Collects Basic Block Vectors (BBVs) for use with the SimPoint offline tool. A BBV for
one interval is a vector of basic block entry frequencies, normalized to unit L1 norm.

### Output Type

```rust
/// Exported data for the SimPoint offline tool.
///
/// Compatible with the `.bb` file format used by the original SimPoint tool
/// (Sherwood et al., 2002). Each row is one interval; each column is one
/// basic block's normalized execution count.
pub struct SimPointData {
    /// Instructions per interval (e.g. 100_000_000).
    pub interval:  u64,
    /// Normalized BBVs — `intervals[interval_idx][bb_idx]` = normalized count.
    /// Each row sums to 1.0 (or 0.0 for empty intervals).
    pub intervals: Vec<Vec<f64>>,
    /// Basic block entry PCs (one per column, stable across intervals).
    pub bb_ids:    Vec<u64>,
}

impl SimPointData {
    /// Format as a `.bb` file compatible with the SimPoint tool.
    ///
    /// Format: one line per non-empty interval:
    /// `T:bb_id:count bb_id:count ... :`
    /// where bb_id is 1-based and count is the unnormalized integer entry count.
    pub fn to_bb_file(&self) -> String {
        let mut out = String::new();
        for (i, bbv) in self.intervals.iter().enumerate() {
            out.push('T');
            for (j, &freq) in bbv.iter().enumerate() {
                if freq > 0.0 {
                    // Un-normalize: multiply by interval size for approximate count.
                    let count = (freq * self.interval as f64).round() as u64;
                    out.push_str(&format!(":{j}:{count} ", j + 1, count));
                }
            }
            out.push_str(":\n");
        }
        out
    }
}
```

### `SimPoint` Struct

```rust
use std::collections::HashMap;
use helm_probe::{BranchEvent, CpuProbes};

/// Basic Block Vector collector for SimPoint analysis.
///
/// Subscribes to `Probe<BranchEvent>`. Every branch event marks the end of the
/// current basic block and the start of the next. On each interval boundary,
/// the accumulated BBV is normalized and pushed to `intervals`.
pub struct SimPoint {
    /// Instructions per interval. Default: 100_000_000.
    pub interval: u64,

    /// Completed intervals. Each entry is a normalized BBV.
    intervals: Vec<Vec<f64>>,

    /// PC → stable BB index mapping (assigned in order of first appearance).
    bb_index: HashMap<u64, usize>,

    /// Execution counts for the current interval, keyed by BB index.
    current: HashMap<usize, u64>,

    /// Approximate instruction count within the current interval.
    /// Derived from branch count. Use `probe-full` for exact counts.
    insn_in_interval: u64,
}

impl SimPoint {
    pub fn new(interval: u64) -> Self {
        assert!(interval > 0, "interval must be > 0");
        Self {
            interval,
            intervals: Vec::new(),
            bb_index: HashMap::new(),
            current: HashMap::new(),
            insn_in_interval: 0,
        }
    }

    fn on_branch(&mut self, ev: &BranchEvent) {
        // Assign or look up the BB index for the branch target.
        let len = self.bb_index.len();
        let next_idx = *self.bb_index.entry(ev.target).or_insert(len);
        *self.current.entry(next_idx).or_insert(0) += 1;
        self.insn_in_interval += 1;

        if self.insn_in_interval >= self.interval {
            self.finish_interval();
        }
    }

    /// Commit the current BBV, normalize it, and start a new interval.
    /// Called at an interval boundary or at end-of-simulation.
    pub fn finish_interval(&mut self) {
        let num_bbs = self.bb_index.len();
        let mut bbv = vec![0.0f64; num_bbs];
        let total: u64 = self.current.values().sum();
        if total > 0 {
            for (&idx, &count) in &self.current {
                if idx < num_bbs {
                    bbv[idx] = count as f64 / total as f64;
                }
            }
        }
        self.intervals.push(bbv);
        self.current.clear();
        self.insn_in_interval = 0;
    }

    /// Subscribe to branch events via a `Mutex<SimPoint>` wrapper.
    #[cfg(debug_assertions)]
    pub fn subscribe_to_branches(
        this: &Arc<Mutex<SimPoint>>,
        probes: &mut CpuProbes,
    ) {
        let sp = Arc::clone(this);
        probes.branch.subscribe(move |ev: &BranchEvent| {
            if let Ok(mut s) = sp.lock() { s.on_branch(ev); }
        });
    }

    /// Export all accumulated BBVs.
    pub fn export(&self) -> SimPointData {
        let mut bb_ids = vec![0u64; self.bb_index.len()];
        for (&pc, &idx) in &self.bb_index {
            if idx < bb_ids.len() { bb_ids[idx] = pc; }
        }
        SimPointData {
            interval:  self.interval,
            intervals: self.intervals.clone(),
            bb_ids,
        }
    }

    /// Number of complete intervals collected so far.
    pub fn interval_count(&self) -> usize { self.intervals.len() }

    /// Number of unique basic blocks seen so far.
    pub fn bb_count(&self) -> usize { self.bb_index.len() }
}
```

---

## 6. PowerModel — Per-Class Energy Estimation

**File:** `src/analysis/power.rs`
**Phase:** 3

Derives estimated energy consumption from `InsnMix` counts and per-class energy weights.
Not cycle-accurate; designed for microarchitectural trend analysis and relative comparison
between workloads or configurations.

### Default Energy Weights

```rust
/// Per-class energy weight table (nanojoules per instruction).
///
/// Calibrated approximately against published Cortex-A55 power characterizations
/// at 1 GHz, 0.8 V nominal. Intended for relative comparison only.
/// Override via `PowerModel::with_weights()`.
pub const DEFAULT_WEIGHTS_NJ: [f64; InsnClass::COUNT] = [
    0.10,   // IntAlu   — simple integer, minimum energy
    0.25,   // IntMul   — multiplier pipeline
    0.60,   // IntDiv   — iterative divider
    0.20,   // Load     — L1D hit assumed
    0.20,   // Store    — write buffer, L1D
    0.08,   // Branch   — branch predictor + redirect
    0.30,   // FpScalar — FP pipeline, 1 operand width
    0.80,   // FpVector — wide SIMD execution
    0.70,   // LdstSimd — wide SIMD load/store
    0.40,   // Atomic   — coherence overhead
    0.05,   // Sysreg   — register access
    0.05,   // Barrier  — pipeline fence
    0.03,   // Other    — nop, hint, etc.
];
```

### `PowerModel` Struct

```rust
/// Simple per-class energy model.
///
/// Reads instruction counts from an `InsnMix` at query time. No hot-path
/// overhead — derived metric, not a probe subscription.
pub struct PowerModel {
    weights_nj: [f64; InsnClass::COUNT],
}

impl PowerModel {
    pub fn new() -> Self { Self { weights_nj: DEFAULT_WEIGHTS_NJ } }

    pub fn with_weights(weights: [f64; InsnClass::COUNT]) -> Self {
        Self { weights_nj: weights }
    }

    /// Total estimated energy in nanojoules for all instructions in `mix`.
    pub fn total_energy_nj(&self, mix: &InsnMix) -> f64 {
        (0..InsnClass::COUNT)
            .map(|i| {
                let count = mix.counts[i].load(Ordering::Relaxed);
                count as f64 * self.weights_nj[i]
            })
            .sum()
    }

    /// Estimated average power in milliwatts given `sim_ns` simulated nanoseconds.
    ///
    /// Derivation: Power (mW) = Energy (nJ) / Time (ns) — units work out directly.
    /// Returns 0.0 if `sim_ns == 0`.
    pub fn power_mw(&self, mix: &InsnMix, sim_ns: u64) -> f64 {
        if sim_ns == 0 { return 0.0; }
        self.total_energy_nj(mix) / sim_ns as f64
    }

    /// Full breakdown: `(class, count, weight_nj, total_nj, pct_of_total_energy)`.
    pub fn breakdown(&self, mix: &InsnMix) -> Vec<(InsnClass, u64, f64, f64, f64)> {
        let total_nj = self.total_energy_nj(mix);
        (0..InsnClass::COUNT)
            .map(|i| {
                let count  = mix.counts[i].load(Ordering::Relaxed);
                let energy = count as f64 * self.weights_nj[i];
                let pct    = if total_nj > 0.0 { energy / total_nj * 100.0 } else { 0.0 };
                (InsnClass::from_repr(i), count, self.weights_nj[i], energy, pct)
            })
            .collect()
    }
}
```

---

## 7. DiffAnalysis — Session Differential

**File:** `src/analysis/diff.rs`
**Phase:** 3

Compares two `SpySnapshot` instances captured at different points in time or from
different simulation configurations. Used for A/B analysis: "how does branch predictor
accuracy change when doubling L1D size?"

### Types

```rust
/// A single counter delta between two snapshots.
#[derive(Debug, Clone)]
pub struct CounterDelta {
    pub name:       String,
    pub a:          u64,
    pub b:          u64,
    pub delta:      i64,    // b - a (signed)
    pub pct_change: f64,    // (b - a) / a × 100; f64::INFINITY if a == 0
}

/// Top-N changed PC entries from two HeatMap snapshots.
#[derive(Debug, Clone)]
pub struct HeatMapDelta {
    pub name:        String,
    /// PCs with the largest absolute count increase from A to B.
    pub top_gainers: Vec<(u64, i64)>,   // (pc, count_delta)
    /// PCs with the largest absolute count decrease from A to B.
    pub top_losers:  Vec<(u64, i64)>,
}

/// Complete differential report between two `SpySnapshot` instances.
#[derive(Debug, Clone)]
pub struct DiffReport {
    /// Per-field counter deltas (insn_count, etc.).
    pub counters:       Vec<CounterDelta>,
    /// Per-InsnClass instruction count deltas.
    pub insn_mix:       Vec<CounterDelta>,
    /// Cache hit rate delta (milli-percent units internally).
    pub cache:          Option<CounterDelta>,
    /// Branch predictor miss rate delta.
    pub branch:         Option<CounterDelta>,
    /// HeatMap top-N deltas (hot_pcs, branch_heatmap).
    pub heatmaps:       Vec<HeatMapDelta>,
}
```

### `diff()` Function

```rust
/// Compare snapshot `a` (baseline) against snapshot `b` (candidate).
///
/// Fields present in `b` but not `a` have delta = b value.
/// Fields present in `a` but not `b` have delta = -(a value).
pub fn diff(a: &SpySnapshot, b: &SpySnapshot) -> DiffReport {
    let counters = vec![
        counter_delta("insn_count", a.insn_count, b.insn_count),
    ];

    let insn_mix: Vec<CounterDelta> = (0..InsnClass::COUNT)
        .map(|i| counter_delta(
            &format!("{:?}", InsnClass::from_repr(i)),
            a.insn_mix[i],
            b.insn_mix[i],
        ))
        .collect();

    // Rates are stored as millipercent for integer comparison.
    let cache = match (a.cache_hit_rate, b.cache_hit_rate) {
        (Some(ar), Some(br)) => Some(CounterDelta {
            name: "cache_l1d.hit_rate".into(),
            a: (ar * 1000.0) as u64,
            b: (br * 1000.0) as u64,
            delta: ((br - ar) * 1000.0) as i64,
            pct_change: if ar > 0.0 { (br - ar) / ar * 100.0 } else { f64::INFINITY },
        }),
        _ => None,
    };

    let branch = match (a.branch_miss_rate, b.branch_miss_rate) {
        (Some(ar), Some(br)) => Some(CounterDelta {
            name: "branch_pred.miss_rate".into(),
            a: (ar * 1000.0) as u64,
            b: (br * 1000.0) as u64,
            delta: ((br - ar) * 1000.0) as i64,
            pct_change: if ar > 0.0 { (br - ar) / ar * 100.0 } else { f64::INFINITY },
        }),
        _ => None,
    };

    DiffReport { counters, insn_mix, cache, branch, heatmaps: Vec::new() }
}

fn counter_delta(name: &str, a: u64, b: u64) -> CounterDelta {
    let delta = b as i64 - a as i64;
    let pct_change = if a > 0 { delta as f64 / a as f64 * 100.0 } else { f64::INFINITY };
    CounterDelta { name: name.into(), a, b, delta, pct_change }
}
```

---

## 8. IntervalHistogram

**File:** `src/primitives/interval_histogram.rs`

Captures how a scalar measurement varies over simulation time. A plain histogram loses
phase information; `IntervalHistogram` retains the distribution shape of the time-varying
signal.

### Definition

```rust
/// Collect a scalar measurement every `window_size` instructions; bucket the measurements.
///
/// Example: IPC distribution over 1000-instruction intervals.
pub struct IntervalHistogram {
    name:         String,
    window_size:  u64,
    hist:         Histogram,      // distribution of per-window accumulations
    window_accum: AtomicU64,      // accumulator within current window
    last_window:  AtomicU64,      // window index at last boundary crossing
}

impl IntervalHistogram {
    pub fn new(
        name: impl Into<String>,
        window_size: u64,
        edges: Vec<u64>,
    ) -> Self {
        assert!(window_size > 0, "window_size must be non-zero");
        Self {
            name: name.into(),
            window_size,
            hist: Histogram::new(edges),
            window_accum: AtomicU64::new(0),
            last_window:  AtomicU64::new(0),
        }
    }

    /// Call once per instruction with an increment value and the global instruction count.
    ///
    /// Records a histogram sample when a window boundary is crossed.
    #[inline]
    pub fn tick_with(&self, value: u64, insn_count: u64) {
        let window = insn_count / self.window_size;
        let prev   = self.last_window.load(Ordering::Relaxed);
        if window != prev {
            let sample = self.window_accum.swap(value, Ordering::Relaxed);
            self.hist.record(sample);
            self.last_window.store(window, Ordering::Relaxed);
        } else {
            self.window_accum.fetch_add(value, Ordering::Relaxed);
        }
    }

    pub fn counts(&self)       -> Vec<u64> { self.hist.counts() }
    pub fn approx_mean(&self)  -> f64      { self.hist.approx_mean() }
    pub fn percentile(&self, p: f64) -> f64 { self.hist.percentile(p) }
}
```

---

## 9. CorrelHist2D

**File:** `src/primitives/correl_hist_2d.rs`
**Phase:** 3

Answers joint-distribution questions: "D-cache miss rate conditioned on branch predictor
state". Not achievable with independent counters.

```rust
/// 2D joint histogram. Flat array layout for cache locality.
pub struct CorrelHist2D {
    name:    String,
    edges_x: Vec<u64>,
    edges_y: Vec<u64>,
    counts:  Vec<AtomicU64>,   // flat [x_buckets * y_buckets]
}

impl CorrelHist2D {
    pub fn new(name: impl Into<String>, edges_x: Vec<u64>, edges_y: Vec<u64>) -> Self {
        let nx = edges_x.len() + 1;
        let ny = edges_y.len() + 1;
        Self {
            name: name.into(), edges_x, edges_y,
            counts: (0..nx * ny).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    #[inline]
    pub fn record(&self, x: u64, y: u64) {
        let bx = self.edges_x.partition_point(|&e| x >= e);
        let by = self.edges_y.partition_point(|&e| y >= e);
        let ny = self.edges_y.len() + 1;
        self.counts[bx * ny + by].fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Vec<Vec<u64>> {
        let ny = self.edges_y.len() + 1;
        let nx = self.edges_x.len() + 1;
        (0..nx).map(|bx| {
            (0..ny).map(|by| self.counts[bx * ny + by].load(Ordering::SeqCst)).collect()
        }).collect()
    }
}
```

---

## 10. TraceRing

**File:** `src/primitives/trace_ring.rs`

Lock-free, fixed-capacity, single-producer ring buffer for typed event records. Zero heap
allocation after construction. Replaces the `sim_trace` string log channel for typed trace
data.

```rust
/// Lock-free single-producer ring. N must be a power of 2.
/// T must be Copy (no drop on write, no heap allocation).
pub struct TraceRing<T: Copy + Send, const N: usize> {
    buf:  Box<[MaybeUninit<T>; N]>,
    head: AtomicU64,   // write cursor — only the producer touches this
    tail: AtomicU64,   // read cursor — only the consumer touches this
}

impl<T: Copy + Send, const N: usize> TraceRing<T, N> {
    const _ASSERT_POW2: () = assert!(N > 0 && N.is_power_of_two());

    pub fn new() -> Self {
        Self {
            buf:  Box::new(unsafe { MaybeUninit::uninit().assume_init() }),
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
        }
    }

    /// Write one record. Overwrites oldest if full (lossy).
    #[inline(always)]
    pub fn push(&self, val: T) {
        let h = self.head.load(Ordering::Relaxed);
        let slot = (h as usize) & (N - 1);
        unsafe { (self.buf[slot].as_ptr() as *mut T).write(val); }
        self.head.store(h.wrapping_add(1), Ordering::Release);
    }

    /// Drain all available records into `out`. Consumer side only.
    pub fn drain_into(&self, out: &mut Vec<T>) {
        let head = self.head.load(Ordering::Acquire);
        let mut tail = self.tail.load(Ordering::Relaxed);
        while tail != head {
            let slot = (tail as usize) & (N - 1);
            out.push(unsafe { self.buf[slot].assume_init_read() });
            tail = tail.wrapping_add(1);
        }
        self.tail.store(tail, Ordering::Relaxed);
    }

    /// Number of unconsumed records (approximate — producer may advance concurrently).
    pub fn len(&self) -> usize {
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Relaxed);
        h.wrapping_sub(t) as usize
    }
}
```

### Canonical Branch Trace Record

```rust
/// 32 bytes — two cache lines. `repr(C)` for Python `mmap` + `struct.unpack`.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct BranchRecord {
    pub pc:         u64,
    pub target:     u64,
    pub insn_count: u64,
    /// Bit 0: taken; bit 1: predicted; bits 2–4: kind; bits 5–7: spare.
    pub flags:      u8,
    pub _pad:       [u8; 7],
}

impl BranchRecord {
    pub fn taken(&self)     -> bool { self.flags & 1 != 0 }
    pub fn predicted(&self) -> bool { self.flags & 2 != 0 }
    pub fn kind(&self)      -> u8   { (self.flags >> 2) & 0x7 }
}
```

At 32 bytes/record, a 1M-entry ring = 32 MB (fits in L3). A background drain thread
writes to a typed binary file; Python reads with `mmap` + `struct.unpack_from` — no text
parsing.

---

## 11. Trigger System

**File:** `src/analysis/trigger.rs`

Enables Region-of-Interest (ROI) analysis, warmup skip, and phase-conditional collection.
Triggers are checked in the `pre_step` probe callback — before each instruction executes.

```rust
pub enum TriggerKind {
    AtInsn(u64),                          // fire once when insn_count == N
    EveryN(u64),                          // fire every N instructions
    AtPc(u64),                            // fire when PC == addr
    PcRange(u64, u64),                    // fire when PC in [start, end)
    CounterReaches(Arc<Counter>, u64),    // fire when counter.value() >= N
}

pub struct TriggerCtx {
    pub pc:         u64,
    pub insn_count: u64,
}

/// A single conditional trigger.
///
/// Hot-loop cost (disarmed): one `AtomicBool::load(Relaxed)` ≈ 0.5 ns.
/// Hot-loop cost (armed, not fired): one comparison ≈ 1 ns.
/// Action must not block — for heavy I/O, post to a channel from the action.
pub struct Trigger {
    kind:     TriggerKind,
    action:   Box<dyn Fn(&TriggerCtx) + Send + Sync>,
    armed:    AtomicBool,
    one_shot: bool,
}

impl Trigger {
    pub fn new(kind: TriggerKind, action: impl Fn(&TriggerCtx) + Send + Sync + 'static, one_shot: bool) -> Self {
        Self { kind, action: Box::new(action), armed: AtomicBool::new(true), one_shot }
    }

    pub fn at_insn(n: u64, f: impl Fn(&TriggerCtx) + Send + Sync + 'static) -> Self {
        Self::new(TriggerKind::AtInsn(n), f, true)
    }

    pub fn every_n(n: u64, f: impl Fn(&TriggerCtx) + Send + Sync + 'static) -> Self {
        Self::new(TriggerKind::EveryN(n), f, false)
    }

    pub fn at_pc(addr: u64, f: impl Fn(&TriggerCtx) + Send + Sync + 'static) -> Self {
        Self::new(TriggerKind::AtPc(addr), f, false)
    }

    /// Check this trigger. Returns true if it fired.
    #[inline]
    pub fn check(&self, pc: u64, insn_count: u64) -> bool {
        if !self.armed.load(Ordering::Relaxed) { return false; }
        let fired = match &self.kind {
            TriggerKind::AtInsn(n)           => insn_count == *n,
            TriggerKind::EveryN(n)           => *n > 0 && insn_count % n == 0,
            TriggerKind::AtPc(addr)          => pc == *addr,
            TriggerKind::PcRange(s, e)       => pc >= *s && pc < *e,
            TriggerKind::CounterReaches(c, n) => c.value() >= *n,
        };
        if fired {
            if self.one_shot { self.armed.store(false, Ordering::Relaxed); }
            (self.action)(&TriggerCtx { pc, insn_count });
        }
        fired
    }

    pub fn disarm(&self) { self.armed.store(false, Ordering::Relaxed); }
    pub fn rearm(&self)  { self.armed.store(true,  Ordering::Relaxed); }
    pub fn is_armed(&self) -> bool { self.armed.load(Ordering::Relaxed) }
}
```

---

## 12. Window

**File:** `src/analysis/window.rs`

Gates any analysis primitive to fire only within a specified instruction-count range.
Replaces the `trace_after` concept from `helm-plugin`.

```rust
/// Active only during instruction counts in `[start, end)`.
pub struct Window {
    pub start: u64,
    pub end:   u64,
    active:    AtomicBool,
}

impl Window {
    pub fn new(start: u64, end: u64) -> Self {
        assert!(start < end, "window start must be less than end");
        Self { start, end, active: AtomicBool::new(false) }
    }

    /// Returns true if `insn_count` is in `[start, end)`. Updates the `active` flag.
    #[inline]
    pub fn is_active(&self, insn_count: u64) -> bool {
        let a = insn_count >= self.start && insn_count < self.end;
        self.active.store(a, Ordering::Relaxed);
        a
    }

    /// Return the cached active flag (result of the last `is_active()` call).
    pub fn active(&self) -> bool { self.active.load(Ordering::Relaxed) }
}
```

---

## 13. Per-vCPU Scoreboard Pattern

The Scoreboard pattern provides per-vCPU local analysis state with zero cross-thread
synchronization on the hot path. Merging happens once at `quantum_end`.

### `Scoreboard<T>` (in `src/scoreboard.rs`)

```rust
/// Per-vCPU slot storage.
///
/// Each vCPU index has exactly one slot of type `T`. The hot loop holds a
/// single-threaded reference to its own slot (`slot(vcpu: usize) -> &mut T`).
/// At `quantum_end`, all slots are aggregated into a global summary.
pub struct Scoreboard<T> {
    slots: Vec<T>,
}

impl<T: Default> Scoreboard<T> {
    pub fn new(vcpu_count: usize) -> Self {
        Self { slots: (0..vcpu_count).map(|_| T::default()).collect() }
    }

    /// Exclusive access to one vCPU's slot.
    ///
    /// SAFETY contract: only one OS thread holds a reference to `slot(vcpu)` at
    /// a time. Enforced by the engine: each vCPU quantum runs on exactly one OS
    /// thread, and `quantum_end` is a global barrier.
    pub fn slot(&mut self, vcpu: usize) -> &mut T {
        &mut self.slots[vcpu]
    }

    /// Read-only access to all slots (for aggregation at `quantum_end`).
    pub fn all_slots(&self) -> &[T] { &self.slots }
}
```

### `CacheModel` per-vCPU Usage

```rust
// Declaration in SpySession:
cache_l1d_sb: Option<Scoreboard<CacheModel>>,

// In subscribe(): one probe closure per vCPU that accesses its Scoreboard slot.
// (Implemented inside SpySession::subscribe(); see §15 for full wiring.)

// At quantum_end: aggregate slot stats into the summary CacheModel.
impl SpySession {
    pub fn quantum_end(&mut self, vcpu: usize, insn_count: u64) {
        if let Some(sb) = &self.cache_l1d_sb {
            if let Some(summary) = &self.cache_l1d {
                let slot = &sb.all_slots()[vcpu];
                summary.lock().unwrap().merge_stats(slot);
            }
        }
        if let Some(sb) = &self.branch_pred_sb {
            if let Some(summary) = &self.branch_pred {
                let slot = &sb.all_slots()[vcpu];
                summary.lock().unwrap().merge_stats(slot);
            }
        }
    }
}
```

### `BranchPredictor` per-vCPU Usage

```rust
// For multi-vCPU: one Scoreboard slot per vCPU.
// In subscribe() for vCPU i:
let slot_ptr = scoreboard.slot(i) as *mut BranchPredictor;
probes[i].branch.subscribe(move |ev: &BranchEvent| {
    // SAFETY: hot loop is single-threaded per vCPU; no other thread touches this slot.
    unsafe { (*slot_ptr).predict(ev.pc, ev.taken); }
});
// Note: table state stays per-vCPU; only prediction/misprediction counts are merged.
```

### Why Not `Arc<Mutex<T>>`

`Arc<Mutex<CacheModel>>::lock()` inside a probe callback acquires a lock on every data
memory access — potentially 10–50% of all instructions. At 500 MHz simulation speed,
this adds ~2 µs of contention per instruction cluster. The Scoreboard pattern eliminates
all hot-path synchronization. The only synchronization is at `quantum_end`, once per
`run()` call.

---

## 14. SpySession — Complete Definition

**File:** `src/session.rs`

```rust
use std::sync::{Arc, Mutex};
use helm_probe::{CpuProbes, CpuFaultEvent};
use crate::primitives::{Counter, HeatMap, RingBuffer, EventStream, Trigger, Window};
use crate::analysis::{InsnMix, CacheModel, BranchPredictor, SimPoint};
use crate::scoreboard::Scoreboard;

/// Enriched instruction info (post-step event with class, name, vcpu context).
/// Moved from `helm-plugin::InsnInfo` to `helm-spy::events`.
#[derive(Debug, Clone)]
pub struct InsnInfo {
    pub pc:          u64,
    pub raw:         u32,
    pub size:        u8,
    pub class:       helm_engine::InsnClass,
    pub opcode_name: &'static str,
    pub is_stub:     bool,
    pub vcpu_idx:    usize,
}

/// A complete observation configuration for one simulation run.
///
/// Created via `HelmSim::observe()` (Python) or `SpySession::new()` (Rust).
/// Wired to probe bundles via `subscribe()`. Data collected during `run()`.
/// Delivery is separate: call `session.report(sink, format)` after `run()`.
///
/// # Pinning Contract
///
/// After `subscribe()` is called, `SpySession` must not be moved. Probe
/// closures hold raw pointers into the session fields. Use `Box::pin(...)`.
pub struct SpySession {
    pub name: String,

    // ── Core counters (always enabled) ─────────────────────────────────────
    /// Total instructions retired. Incremented on every `post_step` probe.
    pub insn_count: Counter,

    /// Instruction class distribution.
    pub insn_mix: InsnMix,

    /// Hot PC heatmap. Maps PC → retire count.
    pub hot_pcs: HeatMap,

    /// Branch target heatmap. Maps branch target PC → taken count.
    pub branch_heatmap: HeatMap,

    // ── Optional analysis models ───────────────────────────────────────────
    /// L1 data cache model. `None` unless `track_memory()` is called.
    pub cache_l1d: Option<Arc<Mutex<CacheModel>>>,

    /// Branch predictor model. `None` unless `track_branches()` is called.
    pub branch_pred: Option<Arc<Mutex<BranchPredictor>>>,

    // ── Trace capture ──────────────────────────────────────────────────────
    /// Ring buffer of recent handled CPU faults (TLB misses, SVC, aborts).
    pub fault_history: RingBuffer<CpuFaultEvent>,

    /// Optional bounded instruction event stream with full `InsnInfo` context.
    /// `None` by default; enabled via `enable_exec_stream(max_events)`.
    pub exec_stream: Option<EventStream<InsnInfo>>,

    // ── Conditional collection ─────────────────────────────────────────────
    /// Triggers checked on every `pre_step`. Actions must not block.
    pub triggers: Vec<Trigger>,

    /// Time windows. Primitives inside a window only record when active.
    pub windows: Vec<Window>,

    // ── Phase 3 models ─────────────────────────────────────────────────────
    /// SimPoint BBV collector. `None` unless `track_simpoint()` is called.
    pub simpoint: Option<Arc<Mutex<SimPoint>>>,

    // ── Per-vCPU Scoreboard slots (internal) ──────────────────────────────
    cache_l1d_sb:   Option<Scoreboard<CacheModel>>,
    branch_pred_sb: Option<Scoreboard<BranchPredictor>>,
}

impl SpySession {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name:           name.into(),
            insn_count:     Counter::new("insn_count"),
            insn_mix:       InsnMix::new("insn_mix"),
            hot_pcs:        HeatMap::new("hot_pcs"),
            branch_heatmap: HeatMap::new("branch_heatmap"),
            cache_l1d:      None,
            branch_pred:    None,
            fault_history:  RingBuffer::new("fault_history", 128),
            exec_stream:    None,
            triggers:       Vec::new(),
            windows:        Vec::new(),
            simpoint:       None,
            cache_l1d_sb:   None,
            branch_pred_sb: None,
        }
    }

    /// Builder: enable L1D cache simulation.
    pub fn with_cache_l1d(mut self, sets: usize, ways: usize, line_size: usize) -> Self {
        self.cache_l1d = Some(Arc::new(Mutex::new(CacheModel::new("l1d", sets, ways, line_size))));
        self
    }

    /// Builder: enable branch predictor simulation.
    pub fn with_branch_predictor(mut self, kind: PredictorKind) -> Self {
        self.branch_pred = Some(Arc::new(Mutex::new(BranchPredictor::new("branch_pred", kind))));
        self
    }

    /// Enable full instruction event stream capture.
    pub fn enable_exec_stream(&mut self, max_events: usize) {
        self.exec_stream = Some(EventStream::new("exec_stream", max_events));
    }

    /// Enable SimPoint BBV collection.
    pub fn enable_simpoint(&mut self, interval: u64) {
        self.simpoint = Some(Arc::new(Mutex::new(SimPoint::new(interval))));
    }

    /// Add a trigger to the session. Must be called before `subscribe()`.
    pub fn add_trigger(&mut self, t: Trigger) { self.triggers.push(t); }

    /// Add a time window to the session. Must be called before `subscribe()`.
    pub fn add_window(&mut self, w: Window) { self.windows.push(w); }
}
```

---

## 15. subscribe() — Probe Wiring Method

`subscribe()` connects all configured primitives to the probe bundles. It must be called
exactly once, after all primitives are configured and before the first `run()`.

```rust
impl SpySession {
    /// Wire all configured primitives to the given probe bundles.
    ///
    /// Must be called exactly once. After this call, `SpySession` must not
    /// be moved (probe closures hold raw pointers into self).
    #[cfg(debug_assertions)]
    pub fn subscribe(&mut self, probes: &mut CpuProbes) {
        // 1. insn_count: one fetch_add per retired instruction.
        {
            let ptr = &self.insn_count as *const Counter;
            probes.post_step.subscribe(move |_ev| {
                unsafe { (*ptr).inc(); }
            });
        }

        // 2. insn_mix: classify opcode and increment the class bucket.
        self.insn_mix.subscribe_to_steps(probes);

        // 3. hot_pcs: track retire PC frequency.
        {
            let ptr = &self.hot_pcs as *const HeatMap;
            probes.post_step.subscribe(move |ev| {
                unsafe { (*ptr).inc(ev.pc); }
            });
        }

        // 4. branch_heatmap: track taken branches by target PC.
        {
            let ptr = &self.branch_heatmap as *const HeatMap;
            probes.branch.subscribe(move |ev| {
                if ev.taken { unsafe { (*ptr).inc(ev.target); } }
            });
        }

        // 5. cache_l1d: subscribe to memory access probe.
        if let Some(cache) = &self.cache_l1d {
            let c = Arc::clone(cache);
            probes.mem.subscribe(move |ev| {
                if let Ok(mut lk) = c.try_lock() { lk.access(ev.addr); }
            });
        }

        // 6. branch_pred: subscribe to branch probe via Mutex wrapper.
        if let Some(pred) = &self.branch_pred {
            BranchPredictor::subscribe_to_branches(pred, probes);
        }

        // 7. fault_history: push fault events into the ring buffer.
        {
            let ptr = &self.fault_history as *const RingBuffer<CpuFaultEvent>;
            probes.fault.subscribe(move |ev| {
                unsafe { (*ptr).push(ev.clone()); }
            });
        }

        // 8. exec_stream: capture full InsnInfo if enabled.
        if let Some(stream) = &self.exec_stream {
            let ptr = stream as *const EventStream<InsnInfo>;
            probes.post_step.subscribe(move |ev| {
                let info = InsnInfo {
                    pc: ev.pc, raw: ev.raw, size: 4,
                    class: helm_engine::classify_aarch64_opcode(ev.raw),
                    opcode_name: helm_engine::opcode_name(ev.raw),
                    is_stub: helm_engine::is_stub_opcode(ev.raw),
                    vcpu_idx: 0,  // single-vCPU; multi-vCPU sets this per-vCPU
                };
                unsafe { (*ptr).push(info); }
            });
        }

        // 9. triggers: checked on pre_step before instruction executes.
        if !self.triggers.is_empty() {
            let tptr      = self.triggers.as_ptr();
            let tlen      = self.triggers.len();
            let iptr      = &self.insn_count as *const Counter;
            probes.pre_step.subscribe(move |ev| {
                let n = unsafe { (*iptr).value() };
                for i in 0..tlen {
                    unsafe { (*tptr.add(i)).check(ev.pc, n); }
                }
            });
        }

        // 10. simpoint: subscribe to branch events.
        if let Some(sp) = &self.simpoint {
            SimPoint::subscribe_to_branches(sp, probes);
        }
    }
}
```

### Subscription Order Invariants

1. `insn_count` is subscribed before `exec_stream` and `triggers` — all use the count.
2. `triggers` are on `pre_step`; `insn_count`/`insn_mix`/`hot_pcs` are on `post_step`.
   `pre_step` fires before the instruction executes; `post_step` fires after.
3. `fault_history` uses `probes.fault` — independent ordering from `post_step`.
4. `branch_heatmap` and `branch_pred` use `probes.branch` — independent from `post_step`.

---

## 16. SpySnapshot — Differential Baseline

**File:** `src/session.rs`

A `SpySnapshot` is a frozen point-in-time copy of an `SpySession`. It is a pure
value type (no pointers to the live session) used for differential analysis and for
Python-side mid-simulation queries without locking the session for long.

```rust
/// A frozen snapshot of an `SpySession` at a specific instruction count.
///
/// Created by `SpySession::snapshot()`. Cheap to clone (all primitive values).
#[derive(Debug, Clone)]
pub struct SpySnapshot {
    /// Instruction count at snapshot time.
    pub insn_count: u64,

    /// Per-class instruction counts at snapshot time.
    pub insn_mix: [u64; InsnClass::COUNT],

    /// Cache hit rate. `None` if cache was not configured.
    pub cache_hit_rate: Option<f64>,

    /// Cache MPKI at snapshot time.
    pub cache_mpki: Option<f64>,

    /// Raw cache hit count.
    pub cache_hits: Option<u64>,

    /// Raw cache miss count.
    pub cache_misses: Option<u64>,

    /// Branch predictor miss rate. `None` if not configured.
    pub branch_miss_rate: Option<f64>,

    /// Branch predictor MPKI.
    pub branch_mpki: Option<f64>,

    /// Fault events in the ring buffer at snapshot time.
    pub fault_history: Vec<CpuFaultEvent>,

    /// Top-20 hot PCs at snapshot time.
    pub hot_pcs_top20: Vec<(u64, u64)>,
}

impl SpySession {
    /// Capture a point-in-time snapshot. Thread-safe (uses SeqCst reads on atomics).
    pub fn snapshot(&self) -> SpySnapshot {
        let insn_count = self.insn_count.value();
        let insn_mix: [u64; InsnClass::COUNT] =
            std::array::from_fn(|i| self.insn_mix.counts[i].load(Ordering::SeqCst));

        let (cache_hit_rate, cache_mpki, cache_hits, cache_misses) =
            self.cache_l1d.as_ref()
                .and_then(|c| c.lock().ok())
                .map(|c| (Some(c.hit_rate()), Some(c.mpki(insn_count)), Some(c.hits()), Some(c.misses())))
                .unwrap_or((None, None, None, None));

        let (branch_miss_rate, branch_mpki) =
            self.branch_pred.as_ref()
                .and_then(|p| p.lock().ok())
                .map(|p| (Some(p.miss_rate()), Some(p.mpki(insn_count))))
                .unwrap_or((None, None));

        SpySnapshot {
            insn_count,
            insn_mix,
            cache_hit_rate,
            cache_mpki,
            cache_hits,
            cache_misses,
            branch_miss_rate,
            branch_mpki,
            fault_history: self.fault_history.snapshot(),
            hot_pcs_top20: self.hot_pcs.top(20),
        }
    }
}
```

---

## 17. PyO3 Exposure

**File:** `runtime/helm-python/src/observe.rs`

The `SpySession` is exposed to Python through `helm-python` as a `#[pyclass]` wrapper.

```rust
use pyo3::prelude::*;
use helm_spy::session::{SpySession, SpySnapshot};
use std::sync::{Arc, Mutex};

/// Python-visible wrapper for `SpySession`.
#[pyclass(name = "SpySession")]
pub struct PySpySession {
    pub inner: Arc<Mutex<SpySession>>,
}

#[pymethods]
impl PySpySession {
    // Configuration (call before run())

    pub fn track_memory(&self, l1d_size: usize, l1d_assoc: usize, line_size: usize) -> PyResult<()> {
        let sets = l1d_size / l1d_assoc / line_size;
        let mut s = self.inner.lock().unwrap();
        let m = CacheModel::new("l1d", sets, l1d_assoc, line_size);
        s.cache_l1d = Some(Arc::new(Mutex::new(m)));
        Ok(())
    }

    pub fn track_branches(&self, predictor: &str) -> PyResult<()> {
        let kind = match predictor {
            "bimodal-4k"  => PredictorKind::BiModal { table_bits: 12 },
            "gshare-4k"   => PredictorKind::GShare  { history_bits: 12, table_bits: 12 },
            "gshare-8k"   => PredictorKind::GShare  { history_bits: 14, table_bits: 14 },
            "perfect"     => PredictorKind::Perfect,
            other => return Err(PyValueError::new_err(format!("unknown predictor: {other}"))),
        };
        self.inner.lock().unwrap().branch_pred =
            Some(Arc::new(Mutex::new(BranchPredictor::new("branch_pred", kind))));
        Ok(())
    }

    pub fn enable_exec_stream(&self, max_events: usize) {
        self.inner.lock().unwrap().enable_exec_stream(max_events);
    }

    pub fn track_simpoint(&self, interval: u64) {
        self.inner.lock().unwrap().enable_simpoint(interval);
    }

    // Scalar accessors (call after run())

    #[getter]
    pub fn insn_count(&self) -> u64 {
        self.inner.lock().unwrap().insn_count.value()
    }

    #[getter]
    pub fn insn_mix(&self, py: Python) -> PyObject {
        let rows = self.inner.lock().unwrap().insn_mix.table();
        rows.into_iter()
            .map(|(cls, count, pct)| (format!("{:?}", cls), count, pct).into_py(py))
            .collect::<Vec<_>>()
            .into_py(py)
    }

    #[getter]
    pub fn cache_l1d(&self, py: Python) -> PyObject {
        match &self.inner.lock().unwrap().cache_l1d {
            Some(arc) => PyCacheModel { inner: Arc::clone(arc) }.into_py(py),
            None => py.None(),
        }
    }

    #[getter]
    pub fn branch_pred(&self, py: Python) -> PyObject {
        match &self.inner.lock().unwrap().branch_pred {
            Some(arc) => PyBranchPredictor { inner: Arc::clone(arc) }.into_py(py),
            None => py.None(),
        }
    }

    pub fn top_pcs(&self, n: usize) -> Vec<(u64, u64)> {
        self.inner.lock().unwrap().hot_pcs.top(n)
    }

    pub fn snapshot(&self) -> PySpySnapshot {
        PySpySnapshot { inner: self.inner.lock().unwrap().snapshot() }
    }
}

/// Proxy for `CacheModel` query methods.
///
/// Python usage:
///   `session.cache_l1d.hit_rate`
///   `session.cache_l1d.miss_rate`
///   `session.cache_l1d.mpki(insn_count)`
///   `session.cache_l1d.hits`
///   `session.cache_l1d.misses`
#[pyclass(name = "CacheModel")]
pub struct PyCacheModel { inner: Arc<Mutex<CacheModel>> }

#[pymethods]
impl PyCacheModel {
    #[getter] pub fn hit_rate(&self)  -> f64 { self.inner.lock().unwrap().hit_rate() }
    #[getter] pub fn miss_rate(&self) -> f64 { self.inner.lock().unwrap().miss_rate() }
    pub fn mpki(&self, insn_count: u64) -> f64 { self.inner.lock().unwrap().mpki(insn_count) }
    #[getter] pub fn hits(&self)   -> u64 { self.inner.lock().unwrap().hits() }
    #[getter] pub fn misses(&self) -> u64 { self.inner.lock().unwrap().misses() }
}

/// Proxy for `BranchPredictor` query methods.
///
/// Python usage:
///   `session.branch_pred.miss_rate`
///   `session.branch_pred.predictions`
///   `session.branch_pred.mispredictions`
///   `session.branch_pred.mpki(insn_count)`
#[pyclass(name = "BranchPredictor")]
pub struct PyBranchPredictor { inner: Arc<Mutex<BranchPredictor>> }

#[pymethods]
impl PyBranchPredictor {
    #[getter] pub fn miss_rate(&self)       -> f64 { self.inner.lock().unwrap().miss_rate() }
    #[getter] pub fn predictions(&self)     -> u64  { self.inner.lock().unwrap().predictions }
    #[getter] pub fn mispredictions(&self)  -> u64  { self.inner.lock().unwrap().mispredictions }
    pub fn mpki(&self, insn_count: u64) -> f64 { self.inner.lock().unwrap().mpki(insn_count) }
}

/// Python wrapper for `SpySnapshot` (used in differential analysis).
#[pyclass(name = "SpySnapshot")]
pub struct PySpySnapshot { inner: SpySnapshot }

#[pymethods]
impl PySpySnapshot {
    #[getter] pub fn insn_count(&self)       -> u64         { self.inner.insn_count }
    #[getter] pub fn cache_hit_rate(&self)   -> Option<f64> { self.inner.cache_hit_rate }
    #[getter] pub fn branch_miss_rate(&self) -> Option<f64> { self.inner.branch_miss_rate }
}
```

### Python Usage

```python
s = sim.spy()
s.track_memory(l1d_size=32768, l1d_assoc=8, line_size=64)
s.track_branches(predictor="gshare-4k")
sim.run(100_000_000)

print(f"Instructions:       {s.insn_count:,}")
print(f"L1D hit rate:       {s.cache_l1d.hit_rate:.2%}")
print(f"L1D MPKI:           {s.cache_l1d.mpki(s.insn_count):.2f}")
print(f"Branch miss rate:   {s.branch_pred.miss_rate:.2%}")
print(f"Branch MPKI:        {s.branch_pred.mpki(s.insn_count):.2f}")

for class_name, count, pct in s.insn_mix:
    print(f"  {class_name:12s}: {count:12,}  ({pct:5.1f}%)")

# Differential analysis
snap_before = s.snapshot()
sim.run(100_000_000)
snap_after  = s.snapshot()

from helm.observe import diff
report = diff(snap_before, snap_after)
for d in report.counters:
    print(f"{d.name}: {d.a} → {d.b}  (Δ {d.delta:+d}, {d.pct_change:+.1f}%)")

# Sweep L1D sizes
for l1d_size in [16*1024, 32*1024, 64*1024]:
    sim.reset()
    s = sim.spy()
    s.track_memory(l1d_size=l1d_size, l1d_assoc=8, line_size=64)
    sim.run(50_000_000)
    print(f"L1D {l1d_size//1024}K: hit={s.cache_l1d.hit_rate:.2%}  mpki={s.cache_l1d.mpki(s.insn_count):.1f}")
```

---

## 18. Module Structure

```
framework/helm-spy/
├── Cargo.toml
└── src/
    ├── lib.rs                   # re-exports all public types
    ├── session.rs               # SpySession, SpySnapshot, subscribe()
    ├── scoreboard.rs            # Scoreboard<T> — per-vCPU slot storage
    ├── events.rs                # Re-exports: InsnClass, BranchKind, InsnInfo
    ├── primitives/
    │   ├── mod.rs
    │   ├── counter.rs           # Counter (AtomicU64 + name)
    │   ├── per_vcpu.rs          # PerVcpuCounter — per-vCPU atomic slots
    │   ├── indexed_counter.rs   # IndexedCounter — fixed-size AtomicU64 array
    │   ├── histogram.rs         # Histogram — lock-free bucket distribution
    │   ├── interval_histogram.rs # IntervalHistogram — time-series distribution
    │   ├── correl_hist_2d.rs    # CorrelHist2D — joint 2D distribution
    │   ├── heatmap.rs           # HeatMap — DashMap<u64, u64> or sharded
    │   ├── ring_buffer.rs       # RingBuffer<T> — Mutex<VecDeque<T>>
    │   ├── event_stream.rs      # EventStream<T> — bounded Mutex<Vec<T>>
    │   └── trace_ring.rs        # TraceRing<T, N> — SPSC lock-free ring + BranchRecord
    └── analysis/
        ├── mod.rs
        ├── insn_mix.rs          # InsnMix — classify + count (Phase 2)
        ├── cache.rs             # CacheModel + CacheResult (Phase 2)
        ├── branch_pred.rs       # BranchPredictor + PredictorKind (Phase 2/3)
        ├── simpoint.rs          # SimPoint + SimPointData (Phase 3)
        ├── power.rs             # PowerModel — per-class energy × count (Phase 3)
        ├── diff.rs              # DiffAnalysis + DiffReport (Phase 3)
        ├── trigger.rs           # Trigger + TriggerKind
        └── window.rs            # Window — instruction-count-gated collection
```

### Crate DAG Position

```
helm-probe        (zero deps; Layer 1)
    │
    └── helm-spy   (deps: helm-probe, helm-engine; Layer 2)
            │    (no dep on helm-report — collection ≠ delivery)
            │
    helm-report    (deps: helm-spy for types; Layer 3)
    helm-engine    (deps: helm-probe, helm-spy, helm-report)
    helm-python    (deps: helm-engine; PyO3 boundary)
```

`helm-spy` does not depend on `helm-debug`, `helm-engine` business logic, or
`helm-report`. All dependencies flow upward from `helm-spy` to consumers.

---

## 19. Design Decisions

### No Mutex in Hot-Loop Probe Callbacks

`Arc<Mutex<T>>::lock()` inside probe callbacks is **banned** in `helm-spy`. It
introduces cross-thread synchronization on the hot path and can cause priority inversion
under multi-vCPU simulation. The canonical pattern:

| Pattern | Use Case |
|---|---|
| `AtomicU64::fetch_add(Relaxed)` | Counters, InsnMix — the standard path |
| `Scoreboard<LocalBuf>` per-vCPU | CacheModel, BranchPredictor in SE multi-vCPU mode |
| `Mutex<T>` locked in `quantum_end()` | CacheModel in FS mode (low event rate) |
| `TraceRing::push()` (lock-free SPSC) | BranchRecord, typed trace capture |

### CacheModel Uses `&mut self`

LRU counter update is an inherently sequential per-access operation. Making `access()`
take `&self` would require interior mutability on every way's LRU counter — up to 64
atomics per access instead of one — which is worse on both performance and cache footprint.
Thread safety is achieved at a higher level: `Scoreboard<CacheModel>` slots or a `Mutex`
locked only at quantum boundaries.

### BranchPredictor Wrapped in `Mutex` for Single-vCPU

The history shift-register update is sequential. The misprediction rate is interesting at
simulation granularity (post-run), not at per-instruction granularity. Locking once per
branch (~5–10% of instruction rate) is acceptable in dev mode. In release, the probe
ZST eliminates the entire path.

### SimPoint Uses `Mutex<SimPoint>`

`SimPoint::on_branch()` takes `&mut self` (HashMap mutation). The branch rate (~10% of
instructions) makes the Mutex acceptable. A Scoreboard-based per-vCPU variant would
require merging `HashMap` state at `quantum_end`, which is a linear-time operation over
the number of unique basic blocks — acceptable but more complex.

### `QuantumObserver` Trait

```rust
/// Notification sent to each observer when a vCPU quantum completes.
/// All cross-vCPU merging of per-vCPU local state happens here — never in callbacks.
pub trait QuantumObserver: Send + Sync {
    fn quantum_end(&mut self, vcpu: usize, insn_count: u64);
}
```

`SpySession` implements `QuantumObserver` and is registered with `HelmEngine` during
`build_simulator()`. Merging per-vCPU `CacheModel` hit/miss counters into the global
aggregate is the canonical `quantum_end` action.

# helm-spy — LLD: Primitives

> **Document:** Low-Level Design — Primitives Layer (`src/primitives/`)
> **Crate:** `framework/helm-spy`
> **See also:** [HLD.md](HLD.md) for architecture overview and Python API

---

## 1. Crate Structure

```
framework/helm-spy/
├── Cargo.toml
└── src/
    ├── lib.rs                   # crate root; pub use re-exports
    ├── events.rs                # InsnInfo, BranchInfo, MemInfo, SyscallInfo, FaultInfo,
    │                            # InsnClass, BranchKind, ArchContext, FaultKind
    ├── quantum.rs               # QuantumObserver trait
    ├── trigger.rs               # TriggerKind, Trigger, TriggerCtx
    ├── window.rs                # Window, Windowed<T>
    ├── session.rs               # SpySession — user-facing aggregator
    ├── bridge.rs                # ProbePluginBridge (moved from helm-plugin)
    └── primitives/
        ├── mod.rs               # pub use of all primitives
        ├── counter.rs           # Counter, IndexedCounter
        ├── per_vcpu.rs          # PerVcpuCounter, Scoreboard<T>
        ├── histogram.rs         # Histogram, IntervalHistogram
        ├── heatmap.rs           # HeatMap (dashmap or sharded fallback)
        ├── ring.rs              # RingBuffer<T>, EventStream<T>
        ├── trace_ring.rs        # TraceRing<T, N>, BranchRecord
        └── correl2d.rs          # CorrelHist2D
```

---

## 2. `Cargo.toml`

```toml
[package]
name        = "helm-spy"
version.workspace = true
edition.workspace = true
description = "Analysis primitives for helm-ng instrumentation (Layer 2)"

[lints]
workspace = true

[features]
# DashMap-backed HeatMap (recommended). Without this feature, HeatMap uses
# a 16-shard Mutex<HashMap> fallback with no external dependencies.
dashmap = ["dep:dashmap"]

[dependencies]
helm-probe.workspace  = true
helm-core.workspace   = true
thiserror.workspace   = true

# Optional: DashMap for HeatMap
dashmap = { version = "6", optional = true }

[dev-dependencies]
# No additional dev deps; integration tests live in tests/ alongside the crate.
```

No dependency on `helm-report`, `helm-plugin`, `helm-debug`, or `helm-engine`.

---

## 3. `src/events.rs` — Event Types

Event types are the richer cousins of `helm-probe` event types. The `ProbePluginBridge`
performs the enrichment step (classification, context, atomic flag).

```rust
// src/events.rs

/// Instruction class for IndexedCounter and mix analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum InsnClass {
    IntAlu   = 0,
    Load     = 1,
    Store    = 2,
    Branch   = 3,
    FpAlu    = 4,
    SimdAlu  = 5,
    System   = 6,
    Nop      = 7,
}

impl InsnClass {
    pub const COUNT: usize = 8;
    pub const LABELS: [&'static str; Self::COUNT] =
        ["IntAlu", "Load", "Store", "Branch", "FpAlu", "SimdAlu", "System", "Nop"];
}

/// Branch type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BranchKind {
    DirectCond    = 0,
    DirectUncond  = 1,
    Call          = 2,
    Return        = 3,
    IndirectJump  = 4,
    IndirectCall  = 5,
}

impl BranchKind {
    pub const COUNT: usize = 6;
    pub const LABELS: [&'static str; Self::COUNT] =
        ["DirectCond", "DirectUncond", "Call", "Return", "IndirectJump", "IndirectCall"];
}

/// Optional full register context. Default is None (zero cost to construct).
/// Aarch64 variant is 256 bytes — use sparingly; only opt-in via probe-full.
#[derive(Debug, Clone, Default)]
pub enum ArchContext {
    #[default]
    None,
    Aarch64 { x: [u64; 31], sp: u64, pc: u64, nzcv: u32, fpsr: u32 },
    Riscv64 { x: [u64; 32], pc: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    InsnAbort, DataAbort, StoreAbort, Svc, Undefined, Other,
}

/// Emitted once per retired instruction. Enriched from CpuStepEvent by ProbePluginBridge.
#[derive(Debug, Clone)]
pub struct InsnInfo {
    pub vcpu_idx:    usize,
    pub pc:          u64,
    pub raw:         u32,
    pub size:        u8,
    pub class:       InsnClass,
    pub opcode_name: &'static str,
    pub is_stub:     bool,
    pub context:     ArchContext,
    pub insn_count:  u64,
}

/// Branch event. Enriched from BranchEvent by ProbePluginBridge (passthrough, already rich).
#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub pc:         u64,
    pub target:     u64,
    pub taken:      bool,
    pub kind:       BranchKind,
    pub insn_count: u64,
}

/// Memory access event. Enriched from MemAccessEvent; is_atomic from AccessType.
#[derive(Debug, Clone)]
pub struct MemInfo {
    pub vaddr:     u64,
    pub size:      u8,
    pub is_store:  bool,
    pub is_atomic: bool,
    pub pc:        u64,
}

/// Syscall entry.
#[derive(Debug, Clone)]
pub struct SyscallInfo {
    pub vcpu_idx: usize,
    pub nr:       u64,
    pub args:     [u64; 6],
    pub pc:       u64,
}

/// Syscall return.
#[derive(Debug, Clone)]
pub struct SyscallRetInfo {
    pub vcpu_idx: usize,
    pub nr:       u64,
    pub retval:   i64,
}

/// Fault / exception delivery. Enriched from CpuFaultEvent by ProbePluginBridge.
#[derive(Debug, Clone)]
pub struct FaultInfo {
    pub vcpu_idx:   usize,
    pub pc:         u64,
    pub raw:        u32,
    pub kind:       FaultKind,
    pub message:    String,
    pub insn_count: u64,
    pub context:    ArchContext,
}
```

---

## 4. `src/quantum.rs` — `QuantumObserver` Trait

```rust
// src/quantum.rs

/// Implemented by any primitive or aggregate that needs to finalize
/// per-vCPU local state after a vCPU quantum ends.
///
/// # Contract
///
/// - Called by the engine at every `run()` return, for each vCPU that ran.
/// - Called before every `checkpoint_save()`.
/// - The hot loop is NOT executing when this is called.
/// - This method MAY allocate, acquire Mutex locks, or do I/O.
/// - MUST NOT be called from inside a probe callback.
pub trait QuantumObserver: Send + Sync {
    fn quantum_end(&mut self, vcpu: usize, insn_count: u64);
}
```

### 4.1 When `quantum_end()` is called

```
HelmEngine::run(n) {
    loop {
        step_aarch64_fs(...)
    }
    // After the loop:
    self.probes.post_step   ← already fired per instruction
    // Now call:
    self.observe_session.quantum_end(vcpu=0, insn_count=self.insn_count)
}
```

`SpySession::quantum_end()` dispatches to all registered `QuantumObserver`
implementations in the order they were registered. Order is not guaranteed to be
stable across sessions, so observers must not depend on relative ordering.

### 4.2 Which primitives implement `QuantumObserver`

| Primitive | Needs `QuantumObserver`? | Reason |
|---|---|---|
| `Counter` | No | `AtomicU64` is globally visible without a merge step |
| `IndexedCounter` | No | Same — per-slot atomics |
| `PerVcpuCounter` | No | `total()` aggregates Scoreboard slots at query time |
| `Histogram` | No | Per-bucket atomics; no per-vCPU local state |
| `IntervalHistogram` | Yes (if per-vCPU local) | Window accumulator per vCPU; merge at quantum end |
| `HeatMap` | Yes (if local-merge variant) | Per-vCPU local HashMap flushed into DashMap |
| `RingBuffer<T>` | No | Mutex is always global; no per-vCPU state |
| `EventStream<T>` | No | Same |
| `TraceRing<T, N>` | No | Consumer drains independently; SPSC has no per-vCPU aggregate |
| `CorrelHist2D` | No | Per-bucket atomics |

---

## 5. `src/primitives/counter.rs` — `Counter` and `IndexedCounter`

### 5.1 `Counter`

```rust
// src/primitives/counter.rs

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Monotonic 64-bit counter. Thread-safe via AtomicU64.
///
/// All operations use `Ordering::Relaxed` — correct for independent counters
/// where only the final value matters, not relative ordering with other variables.
pub struct Counter {
    name:  String,
    value: AtomicU64,
}

impl Counter {
    pub fn new(name: impl Into<String>) -> Arc<Self> {
        Arc::new(Self { name: name.into(), value: AtomicU64::new(0) })
    }

    /// Increment by 1. Lock-free, no allocation. Hot-path safe.
    #[inline(always)]
    pub fn inc(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment by n. Lock-free, no allocation. Hot-path safe.
    #[inline(always)]
    pub fn add(&self, n: u64) {
        self.value.fetch_add(n, Ordering::Relaxed);
    }

    /// Read the current value. Uses Relaxed — caller is responsible for
    /// appropriate synchronization before reading (quantum boundary is sufficient).
    #[inline]
    pub fn value(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    pub fn name(&self) -> &str { &self.name }

    /// Reset to zero. Cold path only — not safe to call while hot loop runs.
    pub fn reset(&self) {
        self.value.store(0, Ordering::Relaxed);
    }
}

// SAFETY: AtomicU64 is Send + Sync; String is Send + Sync.
// The derive-based Send/Sync is correct here.
```

#### Subscribe pattern

```rust
impl Counter {
    /// Subscribe to CpuProbes.post_step — increment once per retired instruction.
    #[cfg(debug_assertions)]
    pub fn subscribe_to_steps(self: &Arc<Self>, probes: &mut CpuProbes) {
        use helm_probe::CpuStepEvent;
        let counter = Arc::clone(self);
        probes.post_step.subscribe(move |_ev: &CpuStepEvent| {
            counter.inc();
        });
    }

    /// Subscribe to CpuProbes.branch — increment once per branch instruction.
    #[cfg(debug_assertions)]
    pub fn subscribe_to_branches(self: &Arc<Self>, probes: &mut CpuProbes) {
        use helm_probe::BranchEvent;
        let counter = Arc::clone(self);
        probes.branch.subscribe(move |_ev: &BranchEvent| {
            counter.inc();
        });
    }
}
```

---

### 5.2 `IndexedCounter`

Replaces `HowVec`. A fixed array of `AtomicU64`, one per dimension label.
Designed for enum-indexed access (e.g. `InsnClass`, `BranchKind`) where the
index comes from a `match` on a `#[repr(u8)]` enum — the bounds check is
elided by the optimizer in this case.

```rust
/// Fixed-size counter array indexed by a dimension (e.g. InsnClass, BranchKind).
/// Lock-free: one AtomicU64 per label. No allocation on inc().
pub struct IndexedCounter {
    name:    String,
    labels:  Vec<&'static str>,
    buckets: Vec<AtomicU64>,
}

impl IndexedCounter {
    pub fn new(name: impl Into<String>, labels: &[&'static str]) -> Arc<Self> {
        let n = labels.len();
        Arc::new(Self {
            name:    name.into(),
            labels:  labels.to_vec(),
            buckets: (0..n).map(|_| AtomicU64::new(0)).collect(),
        })
    }

    /// Convenience: create an InsnClass-indexed counter.
    pub fn for_insn_class(name: impl Into<String>) -> Arc<Self> {
        Self::new(name, &InsnClass::LABELS)
    }

    /// Convenience: create a BranchKind-indexed counter.
    pub fn for_branch_kind(name: impl Into<String>) -> Arc<Self> {
        Self::new(name, &BranchKind::LABELS)
    }

    /// Increment bucket `idx`. Panics in debug if idx >= len.
    /// In release builds, optimizer may elide bounds check for enum-derived index.
    #[inline(always)]
    pub fn inc(&self, idx: usize) {
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    /// Read all bucket values. Cold path — allocates a Vec.
    pub fn values(&self) -> Vec<(&'static str, u64)> {
        self.labels.iter().zip(self.buckets.iter())
            .map(|(&label, bucket)| (label, bucket.load(Ordering::Relaxed)))
            .collect()
    }

    /// Total across all buckets.
    pub fn total(&self) -> u64 {
        self.buckets.iter().map(|b| b.load(Ordering::Relaxed)).sum()
    }

    /// Fraction of total for bucket `idx`. Returns 0.0 if total is 0.
    pub fn fraction(&self, idx: usize) -> f64 {
        let total = self.total();
        if total == 0 { return 0.0; }
        self.buckets[idx].load(Ordering::Relaxed) as f64 / total as f64
    }

    /// Sorted table: (label, count, fraction), descending by count.
    pub fn table(&self) -> Vec<(&'static str, u64, f64)> {
        let total = self.total();
        let mut rows: Vec<_> = self.labels.iter().zip(self.buckets.iter())
            .map(|(&label, bucket)| {
                let count = bucket.load(Ordering::Relaxed);
                let frac = if total == 0 { 0.0 } else { count as f64 / total as f64 };
                (label, count, frac)
            })
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1));
        rows
    }

    pub fn name(&self) -> &str { &self.name }

    pub fn reset(&self) {
        for bucket in &self.buckets {
            bucket.store(0, Ordering::Relaxed);
        }
    }
}
```

#### Subscribe pattern

```rust
impl IndexedCounter {
    /// Subscribe to post_step — increment the InsnClass bucket for each retired instruction.
    /// Caller is responsible for providing a classifier closure that maps raw → usize.
    #[cfg(debug_assertions)]
    pub fn subscribe_to_steps_with<F>(
        self: &Arc<Self>,
        probes: &mut CpuProbes,
        classify: F,
    )
    where
        F: Fn(u32) -> usize + Send + Sync + 'static,
    {
        use helm_probe::CpuStepEvent;
        let counter = Arc::clone(self);
        probes.post_step.subscribe(move |ev: &CpuStepEvent| {
            let idx = classify(ev.raw);
            // idx must be in bounds; classify should return a value from InsnClass as usize
            if idx < counter.buckets.len() {
                counter.inc(idx);
            }
        });
    }
}
```

---

## 6. `src/primitives/per_vcpu.rs` — `PerVcpuCounter`

### 6.1 `Scoreboard<T>`

`Scoreboard<T>` is re-used from the existing codebase (currently in `helm-plugin`; moved
to `helm-spy`). It provides per-vCPU slot access via `UnsafeCell` — no atomic ops,
no locking.

```rust
// src/primitives/per_vcpu.rs  (or shared scoreboard.rs)

use std::cell::UnsafeCell;

/// Per-slot UnsafeCell storage. Provides lock-free per-vCPU slot writes.
///
/// # Safety Invariant
///
/// Each slot index MUST be written by exactly one thread at a time. The caller
/// is responsible for ensuring that `get_mut(idx)` is only called from the thread
/// that owns vCPU `idx`. `iter()` is only safe after the hot loop has stopped
/// (e.g., inside `quantum_end()` or `total()`).
///
/// This invariant holds in helm-ng because:
/// - The engine runs each vCPU on a single thread per quantum.
/// - `iter()` / `total()` are called at quantum boundaries (hot loop not running).
pub struct Scoreboard<T> {
    slots: Vec<UnsafeCell<T>>,
}

// SAFETY: T: Send is required; each slot is accessed by at most one thread at a time
// per the Invariant above. Vec<UnsafeCell<T>> is not auto-Sync, so we impl manually.
unsafe impl<T: Send> Send for Scoreboard<T> {}
unsafe impl<T: Send> Sync for Scoreboard<T> {}

impl<T: Default> Scoreboard<T> {
    pub fn new(n_vcpus: usize) -> Self {
        Self {
            slots: (0..n_vcpus).map(|_| UnsafeCell::new(T::default())).collect(),
        }
    }

    /// Immutable reference to slot `idx`. Safe to call from the owning thread.
    #[inline]
    pub fn get(&self, idx: usize) -> &T {
        // SAFETY: only the vCPU-owning thread reads this slot during hot loop
        unsafe { &*self.slots[idx].get() }
    }

    /// Mutable reference to slot `idx`. Safe to call from the owning thread.
    #[allow(clippy::mut_from_ref)]
    #[inline]
    pub fn get_mut(&self, idx: usize) -> &mut T {
        // SAFETY: only the vCPU-owning thread writes this slot during hot loop
        unsafe { &mut *self.slots[idx].get() }
    }

    pub fn len(&self) -> usize { self.slots.len() }
    pub fn is_empty(&self) -> bool { self.slots.is_empty() }

    /// Iterate all slots. Only safe when hot loop is not running
    /// (i.e., inside quantum_end() or at session query time).
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.slots.iter().map(|c| unsafe { &*c.get() })
    }
}
```

### 6.2 `PerVcpuCounter`

```rust
/// Per-vCPU instruction counter. Zero cross-thread synchronization in the hot path.
///
/// Uses a Scoreboard<u64> — one slot per vCPU, accessed only by the owning thread
/// during hot-loop execution. `total()` and `per_vcpu()` are cold-path only.
pub struct PerVcpuCounter {
    name:  String,
    slots: Scoreboard<u64>,
}

impl PerVcpuCounter {
    pub fn new(name: impl Into<String>, n_vcpus: usize) -> Arc<Self> {
        Arc::new(Self {
            name:  name.into(),
            slots: Scoreboard::new(n_vcpus),
        })
    }

    /// Increment the counter for vCPU `vcpu`. No atomic ops — pure slot write.
    /// MUST only be called from the thread owning vCPU `vcpu`.
    #[inline(always)]
    pub fn inc(&self, vcpu: usize) {
        *self.slots.get_mut(vcpu) += 1;
    }

    /// Add `n` to the counter for vCPU `vcpu`. Hot-path safe (no atomic).
    #[inline(always)]
    pub fn add(&self, vcpu: usize, n: u64) {
        *self.slots.get_mut(vcpu) += n;
    }

    /// Sum across all vCPU slots. Cold path — safe only outside the hot loop.
    pub fn total(&self) -> u64 {
        self.slots.iter().sum()
    }

    /// Per-vCPU values. Cold path — allocates a Vec.
    pub fn per_vcpu(&self) -> Vec<u64> {
        self.slots.iter().copied().collect()
    }

    pub fn name(&self) -> &str { &self.name }

    pub fn reset(&self) {
        for slot in self.slots.iter() {
            // SAFETY: we are at a quantum boundary; hot loop is not running
            // Use get_mut via index to reset:
            let _ = slot; // iter gives &T; reset via index loop instead
        }
        for i in 0..self.slots.len() {
            *self.slots.get_mut(i) = 0;
        }
    }
}
```

#### Subscribe pattern

```rust
impl PerVcpuCounter {
    /// Subscribe to post_step for vCPU `vcpu_idx`.
    /// One subscription per vCPU — called once per vCPU during session.subscribe().
    #[cfg(debug_assertions)]
    pub fn subscribe_to_steps_for_vcpu(
        self: &Arc<Self>,
        probes: &mut CpuProbes,
        vcpu_idx: usize,
    ) {
        use helm_probe::CpuStepEvent;
        let counter = Arc::clone(self);
        probes.post_step.subscribe(move |_ev: &CpuStepEvent| {
            // SAFETY: this closure runs only on the thread owning vcpu_idx
            counter.inc(vcpu_idx);
        });
    }
}
```

---

## 7. `src/primitives/histogram.rs` — `Histogram` and `IntervalHistogram`

### 7.1 `Histogram`

Fixed-bucket histogram. Bucket edges are specified at construction and never change.
`record(val)` uses binary search (`partition_point`) to find the correct bucket.

```rust
// src/primitives/histogram.rs

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Fixed-bucket histogram with lock-free per-bucket AtomicU64 counters.
///
/// # Bucket layout
///
/// Given `bucket_edges = [0, 10, 100, 1000]`:
/// - bucket 0: val < 10     (val in [MIN, 10))
/// - bucket 1: val in [10, 100)
/// - bucket 2: val in [100, 1000)
/// - bucket 3: val >= 1000  (overflow bucket)
///
/// Total buckets = `bucket_edges.len() + 1`.
pub struct Histogram {
    name:         String,
    bucket_edges: Vec<u64>,           // sorted; length N → N+1 buckets
    buckets:      Vec<AtomicU64>,     // length N+1
}

impl Histogram {
    pub fn new(name: impl Into<String>, mut bucket_edges: Vec<u64>) -> Arc<Self> {
        bucket_edges.sort_unstable();
        bucket_edges.dedup();
        let n_buckets = bucket_edges.len() + 1;
        Arc::new(Self {
            name: name.into(),
            bucket_edges,
            buckets: (0..n_buckets).map(|_| AtomicU64::new(0)).collect(),
        })
    }

    /// Record a value. Finds the correct bucket via binary search.
    /// Hot-path safe (no alloc, no lock). Binary search on N edges: O(log N).
    #[inline]
    pub fn record(&self, val: u64) {
        // `partition_point` returns the first index where edge > val,
        // which is exactly the bucket index for this value.
        let idx = self.bucket_edges.partition_point(|&edge| val >= edge);
        // idx is in [0, bucket_edges.len()], i.e., always a valid bucket index.
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    /// Total count across all buckets.
    pub fn total(&self) -> u64 {
        self.buckets.iter().map(|b| b.load(Ordering::Relaxed)).sum()
    }

    /// All bucket counts. Cold path — allocates a Vec.
    pub fn counts(&self) -> Vec<u64> {
        self.buckets.iter().map(|b| b.load(Ordering::Relaxed)).collect()
    }

    /// Approximate `p`-th percentile value (0.0 ≤ p ≤ 1.0).
    /// Returns the lower edge of the bucket containing the p-th percentile.
    /// Returns 0 if the histogram is empty.
    pub fn percentile(&self, p: f64) -> u64 {
        debug_assert!((0.0..=1.0).contains(&p));
        let total = self.total();
        if total == 0 { return 0; }
        let target = (p * total as f64) as u64;
        let mut cumulative = 0u64;
        for (i, bucket) in self.buckets.iter().enumerate() {
            cumulative += bucket.load(Ordering::Relaxed);
            if cumulative >= target {
                // Return the lower edge of this bucket (or 0 for bucket 0)
                return if i == 0 { 0 } else { self.bucket_edges[i - 1] };
            }
        }
        // p == 1.0: return the last edge
        self.bucket_edges.last().copied().unwrap_or(0)
    }

    pub fn name(&self) -> &str { &self.name }

    pub fn reset(&self) {
        for b in &self.buckets { b.store(0, Ordering::Relaxed); }
    }
}
```

#### Subscribe pattern for memory access latency

```rust
impl Histogram {
    /// Subscribe to memory accesses — record address modulo some value as a
    /// distribution (or use a value derived from MemInfo fields).
    #[cfg(debug_assertions)]
    pub fn subscribe_to_mem<F>(
        self: &Arc<Self>,
        probes: &mut CpuProbes,
        extractor: F,
    )
    where
        F: Fn(&helm_probe::MemAccessEvent) -> u64 + Send + Sync + 'static,
    {
        use helm_probe::MemAccessEvent;
        let hist = Arc::clone(self);
        probes.mem.subscribe(move |ev: &MemAccessEvent| {
            hist.record(extractor(ev));
        });
    }
}
```

---

### 7.2 `IntervalHistogram`

Samples a scalar measurement every `window_size` instructions and buckets the per-window
value. Captures the shape of a time-varying signal (e.g. IPC distribution, miss-rate
distribution) that a plain histogram would collapse into one dimension.

**Design note:** The two-`AtomicU64` design (shared `window_accum` + `last_window`) is
for single-producer scenarios where one probe subscriber calls `tick_with()` from one
vCPU. For multi-vCPU, use a `Scoreboard<IntervalHistogram>` — one per vCPU — and merge
in `quantum_end()`.

```rust
/// Time-series histogram. Accumulates a scalar over a window of N instructions,
/// then records the accumulated value into a Histogram at each window boundary.
///
/// **Single-producer:** only one thread should call `tick_with()` at a time.
/// For multi-vCPU, create one `IntervalHistogram` per vCPU and merge in quantum_end().
pub struct IntervalHistogram {
    name:         String,
    window_size:  u64,
    hist:         Arc<Histogram>,
    // Accumulator for the current window. Atomics used for interior mutability
    // in a shared reference; single-producer means no ABA or torn-read issues.
    window_accum: AtomicU64,
    last_window:  AtomicU64,
}

impl IntervalHistogram {
    pub fn new(
        name: impl Into<String>,
        window_size: u64,
        bucket_edges: Vec<u64>,
    ) -> Arc<Self> {
        assert!(window_size > 0, "window_size must be > 0");
        let hist_name = format!("{}.hist", name.as_ref() as &str);
        Arc::new(Self {
            name:         name.into(),
            window_size,
            hist:         Histogram::new(hist_name, bucket_edges),
            window_accum: AtomicU64::new(0),
            last_window:  AtomicU64::new(u64::MAX),
        })
    }

    /// Call on every instruction with the scalar value to accumulate
    /// (e.g., number of cache misses this instruction = 0 or 1).
    ///
    /// When `insn_count` crosses a window boundary, the accumulated
    /// value is committed to the inner `Histogram` and the accumulator resets.
    ///
    /// Hot-path safe: one division + one conditional atomic swap.
    #[inline]
    pub fn tick_with(&self, value: u64, insn_count: u64) {
        let window = insn_count / self.window_size;
        let prev = self.last_window.load(Ordering::Relaxed);
        if window != prev {
            // Window boundary crossed: commit and reset.
            // `swap` both reads the old accumulated value and stores the new
            // value (first contribution of the new window) atomically.
            let committed = self.window_accum.swap(value, Ordering::Relaxed);
            if prev != u64::MAX {
                // Only record if this is not the very first window boundary
                self.hist.record(committed);
            }
            self.last_window.store(window, Ordering::Relaxed);
        } else {
            self.window_accum.fetch_add(value, Ordering::Relaxed);
        }
    }

    /// Access the underlying Histogram for percentile / counts queries.
    pub fn histogram(&self) -> &Histogram { &self.hist }

    /// Shorthand: percentile of the distribution of per-window values.
    pub fn percentile(&self, p: f64) -> u64 { self.hist.percentile(p) }

    pub fn name(&self) -> &str { &self.name }
}
```

---

## 8. `src/primitives/heatmap.rs` — `HeatMap`

Per-PC `u64` counter map. Used for hot-PC analysis and branch target distribution.

Two backends:
- **`dashmap` feature** (recommended): `DashMap<u64, u64>` — fine-grained sharding,
  contention-free for disjoint PCs across vCPUs.
- **Fallback (no feature)**: `[Mutex<HashMap<u64, u64>>; 16]` sharded by `pc & 0xF` —
  no external dependency, adequate for low vCPU counts.

```rust
// src/primitives/heatmap.rs

use std::sync::Arc;

/// Per-PC u64 counter map for hot-path PC counting.
///
/// `inc(pc)` acquires a shard lock (brief critical section, no global lock).
/// `top(n)` iterates the entire map — call only on the cold path.
pub struct HeatMap {
    name:  String,
    inner: HeatMapInner,
}

#[cfg(feature = "dashmap")]
use dashmap::DashMap;

enum HeatMapInner {
    #[cfg(feature = "dashmap")]
    Dash(DashMap<u64, u64>),
    Sharded(ShardedMap),
}

/// 16-shard fallback. Shard selected by low bits of PC.
struct ShardedMap {
    shards: [std::sync::Mutex<std::collections::HashMap<u64, u64>>; 16],
}

impl ShardedMap {
    fn new() -> Self {
        // Array initialization requires Default; Mutex<HashMap> is not Copy.
        // Use array::from_fn (stable since Rust 1.63).
        Self {
            shards: std::array::from_fn(|_| {
                std::sync::Mutex::new(std::collections::HashMap::new())
            }),
        }
    }

    #[inline]
    fn inc(&self, pc: u64) {
        let shard_idx = (pc & 0xF) as usize;
        let mut shard = self.shards[shard_idx].lock().unwrap();
        *shard.entry(pc).or_insert(0) += 1;
    }

    fn all_entries(&self) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        for shard in &self.shards {
            let s = shard.lock().unwrap();
            out.extend(s.iter().map(|(&pc, &count)| (pc, count)));
        }
        out
    }
}

impl HeatMap {
    pub fn new(name: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            #[cfg(feature = "dashmap")]
            inner: HeatMapInner::Dash(DashMap::new()),
            #[cfg(not(feature = "dashmap"))]
            inner: HeatMapInner::Sharded(ShardedMap::new()),
        })
    }

    /// Increment the counter for `pc`. Acquires a shard lock briefly.
    /// Hot-path safe (no global lock, no alloc for existing keys).
    #[inline]
    pub fn inc(&self, pc: u64) {
        match &self.inner {
            #[cfg(feature = "dashmap")]
            HeatMapInner::Dash(map) => {
                *map.entry(pc).or_insert(0) += 1;
            }
            HeatMapInner::Sharded(sharded) => {
                sharded.inc(pc);
            }
        }
    }

    /// Return the top `n` (pc, count) pairs, sorted descending by count.
    /// Cold path — iterates the entire map; O(N log N).
    pub fn top(&self, n: usize) -> Vec<(u64, u64)> {
        let mut entries = match &self.inner {
            #[cfg(feature = "dashmap")]
            HeatMapInner::Dash(map) => {
                map.iter().map(|r| (*r.key(), *r.value())).collect::<Vec<_>>()
            }
            HeatMapInner::Sharded(sharded) => sharded.all_entries(),
        };
        entries.sort_unstable_by(|a, b| b.1.cmp(&a.1));
        entries.truncate(n);
        entries
    }

    /// Total count across all PCs.
    pub fn total(&self) -> u64 {
        match &self.inner {
            #[cfg(feature = "dashmap")]
            HeatMapInner::Dash(map) => map.iter().map(|r| *r.value()).sum(),
            HeatMapInner::Sharded(sharded) => {
                sharded.all_entries().iter().map(|(_, c)| c).sum()
            }
        }
    }

    pub fn name(&self) -> &str { &self.name }
}
```

#### Subscribe pattern

```rust
impl HeatMap {
    /// Subscribe to post_step — increment count for the instruction PC.
    #[cfg(debug_assertions)]
    pub fn subscribe_to_steps(self: &Arc<Self>, probes: &mut CpuProbes) {
        use helm_probe::CpuStepEvent;
        let hm = Arc::clone(self);
        probes.post_step.subscribe(move |ev: &CpuStepEvent| {
            hm.inc(ev.pc);
        });
    }

    /// Subscribe to branch probe — increment count for branch instruction PCs.
    #[cfg(debug_assertions)]
    pub fn subscribe_to_branches(self: &Arc<Self>, probes: &mut CpuProbes) {
        use helm_probe::BranchEvent;
        let hm = Arc::clone(self);
        probes.branch.subscribe(move |ev: &BranchEvent| {
            hm.inc(ev.pc);
        });
    }
}
```

---

## 9. `src/primitives/ring.rs` — `RingBuffer<T>` and `EventStream<T>`

These are the two `Mutex`-using primitives. They are suitable for low-rate events
(faults, MMU walks, syscalls) only. Do not use for per-instruction callbacks.

### 9.1 `RingBuffer<T>`

```rust
// src/primitives/ring.rs

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Circular buffer of typed events. Fixed capacity; overwrites oldest on push.
///
/// Holds a `Mutex<VecDeque<T>>`. Not suitable for per-instruction callbacks —
/// use `TraceRing<T, N>` (lock-free) for high-rate events.
pub struct RingBuffer<T: Clone + Send> {
    name:     String,
    capacity: usize,
    buf:      Mutex<VecDeque<T>>,
}

impl<T: Clone + Send> RingBuffer<T> {
    pub fn new(name: impl Into<String>, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            capacity,
            buf: Mutex::new(VecDeque::with_capacity(capacity)),
        })
    }

    /// Push an event. If at capacity, the oldest event is dropped.
    /// Acquires the Mutex — hot-path cost: one lock acquisition + VecDeque push.
    pub fn push(&self, ev: T) {
        let mut buf = self.buf.lock().unwrap();
        if buf.len() == self.capacity {
            buf.pop_front();   // drop oldest
        }
        buf.push_back(ev);
    }

    /// Clone the current buffer contents. Cold path — allocates a Vec.
    pub fn snapshot(&self) -> Vec<T> {
        let buf = self.buf.lock().unwrap();
        buf.iter().cloned().collect()
    }

    /// Current number of stored events.
    pub fn len(&self) -> usize {
        self.buf.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool { self.len() == 0 }
    pub fn capacity(&self) -> usize { self.capacity }
    pub fn name(&self) -> &str { &self.name }
}
```

### 9.2 `EventStream<T>`

```rust
/// Bounded event log. Records up to `max` events, then stops (does not overwrite).
/// Suitable for syscall traces, fault logs, or any low-rate event stream where
/// recording a complete prefix is more useful than a rolling window.
pub struct EventStream<T: Clone + Send> {
    name:   String,
    max:    usize,
    events: Mutex<Vec<T>>,
}

impl<T: Clone + Send> EventStream<T> {
    pub fn new(name: impl Into<String>, max: usize) -> Arc<Self> {
        Arc::new(Self {
            name:   name.into(),
            max,
            events: Mutex::new(Vec::with_capacity(max.min(4096))),
        })
    }

    /// Record an event. No-op (silent drop) if already at `max`.
    /// Acquires the Mutex — suitable for low-rate events only.
    pub fn push(&self, ev: T) {
        let mut events = self.events.lock().unwrap();
        if events.len() < self.max {
            events.push(ev);
        }
        // Silent drop when full — caller can check is_full() if needed.
    }

    /// Drain all recorded events, leaving the stream empty.
    /// Cold path — allocates no extra memory (returns the inner Vec).
    pub fn drain(&self) -> Vec<T> {
        let mut events = self.events.lock().unwrap();
        std::mem::take(&mut *events)
    }

    /// Returns true if no more events will be recorded (len == max).
    pub fn is_full(&self) -> bool {
        self.events.lock().unwrap().len() >= self.max
    }

    pub fn len(&self) -> usize { self.events.lock().unwrap().len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    pub fn max(&self) -> usize { self.max }
    pub fn name(&self) -> &str { &self.name }
}
```

---

## 10. `src/primitives/trace_ring.rs` — `TraceRing<T, N>` and `BranchRecord`

`TraceRing` is the high-rate typed event trace primitive. It is a lock-free
single-producer single-consumer (SPSC) ring buffer for `Copy` types. Zero heap
allocation after construction. Used for dense branch and instruction traces.

### 10.1 Design invariants

| Invariant | Mechanism |
|---|---|
| Single producer | One vCPU's probe closure calls `push()`; enforced by ownership |
| Single consumer | One drain thread (or the Python caller at quantum end) calls `drain_into()` |
| No ABA | 64-bit head/tail counters never wrap in practice (≫ 2^64 instructions) |
| Lossy on overflow | `push()` overwrites oldest slot; caller checks `dropped()` count |
| No heap alloc on push | `ptr::write` into a pre-allocated `Box<[MaybeUninit<T>; N]>` |
| N must be power of 2 | Enables `& (N-1)` index masking instead of `%` |

### 10.2 `TraceRing<T, N>`

```rust
// src/primitives/trace_ring.rs

use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Lock-free single-producer single-consumer ring buffer.
///
/// # Type parameters
/// - `T`: must be `Copy + Send` (no drop, no allocation on write)
/// - `N`: capacity; MUST be a power of 2 (checked with `assert!` in `new()`)
///
/// # Producer/consumer roles
/// - **Producer** (hot path): calls `push(val)` from the vCPU probe closure.
/// - **Consumer** (cold path): calls `drain_into(out)` from the drain thread
///   or the Python caller at quantum boundaries.
///
/// # Ordering
/// - `push()`: writes slot, then `Release`-stores `head`.
/// - `drain_into()`: `Acquire`-loads `head` to observe all writes before the store.
/// - `tail` is updated by the consumer only; producer never reads `tail`.
pub struct TraceRing<T: Copy + Send, const N: usize> {
    buf:     Box<[MaybeUninit<T>; N]>,
    /// Write cursor. Producer-only. Monotonically increasing.
    head:    AtomicU64,
    /// Read cursor. Consumer-only. Monotonically increasing.
    tail:    AtomicU64,
    /// Count of events dropped due to the ring being full.
    dropped: AtomicU64,
}

// SAFETY: T: Send + Copy. The buf is behind a Box (heap-stable pointer). AtomicU64 is Sync.
unsafe impl<T: Copy + Send, const N: usize> Send for TraceRing<T, N> {}
unsafe impl<T: Copy + Send, const N: usize> Sync for TraceRing<T, N> {}

impl<T: Copy + Send, const N: usize> TraceRing<T, N> {
    pub fn new() -> Arc<Self> {
        assert!(N.is_power_of_two(), "TraceRing N must be a power of 2");
        // SAFETY: MaybeUninit<T> does not require initialization.
        let buf = Box::new(unsafe {
            MaybeUninit::<[MaybeUninit<T>; N]>::uninit().assume_init()
        });
        Arc::new(Self {
            buf,
            head:    AtomicU64::new(0),
            tail:    AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        })
    }

    /// Write one record. Overwrites the oldest slot if the ring is full (lossy).
    ///
    /// # Hot-path cost
    /// 1× `AtomicU64::load(Relaxed)` (head) +
    /// 1× `ptr::write` into pre-allocated slot +
    /// 1× `AtomicU64::store(Release)` (head)
    ///
    /// No allocation. No lock. No branch except the overflow check.
    #[inline(always)]
    pub fn push(&self, val: T) {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);   // observe consumer progress

        if head.wrapping_sub(tail) >= N as u64 {
            // Ring is full — drop the oldest slot (advance tail from producer side).
            // In strict SPSC, the producer should not advance tail. Here we choose
            // lossy overwrite: increment dropped counter and overwrite.
            self.dropped.fetch_add(1, Ordering::Relaxed);
            // Overwrite the slot anyway (advance tail to maintain ring invariant):
            self.tail.fetch_add(1, Ordering::Relaxed);
        }

        let slot_idx = (head as usize) & (N - 1);
        // SAFETY: slot_idx is always in [0, N-1]; MaybeUninit<T> accepts any write.
        unsafe {
            std::ptr::write(self.buf[slot_idx].as_ptr() as *mut T, val);
        }

        // Release store — ensures the slot write is visible before the head update.
        self.head.store(head.wrapping_add(1), Ordering::Release);
    }

    /// Drain all available records into `out`. Does not allocate — appends to `out`.
    ///
    /// # Cold-path cost
    /// 1× `AtomicU64::load(Acquire)` (head) + N× `ptr::read` for available records.
    pub fn drain_into(&self, out: &mut Vec<T>) {
        // Acquire load — synchronizes with the Release store in push().
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Relaxed);

        let available = head.wrapping_sub(tail) as usize;
        out.reserve(available);

        for i in 0..available {
            let slot_idx = ((tail + i as u64) as usize) & (N - 1);
            // SAFETY: slot_idx is in [0, N-1]; slot was written by push() before
            // the Release store, and we Acquire-loaded head above.
            let val = unsafe { std::ptr::read(self.buf[slot_idx].as_ptr() as *const T) };
            out.push(val);
        }

        // Advance tail past the drained records.
        self.tail.store(head, Ordering::Relaxed);
    }

    /// Count of events dropped due to ring overflow.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Number of unread records currently in the ring.
    pub fn available(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Relaxed);
        head.wrapping_sub(tail) as usize
    }

    /// Total capacity.
    pub fn capacity(&self) -> usize { N }
}
```

### 10.3 `BranchRecord` — canonical typed trace record

```rust
/// 32-byte branch trace record. Two cache lines. `repr(C)` for Python `mmap` +
/// `struct.unpack_from`. Used with `TraceRing<BranchRecord, 1_048_576>` (32 MB).
///
/// Python consumer:
/// ```python
/// import mmap, struct
/// fmt = "=QQQBxxxxxxx"  # pc(8), target(8), insn_count(8), flags(1), pad(7)
/// with open("branch.trace", "rb") as f:
///     mm = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
///     for record in struct.iter_unpack(fmt, mm[header_size:]):
///         pc, target, insn_count, flags = record
///         taken     = bool(flags & 0x01)
///         predicted = bool(flags & 0x02)
///         kind      = (flags >> 2) & 0x07
/// ```
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct BranchRecord {
    /// PC of the branch instruction.
    pub pc:         u64,
    /// Branch target address.
    pub target:     u64,
    /// Monotonic instruction retirement count at branch execution.
    pub insn_count: u64,
    /// Packed flags:
    /// - bit 0: taken (1 = taken, 0 = not taken)
    /// - bit 1: predicted correctly (1 = correct, 0 = mispredicted)
    /// - bits 2-4: BranchKind (DirectCond=0, DirectUncond=1, Call=2, Return=3,
    ///             IndirectJump=4, IndirectCall=5)
    /// - bits 5-7: spare (zero)
    pub flags:      u8,
    pub _pad:       [u8; 7],
}

impl BranchRecord {
    pub fn new(
        pc: u64,
        target: u64,
        insn_count: u64,
        taken: bool,
        predicted: bool,
        kind: BranchKind,
    ) -> Self {
        let flags = (taken as u8)
            | ((predicted as u8) << 1)
            | ((kind as u8) << 2);
        Self { pc, target, insn_count, flags, _pad: [0; 7] }
    }

    pub fn taken(&self)     -> bool       { self.flags & 0x01 != 0 }
    pub fn predicted(&self) -> bool       { self.flags & 0x02 != 0 }
    pub fn kind(&self)      -> BranchKind {
        match (self.flags >> 2) & 0x07 {
            0 => BranchKind::DirectCond,
            1 => BranchKind::DirectUncond,
            2 => BranchKind::Call,
            3 => BranchKind::Return,
            4 => BranchKind::IndirectJump,
            _ => BranchKind::IndirectCall,
        }
    }
}

const _: () = assert!(std::mem::size_of::<BranchRecord>() == 32);
```

#### Subscribe pattern

```rust
impl TraceRing<BranchRecord, { 1 << 20 }> {
    /// Subscribe to the branch probe — push a BranchRecord on each branch.
    #[cfg(debug_assertions)]
    pub fn subscribe_to_branches(
        self: &Arc<Self>,
        probes: &mut CpuProbes,
        insn_counter: Arc<Counter>,
    ) {
        use helm_probe::BranchEvent;
        let ring = Arc::clone(self);
        let ctr  = insn_counter;
        probes.branch.subscribe(move |ev: &BranchEvent| {
            ring.push(BranchRecord::new(
                ev.pc,
                ev.target,
                ctr.value(),      // approximate — relaxed read is fine for traces
                ev.taken,
                false,            // predicted: filled later by BranchPredictor if present
                ev.kind.into(),
            ));
        });
    }
}
```

---

## 11. `src/primitives/correl2d.rs` — `CorrelHist2D`

2D joint histogram. Answers "what is the joint distribution of X and Y?" where
neither axis can be answered by independent 1D histograms.

Example: IPC (instructions per cycle) vs. L1D miss rate — reveals whether high-IPC
periods correlate with low miss rates, or whether they are independent.

```rust
// src/primitives/correl2d.rs

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// 2D joint histogram. Row-major flat storage for cache locality.
///
/// # Bucket layout
///
/// Given `edges_x = [0, 10, 100]` and `edges_y = [0, 5, 50]`:
/// - x_buckets = 4 (including overflow)
/// - y_buckets = 4 (including overflow)
/// - `counts[bx * y_buckets + by]` = count for (x in bucket bx, y in bucket by)
///
/// Total cells = (edges_x.len() + 1) × (edges_y.len() + 1).
pub struct CorrelHist2D {
    name:      String,
    edges_x:   Vec<u64>,
    edges_y:   Vec<u64>,
    x_buckets: usize,    // edges_x.len() + 1
    y_buckets: usize,    // edges_y.len() + 1
    counts:    Vec<AtomicU64>,
}

impl CorrelHist2D {
    pub fn new(
        name:   impl Into<String>,
        mut edges_x: Vec<u64>,
        mut edges_y: Vec<u64>,
    ) -> Arc<Self> {
        edges_x.sort_unstable(); edges_x.dedup();
        edges_y.sort_unstable(); edges_y.dedup();
        let x_buckets = edges_x.len() + 1;
        let y_buckets = edges_y.len() + 1;
        let n_cells = x_buckets * y_buckets;
        Arc::new(Self {
            name: name.into(),
            edges_x,
            edges_y,
            x_buckets,
            y_buckets,
            counts: (0..n_cells).map(|_| AtomicU64::new(0)).collect(),
        })
    }

    /// Record a joint observation (x, y). Lock-free.
    /// Hot-path cost: 2× `partition_point` (binary search) + 1× `fetch_add(Relaxed)`.
    #[inline]
    pub fn record(&self, x: u64, y: u64) {
        let bx = self.edges_x.partition_point(|&e| x >= e);
        let by = self.edges_y.partition_point(|&e| y >= e);
        let idx = bx * self.y_buckets + by;
        self.counts[idx].fetch_add(1, Ordering::Relaxed);
    }

    /// Total count across all cells.
    pub fn total(&self) -> u64 {
        self.counts.iter().map(|c| c.load(Ordering::Relaxed)).sum()
    }

    /// Row-major count matrix: counts[x_bucket][y_bucket].
    /// Cold path — allocates a Vec<Vec<u64>>.
    pub fn matrix(&self) -> Vec<Vec<u64>> {
        (0..self.x_buckets)
            .map(|bx| {
                (0..self.y_buckets)
                    .map(|by| self.counts[bx * self.y_buckets + by].load(Ordering::Relaxed))
                    .collect()
            })
            .collect()
    }

    /// Marginal distribution over X (sum across Y for each X bucket).
    pub fn marginal_x(&self) -> Vec<u64> {
        (0..self.x_buckets)
            .map(|bx| {
                (0..self.y_buckets)
                    .map(|by| self.counts[bx * self.y_buckets + by].load(Ordering::Relaxed))
                    .sum()
            })
            .collect()
    }

    /// Marginal distribution over Y (sum across X for each Y bucket).
    pub fn marginal_y(&self) -> Vec<u64> {
        (0..self.y_buckets)
            .map(|by| {
                (0..self.x_buckets)
                    .map(|bx| self.counts[bx * self.y_buckets + by].load(Ordering::Relaxed))
                    .sum()
            })
            .collect()
    }

    pub fn name(&self) -> &str { &self.name }
    pub fn x_bucket_count(&self) -> usize { self.x_buckets }
    pub fn y_bucket_count(&self) -> usize { self.y_buckets }

    pub fn reset(&self) {
        for cell in &self.counts { cell.store(0, Ordering::Relaxed); }
    }
}
```

---

## 12. `src/trigger.rs` — `TriggerKind`, `Trigger`, `TriggerCtx`

Full implementation of the trigger system described in HLD §6.

```rust
// src/trigger.rs

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crate::primitives::counter::Counter;

/// Context passed to a trigger action closure when the trigger fires.
#[derive(Debug, Clone, Copy)]
pub struct TriggerCtx {
    pub pc:         u64,
    pub insn_count: u64,
}

/// Specifies when a Trigger fires.
pub enum TriggerKind {
    /// Fire once when insn_count == N.
    AtInsn(u64),
    /// Fire every N instructions (insn_count % N == 0).
    EveryN(u64),
    /// Fire when PC equals the given address (exact match).
    AtPc(u64),
    /// Fire while PC is in [start, end) — fires on EVERY matching instruction.
    /// Use `one_shot = true` to fire only on entry.
    PcRange(u64, u64),
    /// Fire when the given Counter's value reaches or exceeds threshold.
    CounterReaches(Arc<Counter>, u64),
}

/// A single conditional action wired into the pre-step probe.
pub struct Trigger {
    /// The condition to evaluate.
    pub kind:     TriggerKind,
    /// Action closure. Must not block or acquire hot-path locks.
    action:       Box<dyn Fn(&TriggerCtx) + Send + Sync>,
    /// When false, `check()` returns immediately (branch predicted-not-taken).
    armed:        AtomicBool,
    /// If true, disarms after the first fire.
    pub one_shot: bool,
}

impl Trigger {
    pub fn new(
        kind:     TriggerKind,
        action:   impl Fn(&TriggerCtx) + Send + Sync + 'static,
        one_shot: bool,
    ) -> Self {
        Self {
            kind,
            action:   Box::new(action),
            armed:    AtomicBool::new(true),
            one_shot,
        }
    }

    /// Arm the trigger (enables `check()`).
    pub fn arm(&self) { self.armed.store(true, Ordering::Relaxed); }

    /// Disarm the trigger (disables `check()` without removing the trigger).
    pub fn disarm(&self) { self.armed.store(false, Ordering::Relaxed); }

    pub fn is_armed(&self) -> bool { self.armed.load(Ordering::Relaxed) }

    /// Called from the pre-step probe subscriber.
    ///
    /// Hot-loop cost when disarmed: 1× `AtomicBool::load(Relaxed)` + branch.
    /// Hot-loop cost when armed: 1× load + 1× match comparison + conditional action.
    #[inline]
    pub fn check(&self, pc: u64, insn_count: u64) -> bool {
        // Fast path: disarmed triggers are essentially free.
        if !self.armed.load(Ordering::Relaxed) { return false; }

        let fired = match &self.kind {
            TriggerKind::AtInsn(n) => insn_count == *n,
            TriggerKind::EveryN(n) => *n > 0 && insn_count % n == 0,
            TriggerKind::AtPc(addr) => pc == *addr,
            TriggerKind::PcRange(start, end) => pc >= *start && pc < *end,
            TriggerKind::CounterReaches(ctr, threshold) => ctr.value() >= *threshold,
        };

        if fired {
            if self.one_shot {
                // Disarm before calling the action (prevents re-entrant fire
                // if the action itself calls check() indirectly).
                self.armed.store(false, Ordering::Relaxed);
            }
            (self.action)(&TriggerCtx { pc, insn_count });
        }

        fired
    }
}
```

---

## 13. `src/window.rs` — `Window` and `Windowed<T>`

```rust
// src/window.rs

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A closed instruction-count range [start, end) for gating observation.
pub struct Window {
    pub start: u64,
    pub end:   u64,
    /// Cached active state. Updated by `is_active()`.
    active:    AtomicBool,
}

impl Window {
    pub fn new(start: u64, end: u64) -> Arc<Self> {
        assert!(end > start, "Window end must be greater than start");
        Arc::new(Self {
            start,
            end,
            active: AtomicBool::new(false),
        })
    }

    /// Returns true iff insn_count is inside [start, end).
    /// Also updates the cached `active` state — cheap AtomicBool store.
    #[inline]
    pub fn is_active(&self, insn_count: u64) -> bool {
        let in_range = insn_count >= self.start && insn_count < self.end;
        self.active.store(in_range, Ordering::Relaxed);
        in_range
    }

    /// Read cached active state (valid after `is_active()` was called this quantum).
    #[inline]
    pub fn is_active_cached(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }
}

/// Wraps a collection primitive `T` and gates recording to inside-window only.
///
/// The inner `T` is still owned — `Windowed<T>` is transparent otherwise.
/// `get_if_active()` is the gate: returns `Some(&inner)` inside the window,
/// `None` outside. The caller skips the `record()` / `inc()` call entirely
/// when `None` is returned, achieving zero collection cost outside the window.
pub struct Windowed<T> {
    pub window: Arc<Window>,
    pub inner:  T,
}

impl<T> Windowed<T> {
    pub fn new(window: Arc<Window>, inner: T) -> Self {
        Self { window, inner }
    }

    /// Returns `Some(&inner)` when inside the window, `None` otherwise.
    /// `insn_count` is used to update the window's cached `active` state.
    #[inline]
    pub fn get_if_active(&self, insn_count: u64) -> Option<&T> {
        if self.window.is_active(insn_count) { Some(&self.inner) } else { None }
    }

    /// Direct access to the inner primitive, bypassing the window gate.
    /// Use for cold-path queries (e.g., `windowed_counter.inner.value()`).
    pub fn inner(&self) -> &T { &self.inner }

    pub fn window(&self) -> &Window { &self.window }
}

impl<T: Clone> Clone for Windowed<T> {
    fn clone(&self) -> Self {
        Self {
            window: Arc::clone(&self.window),
            inner:  self.inner.clone(),
        }
    }
}
```

#### Windowed subscribe pattern

```rust
// Pattern: Windowed<Arc<Counter>> — gate instruction counting to a window.
//
// During session.subscribe():
let windowed_counter: Windowed<Arc<Counter>> = Windowed::new(
    Arc::clone(&window),
    Counter::new("windowed.insn_count"),
);
let wc = Arc::new(windowed_counter);  // wrap in Arc for closure capture

let wc2 = Arc::clone(&wc);
cpu_probes.post_step.subscribe(move |ev: &CpuStepEvent| {
    // insn_count carried on the event only with probe-full feature.
    // Without it, track separately via a shared AtomicU64.
    if let Some(counter) = wc2.get_if_active(ev.insn_count) {
        counter.inc();
    }
});
```

---

## 14. `src/lib.rs` — Crate Root

```rust
// src/lib.rs

//! helm-spy — Layer 2 analysis primitives for the helm-ng instrumentation stack.
//!
//! See [HLD.md](../docs/design/helm-spy/HLD.md) for architecture overview.

pub mod events;
pub mod quantum;
pub mod trigger;
pub mod window;
pub mod session;
pub mod bridge;
pub mod primitives;

// Flatten the most-used primitives to the crate root.
pub use events::{
    ArchContext, BranchInfo, BranchKind, FaultInfo, FaultKind,
    InsnClass, InsnInfo, MemInfo, SyscallInfo, SyscallRetInfo,
};
pub use primitives::{
    counter::{Counter, IndexedCounter},
    correl2d::CorrelHist2D,
    heatmap::HeatMap,
    histogram::{Histogram, IntervalHistogram},
    per_vcpu::{PerVcpuCounter, Scoreboard},
    ring::{EventStream, RingBuffer},
    trace_ring::{BranchRecord, TraceRing},
};
pub use quantum::QuantumObserver;
pub use session::SpySession;
pub use trigger::{Trigger, TriggerCtx, TriggerKind};
pub use window::{Window, Windowed};
```

---

## 15. Implementation Notes and Edge Cases

### 15.1 `TraceRing` SPSC correctness argument

The SPSC invariant — exactly one writer, exactly one reader at a time — is enforced by
the caller:

- `push()` is only called from within a probe closure, which is registered by one
  subscriber per `TraceRing` instance. Multiple vCPUs each have their own `TraceRing`
  instance (one per vCPU, created by `SpySession`).
- `drain_into()` is only called at quantum boundaries, when the hot loop is not running.

The Release/Acquire ordering pair is:
- `push()`: `self.head.store(head + 1, Ordering::Release)` — ensures the `ptr::write`
  of the slot is visible before the head counter advances.
- `drain_into()`: `self.head.load(Ordering::Acquire)` — synchronizes with the Release
  store, guaranteeing that all slot writes up to `head` are visible to the consumer.

No `SeqCst` or fences are needed for SPSC.

### 15.2 `IndexedCounter` with `#[repr(u8)]` enum

For enum-indexed access:

```rust
let class: InsnClass = classify_aarch64_opcode(raw).0;
mix.inc(class as usize);
```

The Rust optimizer can elide the bounds check on `self.buckets[idx]` because:
1. `InsnClass` is `#[repr(u8)]` with values `0..7`.
2. `InsnClass::COUNT = 8` matches the `buckets.len()` at construction.
3. The LLVM range-analysis proof closes the loop.

This does NOT hold for arbitrary `usize` inputs — always use enum-derived indices or
check bounds explicitly in `inc()` for runtime-constructed indices.

### 15.3 `IntervalHistogram` single-producer requirement

If `tick_with()` is called from multiple vCPU threads simultaneously (without a
per-vCPU instance), the `window_accum` and `last_window` updates are not atomic with
respect to each other — a window boundary can be detected by two threads simultaneously,
each swapping the accumulator and recording a possibly-incomplete value.

**Correct pattern for multi-vCPU:**

```rust
// In SpySession, create one IntervalHistogram per vCPU:
let per_vcpu_hist: Vec<Arc<IntervalHistogram>> = (0..n_vcpus)
    .map(|i| IntervalHistogram::new(format!("ipc_hist.vcpu{i}"), 1000, edges.clone()))
    .collect();

// Subscribe each to the corresponding vCPU's probe:
for (vcpu_idx, hist) in per_vcpu_hist.iter().enumerate() {
    let h = Arc::clone(hist);
    cpu_probes[vcpu_idx].post_step.subscribe(move |ev| {
        h.tick_with(1 /* or a derived metric */, ev.insn_count);
    });
}

// In quantum_end: merge all per-vCPU histograms into a shared one if needed.
```

### 15.4 `HeatMap` and the DashMap optional dep

```toml
# In the workspace Cargo.toml, add to [workspace.dependencies]:
dashmap = "6"

# In helm-spy/Cargo.toml:
[features]
dashmap = ["dep:dashmap"]

[dependencies]
dashmap = { version = "6", optional = true }
```

When `dashmap` is not enabled, the sharded `[Mutex<HashMap>; 16]` fallback provides
equivalent semantics with 16× lower worst-case contention than a single global `Mutex`.
For benchmarks where `HeatMap` is not on the critical path, either backend is adequate.

### 15.5 Closures capturing `Arc<Self>` — lifetime and safety

All `subscribe_to_*()` methods capture `Arc::clone(self)`, not `&self`. The probe
(which holds the closure) is owned by `HelmEngine<T>`. The `Arc` ensures that the
primitive outlives the probe's closure — the probe cannot be dropped while the `Arc`
count is non-zero, and the `Arc`-held primitive cannot be dropped while the probe holds
the closure.

There is no `unsafe` in the subscribe patterns. The `unsafe` is confined to:
- `Scoreboard::get_mut()` — per-slot UnsafeCell access
- `TraceRing::push()` — `ptr::write` into `MaybeUninit` slot
- `TraceRing::drain_into()` — `ptr::read` from initialized slot

Each `unsafe` block has a `// SAFETY:` comment explaining the invariant that makes it
sound.

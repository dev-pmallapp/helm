# helm-spy — High-Level Design

> **Crate:** `debug/helm-spy`
> **Layer:** 2 — Analysis Primitives
> **Replaces:** `helm-plugin` (motivation; helm-plugin is not present as a dependency)
> **Status:** Implemented; 74 tests pass

---

## 1. Purpose and Motivation

`helm-spy` is the analysis-primitive and session layer for helm-ng instrumentation.
It is located at `debug/helm-spy/` and is **never on the hot path** — it is analysis
tooling, not the execution engine.

### 1.1 Core principle: collection is not delivery

Everything in `helm-spy` accumulates data. Nothing in this crate formats, delivers,
or writes to I/O. This separation is enforced structurally: `helm-spy` has no
dependency on any reporting or delivery crate.

```
COLLECTION (per-instruction, per-event)
    counter.inc()                  <- fetch_add(Relaxed)
    indexed_counter.inc(class)     <- slice index + fetch_add(Relaxed)
    heatmap.inc(pc)                <- DashMap shard lock
    trace_ring.push(branch_record) <- lock-free SPSC

    (no Mutex in callbacks; no heap allocation per event)

DELIVERY (explicit, off hot path)
    session.snapshot()             <- returns SpySnapshot
    caller inspects fields directly
```

**Enforced rules:**
1. No heap allocation per event in the hot path.
2. No `Mutex::lock()` in the hot loop — only `RingBuffer<T>` and `EventStream<T>`
   use Mutex, and they are explicitly for low-rate events (faults, syscalls).
3. `helm-spy` has no dependency on any reporting or delivery crate. Data is
   collected here; delivery happens in caller code.

### 1.2 Dependency

`helm-spy` has exactly one external dependency: `dashmap` (for `HeatMap`).
It has no helm-* dependencies. It is a standalone analysis layer.

---

## 2. Position in the Instrumentation Stack

```
┌─────────────────────────────────────────────────────────────────┐
│  LAYER 1 — helm-probe (not yet wired to helm-spy)               │
│  Probe<T> typed event points. ProbePluginBridge not yet built.  │
└──────────────────────────────┬──────────────────────────────────┘
                               │  NOT YET IMPLEMENTED
                               │  ProbePluginBridge (src/bridge.rs)
                               │  will subscribe probe events,
                               │  enrich to InsnInfo/BranchInfo/etc.
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│  LAYER 2 — helm-spy  (this crate)                               │
│  Collection primitives. Session aggregator. Analysis models.    │
│  Counter, IndexedCounter, PerVcpuCounter, Histogram,            │
│  IntervalHistogram, HeatMap, RingBuffer, EventStream,           │
│  TraceRing, CorrelHist2D, Trigger, Window, QuantumObserver      │
│  InsnMix, CacheModel, BranchPredictor                           │
│  SpySession, SpySnapshot                                        │
└─────────────────────────────────────────────────────────────────┘
                               │  (not yet connected)
                               ▼
  LAYER 3 — helm-report (not yet built)
  Formatters + Sinks. Reads SpySnapshot; not a dep of helm-spy.
```

---

## 3. What Is Implemented

### 3.1 Primitives (`src/primitives/`)

All 10 primitives are implemented and tested:

| Primitive | File | Description |
|---|---|---|
| `Counter` | `counter.rs` | Monotonic `AtomicU64`. `inc()`, `add(n)`, `value()`, `reset()`. |
| `PerVcpuCounter` | `counter.rs` | `Vec<AtomicU64>` slot per vCPU. `inc(vcpu)`, `add(vcpu, n)`, `total()`, `per_vcpu()`. |
| `IndexedCounter` | `indexed.rs` | Fixed `Vec<AtomicU64>` keyed by label index. `inc(idx)`, `add(idx, n)`, `value(idx)`, `total()`, `fraction(idx)`, `table()`. |
| `Histogram` | `histogram.rs` | Fixed-bucket `Vec<AtomicU64>` with edge list. `record(val)`, `counts()`, `total()`, `percentile(p)`, `reset()`. Bucketing via `partition_point`. |
| `IntervalHistogram` | `histogram.rs` | Wraps `Histogram`. `tick(value, insn_count)` — records per-window accumulated value when insn_count crosses a window boundary. |
| `HeatMap` | `heatmap.rs` | `DashMap<u64, u64>`. `inc(pc)`, `top(n)`, `get(pc)`, `len()`, `clear()`. |
| `RingBuffer<T>` | `ringbuf.rs` | `Mutex<VecDeque<T>>`, fixed capacity. Overwrites oldest on overflow. `push(val)`, `snapshot()`, `clear()`, `capacity()`. |
| `EventStream<T>` | `ringbuf.rs` | `Mutex<Vec<T>>`, bounded at `max`. Stops when full; returns false. `push(val) -> bool`, `drain() -> Vec<T>`, `len()`. |
| `TraceRing<T: Copy+Send>` | `trace_ring.rs` | Lock-free SPSC ring buffer. Capacity must be power of 2. `push(val) -> bool`, `drain_into(&mut Vec<T>)`, `len()`, `capacity()`. |
| `CorrelHist2D` | `correl.rs` | 2D joint histogram. Flat `Vec<AtomicU64>` row-major. `record(x, y)`, `get(xi, yi)`, `matrix()`, `total()`, `reset()`. |

Also exported from `primitives/mod.rs`: `BranchRecord` (`repr(C)`, 32 bytes, from `trace_ring.rs`).

### 3.2 Event Types (`src/events.rs`)

- `InsnClass` enum — 11 variants (IntAlu=0 through Unknown=10), `COUNT: usize = 11`, `LABELS: [&str; 11]`
- `BranchKind` enum — 6 variants
- `ArchContext` enum — `None` (default), `Aarch64 { x, sp, pc, nzcv, fpsr }`, `Riscv64 { x, pc }`
- `FaultKind` enum — `InsnAbort, DataAbort, StoreAbort, Svc, Undefined, Other`
- `InsnInfo` struct — vcpu_idx, pc, raw, size, class, opcode_name, is_stub, context, insn_count
- `BranchInfo` struct — pc, target, taken, kind, insn_count
- `MemInfo` struct — vaddr, size, is_store, is_atomic, pc
- `SyscallInfo` struct — vcpu_idx, nr, args [u64; 6], pc
- `SyscallRetInfo` struct — vcpu_idx, nr, retval
- `FaultInfo` struct — vcpu_idx, pc, raw, kind, message, insn_count, context

### 3.3 Trigger System (`src/trigger.rs`)

`TriggerKind` enum with 4 variants:
- `AtInsn(u64)` — fires once when insn_count == N
- `EveryN(u64)` — fires when insn_count % N == 0 (never fires if N == 0)
- `AtPc(u64)` — fires when pc == addr (exact match)
- `PcRange(u64, u64)` — fires while pc in [start, end)

`Trigger` struct: `kind`, `action: Box<dyn Fn(u64, u64) + Send + Sync>` (args: pc, insn_count),
`armed: AtomicBool`, `one_shot: bool`. Methods: `check(pc, insn_count) -> bool`,
`is_armed()`, `arm()`, `disarm()`.

Hot-loop cost: one `AtomicBool::load(Relaxed)` + one comparison.

### 3.4 Window Gating (`src/window.rs`)

`Window { start: u64, end: u64, active: AtomicBool }` — instruction-count range [start, end).
`is_active(insn_count) -> bool` (updates cached state), `is_active_cached() -> bool`.

`Windowed<T> { window: Arc<Window>, inner: T }` — gates access to `inner` to inside-window only.
`get_if_active(insn_count) -> Option<&T>`.

### 3.5 QuantumObserver trait (`src/quantum.rs`)

```rust
pub trait QuantumObserver: Send + Sync {
    fn quantum_end(&mut self, vcpu: usize, insn_count: u64);
}
```

Called at every `run()` return and before checkpoint save. Off hot path — may use Mutex.

### 3.6 Analysis Models (`src/analysis/`)

- `InsnMix` — wraps `IndexedCounter` over `InsnClass`. `record(class)`, `table()`, `total()`, `value(class)`, `fraction(class)`, `reset()`.
- `CacheModel` — LRU set-associative cache. `access(addr)`, `hit_rate()`, `hits()`, `misses()`, `mpki(insn_count)`, `reset()`.
- `BranchPredictor` — 2-bit saturating counters. `PredictorKind` enum: `BiModal { bits }` and `GShare { hist_bits, table_bits }`. `predict_and_update(pc, taken)`, `miss_rate()`, `mpki(insn_count)`, `predictions()`, `mispredictions()`, `reset()`.

### 3.7 Session (`src/session.rs`)

`SpySession` — user-facing aggregator. Owns all configured primitives as direct fields
(not Arc-wrapped). Builder pattern for optional models.

`SpySnapshot` — point-in-time snapshot. Fields: `insn_count: u64`, `insn_mix_table`,
`hot_pcs_top20`, `cache_hit_rate: Option<f64>`, `branch_miss_rate: Option<f64>`.

---

## 4. What Is NOT Yet Wired / Implemented

| Feature | Status |
|---|---|
| `ProbePluginBridge` (`src/bridge.rs`) | Not yet built; helm-spy does not subscribe to helm-probe events |
| PyO3 Python bindings | Not yet built; no `#[pyclass]` in this crate |
| `helm-report` delivery layer | Not yet built; `SpySnapshot` fields are public for direct inspection |
| `SimPoint` basic-block vector computation | Phase 3, not yet designed for this crate |
| `PowerModel` per-class energy estimation | Phase 3, not yet designed |
| `DiffAnalysis` session differential | Phase 3, not yet designed |
| `CpuProbes`/`GicProbes` wiring | No `subscribe()` method on `SpySession` yet |
| `QuantumObserver` registration in `SpySession` | Not implemented; `SpySession` does not hold observers |

---

## 5. Hot-Path Cost Summary

| Primitive | Operations | Lock? | Alloc? |
|---|---|---|---|
| `Counter` | 1x `fetch_add(Relaxed)` | No | No |
| `PerVcpuCounter` | 1x slice index + `fetch_add(Relaxed)` | No | No |
| `IndexedCounter` | 1x slice index + `fetch_add(Relaxed)` | No | No |
| `Histogram` | 1x `partition_point` + `fetch_add(Relaxed)` | No | No |
| `IntervalHistogram` | 1x division + `swap` + conditional `record()` | No | No |
| `HeatMap` | 1x DashMap shard lock (brief critical section) | Shard | No |
| `RingBuffer<T>` | 1x `Mutex::lock()` + `VecDeque::push_back()` | Yes | On evict |
| `EventStream<T>` | 1x `Mutex::lock()` + `Vec::push()` (until full) | Yes | On push |
| `TraceRing<T>` | 1x `ptr::write` + `AtomicUsize::store(Release)` | No | No |
| `CorrelHist2D` | 2x `partition_point` + `fetch_add(Relaxed)` | No | No |

`RingBuffer<T>` and `EventStream<T>` are suitable only for low-rate events (faults,
syscalls). For per-instruction traces, use `TraceRing<T>`.

---

## 6. QuantumObserver Flush Protocol

The `QuantumObserver` trait enables primitives with local per-vCPU state to merge
into shared aggregates at quantum boundaries (i.e., every `run()` return), without
holding locks during the hot loop.

Invariant: no `Mutex::lock()` inside probe callbacks. All global-lock aggregation
is deferred to `quantum_end()`, which runs on the cold path.

Currently no primitives in `helm-spy` are registered as `QuantumObserver` implementors
in `SpySession` — the trait exists for future use when probe-subscription wiring is added.

---

## 7. Module Structure

```
debug/helm-spy/
├── Cargo.toml               (deps: dashmap only)
└── src/
    ├── lib.rs               (crate root; pub mod declarations)
    ├── events.rs            (InsnInfo, BranchInfo, MemInfo, SyscallInfo, FaultInfo,
    │                         InsnClass, BranchKind, ArchContext, FaultKind)
    ├── quantum.rs           (QuantumObserver trait)
    ├── trigger.rs           (TriggerKind enum, Trigger struct)
    ├── window.rs            (Window, Windowed<T>)
    ├── session.rs           (SpySession, SpySnapshot)
    ├── analysis/
    │   ├── mod.rs           (pub use InsnMix, CacheModel, BranchPredictor, PredictorKind)
    │   ├── insn_mix.rs      (InsnMix, INSN_CLASS_LABELS const)
    │   ├── cache.rs         (CacheModel, CacheState)
    │   └── branch_pred.rs   (BranchPredictor, PredictorKind)
    └── primitives/
        ├── mod.rs           (pub use all primitives)
        ├── counter.rs       (Counter, PerVcpuCounter)
        ├── indexed.rs       (IndexedCounter)
        ├── histogram.rs     (Histogram, IntervalHistogram)
        ├── heatmap.rs       (HeatMap)
        ├── ringbuf.rs       (RingBuffer<T>, EventStream<T>)
        ├── trace_ring.rs    (TraceRing<T>, BranchRecord)
        └── correl.rs        (CorrelHist2D)
```

---

## 8. Test Coverage

74 unit tests, all inline in source files. Run with:

```
cargo test --package helm-spy
```

See `docs/design/helm-spy/TEST.md` for the full test inventory.

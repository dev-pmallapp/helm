# Instrumentation v2 — Redesign Plan

> **Status**: Phase 1 complete. Probes wired into engine + GIC. Phase 3 (Python API, plugin removal) pending.
>
> | Deliverable | Status | Notes |
> |---|---|---|
> | `framework/helm-probe` | ✅ Implemented + wired | 17 tests; CpuProbes in HelmEngine, GicProbes in GicState |
> | `framework/helm-diag` | ✅ Implemented + wired | 50 tests; helm-arch + helm-engine use sim_stub!/warn!/info! |
> | `debug/helm-spy` | ✅ Implemented, standalone | 74 tests; not yet wired to probe events |
> | `debug/helm-report` | ✅ Implemented, standalone | 62 tests; local SpySessionSnapshot |
> | Workspace wiring | ✅ All four crates in workspace | debug/* glob added |
> | Probe wiring: helm-engine SE loop | ✅ Done | pre_step, post_step, branch, mem, fault |
> | Probe wiring: helm-engine FS loop | ✅ Done | pre_step, post_step, branch, fault variants |
> | Probe wiring: helm-hw-intc GICv2 | ✅ Done | irq_asserted, irq_deasserted, eoi (feature="probe") |
> | ProbePluginBridge | ☐ Phase 2 | Connects probe events → helm-spy SpySession |
> | helm-spy Python API (sim.spy()) | ☐ Phase 3 | PyO3 bindings for SpySession |
> | helm-plugin removal | ☐ Phase 3 | Still active; backward compat needed |
> | helm-report ← helm-spy dep | ☐ Phase 3 | SpySessionSnapshot migration |
>
> **Scope**: Eliminate `sim_trace` / `sim_branch!` / `sim_stub!` / `sim_warn!` / `sim_info!`.
> Restructure `helm-probe`, `helm-plugin`, and `helm-debug` into a coherent system for
> architectural exploration.
>
> **Core constraints:**
> 1. **Collection fully decoupled from delivery** — probe sites write to typed in-process structures; delivery (file/stderr/TCP/Python) is a separate, explicit, triggered operation.
> 2. **Zero hot-loop allocation** — no `String`, `Box`, `Vec::push`, or `Mutex::lock` in the per-instruction path.
> 3. **Zero hot-loop cross-thread synchronization** — per-thread local state, merged at quantum boundaries.
> 4. **Typed everywhere** — no string formatting of analysis data; typed `repr(C)` binary for traces.

---

## 1. What Is Wrong With the Current System

### 1.1 sim_trace: the string-log problem

`sim_trace` turns everything into a formatted string, mixes levels (`INFO`, `STUB`, `WARN`,
`BRNC`) into one channel, and writes them to a file or stderr. This has several fatal flaws
for architectural exploration:

- **Unanalyzable**: `branch_trace.py` must parse `[BRNC] sim_ns=... pc=... | -> 0x4000`
  back into numbers. String → program → string → parse is not a data pipeline.
- **No structure**: a branch at pc=X to target=Y with `taken=true` and `kind=Call` is
  identical in the log to a branch at the same PC with different kind. You cannot group
  by kind, correlate with IPC, or plot a histogram from string output.
- **Single channel**: every diagnostic (stubs, warnings, branches, boot messages) competes
  in one stream. You cannot subscribe only to branches.
- **Formatting in the hot loop**: `format!("-> {:#018x}", target)` allocates a String on
  every branch in dev builds. This distorts performance measurements.
- **sim_branch! has cost in release**: unlike `Probe<T>` (ZST in release), `sim_branch!`
  always calls `emit()` + `try_send()`, even in `--release`. This is inconsistent and wrong
  for a perf-critical simulator.

### 1.2 helm-plugin: collection and delivery are mixed

Every plugin (`BranchTrace`, `HotBlocks`, `CacheSim`, …) owns BOTH:
- The analysis state (`Arc<Mutex<HashMap<u64, BranchRecord>>>`)
- The delivery (`fn atexit(&mut self) { eprintln!(...) }`)

This means:
- You cannot redirect a plugin's output to a file without modifying the plugin.
- You cannot query a plugin's state mid-simulation from Python without adding a new method.
- `Arc<Mutex<T>>` inside every hot-loop callback is a contention bottleneck under
  multi-vCPU simulation.
- `PluginArgs` (`HashMap<String, String>`) errors manifest at runtime, not compile time.
- There is no way to compose primitives (e.g. "count L1 misses while a specific function
  is executing") without writing a new bespoke plugin.

### 1.3 helm-debug: scope confusion

`helm-debug` currently contains:
- `GdbServer` (stub) — appropriate here
- `CheckpointManager` (stub) — appropriate here
- `TraceLogger` (stub, routes sim_trace events to `.jsonl`) — should not exist
- `sim_trace` module — the entire string-log infrastructure

After the redesign, `helm-debug` should be pure developer tooling: GDB, checkpoints,
watchpoints, breakpoints. Nothing else.

---

## 2. Core Design Principle: Collection ≠ Delivery

This is the single most important principle of the redesign.

```
┌──────────────────────────────────────────────────────┐
│  COLLECTION (hot path — per instruction/event)        │
│                                                        │
│  probe!(probes.post_step, CpuStepEvent{pc, raw})      │
│      ↓ subscriber (in-process, zero-alloc)            │
│  counter.inc()              ← atomic add              │
│  histogram.record(class)    ← array index + add       │
│  ring_buf.push(event)       ← fixed-size circular     │
│  heatmap.inc(pc)            ← unsynchronized per-thd  │
└──────────────────────────────────────────────────────┘
                        ↓  (triggered, explicit)
┌──────────────────────────────────────────────────────┐
│  DELIVERY (cold path — explicit, triggered, async)    │
│                                                        │
│  session.report()                                      │
│      ↓                                                 │
│  ReportFormatter::format(data)  → formatted bytes     │
│      ↓                                                 │
│  Sink::deliver(bytes)  → File | Stderr | TCP | Null   │
│                        → Python buffer | protobuf      │
└──────────────────────────────────────────────────────┘
```

**Rules:**
1. Nothing in the collection path allocates (except EventStream, which is opt-in).
2. Nothing in the collection path blocks on I/O or a lock (except EventStream Mutex, bounded).
3. Delivery is always explicit: `session.report(sink)`, or scheduled via `ReportSchedule`.
4. Formatting is the Sink's concern — not the collection primitive's concern.
5. The same collected data can be delivered to multiple sinks simultaneously.

---

## 3. New Crate Structure

### Current (to be replaced)

```
framework/helm-probe/    ← keep, minimal change
framework/helm-plugin/   ← restructure → helm-spy
runtime/helm-debug/      ← narrow; extract sim_trace → helm-diag
  src/sim_trace.rs       ← MOVE to framework/helm-diag/ (keep mechanism, rename)
  src/lib.rs             ← keep GdbServer, CheckpointManager; lose TraceLogger
```

### New

```
framework/helm-probe/     ← Layer 1: zero-cost typed probe points (keep)
framework/helm-diag/      ← Layer 1b: diagnostic emit (tiny; extracted from helm-debug)
debug/helm-spy/   ← Layer 2: analysis primitives (replaces helm-plugin)
debug/helm-report/    ← Layer 3: delivery (new; replaces sim_trace backend)
runtime/helm-debug/       ← Layer 4: GDB RSP + Checkpoint + Breakpoint (narrowed)
```

### Crate DAG

```
helm-core   (zero deps)
helm-probe  (zero deps)
helm-diag   (deps: none, or helm-core for AttrRegistry)
    ↑ sim_stub!/sim_warn!/sim_info! call sites
helm-arch, helm-devices, helm-hw-*  (dep: helm-diag for diagnostics)

helm-spy  (deps: helm-probe, helm-diag types)
    ↑ subscribed during build_simulator()
helm-report   (deps: helm-spy for types)
helm-debug    (deps: helm-probe, helm-diag; opens DiagSink at startup)
helm-engine   (deps: helm-probe, helm-diag, helm-spy, helm-report, helm-debug)
helm-python   (deps: helm-engine; PyO3 boundary)
```

No cycles. `helm-arch` and `helm-devices` depend on `helm-diag` but not on `helm-debug`.
This resolves the current layer violation where `helm-arch` transitively depends on
`helm-debug` via `sim_stub!`.

---

## 4. Layer 1: `helm-probe` (kept, minor extensions)

### Changes from current design

1. Add `BranchEvent` to standard event types (currently missing from the probe layer).
2. Add `branch: Probe<BranchEvent>` to `CpuProbes`.
3. Add `MmioEvent` to `SystemMem` dispatch sites.
4. No other changes — the `Probe<T>` + `probe!()` design is correct.

### `BranchEvent` (new)

```rust
// framework/helm-probe/src/events.rs
#[derive(Debug, Clone)]
pub struct BranchEvent {
    pub pc:     u64,
    pub target: u64,
    pub taken:  bool,
    pub kind:   BranchKind,   // DirectCond | DirectUncond | Call | Return | IndirectJump | IndirectCall
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchKind {
    DirectCond, DirectUncond, Call, Return, IndirectJump, IndirectCall,
}
```

`branch.rs` in `helm-arch` calls `probe!(probes.branch, BranchEvent { pc, target, taken, kind })`
instead of `sim_branch!(pc=pc, target=target)`. **sim_branch! is deleted.**

---

## 5. Layer 2: `helm-spy` (new — replaces `helm-plugin`)

### 5.1 Philosophy

`helm-spy` provides **analysis primitives** — data structures that can be subscribed to
probe events and accumulate observations. Primitives are composable: you build a session
from primitives, not from monolithic plugins. The `HelmPlugin` trait and `PluginRegistry`
are removed.

### 5.2 Analysis Primitives

Each primitive is a struct that:
- Holds its data (counter, array, map, ring)
- Exposes a `subscribe_to(probe: &mut Probe<T>)` method that registers a closure
- Exposes query methods (`value()`, `top(n)`, `percentile(p)`, etc.)
- Is `Clone`-able for checkpointing
- Has no delivery logic — no `atexit()`, no `eprintln!()`

```rust
// debug/helm-spy/src/primitives/

/// Monotonic counter. Thread-safe via AtomicU64.
pub struct Counter {
    name: String,
    value: AtomicU64,
}
impl Counter {
    pub fn inc(&self) { self.value.fetch_add(1, Ordering::Relaxed); }
    pub fn add(&self, n: u64) { self.value.fetch_add(n, Ordering::Relaxed); }
    pub fn value(&self) -> u64 { self.value.load(Ordering::Relaxed) }
    pub fn subscribe_to_steps(&self, probes: &mut CpuProbes) { … }
}

/// Fixed-bucket histogram. Lock-free array of AtomicU64.
pub struct Histogram {
    name: String,
    buckets: Vec<AtomicU64>,
    bucket_fn: Box<dyn Fn(&InsnInfo) -> usize + Send + Sync>,
}
impl Histogram {
    pub fn record(&self, idx: usize) { … }
    pub fn counts(&self) -> Vec<u64> { … }
    pub fn percentile(&self, p: f64) -> usize { … }
    pub fn subscribe_to_insns(&self, probes: &mut CpuProbes) { … }
}

/// Per-PC u64 counter map. Lock-free via DashMap or sharded.
pub struct HeatMap {
    name: String,
    counts: DashMap<u64, u64>,
}
impl HeatMap {
    pub fn inc(&self, pc: u64) { … }
    pub fn top(&self, n: usize) -> Vec<(u64, u64)> { … }  // (pc, count)
    pub fn subscribe_to_steps(&self, probes: &mut CpuProbes) { … }
    pub fn subscribe_to_branches(&self, probes: &mut CpuProbes) { … }
}

/// Circular buffer of typed events. Fixed capacity, overwrites oldest.
/// Bounded Mutex<VecDeque<T>> — small constant overhead per event.
pub struct RingBuffer<T: Clone + Send> {
    name: String,
    capacity: usize,
    buf: Mutex<VecDeque<T>>,
}
impl<T: Clone + Send> RingBuffer<T> {
    pub fn push(&self, ev: T) { … }
    pub fn snapshot(&self) -> Vec<T> { … }  // clone current contents
}

/// Growing event stream. Bounded — stops recording at `max`.
pub struct EventStream<T: Clone + Send> {
    name: String,
    max: usize,
    events: Mutex<Vec<T>>,
}
impl<T: Clone + Send> EventStream<T> {
    pub fn push(&self, ev: T) { … }
    pub fn drain(&self) -> Vec<T> { … }
}

/// Per-thread, per-vCPU counter. Zero cross-thread sync during hot loop.
/// Aggregated on read.
pub struct PerVcpuCounter {
    name: String,
    slots: Vec<AtomicU64>,   // one per vCPU
}
impl PerVcpuCounter {
    pub fn inc(&self, vcpu: usize) { … }
    pub fn total(&self) -> u64 { … }
    pub fn per_vcpu(&self) -> Vec<u64> { … }
}
```

### 5.3 Instruction-Class Analysis (replaces `HowVec`, `InsnCount`)

```rust
pub struct InsnMix {
    name: String,
    counts: [AtomicU64; InsnClass::COUNT],
}
impl InsnMix {
    pub fn record(&self, class: InsnClass) {
        self.counts[class as usize].fetch_add(1, Ordering::Relaxed);
    }
    pub fn subscribe_to_insns(&mut self, probes: &mut CpuProbes) {
        let ptr = Arc::new(self as *const _);  // or use Arc<Self>
        probes.post_step.subscribe(move |ev| { … });
    }
    pub fn table(&self) -> Vec<(InsnClass, u64, f64)> { … }  // class, count, pct
}
```

### 5.4 Cache Model (replaces `CacheSim`)

```rust
pub struct CacheModel {
    name: String,
    sets: usize, ways: usize, line_size: usize,
    tags: Vec<Vec<u64>>,    // [set][way] = tag
    lru:  Vec<Vec<u8>>,     // [set][way] = LRU counter
    // Stats
    hits:   AtomicU64,
    misses: AtomicU64,
}
impl CacheModel {
    pub fn access(&mut self, vaddr: u64) -> CacheResult { … }
    pub fn hit_rate(&self) -> f64 { … }
    // Note: CacheModel is NOT thread-safe (uses &mut self for LRU update).
    // Subscribe via a Mutex<CacheModel> wrapper.
    pub fn subscribe_to_mem(&self, probes: &mut CpuProbes) { … }
}
```

### 5.5 Branch Predictor Model (new)

```rust
pub struct BranchPredictor {
    name: String,
    kind: PredictorKind,   // BiModal | TwoLevel | GShare | Perfect
    predictions: u64,
    mispredictions: u64,
    table: Vec<u8>,        // 2-bit saturating counter table
}
impl BranchPredictor {
    pub fn predict(&mut self, pc: u64, taken: bool) -> bool { … }
    pub fn miss_rate(&self) -> f64 { … }
    pub fn subscribe_to_branches(&mut self, probes: &mut CpuProbes) { … }
}
```

### 5.6 Conditional Trigger (replaces ad-hoc counters + trace_after)

```rust
pub struct Trigger {
    name: String,
    condition: Box<dyn Fn() -> bool + Send + Sync>,
    action: Box<dyn Fn() + Send + Sync>,
    fired: AtomicBool,
    one_shot: bool,
}
impl Trigger {
    /// Fire when instruction count reaches N.
    pub fn at_insn(n: u64, action: impl Fn() + Send + Sync + 'static) -> Self { … }
    /// Fire when PC == target.
    pub fn at_pc(pc: u64, action: impl Fn() + Send + Sync + 'static) -> Self { … }
    /// Fire when arbitrary predicate returns true.
    pub fn when(pred: impl Fn() -> bool + Send + Sync + 'static,
                action: impl Fn() + Send + Sync + 'static) -> Self { … }
}
```

### 5.7 Time Window (replaces `trace_after`)

```rust
/// Activate an analysis primitive only between instruction counts [start, end).
pub struct Window {
    start: u64,
    end:   u64,
    active: AtomicBool,
}
impl Window {
    pub fn is_active(&self, insn_count: u64) -> bool { … }
    /// Wrap a primitive: only record events while inside the window.
    pub fn gate<T>(&self, inner: Arc<T>) -> Windowed<T> { … }
}
```

### 5.8 IndexedCounter — Dimension-Keyed Counter Array (new — critical)

The most important primitive missing from the existing design. Answers "per-class" questions
without bespoke plugins: SIMD utilization, miss rate by instruction class, branch type
breakdown.

```rust
/// Fixed-size counter array indexed by a dimension (e.g. InsnClass, BranchKind).
/// Lock-free: one AtomicU64 per bucket.
pub struct IndexedCounter {
    name:    String,
    labels:  Vec<&'static str>,     // e.g. ["IntAlu", "Load", "Store", "Branch", "SIMD"]
    buckets: Vec<AtomicU64>,        // one per label; cache-line padded for high-rate dims
}

impl IndexedCounter {
    #[inline(always)]
    pub fn inc(&self, idx: usize) {
        // Bounds check elided by optimizer when idx comes from a match on an enum.
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
    }
    pub fn values(&self) -> Vec<(&&'static str, u64)> { … }
    pub fn total(&self) -> u64 { … }
    pub fn fraction(&self, idx: usize) -> f64 { … }
}
```

Hot-loop cost: array index + `fetch_add(Relaxed)` ≈ 1–2 ns, no allocation, no locking.
This replaces `HowVec` as a first-class primitive rather than a one-off plugin.

### 5.9 IntervalHistogram — Time-Series Distribution (new — critical)

Answers "IPC distribution over 1000-instruction intervals" and "how does cache miss rate
vary over simulation time?" A plain histogram loses phase information; an IntervalHistogram
captures the shape of the time-varying signal.

```rust
/// Collect a scalar measurement every `window_size` instructions; bucket the measurements.
pub struct IntervalHistogram {
    window_size:    u64,
    hist:           Histogram,      // bucketed distribution of per-window values
    window_accum:   AtomicU64,      // accumulator within current window
    last_window:    AtomicU64,      // last window index seen
}

impl IntervalHistogram {
    /// Call on every instruction. Records a sample when a window boundary is crossed.
    #[inline]
    pub fn tick_with(&self, value: u64, insn_count: u64) {
        let window = insn_count / self.window_size;
        let prev = self.last_window.load(Ordering::Relaxed);
        if window != prev {
            // Window boundary: commit accumulated value and reset.
            let sample = self.window_accum.swap(value, Ordering::Relaxed);
            self.hist.record(sample);
            self.last_window.store(window, Ordering::Relaxed);
        } else {
            self.window_accum.fetch_add(value, Ordering::Relaxed);
        }
    }
}
```

### 5.10 CorrelHist2D — Joint Distribution (new — Phase 3)

Answers "D-cache miss rate conditioned on branch predictor state" and similar two-dimensional
architectural questions. Not achievable with any combination of independent counters.

```rust
pub struct CorrelHist2D {
    name:     String,
    edges_x:  Vec<u64>,
    edges_y:  Vec<u64>,
    counts:   Vec<AtomicU64>,   // flat [x_buckets * y_buckets] for cache locality
}

impl CorrelHist2D {
    #[inline]
    pub fn record(&self, x: u64, y: u64) {
        let bx = self.edges_x.partition_point(|&e| x >= e);
        let by = self.edges_y.partition_point(|&e| y >= e);
        self.counts[bx * self.edges_y.len() + by].fetch_add(1, Ordering::Relaxed);
    }
}
```

### 5.11 TraceRing — Typed Binary Ring Buffer (new — replaces sim_trace for traces)

A lock-free, fixed-capacity, single-producer ring for typed event records. Zero heap
allocation after construction. This is the fundamental data structure for all trace
collection — it replaces the `sim_trace` string log channel for analysis data.

```rust
/// Lock-free single-producer ring. N must be a power of 2.
/// T must be Copy (no drop, no allocation on write).
pub struct TraceRing<T: Copy + Send, const N: usize> {
    buf:  Box<[MaybeUninit<T>; N]>,
    head: AtomicU64,   // write cursor — only the producer touches this
    tail: AtomicU64,   // read cursor — only the consumer touches this
}

impl<T: Copy + Send, const N: usize> TraceRing<T, N> {
    /// Write one record. Overwrites oldest if full (lossy — caller notes drops).
    #[inline(always)]
    pub fn push(&self, val: T) {
        let h = self.head.load(Ordering::Relaxed);
        unsafe { (*self.buf[h as usize & (N - 1)].as_ptr() as *mut T).write(val); }
        self.head.store(h.wrapping_add(1), Ordering::Release);
    }

    /// Read all available records (consumer side). Zero-copy.
    pub fn drain_into(&self, out: &mut Vec<T>) { … }
}
```

The canonical branch trace record:

```rust
/// 32 bytes — two cache words, fully typed, `repr(C)` for Python `mmap` + `struct.unpack`.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct BranchRecord {
    pub pc:         u64,
    pub target:     u64,
    pub insn_count: u64,
    pub flags:      u8,    // bits: taken=0, predicted=1, kind=2..4, spare=5..7
    pub _pad:       [u8; 7],
}
```

At 32 bytes/record: a 1M-entry ring = 32 MB (fits in L3). Background drain thread writes
to a typed binary file; Python reads with `mmap` + `struct.unpack_from` — no text parsing.

### 5.12 Trigger System — Arm/Disarm Collection (new — critical)

The most important feature currently absent from helm-ng. Without triggers, you cannot do
ROI (Region of Interest) analysis, warmup skip, or phase-conditional collection.

```rust
pub enum TriggerKind {
    AtInsn(u64),                    // fire once when insn_count == N
    EveryN(u64),                    // fire every N instructions
    AtPc(u64),                      // fire when PC == addr
    PcRange(u64, u64),              // fire when PC enters [start, end)
    CounterReaches(Arc<Counter>, u64), // fire when counter.value() >= N
}

/// Single trigger. Checked on every instruction.
pub struct Trigger {
    kind:    TriggerKind,
    action:  Box<dyn Fn(&TriggerCtx) + Send + Sync>,
    armed:   AtomicBool,    // false = check skipped (predicted-not-taken)
    one_shot: bool,
}

impl Trigger {
    /// Called in the pre-step probe. Returns true on fire.
    #[inline]
    pub fn check(&self, pc: u64, insn_count: u64) -> bool {
        if !self.armed.load(Ordering::Relaxed) { return false; }
        let fired = match &self.kind {
            TriggerKind::AtInsn(n)       => insn_count == *n,
            TriggerKind::EveryN(n)       => insn_count % n == 0,
            TriggerKind::AtPc(addr)      => pc == *addr,
            TriggerKind::PcRange(s, e)   => pc >= *s && pc < *e,
            TriggerKind::CounterReaches(c, n) => c.value() >= *n,
        };
        if fired {
            if self.one_shot { self.armed.store(false, Ordering::Relaxed); }
            (self.action)(&TriggerCtx { pc, insn_count });
        }
        fired
    }
}
```

Hot-loop cost: one `AtomicBool::load(Relaxed)` + one comparison ≈ 1 ns.
Predicted-not-taken when not yet armed = essentially free.

**Trigger actions must never block.** For heavy actions (file write, Python callback),
the action posts a message to a channel; a background thread does the I/O.

### 5.13 SimPoint BBV Computation (new — Phase 3)

```rust
pub struct SimPoint {
    interval: u64,          // instructions per interval (default 100M)
    bbv: Vec<Vec<u64>>,     // basic block vectors, one per interval
    current: HashMap<u64, u64>,  // current interval's BB counts
    insn_in_interval: u64,
}
impl SimPoint {
    pub fn subscribe_to_branches(&mut self, probes: &mut CpuProbes) { … }
    pub fn finish_interval(&mut self) { … }  // called by timer trigger
    pub fn export(&self) -> SimPointData { … }  // for SimPoint tool
}
```

### 5.14 Quantum-End Flush Protocol

The hot loop must never acquire cross-thread locks. Per-vCPU local state (thread-local
hashmaps, local counters) is the canonical collection pattern. Merging happens at quantum
boundaries — once per `run()` call.

```rust
/// Notification sent to each observer when a vCPU quantum completes.
pub trait QuantumObserver: Send + Sync {
    fn quantum_end(&mut self, vcpu: usize, insn_count: u64);
}
```

The existing `Scoreboard<T>` is exactly the right primitive for per-vCPU local slots — it
is kept and used by `helm-spy` as the lock-free local state backing. Plugins that need
cross-vCPU aggregation do it in `quantum_end()`.

**Rule**: no `Mutex::lock()` inside probe callbacks. `Arc<Mutex<T>>` inside callbacks is
banned. All cross-thread merging happens in `quantum_end()`.

### 5.15 SpySession

A `SpySession` is a **standalone attachable component** — it takes a `System` reference
and subscribes to its probes independently. It is NOT a SimObject; it does not appear
in the child hierarchy. Users can create multiple independent sessions, detach/reattach
them, and pass them across function boundaries.

In Python, it is constructed as `helm.SpySession(system, ...)` — not `system.spy()`.

```rust
pub struct SpySession {
    pub insn_count:     Counter,
    pub insn_mix:       InsnMix,
    pub hot_pcs:        HeatMap,
    pub branch_heatmap: HeatMap,
    pub cache_l1d:      Option<CacheModel>,
    pub branch_pred:    Option<BranchPredictor>,
    pub fault_history:  RingBuffer<CpuFaultEvent>,
    // … etc.
    triggers: Vec<Trigger>,
    windows:  Vec<Window>,
}

impl SpySession {
    /// Attach to a System's probes. Called by `helm.SpySession(system, ...)`.
    pub fn attach(&mut self, probes: &mut CpuProbes, gic: Option<&mut GicProbes>) { … }

    /// Detach from probes. Metrics are frozen at detach time.
    pub fn detach(&mut self) { … }
}
```

---

## 6. Layer 3: `helm-report` (new — replaces sim_trace delivery)

### 6.1 The Sink trait

```rust
// debug/helm-report/src/sink/mod.rs

/// A delivery destination for analysis reports.
pub trait Sink: Send + Sync {
    /// Write a single report (formatted bytes or structured data).
    fn write(&self, data: &[u8]) -> std::io::Result<()>;
    /// Flush any buffered data.
    fn flush(&self) -> std::io::Result<()> { Ok(()) }
}

pub struct FileSink   { path: PathBuf, file: Mutex<BufWriter<File>> }
pub struct StderrSink;
pub struct TcpSink    { stream: Mutex<TcpStream> }
pub struct NullSink;

// Async variant: background drain thread (like current MonitorSink)
pub struct AsyncFileSink {
    tx: SyncSender<Vec<u8>>,
    handle: Option<JoinHandle<()>>,
}
```

`FileSink` / `AsyncFileSink` replace `MonitorSink`. The URI-based constructor is kept:
```rust
pub fn sink_from_uri(uri: &str) -> Box<dyn Sink> { … }
// "stderr:" → StderrSink
// "file:/path" → AsyncFileSink
// "tcp:host:port" → TcpSink
// "null:" → NullSink
```

### 6.2 ReportFormatter

```rust
pub trait ReportFormatter: Send + Sync {
    fn format_session(&self, session: &SpySession) -> Vec<u8>;
    fn format_counter(&self, c: &Counter) -> Vec<u8>;
    fn format_histogram(&self, h: &Histogram) -> Vec<u8>;
    // … etc.
}

pub struct TextFormatter;    // human-readable (current atexit style)
pub struct JsonFormatter;    // JSON: {"name":"insn_count","value":12345678}
pub struct CsvFormatter;     // CSV: timestamp,metric,value
pub struct GemstatsFormatter;// gem5 stats.txt compatible
```

### 6.3 Binary Trace Sink (new — for TraceRing drain)

`TraceRing<T, N>` is drained by a background thread to a binary file. Python reads it
with `mmap` + `struct.unpack_from` — no text parsing.

```rust
/// Drains a TraceRing to a typed binary file asynchronously.
pub struct BinaryTraceSink<T: Copy + Send + 'static> {
    ring:   Arc<TraceRing<T, 1048576>>,   // 1M slots
    path:   PathBuf,
    handle: Option<JoinHandle<()>>,
}

impl<T: Copy + Send + 'static> BinaryTraceSink<T> {
    pub fn open(ring: Arc<TraceRing<T, 1048576>>, path: &Path) -> Self { … }
    // Background thread: ring.drain_into(&mut buf); file.write_all(cast_to_bytes(&buf));
}
```

Format header:

```c
// trace_header.h — also generated for Python
typedef struct {
    uint32_t magic;       // 0x48454C4D ("HELM")
    uint32_t version;     // 1
    uint32_t record_size; // sizeof(T)
    uint32_t record_count;
    char     type_name[64]; // e.g. "BranchRecord"
} TraceHeader;
```

Python consumer:
```python
import mmap, struct
hdr_fmt = "=IIII64s"
rec_fmt = "=QQQBxxxxxxx"  # BranchRecord: pc, target, insn_count, flags, 7-pad
with open("branch.trace", "rb") as f:
    mm = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
    hdr = struct.unpack_from(hdr_fmt, mm, 0)
    records = struct.iter_unpack(rec_fmt, mm[struct.calcsize(hdr_fmt):])
```

### 6.4 Report delivery

```rust
pub struct Report {
    session: Arc<SpySession>,
    formatter: Box<dyn ReportFormatter>,
    sinks: Vec<Box<dyn Sink>>,
}

impl Report {
    pub fn deliver(&self) {
        let data = self.formatter.format_session(&self.session);
        for sink in &self.sinks {
            let _ = sink.write(&data);
        }
    }
}

pub struct ReportSchedule {
    triggers: Vec<ReportTrigger>,
}

pub enum ReportTrigger {
    AtExit,
    EveryN(u64),              // every N instructions
    OnCounter { name: String, threshold: u64 },
    OnPc(u64),
    Explicit,
}
```

### 6.4 Diagnostic events — the `helm-diag` micro-crate

**Decision: keep a thin dedicated diagnostic channel; move it out of `helm-debug` to break the layer violation.**

The `log` crate was considered and rejected for diagnostic call sites. The reason: `log`
cannot carry structured fields (`component=`, `pc=`, `sim_ns=`) without reformatting them
into strings, and its `max_level_release` cost is a branch on a static filter — acceptable
for application code but measurable at 50+ device-stub call sites under tight simulation
loops.

The existing `sim_trace::emit()` mechanism is functionally correct for diagnostics. The
problem is its location: `helm-debug` depends on `helm-arch` which creates a circular dep.

**Fix: extract to `helm-diag` (new, tiny crate, ~200 lines):**

```
framework/helm-diag/
  src/
    lib.rs     emit(), SIM_MONITOR thread-local, sim_stub!, sim_warn!, sim_info!
    sink.rs    DiagSink (background drain thread, URI-based backend)
    entry.rs   DiagEntry { component, level, pc, sim_ns, sim_insns, message }
```

`helm-arch`, `helm-devices`, and `helm-engine` depend on `helm-diag` (not `helm-debug`).
`helm-debug` depends on `helm-diag` to open a `DiagSink` at startup — it owns the
lifecycle but not the emit path.

```rust
// Before (deleted):
sim_branch!(pc=pc, target=target);    // ← hot path, always-on string alloc

// Diagnostic macros survive but only for low-rate events:
sim_stub!(component="gicv2-dist", pc=self.pc, "MRS ID_AA64MMFR4_EL1 → 0");
sim_warn!(component="mmu", pc=pc, "page table walk hit limit");
sim_info!(component="loader", "ELF loaded at {:#x}", load_addr);
```

`sim_branch!` is **deleted** — the only survivor of the four original macros as branch
events become `Probe<BranchEvent>`. The diagnostic macros (`sim_stub!`, `sim_warn!`,
`sim_info!`) move from `helm-debug::sim_trace` to `helm-diag`.

**Layer DAG after change:**
```
helm-core / helm-diag  ←  helm-arch (stubs), helm-devices (stubs), helm-engine
helm-debug             ←  helm-engine (GDB hook), helm-python (checkpoint API)
                          opens DiagSink via helm-diag
```

No cycle. `helm-arch` no longer depends on `helm-debug`.

---

## 7. Layer 4: `helm-debug` (narrowed)

`helm-debug` loses `sim_trace.rs` entirely. It keeps and extends:

```
runtime/helm-debug/
  src/
    lib.rs
    checkpoint.rs     CheckpointManager — serialize ArchState to CBOR (Phase 2)
    gdb/              GDB RSP stub (Phase 2)
      mod.rs
      rsp.rs          packet encode/decode
      target.rs       GdbTarget trait
    watchpoint.rs     WatchpointEngine — software watchpoints via probe subscription
    breakpoint.rs     BreakpointEngine — PC breakpoints via pre_step probe
    inspect.rs        InspectionAPI — dump arch state, memory range, devices on demand
```

### WatchpointEngine

```rust
pub struct WatchpointEngine {
    watchpoints: Vec<Watchpoint>,
}

pub struct Watchpoint {
    pub addr:  u64,
    pub size:  usize,
    pub kind:  WatchKind,  // Read | Write | ReadWrite
    pub action: Box<dyn Fn(&MemAccessEvent) + Send + Sync>,
}

impl WatchpointEngine {
    /// Subscribe to memory probes. Each access checks all watchpoints.
    pub fn subscribe(&mut self, probes: &mut CpuProbes) { … }
}
```

### BreakpointEngine

```rust
pub struct BreakpointEngine {
    breakpoints: Vec<Breakpoint>,
}

pub struct Breakpoint {
    pub pc:     u64,
    pub action: Box<dyn Fn(u64) + Send + Sync>,
}

impl BreakpointEngine {
    /// Subscribe to pre_step probes. Fires action when PC matches.
    pub fn subscribe(&mut self, probes: &mut CpuProbes) { … }
}
```

---

## 8. Python API (redesigned)

```python
# Observation setup — SpySession is a standalone attachable component.
# Constructed independently, not via system.spy().
spy = helm.SpySession(system,
    cache_l1d_size=32768,
    cache_l1d_ways=8,
    cache_l1d_line=64,
    predictor="gshare",
    predictor_bits=10,
)

# Run
system.run(200_000_000)

# Query directly from Python (collection ≠ delivery)
print(f"Insns: {spy.insn_count}")
print(f"L1D hit rate: {spy.cache_hit_rate:.2%}")
print(f"Branch miss rate: {spy.branch_miss_rate:.2%}")
print(f"Top 10 hot PCs: {spy.hot_pcs(10)}")

# Deliver results explicitly (decoupled from collection)
spy.report(sink="stderr:", format="text")               # human readable
spy.report(sink="file:/tmp/perf.json", format="json")   # machine readable
spy.report(sink="tcp:localhost:9001", format="gemstats") # gem5 stats.txt format

# Detach — freeze metrics, unsubscribe from probes
spy.detach()

# Windowed tracing (replaces trace_after)
spy = helm.SpySession(system,
    cache_l1d_size=32768,
    predictor="gshare",
    window_start=1_000_000,
    window_end=2_000_000,
)
system.run(3_000_000)
print(f"Windowed hit rate: {spy.cache_hit_rate:.2%}")
spy.detach()

# Triggers
spy = helm.SpySession(system, predictor="gshare")
spy.on_insn(100_000_000, lambda: spy.report(sink="stderr:"))
spy.on_pc(0xffff_8000_0000_1234, lambda: spy.snapshot("boot-kmain.json"))
system.run(200_000_000)
spy.detach()

# Debugging (independent of SpySession)
system.breakpoint(pc=0x4000_0000, action=lambda: print("hit boot entry"))
system.watchpoint(addr=0x1000, size=8, kind="write")
```

### Architectural Exploration Workflow Example

```python
# Q: How does L1D size affect miss rate?
results = []
for l1d_size in [16*1024, 32*1024, 64*1024, 128*1024]:
    system.reset()
    spy = helm.SpySession(system, cache_l1d_size=l1d_size, cache_l1d_ways=8)
    system.run(50_000_000)
    results.append({
        "l1d_size": l1d_size,
        "hit_rate": spy.cache_hit_rate,
        "insns": spy.insn_count,
    })
    spy.detach()

# Q: Which branches are hardest to predict?
spy = helm.SpySession(system, predictor="gshare", predictor_bits=12)
system.run(100_000_000)
spy.hot_pcs(20)  # top 20 most-executed PCs
print(f"Branch miss rate: {spy.branch_miss_rate:.2%}")
spy.detach()

# Q: SimPoint — find representative intervals
spy = helm.SpySession(system)
system.run(5_000_000_000)
snap = spy.snapshot()  # frozen metrics as dict
spy.detach()
```

---

## 9. Backward Compatibility and Migration

### What is deleted / moved

| Item | Action | Replacement |
|---|---|---|
| `helm_debug::sim_trace` module | **MOVE** to `helm_diag` | `helm_diag::emit()`, `DiagSink` |
| `sim_trace::MonitorSink` | Rename+move | `helm_diag::DiagSink` (diagnostic) + `helm_report::AsyncFileSink` (analysis) |
| `sim_trace::MonitorEntry` | Rename+move | `helm_diag::DiagEntry` (diagnostic); typed structs for analysis |
| `sim_trace::Level` | Rename+move | `helm_diag::DiagLevel` |
| `sim_trace::Monitor` | Rename+move | `helm_diag::DiagMonitor` |
| `sim_branch!()` | **DELETE** | `probe!(probes.branch, BranchEvent{...})` |
| `sim_stub!()` | Move to helm-diag | `helm_diag::sim_stub!()` (unchanged semantics) |
| `sim_warn!()` | Move to helm-diag | `helm_diag::sim_warn!()` (unchanged semantics) |
| `sim_info!()` | Move to helm-diag | `helm_diag::sim_info!()` (unchanged semantics) |
| `helm_plugin::HelmPlugin` trait | **DELETE** | `helm_spy` primitives (composable) |
| `helm_plugin::PluginRegistry` | **DELETE** | `helm_spy::SpySession::subscribe()` |
| `helm_plugin::PluginArgs` | **DELETE** | Rust typed builder / Python kwargs |
| `helm_debug::TraceLogger` | **DELETE** | Was a stub; never implemented |

### What is kept with changes

| Kept | Change |
|---|---|
| `helm_plugin::InsnInfo` | Add `vcpu_idx`; move to `helm_spy::events` |
| `helm_plugin::BranchInfo` | Move to `helm_spy::events`; keep fields |
| `helm_plugin::MemInfo` | Move to `helm_spy::events` |
| `helm_plugin::FaultInfo` | Move to `helm_spy::events` |
| `helm_plugin::InsnClass` | Move to `helm_spy::events` |
| `helm_debug::CheckpointManager` | Keep; implement CBOR in Phase 2 |
| `helm_debug::GdbServer` | Keep; implement RSP in Phase 2 |
| `helm-probe` (all) | Keep; add `BranchEvent`, `MmioEvent` |

### Python API migration

```python
# Old
sim.add_plugin("hotblocks")
sim.add_plugin("howvec")
sim.add_plugin("cache", args="l1d_size=32KB,l1d_assoc=8")
sim.add_plugin("branch-trace", args="top=20")
sim.finish()

# New — SpySession is standalone, configured via constructor kwargs
spy = helm.SpySession(system, cache_l1d_size=32768, cache_l1d_ways=8)
system.run(N)
spy.report(sink="stderr:", format="text")   # replaces finish()
print(spy.hot_pcs(20))                      # replaces hotblocks' top()
print(spy.insn_mix())                       # replaces howvec
print(f"Branch miss rate: {spy.branch_miss_rate:.2%}")
spy.detach()
```

---

## 10. New Crate DAG

```
helm-probe        (zero deps; Layer 1)
    │
    └── helm-spy  (deps: helm-probe; Layer 2)
            │  (no dep on helm-report — collection ≠ delivery)
            │
    helm-report   (deps: helm-spy for types; Layer 3)
            │
    helm-debug    (deps: helm-probe; Layer 4 — no dep on helm-spy or helm-report)
            │
    helm-engine   (deps: helm-probe, helm-spy, helm-report, helm-debug)
    helm-python   (deps: helm-engine; exposes Python API)
```

`helm-debug` does not depend on `helm-spy` or `helm-report`. It is pure developer
tooling. Watchpoints and breakpoints subscribe directly to probes.

---

## 11. Implementation Phases

### Phase 1 — Foundation (prerequisite for everything)
1. Create `helm-diag`: move `sim_trace.rs` from `helm-debug` to `framework/helm-diag`
   - Rename `MonitorSink` → `DiagSink`, `MonitorEntry` → `DiagEntry`, `Level` → `DiagLevel`
   - Update all `use helm_debug::sim_trace::*` → `use helm_diag::*` (~50 call sites)
   - `helm-arch`, `helm-devices`, `helm-hw-*` dep updated to `helm-diag`
   - `helm-debug` depends on `helm-diag`; no circular dep
2. Implement `helm-probe`: `Probe<T>`, `probe!()`, all event types including `BranchEvent`
3. Wire `CpuProbes` (pre_step, post_step, fault, mem, branch) into FS and SE step loops
4. Delete `sim_branch!`; replace all call sites in `branch.rs` with `probe!(probes.branch, ...)`
5. Delete `helm_debug::TraceLogger` (was a stub)
6. `cargo build --workspace` must pass with zero warnings

### Phase 2 — Collection Primitives (`helm-spy`)
1. `Counter`, `PerVcpuCounter`, `InsnMix` (replaces InsnCount + HowVec)
2. `HeatMap` (replaces HotBlocks)
3. `RingBuffer<T>`, `EventStream<T>` (replaces ExecLog, FaultDetect history)
4. `CacheModel` (replaces CacheSim — same algorithm, new collection API)
5. `BranchPredictor` stubs (BiModal, GShare)
6. `SpySession` + `subscribe()`
7. Python PyO3 bindings: `sim.spy()`, `session.track_*()`, `session.query_*()`

### Phase 3 — Delivery (`helm-report`)
1. `Sink` trait + `FileSink`, `StderrSink`, `NullSink`, `AsyncFileSink`
2. `TextFormatter` (human-readable; replicates current atexit output)
3. `JsonFormatter`
4. `session.report(sink, format)` — Python API
5. `ReportSchedule` (at-exit, on-trigger)

### Phase 4 — Developer Tooling (`helm-debug`)
1. `WatchpointEngine` — subscribe to mem probes
2. `BreakpointEngine` — subscribe to pre_step probe
3. `CheckpointManager` — CBOR serialization
4. `GdbServer` — RSP stub → real implementation
5. `InspectionAPI` — dump state on demand from Python

### Phase 5 — Advanced Exploration
1. `Trigger` + `Window` primitives
2. `BranchPredictor` full implementations
3. `SimPoint` BBV computation
4. Differential analysis (compare two SpySessions)
5. `GemstatsFormatter` (gem5 stats.txt compatibility)
6. Power estimation model (per-class instruction energy × count)

---

## 12. Design Decisions Reached (from multi-agent analysis)

These were debated and resolved before writing this plan:

| Question | Decision | Rationale |
|---|---|---|
| Hot-loop synchronization model | **Per-vCPU local state + quantum_end flush** | Zero atomic ops in hot loop; existing `Scoreboard<T>` is exactly the right primitive |
| Replace sim_stub!/sim_warn! | **Keep dedicated diagnostic channel (helm-diag)** | `log` crate cannot carry typed structured fields (component, pc, sim_ns) |
| Replace sim_branch! | **Delete; use probe!(probes.branch, BranchEvent{...})** | Zero cost in release; typed; feeds into analysis framework |
| helm-debug scope | **GDB RSP + CheckpointManager + Breakpoint/Watchpoint only** | Delivery is not its concern; diagnostic macros move to helm-diag |
| Analysis output format | **Typed binary (`repr(C)`) + text/JSON via helm-report** | String logs are unanalyzable at scale; Python needs `mmap`-able binary |
| Mutex in probe callbacks | **Banned** | `Arc<Mutex<T>>` in hot callbacks is a performance bug; use Scoreboard |
| helm-plugin (old) | **Replaced by helm-spy** | HelmPlugin trait is monolithic; PluginArgs is stringly-typed; atexit() couples delivery to collection |

## 13. Open Questions

1. **DashMap vs sharded counters for HeatMap**: `DashMap` (external dep) vs manual
   sharding with `[Mutex<HashMap>; N]`. Sharding has no external dep but is more complex.
   Decision needed before Phase 2 begins.

2. **EventStream<InsnInfo> with ArchContext**: `ArchContext::Aarch64 { x: [u64; 31], sp, pc, nzcv }`
   is 256 bytes. An EventStream capturing 1M instructions = 256 MB. Need a compact form
   or explicit opt-in. Proposal: `EventStream<CompactInsnEvent>` with no ArchContext by
   default; separate `EventStream<InsnInfo>` (with context) only when requested.

3. **helm-plugin removal vs deprecation**: Should `HelmPlugin` trait and `PluginRegistry`
   be removed immediately (breaking change) or kept deprecated until Phase 3? Recommendation:
   remove immediately — the old API is unusable once probe wiring replaces the call sites.

4. **Async delivery in helm-report**: `AsyncFileSink` needs a background thread (like
   `MonitorSink`). Should this thread be owned per-sink (current approach) or shared via a
   global I/O thread pool? Per-sink is simpler; shared pool avoids N threads for N sinks.

5. **Python lambda callbacks in Trigger/Watchpoint**: Python lambdas acquire the GIL.
   A trigger firing mid-quantum will stall the engine. Need a buffered approach: fire sets
   a flag, Python checks the flag at quantum boundaries. Same pattern as PyO3 syscall
   callbacks.

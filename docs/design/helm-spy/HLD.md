# helm-spy — High-Level Design

> **Crate:** `debug/helm-spy`
> **Layer:** 2 — Analysis Primitives
> **Replaces:** `helm-plugin` (deleted in Instrumentation v2)
> **Status:** Design-complete; implementation Phase 2

---

## 1. Purpose and Motivation

`helm-spy` is the analysis-primitive layer of the Instrumentation v2 stack. It
replaces `helm-plugin` entirely.

### 1.1 What was wrong with helm-plugin

`helm-plugin` coupled **collection** (counting, tracing, accumulating) with **delivery**
(printing, formatting, writing to a file). Every plugin held both its analysis state
(`Arc<Mutex<HashMap<u64, BranchRecord>>>`) and its output method (`fn atexit(&mut self)
{ eprintln!(...) }`). The consequences:

- `Arc<Mutex<T>>` inside per-instruction callbacks is a contention bottleneck; each
  instruction step contends on the same lock across all registered plugins.
- Plugin output cannot be redirected without modifying the plugin.
- Plugin state cannot be queried mid-simulation from Python.
- Plugins cannot be composed: "count SIMD instructions while a specific function
  runs" requires writing a new bespoke plugin, not combining Counter + Window.
- `PluginArgs` (`HashMap<String, String>`) turns configuration errors into runtime panics.

### 1.2 The core principle: collection is not delivery

```
┌──────────────────────────────────────────────────────┐
│  COLLECTION  (hot path — per instruction/event)       │
│                                                        │
│  probe!(probes.post_step, CpuStepEvent { pc, raw })   │
│      ↓ subscriber closure (no heap alloc, no lock)    │
│  counter.inc()                   ← fetch_add Relaxed  │
│  indexed_counter.inc(class)      ← array index + add  │
│  heatmap.inc(pc)                 ← DashMap shard lock │
│  trace_ring.push(branch_record)  ← lock-free SPSC     │
└──────────────────────────────────────────────────────┘
                        ↓  (triggered, explicit)
┌──────────────────────────────────────────────────────┐
│  DELIVERY  (cold path — explicit, triggered, async)   │
│                                                        │
│  session.report(sink="file:/tmp/perf.json", fmt="json")│
│      ↓                                                 │
│  helm-report::ReportFormatter::format(session_data)   │
│      ↓                                                 │
│  helm-report::Sink::deliver(bytes)                    │
│      → File | Stderr | TCP | Null | Python buffer     │
└──────────────────────────────────────────────────────┘
```

**Rules (no exceptions):**

1. Nothing in the collection path allocates on the heap per-event.
2. Nothing in the collection path calls `Mutex::lock()` per-event — except `RingBuffer<T>`
   and `EventStream<T>`, which are explicitly opt-in, documented as bounded-overhead
   primitives suitable only for low-rate events (faults, syscalls).
3. Delivery is always explicit: `session.report(sink, format)` or a scheduled trigger.
   It is never invoked from inside a probe callback.
4. The same collected data can be formatted by multiple `Formatter` types and delivered
   to multiple sinks simultaneously. Collection primitives are format-agnostic.
5. `helm-spy` has no dependency on `helm-report`. Collection structs know nothing
   about how their data will be rendered.

---

## 2. Position in the Instrumentation Stack

```
┌─────────────────────────────────────────────────────────────────────────┐
│  LAYER 1 — helm-probe                                                   │
│  Zero-cost typed probe points. ZST in release. One branch in dev.      │
│  Probe<CpuStepEvent>, Probe<BranchEvent>, Probe<MemAccessEvent>, …     │
└──────────────────────────────┬──────────────────────────────────────────┘
                               │  ProbePluginBridge (in helm-spy)
                               │  Subscribes probe events, enriches to
                               │  InsnInfo / BranchInfo / MemInfo / FaultInfo
                               ▼
┌─────────────────────────────────────────────────────────────────────────┐
│  LAYER 2 — helm-spy  (this crate)                                  │
│  Typed analysis primitives. Subscribe to probes. Accumulate data.      │
│  Counter, IndexedCounter, HeatMap, TraceRing, Histogram, …             │
│  SpySession: user-facing aggregator wired from Python/CLI.         │
└──────────────────────────────┬──────────────────────────────────────────┘
                               │  (never called from collection path)
                               │  session.report(sink, format)
                               ▼
┌─────────────────────────────────────────────────────────────────────────┐
│  LAYER 3 — helm-report                                                  │
│  Formatters (Text, JSON, CSV, GemStats) + Sinks (File, Stderr, TCP).  │
│  ReportSchedule for at-exit / periodic / trigger-based delivery.       │
└─────────────────────────────────────────────────────────────────────────┘
```

**Dependency direction**: events flow downward only. `helm-probe` has no knowledge of
`helm-spy`. `helm-spy` has no knowledge of `helm-report`.

---

## 3. Event Types

Event types live in `src/events.rs`. They are richer than `helm-probe` event types: they
carry classification, context, and enrichment performed by the `ProbePluginBridge`.
These types were previously in `helm-plugin` and are moved here in the v2 redesign.

### 3.1 `InsnInfo`

```rust
/// Emitted once per retired instruction (post-step).
#[derive(Debug, Clone)]
pub struct InsnInfo {
    pub vcpu_idx:    usize,
    pub pc:          u64,
    pub raw:         u32,
    pub size:        u8,           // 4 for AArch64, 2 or 4 for RISC-V (C extension)
    pub class:       InsnClass,
    pub opcode_name: &'static str,
    pub is_stub:     bool,         // true if execute() returned Unimplemented
    pub context:     ArchContext,  // ArchContext::None by default; opt-in for full regs
    pub insn_count:  u64,          // monotonic retirement count for this vCPU
}
```

### 3.2 `BranchInfo`

```rust
#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub pc:         u64,
    pub target:     u64,
    pub taken:      bool,
    pub kind:       BranchKind,
    pub insn_count: u64,
}
```

### 3.3 `MemInfo`

```rust
#[derive(Debug, Clone)]
pub struct MemInfo {
    pub vaddr:     u64,
    pub size:      u8,
    pub is_store:  bool,
    pub is_atomic: bool,   // true for LSE atomics (LDADD, CAS, SWP, etc.)
    pub pc:        u64,
}
```

### 3.4 `SyscallInfo` / `SyscallRetInfo`

```rust
#[derive(Debug, Clone)]
pub struct SyscallInfo {
    pub vcpu_idx: usize,
    pub nr:       u64,
    pub args:     [u64; 6],
    pub pc:       u64,
}

#[derive(Debug, Clone)]
pub struct SyscallRetInfo {
    pub vcpu_idx: usize,
    pub nr:       u64,
    pub retval:   i64,
}
```

### 3.5 `FaultInfo`

```rust
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

### 3.6 Classification enums

```rust
/// Instruction class — index into IndexedCounter for instruction mix analysis.
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

/// Branch type — for branch mix analysis and predictor simulation.
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

/// Optional full register dump. Default is None (zero cost to construct).
#[derive(Debug, Clone, Default)]
pub enum ArchContext {
    #[default]
    None,
    Aarch64 { x: [u64; 31], sp: u64, pc: u64, nzcv: u32, fpsr: u32 },
    Riscv64 { x: [u64; 32], pc: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    InsnAbort, DataAbort, StoreAbort, Svc, Undefined, Other
}
```

---

## 4. Primitives Catalog

All primitives live in `src/primitives/`. Each primitive:
- holds its own data (atomics, ring, map, histogram)
- exposes a `subscribe_to_*(probe: &mut Probe<T>)` method that registers a closure
  capturing `Arc<Self>`
- exposes read methods (`value()`, `top(n)`, `percentile(p)`, etc.)
- has **no** delivery logic — no `atexit()`, no formatting, no I/O

| Primitive | Module | Description |
|---|---|---|
| `Counter` | `counter` | Monotonic `AtomicU64`. `inc()`, `add(n)`, `value()`. Thread-safe. |
| `IndexedCounter` | `indexed` | Fixed `Vec<AtomicU64>` per dimension label. Replaces `HowVec`. `inc(idx)`, `values()`, `fraction(idx)`. |
| `PerVcpuCounter` | `per_vcpu` | One slot per vCPU via `Scoreboard<u64>` (UnsafeCell, no locking). `inc(vcpu)`, `total()`, `per_vcpu()`. |
| `Histogram` | `histogram` | Fixed-bucket `Vec<AtomicU64>` with `bucket_edges`. `record(val)`, `percentile(p)`, `counts()`. |
| `IntervalHistogram` | `interval_hist` | Time-series histogram: samples a scalar every N instructions, buckets the per-window value. `tick_with(value, insn_count)`. |
| `HeatMap` | `heatmap` | Per-PC `u64` counter map. `inc(pc)`, `top(n) -> Vec<(u64, u64)>`. Uses `DashMap` (optional feature) or sharded `Mutex`. |
| `RingBuffer<T>` | `ring` | `Mutex<VecDeque<T>>`, fixed capacity, overwrites oldest on push. `push(ev)`, `snapshot() -> Vec<T>`. |
| `EventStream<T>` | `stream` | `Mutex<Vec<T>>`, bounded at `max`, stops recording when full. `push(ev)`, `drain() -> Vec<T>`. |
| `TraceRing<T, N>` | `trace_ring` | Lock-free SPSC ring buffer. `push(val)` (no alloc). `drain_into(out: &mut Vec<T>)`. For high-rate typed binary traces. |
| `CorrelHist2D` | `correl2d` | 2D joint histogram. `record(x, y)`. Flat `Vec<AtomicU64>` storage, row-major layout. |

### 4.1 Hot-path cost summary

| Primitive | Hot-loop operations | Lock? | Allocation? |
|---|---|---|---|
| `Counter` | 1× `fetch_add(Relaxed)` | No | No |
| `IndexedCounter` | 1× slice index + `fetch_add(Relaxed)` | No | No |
| `PerVcpuCounter` | 1× slice index + `+= 1` (UnsafeCell, no atomic) | No | No |
| `Histogram` | 1× `partition_point` + `fetch_add(Relaxed)` | No | No |
| `IntervalHistogram` | 1× division + conditional `swap` + `record()` | No | No |
| `HeatMap` | 1× DashMap shard lock (short critical section) | Shard | No |
| `RingBuffer<T>` | 1× `Mutex::lock()` + `VecDeque::push_back()` + possible drop | Yes | On drop |
| `EventStream<T>` | 1× `Mutex::lock()` + `Vec::push()` (until full) | Yes | On push |
| `TraceRing<T, N>` | 1× `ptr::write` + `AtomicU64::store(Release)` | No | No |
| `CorrelHist2D` | 2× `partition_point` + `fetch_add(Relaxed)` | No | No |

`RingBuffer<T>` and `EventStream<T>` are suitable only for low-rate events (faults,
syscalls, MMU walks). For per-instruction traces, use `TraceRing<T, N>`.

---

## 5. Quantum Flush Protocol

### 5.1 Why quantum boundaries matter

The hot loop runs per-vCPU in a tight loop. Cross-thread synchronization inside that
loop adds measurable latency. `PerVcpuCounter` and per-vCPU `IntervalHistogram`
configurations deliberately avoid cross-thread synchronization by using per-vCPU local
state (`Scoreboard<T>`). This local state must be merged into shared aggregate structures
at a predictable, off-hot-path point.

**Quantum** = the instruction-count range covered by one call to `engine.run(n)`.
Every `run()` return is a quantum boundary. Checkpoint save is also a quantum boundary.

### 5.2 `QuantumObserver` trait

```rust
// src/quantum.rs

/// Implemented by any analysis primitive or aggregate that needs to
/// finalize per-vCPU local state after a vCPU quantum ends.
pub trait QuantumObserver: Send + Sync {
    /// Called by the engine at every `run()` return and before checkpoint save.
    ///
    /// `vcpu`       — the vCPU index whose quantum just completed.
    /// `insn_count` — total retired instruction count for this vCPU so far.
    ///
    /// This method runs on the cold path and may allocate, block on I/O,
    /// or acquire Mutex locks. The hot loop is not executing when this is called.
    fn quantum_end(&mut self, vcpu: usize, insn_count: u64);
}
```

### 5.3 The mandatory invariant

> **No `Mutex::lock()` inside probe callbacks.**

`Arc<Mutex<T>>` captured inside a hot-loop closure is a performance bug. The hot-loop
callback must use only:
- `AtomicU64::fetch_add(Relaxed)` for counters and histograms
- `UnsafeCell` per-vCPU slot writes for per-vCPU state
- `TraceRing::push()` (lock-free SPSC) for event traces
- `DashMap::entry()` for heatmaps (shard-locked, brief critical section)

Any aggregation that requires a global lock is deferred to `quantum_end()`.

### 5.4 When the engine calls `quantum_end()`

The engine calls `SpySession::quantum_end(vcpu, insn_count)`:
1. At the end of every `HelmEngine::run(n)` call, for each vCPU that executed in
   that quantum.
2. Before every `checkpoint_save()` call — to flush all in-flight local state into
   the shared primitives before the state is serialized.

`SpySession::quantum_end()` dispatches to each registered `QuantumObserver`.

### 5.5 Example: per-vCPU aggregation pattern

```
Hot loop, vCPU 0, every instruction:
    probe!(probes.post_step, CpuStepEvent { pc, raw })
        → HeatMap closure:
            // UnsafeCell slot 0 — no atomic, no lock
            local_counts[0].inc(pc)     ← O(1), no sync

run() returns:
    session.quantum_end(vcpu=0, insn_count=1_000_000)
        → HeatMap::quantum_end(0, _):
            // merge vCPU-0 local into shared DashMap
            for (pc, count) in local_counts[0].drain() {
                *global.entry(pc).or_insert(0) += count;
            }
```

`PerVcpuCounter` does **not** implement `QuantumObserver` because its `total()` reads
from Scoreboard slots at query time — no merge needed. Only primitives with per-vCPU
local aggregation buffers need `quantum_end()`.

---

## 6. Trigger System

### 6.1 Purpose

Without triggers, observation windows span the entire simulation or must be managed
manually by the Python caller via `run()` call boundaries. The trigger system provides
**in-engine condition testing** at minimal hot-loop cost, enabling:

- **ROI (Region of Interest) analysis**: collect only during a known phase
- **Warmup skip**: activate a cache model after 1M warmup instructions
- **PC-triggered activation**: arm a trace ring when execution enters a specific function
- **Threshold-based collection**: start collecting when a counter crosses a threshold
- **Periodic sampling**: every N instructions, deliver a report or snap a counter value

### 6.2 `TriggerKind`

```rust
// src/trigger.rs

pub enum TriggerKind {
    /// Fire once when the global instruction count reaches exactly N.
    AtInsn(u64),

    /// Fire periodically — every N instructions (checked via insn_count % N == 0).
    EveryN(u64),

    /// Fire when the instruction PC equals the given address (exact match).
    AtPc(u64),

    /// Fire while the instruction PC is inside [start, end).
    /// Fires on every instruction in range; use one_shot=true for entry-only.
    PcRange(u64, u64),

    /// Fire when the given Counter's value reaches or exceeds threshold N.
    CounterReaches(Arc<Counter>, u64),
}
```

### 6.3 `Trigger` struct

```rust
pub struct Trigger {
    pub kind:     TriggerKind,
    action:       Box<dyn Fn(&TriggerCtx) + Send + Sync>,
    /// When false, `check()` returns immediately. Hot-loop fast-path.
    armed:        AtomicBool,
    /// If true, disarms after the first fire.
    pub one_shot: bool,
}

pub struct TriggerCtx {
    pub pc:         u64,
    pub insn_count: u64,
}
```

**Hot-loop cost**: one `AtomicBool::load(Relaxed)` + one comparison ≈ 1 ns.
Predicted-not-taken when `armed` is `false` — essentially free.

### 6.4 `Trigger::check()` semantics

`check()` is called from the pre-step probe subscriber:

```rust
impl Trigger {
    #[inline]
    pub fn check(&self, pc: u64, insn_count: u64) -> bool {
        // Fast path: if disarmed, skip all evaluation.
        if !self.armed.load(Ordering::Relaxed) { return false; }

        let fired = match &self.kind {
            TriggerKind::AtInsn(n)                => insn_count == *n,
            TriggerKind::EveryN(n)                => insn_count % n == 0,
            TriggerKind::AtPc(addr)               => pc == *addr,
            TriggerKind::PcRange(start, end)      => pc >= *start && pc < *end,
            TriggerKind::CounterReaches(ctr, thr) => ctr.value() >= *thr,
        };

        if fired {
            if self.one_shot {
                self.armed.store(false, Ordering::Relaxed);
            }
            (self.action)(&TriggerCtx { pc, insn_count });
        }
        fired
    }
}
```

### 6.5 Rules for trigger actions

Trigger actions execute inside a probe callback, on the hot path. All collection-path
rules apply:

- Actions must not `Mutex::lock()` any structure shared with the hot path.
- For heavy actions (file I/O, Python callback): post a message to a `SyncSender`
  channel; a background thread handles the I/O.
- Python lambdas acquire the GIL — they must not be called directly from a trigger
  action. The correct pattern: set an `AtomicBool` flag; the Python caller checks
  the flag at `run()` return (quantum boundary).

### 6.6 Python trigger API

```python
# One-shot: snapshot session state to JSON at instruction 100M
session.on_insn(100_000_000, lambda: session.snapshot("mid-run.json"))

# Periodic: deliver a text report to stderr every 10M instructions
session.on_every(10_000_000, lambda: session.report("stderr:", "text"))

# PC entry: activate branch heatmap when entering the kernel text section
session.on_pc(0xffff_8000_0000_0000, session.branch_heatmap.activate)

# PC range: arm the L1D cache model only while in [kernel_start, kernel_end)
session.on_pc_range(0xffff_8000_0000_0000, 0xffff_8001_0000_0000,
                    session.cache_l1d.activate)

# Counter threshold: report when L1D miss count exceeds 1M
session.on_counter_reaches(session.cache_l1d.misses, 1_000_000,
                            lambda: session.report("stderr:", "json"))
```

---

## 7. Time Window

### 7.1 Purpose

A `Window` bounds the instruction-count range during which a primitive records events.
All recording outside `[start, end)` is silently skipped without any hot-path cost
beyond a single comparison.

Use cases:
- Skip boot code; collect only steady-state execution
- Compare two program phases in a single simulation run
- Warmup skip: activate cache model after 1M warmup instructions
- Validate a specific function's behavior in isolation

### 7.2 `Window` and `Windowed<T>`

```rust
// src/window.rs

/// A closed instruction-count range [start, end) for gating observation.
pub struct Window {
    pub start: u64,
    pub end:   u64,
    active:    AtomicBool,   // updated lazily by is_active()
}

impl Window {
    /// Returns true iff insn_count is inside [start, end).
    /// Updates `active` as boundaries are crossed — enables fast-path
    /// skipping in Windowed<T> after the window ends.
    #[inline]
    pub fn is_active(&self, insn_count: u64) -> bool {
        let in_range = insn_count >= self.start && insn_count < self.end;
        self.active.store(in_range, Ordering::Relaxed);
        in_range
    }

    /// Read the cached active state without an insn_count check.
    /// Valid only after at least one `is_active()` call this quantum.
    #[inline]
    pub fn is_active_cached(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }
}

/// Wraps a primitive `T` and gates all recording to inside-window only.
pub struct Windowed<T> {
    pub window: Arc<Window>,
    pub inner:  T,
}

impl<T> Windowed<T> {
    pub fn new(window: Arc<Window>, inner: T) -> Self {
        Self { window, inner }
    }

    /// Access the inner primitive only if inside the window.
    /// Returns None outside the window — callers skip the record call.
    #[inline]
    pub fn get_if_active(&self, insn_count: u64) -> Option<&T> {
        if self.window.is_active(insn_count) { Some(&self.inner) } else { None }
    }
}
```

### 7.3 Python window API

```python
# Collect instruction mix only from instruction 1M to 5M
w = session.window(start=1_000_000, end=5_000_000)
w.track_insns()     # -> Windowed<IndexedCounter>

# Collect branches during [10M, 20M) and [50M, 60M) simultaneously
w1 = session.window(start=10_000_000, end=20_000_000)
w2 = session.window(start=50_000_000, end=60_000_000)
w1.track_branches()
w2.track_branches()

sim.run(100_000_000)
print("Phase 1 hot PCs:", w1.branch_heatmap.top(10))
print("Phase 2 hot PCs:", w2.branch_heatmap.top(10))
```

---

## 8. `SpySession` — User-Facing Aggregator

### 8.1 Role

`SpySession` is the object Python and the CLI interact with directly. It:

1. Owns all configured primitives as named, typed fields (wrapped in `Arc` for closure capture).
2. Wires primitives to probe bundles via `subscribe()` — called once before the first `run()`.
3. Propagates `quantum_end()` to all registered `QuantumObserver` impls.
4. Exposes read methods to Python via PyO3 (query without delivery).
5. Provides `report()` as the explicit delivery entry point (delegates to `helm-report`).

`SpySession` does **not** own the probes. It receives `&mut CpuProbes` during
`subscribe()` and wires closures. The probes remain owned by `HelmEngine<T>`.

### 8.2 Structure sketch

```rust
// src/session.rs

pub struct SpySession {
    // Instruction primitives (always active — track_insns())
    pub insn_count:     Arc<Counter>,
    pub insn_mix:       Arc<IndexedCounter>,   // InsnClass × count
    pub hot_pcs:        Arc<HeatMap>,

    // Branch primitives (track_branches())
    pub branch_count:   Option<Arc<Counter>>,
    pub branch_mix:     Option<Arc<IndexedCounter>>,  // BranchKind × count
    pub branch_heatmap: Option<Arc<HeatMap>>,

    // Memory / cache (track_memory())
    pub mem_count:  Option<Arc<Counter>>,
    pub cache_l1d:  Option<Arc<CacheModel>>,

    // Fault history (track_faults())
    pub fault_history: Arc<RingBuffer<FaultInfo>>,

    // Syscall log (track_syscalls())
    pub syscall_log: Option<Arc<EventStream<SyscallInfo>>>,

    // High-rate typed branch trace (track_branch_trace())
    pub branch_trace: Option<Arc<TraceRing<BranchRecord, 1_048_576>>>,

    // Trigger and window registries
    triggers: Vec<Trigger>,
    windows:  Vec<Arc<Window>>,

    // QuantumObserver dispatch (HeatMap with local merge, IntervalHistogram, etc.)
    quantum_observers: Vec<Box<dyn QuantumObserver>>,

    // Whether subscribe() has been called
    subscribed: bool,
}

impl SpySession {
    /// Wire all configured primitives to probe bundles.
    /// Idempotent — subsequent calls are no-ops.
    pub fn subscribe(
        &mut self,
        cpu_probes: &mut CpuProbes,
        gic_probes: Option<&mut GicProbes>,
    ) { … }

    /// Called by HelmEngine at every run() return and before checkpoint save.
    pub fn quantum_end(&mut self, vcpu: usize, insn_count: u64) {
        for obs in &mut self.quantum_observers {
            obs.quantum_end(vcpu, insn_count);
        }
    }
}
```

### 8.3 Wiring example (inside `subscribe()`)

```rust
// Instruction count
let counter = Arc::clone(&self.insn_count);
cpu_probes.post_step.subscribe(move |_ev: &CpuStepEvent| {
    counter.inc();
});

// Instruction class mix
let mix = Arc::clone(&self.insn_mix);
cpu_probes.post_step.subscribe(move |ev: &CpuStepEvent| {
    let (class, _name, _stub) = classify_aarch64_opcode(ev.raw);
    mix.inc(class as usize);
});

// Hot PC heatmap
let hm = Arc::clone(&self.hot_pcs);
cpu_probes.post_step.subscribe(move |ev: &CpuStepEvent| {
    hm.inc(ev.pc);
});

// Trigger check on pre_step
for trigger in &self.triggers {
    let t = trigger.clone();   // Trigger is Clone; action is Arc-wrapped
    cpu_probes.pre_step.subscribe(move |ev: &CpuStepEvent| {
        // insn_count not on CpuStepEvent without probe-full; use a shared AtomicU64
        t.check(ev.pc, 0 /* filled by probe-full or a separate counter */);
    });
}
```

---

## 9. `ProbePluginBridge`

`ProbePluginBridge` is the Layer 1 → Layer 2 connector. It lives in `src/bridge.rs`.
It subscribes to `Probe<T>` events, enriches them to the richer `helm-spy` event
types, and dispatches to `SpySession` primitives.

This is the same role the `ProbePluginBridge` played in `helm-plugin`. In v2, it
dispatches to composable primitives rather than a `PluginRegistry`.

### 9.1 Enrichment

| `helm-probe` event | `helm-spy` event | Extra fields added |
|---|---|---|
| `CpuStepEvent { pc, raw }` | `InsnInfo` | `class`, `opcode_name`, `is_stub` via `classify_aarch64_opcode(raw)` |
| `CpuFaultEvent { pc, raw, kind }` | `FaultInfo` | `vcpu_idx`, `FaultKind` enum, `insn_count` |
| `MemAccessEvent { addr, size, is_store, pc }` | `MemInfo` | `is_atomic` from `AccessType` on `InstrumentedMem` |
| `BranchEvent { pc, target, taken, kind }` | `BranchInfo` | passthrough (already rich at probe layer) |

### 9.2 Lifecycle

`ProbePluginBridge` is constructed during `build_simulator()` (Python) or at CLI
startup. It is installed before `run()`. It is not checkpointed — subscriptions are
re-registered on `checkpoint_restore()` by calling `subscribe()` again.

---

## 10. Python API

### 10.1 Entry point

```python
# sim.spy() creates and returns an SpySession.
# Primitives are not yet wired to probes — subscribe() fires lazily on first run().
session = sim.spy()
```

### 10.2 `track_*()` — configure collection primitives

```python
# Enable instruction count + instruction mix + hot PC heatmap
session.track_insns()

# Enable branch count + branch mix + branch heatmap
session.track_branches()

# Enable high-rate typed binary branch trace (TraceRing)
session.track_branch_trace()

# Enable L1D cache model
session.track_memory(
    l1d_size=32*1024,    # 32 KB
    l1d_assoc=8,
    line_size=64,
)

# Enable fault history ring buffer (last N faults, overwrite oldest)
session.track_faults(history=128)

# Enable syscall log (bounded EventStream, stops at max)
session.track_syscalls(max=10_000)

# Enable 2D correlation histogram (e.g. IPC vs branch miss rate)
session.track_correl2d(
    name="ipc_vs_miss_rate",
    edges_x=[0, 1, 2, 3, 4, 5, 6, 7, 8],      # IPC buckets
    edges_y=[0, 1, 10, 100, 1000, 10_000],     # miss-count buckets
)
```

Calling `sim.run(n)` after `track_*()` automatically calls `session.subscribe()` the
first time if it has not already been called.

### 10.3 Windowed collection

```python
# Collect branches only during [1M, 2M) instructions
w = session.window(start=1_000_000, end=2_000_000)
w.track_branches()

# Compare boot vs steady-state instruction mix
boot   = session.window(start=0,         end=500_000)
steady = session.window(start=5_000_000, end=10_000_000)
boot.track_insns()
steady.track_insns()

sim.run(15_000_000)

print("Boot insn mix:",   boot.insn_mix.table())
print("Steady insn mix:", steady.insn_mix.table())
```

### 10.4 Trigger-based control

```python
# Snapshot at 50M instructions
session.on_insn(50_000_000, lambda: session.snapshot("50m.json"))

# Periodic status to stderr
session.on_every(10_000_000, lambda: session.report("stderr:", "text"))

# Activate L1D cache model when PC enters user text segment
session.on_pc(0x0000_0000_0040_0000, session.cache_l1d.activate)
```

### 10.5 `query_*()` — direct Python access without delivery

```python
sim.run(200_000_000)

# Scalar queries
print(f"Instructions:       {session.insn_count.value():,}")
print(f"L1D hit rate:       {session.cache_l1d.hit_rate():.2%}")
print(f"Branch miss rate:   {session.branch_pred.miss_rate():.2%}")

# Instruction mix table: [(class_name, count, fraction), ...]
for name, count, frac in session.insn_mix.table():
    print(f"  {name:12s}: {count:12,}  ({frac:.1%})")

# Top N hot PCs: [(pc, count), ...]
for pc, count in session.hot_pcs.top(10):
    print(f"  {pc:#018x}: {count:12,}")

# Histogram percentiles
p50  = session.cache_l1d.latency_hist.percentile(0.50)
p99  = session.cache_l1d.latency_hist.percentile(0.99)

# Recent faults (RingBuffer snapshot — clone of current contents)
for fault in session.fault_history.snapshot():
    print(f"  {fault.kind} at pc={fault.pc:#x}: {fault.message}")

# Per-vCPU instruction counts
for vcpu_idx, count in enumerate(session.insn_count_per_vcpu.per_vcpu()):
    print(f"  vCPU {vcpu_idx}: {count:,} instructions")
```

### 10.6 Explicit delivery (delegates to helm-report)

```python
# Human-readable summary to stderr
session.report(sink="stderr:", format="text")

# Machine-readable JSON to file
session.report(sink="file:/tmp/perf.json", format="json")

# gem5 stats.txt format (for downstream tooling)
session.report(sink="file:/tmp/stats.txt", format="gemstats")

# Deliver to multiple sinks simultaneously
session.report(
    sinks=["stderr:", "file:/tmp/perf.json"],
    format="json",
)
```

### 10.7 Architectural exploration example

```python
# Q: How does L1D size affect miss rate?
results = []
for l1d_kb in [16, 32, 64, 128]:
    sim.reset()
    s = sim.spy()
    s.track_insns()
    s.track_memory(l1d_size=l1d_kb * 1024, l1d_assoc=8)
    sim.run(50_000_000)
    results.append({
        "l1d_kb":    l1d_kb,
        "miss_rate": s.cache_l1d.miss_rate(),
        "mpki":      s.cache_l1d.misses.value() / s.insn_count.value() * 1000,
    })
import json
print(json.dumps(results, indent=2))

# Q: Which branches are hardest to predict?
s = sim.spy()
s.track_branches()
sim.run(100_000_000)
for pc, count in s.branch_heatmap.top(20):
    print(f"  {pc:#x}: {count} taken")
```

---

## 11. Dependency Graph

```
helm-core     (zero deps — ArchState, InsnClass classifier)
helm-probe    (zero deps — Probe<T>, CpuStepEvent, BranchEvent, MemAccessEvent, …)

helm-spy  deps:
    helm-probe    (always; subscribes to Probe<T> bundles)
    helm-core     (ArchState types for ArchContext; classify_aarch64_opcode)
    dashmap       (optional feature "dashmap"; HeatMap backend)
    thiserror     (workspace dep)

              no dep on:
    helm-report   (enforces collection ≠ delivery)
    helm-plugin   (replaced — deleted)
    helm-debug    (not its concern)
    helm-engine   (owned by engine, not a dep of observe)

helm-report   deps:
    helm-spy  (reads SpySession, primitives at delivery time)

helm-engine   deps:
    helm-probe    (owns CpuProbes)
    helm-spy  (owns SpySession; calls quantum_end())
    helm-report   (calls session.report() on trigger or run end)

helm-python   deps:
    helm-engine   (PyO3 boundary; exposes SpySession to Python)
```

The structural absence of `helm-spy → helm-report` is the compile-time enforcement
of the collection ≠ delivery principle.

---

## 12. Phased Implementation

### Phase 1 — Foundation (other crates, prerequisite)

1. Add `BranchEvent` to `helm-probe`; add `branch: Probe<BranchEvent>` to `CpuProbes`.
2. Wire `probe!(probes.branch, BranchEvent { … })` in `helm-arch/src/aarch64/execute/branch.rs`.
3. Delete `sim_branch!`. Move `sim_stub!`, `sim_warn!`, `sim_info!` to new `helm-diag` crate.
4. Delete `TraceLogger` stub from `helm-debug`.

### Phase 2 — Collection Primitives (`helm-spy` created here)

1. `src/primitives/counter.rs` — `Counter`, `IndexedCounter`
2. `src/primitives/per_vcpu.rs` — `PerVcpuCounter` + `Scoreboard<u64>`
3. `src/primitives/histogram.rs` — `Histogram`, `IntervalHistogram`
4. `src/primitives/heatmap.rs` — `HeatMap` with dashmap/sharded backends
5. `src/primitives/ring.rs` — `RingBuffer<T>`, `EventStream<T>`
6. `src/primitives/trace_ring.rs` — `TraceRing<T, N>`, `BranchRecord`
7. `src/primitives/correl2d.rs` — `CorrelHist2D`
8. `src/events.rs` — all event types, `InsnClass`, `BranchKind`, `ArchContext`
9. `src/quantum.rs` — `QuantumObserver` trait
10. `src/trigger.rs` — `TriggerKind`, `Trigger`, `TriggerCtx`
11. `src/window.rs` — `Window`, `Windowed<T>`
12. `src/session.rs` — `SpySession`, `subscribe()`, `quantum_end()`
13. `src/bridge.rs` — `ProbePluginBridge` (moved from `helm-plugin`)
14. PyO3 bindings in `helm-python` — `sim.spy()`, `session.track_*()`, `session.query_*()`
15. Delete `helm-plugin` from workspace

### Phase 3 — Delivery (`helm-report`)

1. `Sink` trait + `FileSink`, `StderrSink`, `NullSink`, `AsyncFileSink`
2. `TextFormatter`, `JsonFormatter`, `GemStatsFormatter`
3. `session.report(sink, format)` Python API
4. `ReportSchedule` (at-exit, every-N, on-trigger)
5. `BinaryTraceSink` — drain `TraceRing<BranchRecord, N>` to typed binary file

### Phase 4 — Advanced Exploration

1. Full `Trigger` + `Window` Python bindings with GIL-safe flag pattern
2. `CacheModel` configurable set-associative simulation
3. `BranchPredictor` models: BiModal, GShare-4K
4. `CorrelHist2D` PyO3 bindings
5. `SimPoint` BBV computation (100M-instruction intervals)
6. `GemStatsFormatter` (gem5 stats.txt compatibility for downstream tooling)

---

## 13. Migration from helm-plugin

| Old API | New API | Notes |
|---|---|---|
| `sim.add_plugin("insn-count")` | `s.track_insns()` | insn_mix and hot_pcs included |
| `sim.add_plugin("howvec")` | `s.track_insns()` | `insn_mix` replaces HowVec |
| `sim.add_plugin("hotblocks")` | `s.track_insns()` | `hot_pcs` replaces HotBlocks |
| `sim.add_plugin("cache", ...)` | `s.track_memory(...)` | typed kwargs replace `PluginArgs` |
| `sim.add_plugin("branch-trace")` | `s.track_branches()` | also enables `track_branch_trace()` for binary |
| `sim.plugin("insn-count").total` | `s.insn_count.value()` | direct field access |
| `sim.plugin("cache").l1d_hit_rate` | `s.cache_l1d.hit_rate()` | method call |
| `sim.plugin("hotblocks").top(10)` | `s.hot_pcs.top(10)` | same semantics |
| `sim.finish()` | `s.report(sink="stderr:", format="text")` | explicit delivery |
| `PluginRegistry` | `SpySession` | composable, not monolithic |
| `HelmPlugin` trait | no equivalent | use primitives directly or compose in Python |
| `PluginArgs` (stringly-typed) | Python kwargs / Rust typed builder | type-safe config |

---

## 14. Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| HeatMap backend | `DashMap` (optional feature) | Battle-tested sharded concurrent hashmap; avoids custom shard logic |
| HeatMap no-feature fallback | `[Mutex<HashMap<u64,u64>>; 16]` shards | Zero external deps when feature disabled |
| TraceRing semantics | SPSC (single-producer, single-consumer) | Avoids CAS loops in `push()`; one producer per vCPU quantum |
| `PerVcpuCounter` sync | `Scoreboard<u64>` (UnsafeCell slots) | Matches existing pattern in codebase; no atomic ops in hot path |
| `Mutex` in callbacks | Banned | Per-instruction mutex acquisition is a performance bug; all merging at `quantum_end()` |
| Subscription closure capture | `Arc::clone(&primitive)` | Probe outlives session; no dangling reference risk |
| Python GIL + trigger actions | Buffer-flag pattern | Triggers set `AtomicBool`; Python checks flag at quantum boundary |
| helm-plugin compatibility | Breaking removal | Probe call sites change; old PluginRegistry subscribers incompatible with new probe wiring |
| `helm-spy` → `helm-report` dep | Absent (by design) | Structural enforcement of collection ≠ delivery |

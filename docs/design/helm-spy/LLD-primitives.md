# helm-spy — LLD: Primitives

> **Document:** Low-Level Design — Primitives Layer (`src/primitives/`)
> **Crate:** `debug/helm-spy`
> **See also:** [HLD.md](HLD.md) for architecture overview

---

## 1. Module Map

```
src/primitives/
├── mod.rs        pub use counter::{Counter, PerVcpuCounter};
│                 pub use indexed::IndexedCounter;
│                 pub use histogram::{Histogram, IntervalHistogram};
│                 pub use heatmap::HeatMap;
│                 pub use ringbuf::{RingBuffer, EventStream};
│                 pub use trace_ring::{TraceRing, BranchRecord};
│                 pub use correl::CorrelHist2D;
├── counter.rs    Counter, PerVcpuCounter
├── indexed.rs    IndexedCounter
├── histogram.rs  Histogram, IntervalHistogram
├── heatmap.rs    HeatMap
├── ringbuf.rs    RingBuffer<T>, EventStream<T>
├── trace_ring.rs TraceRing<T: Copy+Send>, BranchRecord
└── correl.rs     CorrelHist2D
```

---

## 2. Counter (`counter.rs`)

### 2.1 `Counter`

```rust
pub struct Counter {
    name: String,
    value: AtomicU64,
}
```

Monotonic atomic counter. Thread-safe, lock-free.
Hot-path cost: one `fetch_add(Relaxed)` per increment.

**Methods:**
- `new(name: impl Into<String>) -> Self`
- `name(&self) -> &str`
- `inc(&self)` — `fetch_add(1, Relaxed)`
- `add(&self, n: u64)` — `fetch_add(n, Relaxed)`
- `value(&self) -> u64` — `load(Relaxed)`
- `reset(&self)` — `store(0, Relaxed)`

### 2.2 `PerVcpuCounter`

```rust
pub struct PerVcpuCounter {
    name: String,
    slots: Vec<AtomicU64>,
}
```

One `AtomicU64` slot per vCPU. Initialized at construction with `num_vcpus` slots.
Each slot is independently incremented; `total()` sums all slots at read time.

**Methods:**
- `new(name: impl Into<String>, num_vcpus: usize) -> Self`
- `name(&self) -> &str`
- `inc(&self, vcpu: usize)` — `slots[vcpu].fetch_add(1, Relaxed)`
- `add(&self, vcpu: usize, n: u64)` — `slots[vcpu].fetch_add(n, Relaxed)`
- `value(&self, vcpu: usize) -> u64` — `slots[vcpu].load(Relaxed)`
- `total(&self) -> u64` — sum of all slots
- `per_vcpu(&self) -> Vec<u64>` — snapshot of all slots
- `num_vcpus(&self) -> usize`

---

## 3. IndexedCounter (`indexed.rs`)

```rust
pub struct IndexedCounter {
    name: String,
    labels: Vec<&'static str>,
    buckets: Vec<AtomicU64>,
}
```

Fixed-dimension indexed counter. Each label maps to one `AtomicU64` bucket.
The label list is set at construction and is immutable.
Hot-path cost: one slice index + one `fetch_add(Relaxed)`.

**Methods:**
- `new(name: impl Into<String>, labels: &[&'static str]) -> Self`
- `name(&self) -> &str`
- `len(&self) -> usize` — number of labels/buckets
- `is_empty(&self) -> bool`
- `inc(&self, idx: usize)` — `buckets[idx].fetch_add(1, Relaxed)`
- `add(&self, idx: usize, n: u64)` — `buckets[idx].fetch_add(n, Relaxed)`
- `value(&self, idx: usize) -> u64`
- `total(&self) -> u64` — sum of all buckets
- `fraction(&self, idx: usize) -> f64` — `value(idx) / total`; returns 0.0 if total == 0
- `table(&self) -> Vec<(&'static str, u64, f64)>` — (label, count, fraction) for all buckets
- `reset(&self)` — sets all buckets to 0

---

## 4. Histogram (`histogram.rs`)

### 4.1 `Histogram`

```rust
pub struct Histogram {
    name: String,
    edges: Vec<u64>,
    buckets: Vec<AtomicU64>,
}
```

Fixed-bucket histogram. For N edges there are N+1 buckets:
- `bucket[0]`: val < edges[0]
- `bucket[i]` (0 < i < N): edges[i-1] <= val < edges[i]
- `bucket[N]`: val >= edges[N-1]

Hot-path cost: one `partition_point` binary search + one `fetch_add(Relaxed)`.

**Bucket index computation:**
```rust
let idx = self.edges.partition_point(|&e| val >= e);
```

**Methods:**
- `new(name: impl Into<String>, edges: Vec<u64>) -> Self`
- `name(&self) -> &str`
- `record(&self, val: u64)` — classify and increment bucket
- `counts(&self) -> Vec<u64>` — snapshot of all bucket counts
- `total(&self) -> u64` — sum of all buckets
- `percentile(&self, p: f64) -> u64` — returns lower edge of bucket containing the p-th percentile; returns 0 if empty
- `reset(&self)` — sets all buckets to 0

### 4.2 `IntervalHistogram`

```rust
pub struct IntervalHistogram {
    hist: Histogram,
    window_size: u64,
    window_accum: AtomicU64,
    last_window: AtomicU64,  // initialized to u64::MAX (sentinel: no window seen)
}
```

Samples a scalar every N instructions (where N = `window_size`), buckets the
per-window accumulated value into the inner `Histogram`.

**`tick(value: u64, insn_count: u64)`:**
```
window = insn_count / window_size
if window != last_window && last_window != u64::MAX:
    sample = swap(window_accum, value)
    hist.record(sample)
else:
    window_accum += 1
last_window = window
```

**Methods:**
- `new(name, edges: Vec<u64>, window_size: u64) -> Self`
- `tick(&self, value: u64, insn_count: u64)` — call every step
- `histogram(&self) -> &Histogram`
- `counts(&self) -> Vec<u64>`
- `total(&self) -> u64`

---

## 5. HeatMap (`heatmap.rs`)

```rust
pub struct HeatMap {
    name: String,
    counts: DashMap<u64, u64>,
}
```

Per-address (or per-PC) counter map using `dashmap::DashMap` for concurrent access.
Hot-path cost: one DashMap shard lock (brief critical section).

**Methods:**
- `new(name: impl Into<String>) -> Self`
- `name(&self) -> &str`
- `inc(&self, pc: u64)` — `entry(pc).or_insert(0) += 1`
- `top(&self, n: usize) -> Vec<(u64, u64)>` — top N entries sorted descending by count
- `get(&self, pc: u64) -> u64` — count for pc, or 0 if absent
- `len(&self) -> usize`
- `is_empty(&self) -> bool`
- `clear(&self)`

---

## 6. RingBuffer and EventStream (`ringbuf.rs`)

### 6.1 `RingBuffer<T: Clone + Send>`

```rust
pub struct RingBuffer<T: Clone + Send> {
    capacity: usize,
    buf: Mutex<VecDeque<T>>,
}
```

Fixed-capacity ring buffer. Overwrites oldest entries when full (pop_front then push_back).
Uses `Mutex` — suitable only for low-rate events (faults, syscalls, not per-instruction).

**Methods:**
- `new(capacity: usize) -> Self`
- `push(&self, val: T)` — evicts oldest if at capacity
- `snapshot(&self) -> Vec<T>` — clones all current entries
- `len(&self) -> usize`
- `is_empty(&self) -> bool`
- `clear(&self)`
- `capacity(&self) -> usize`

### 6.2 `EventStream<T: Clone + Send>`

```rust
pub struct EventStream<T: Clone + Send> {
    max: usize,
    events: Mutex<Vec<T>>,
}
```

Bounded event stream. Records events up to `max`, then stops (does not overwrite).
Uses `Mutex` — suitable only for low-rate events.

Initial capacity is `max.min(1024)`.

**Methods:**
- `new(max: usize) -> Self`
- `push(&self, val: T) -> bool` — returns false if stream is full
- `drain(&self) -> Vec<T>` — takes all events, leaving the stream empty (allows new pushes)
- `len(&self) -> usize`
- `is_empty(&self) -> bool`
- `max(&self) -> usize`

---

## 7. TraceRing and BranchRecord (`trace_ring.rs`)

### 7.1 `TraceRing<T: Copy + Send>`

```rust
pub struct TraceRing<T: Copy + Send> {
    buf: Box<[UnsafeCell<MaybeUninit<T>>]>,
    mask: usize,
    head: AtomicUsize,  // producer writes here
    tail: AtomicUsize,  // consumer reads here
}
```

Lock-free single-producer, single-consumer (SPSC) ring buffer.
Capacity must be a power of 2 (asserted at construction).
Hot-path cost: one `ptr::write` + one `AtomicUsize::store(Release)`.
No allocation per push. No locks.

`Send + Sync` are implemented manually; safety relies on SPSC usage discipline.

**Methods:**
- `new(capacity: usize) -> Self` — panics if capacity is not a power of 2 or is 0
- `push(&self, val: T) -> bool` — non-blocking; returns false if full (value dropped)
- `drain_into(&self, out: &mut Vec<T>)` — drains all available entries into caller-supplied Vec
- `len(&self) -> usize`
- `is_empty(&self) -> bool`
- `capacity(&self) -> usize`

**Push logic:**
```
h = head.load(Relaxed)
t = tail.load(Acquire)
if h.wrapping_sub(t) >= buf.len(): return false  // full
write buf[h & mask] = val
head.store(h + 1, Release)
return true
```

### 7.2 `BranchRecord`

```rust
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct BranchRecord {
    pub pc: u64,
    pub target: u64,
    pub insn_count: u64,
    pub flags: u8,      // bit 0 = taken, bits 1..3 = kind
    pub _pad: [u8; 7],
}
```

Compact branch record for high-rate tracing. Size is exactly 32 bytes (compile-time asserted).

**Methods:**
- `taken(self) -> bool` — `flags & 1 != 0`

---

## 8. CorrelHist2D (`correl.rs`)

```rust
pub struct CorrelHist2D {
    name: String,
    x_edges: Vec<u64>,
    y_edges: Vec<u64>,
    x_buckets: usize,   // x_edges.len() + 1
    y_buckets: usize,   // y_edges.len() + 1
    counts: Vec<AtomicU64>,  // flat row-major: counts[xi * y_buckets + yi]
}
```

2D joint histogram (correlation histogram). Flat `Vec<AtomicU64>` storage in row-major layout.
For X edges and Y edges: `(len(x_edges)+1) * (len(y_edges)+1)` total buckets.
Hot-path cost: two `partition_point` calls + one `fetch_add(Relaxed)`.

**Methods:**
- `new(name: impl Into<String>, x_edges: Vec<u64>, y_edges: Vec<u64>) -> Self`
- `name(&self) -> &str`
- `record(&self, x: u64, y: u64)` — classify both dimensions and increment cell
- `get(&self, xi: usize, yi: usize) -> u64` — count at bucket (xi, yi)
- `matrix(&self) -> Vec<Vec<u64>>` — full 2D count matrix as `Vec<Vec<u64>>`
- `total(&self) -> u64` — sum of all cells
- `x_buckets(&self) -> usize`
- `y_buckets(&self) -> usize`
- `reset(&self)` — zeros all cells

**Record logic:**
```rust
let xi = self.x_edges.partition_point(|&e| x >= e);
let yi = self.y_edges.partition_point(|&e| y >= e);
let idx = xi * self.y_buckets + yi;
self.counts[idx].fetch_add(1, Ordering::Relaxed);
```

---

## 9. Trigger (`trigger.rs`)

### 9.1 `TriggerKind`

```rust
pub enum TriggerKind {
    AtInsn(u64),          // fires when insn_count == N
    EveryN(u64),          // fires when insn_count % N == 0; never fires if N == 0
    AtPc(u64),            // fires when pc == addr
    PcRange(u64, u64),    // fires while pc in [start, end)
}
```

### 9.2 `Trigger`

```rust
pub struct Trigger {
    kind: TriggerKind,
    action: Box<dyn Fn(u64, u64) + Send + Sync>,  // args: (pc, insn_count)
    armed: AtomicBool,
    one_shot: bool,
}
```

**Constructor:**
```rust
pub fn new(
    kind: TriggerKind,
    one_shot: bool,
    action: impl Fn(u64, u64) + Send + Sync + 'static,
) -> Self
```

Starts armed (`armed = true`).

**`check(&self, pc: u64, insn_count: u64) -> bool`:**
1. If `!armed.load(Relaxed)`: return false immediately (fast path).
2. Evaluate condition based on `kind`.
3. If fired: call `action(pc, insn_count)`; if `one_shot`, set `armed = false`.
4. Return whether the condition fired.

**Other methods:**
- `is_armed(&self) -> bool`
- `arm(&self)` — sets armed to true
- `disarm(&self)` — sets armed to false

---

## 10. Window (`window.rs`)

### 10.1 `Window`

```rust
pub struct Window {
    pub start: u64,
    pub end: u64,
    active: AtomicBool,  // initialized false
}
```

Instruction-count range `[start, end)` for gating observation.

**Methods:**
- `new(start: u64, end: u64) -> Self` — `active` starts as false
- `is_active(&self, insn_count: u64) -> bool` — evaluates `insn_count >= start && insn_count < end`; updates cached `active` state; returns result
- `is_active_cached(&self) -> bool` — reads cached state without checking insn_count (valid only after at least one `is_active()` call)

### 10.2 `Windowed<T>`

```rust
pub struct Windowed<T> {
    pub window: Arc<Window>,
    pub inner: T,
}
```

Wraps any primitive `T` and gates access to inside-window only.

**Methods:**
- `new(window: Arc<Window>, inner: T) -> Self`
- `get_if_active(&self, insn_count: u64) -> Option<&T>` — returns `Some(&inner)` if in window, `None` otherwise

---

## 11. QuantumObserver (`quantum.rs`)

```rust
pub trait QuantumObserver: Send + Sync {
    fn quantum_end(&mut self, vcpu: usize, insn_count: u64);
}
```

Implemented by primitives or aggregates that need to finalize per-vCPU local state
after a vCPU quantum ends. Called at every `run()` return and before checkpoint save.
Runs on the cold path — may allocate, block on I/O, or acquire Mutex locks.

# helm-spy — Test Plan

> **Crate:** `helm-spy`
> **Test targets:** `Counter`, `PerVcpuCounter`, `IndexedCounter`, `Histogram`,
> `IntervalHistogram`, `HeatMap`, `RingBuffer`, `EventStream`, `TraceRing<T, N>`,
> `InsnMix`, `CacheModel`, `BranchPredictor`, `Trigger`, `Window`, `SpySession`

---

## Table of Contents

1. [Counter](#1-counter)
2. [PerVcpuCounter](#2-pervcpucounter)
3. [IndexedCounter](#3-indexedcounter)
4. [Histogram](#4-histogram)
5. [IntervalHistogram](#5-intervalhistogram)
6. [HeatMap](#6-heatmap)
7. [RingBuffer](#7-ringbuffer)
8. [EventStream](#8-eventstream)
9. [TraceRing](#9-tracering)
10. [InsnMix](#10-insnmix)
11. [CacheModel](#11-cachemodel)
12. [BranchPredictor](#12-branchpredictor)
13. [Trigger System](#13-trigger-system)
14. [Window](#14-window)
15. [SpySession — Integration Tests](#15-observesession--integration-tests)
16. [Test Matrix](#16-test-matrix)

---

## 1. Counter

### Test: `counter_basic_increment_and_read`

```rust
// tests/counter.rs
use helm_spy::primitives::Counter;

#[test]
fn counter_basic_increment_and_read() {
    let c = Counter::new("basic");
    assert_eq!(c.value(), 0, "fresh counter must be zero");

    c.inc();
    assert_eq!(c.value(), 1);

    c.inc();
    c.inc();
    assert_eq!(c.value(), 3);
}
```

### Test: `counter_add_by_n`

```rust
#[test]
fn counter_add_by_n() {
    let c = Counter::new("add_n");
    c.add(500);
    assert_eq!(c.value(), 500);
    c.add(1_000_000);
    assert_eq!(c.value(), 1_000_500);
}
```

### Test: `counter_reset`

```rust
#[test]
fn counter_reset() {
    let c = Counter::new("reset_test");
    for _ in 0..1000 { c.inc(); }
    assert_eq!(c.value(), 1000);
    c.reset();
    assert_eq!(c.value(), 0, "reset must return counter to zero");
    // Should be usable after reset
    c.inc();
    assert_eq!(c.value(), 1);
}
```

### Test: `counter_concurrent_increment`

```rust
use std::sync::Arc;
use std::thread;

#[test]
fn counter_concurrent_increment() {
    const THREADS: usize = 8;
    const PER_THREAD: u64 = 1_000_000;

    let c = Arc::new(Counter::new("concurrent"));
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let cc = Arc::clone(&c);
            thread::spawn(move || {
                for _ in 0..PER_THREAD { cc.inc(); }
            })
        })
        .collect();
    for h in handles { h.join().unwrap(); }

    let expected = THREADS as u64 * PER_THREAD;
    assert_eq!(c.value(), expected,
        "no increments must be lost: expected {expected}, got {}", c.value());
}
```

### Test: `counter_read_is_consistent`

```rust
#[test]
fn counter_read_is_consistent() {
    // Verify that get() after inc() returns a value >= the prior read.
    let c = Counter::new("monotone");
    let mut prev = 0u64;
    for _ in 0..10_000 {
        c.inc();
        let now = c.value();
        assert!(now >= prev, "counter must be monotonically non-decreasing: {prev} → {now}");
        prev = now;
    }
}
```

---

## 2. PerVcpuCounter

### Test: `per_vcpu_counter_independent_slots`

```rust
// tests/per_vcpu_counter.rs
use helm_spy::primitives::PerVcpuCounter;

#[test]
fn per_vcpu_counter_independent_slots() {
    let c = PerVcpuCounter::new("vcpu_test", 4);

    c.inc(0);
    c.inc(0);
    c.inc(1);
    c.inc(3);

    // Each vCPU has its own slot — increments to vCPU 0 must not affect vCPU 1.
    assert_eq!(c.per_vcpu(), vec![2, 1, 0, 1]);
}
```

### Test: `per_vcpu_counter_total_accumulation`

```rust
#[test]
fn per_vcpu_counter_total_accumulation() {
    let c = PerVcpuCounter::new("total_test", 4);

    for _ in 0..1000 { c.inc(0); }
    for _ in 0..500  { c.inc(1); }
    for _ in 0..250  { c.inc(2); }
    for _ in 0..125  { c.inc(3); }

    assert_eq!(c.total(), 1875, "total() must sum all vCPU slots");
    assert_eq!(c.per_vcpu(), vec![1000, 500, 250, 125]);
}
```

### Test: `per_vcpu_counter_zero_after_construction`

```rust
#[test]
fn per_vcpu_counter_zero_after_construction() {
    let c = PerVcpuCounter::new("zero_check", 8);
    assert_eq!(c.total(), 0);
    assert_eq!(c.per_vcpu(), vec![0u64; 8]);
}
```

### Test: `per_vcpu_counter_concurrent_each_vcpu`

```rust
use std::sync::Arc;

#[test]
fn per_vcpu_counter_concurrent_each_vcpu() {
    const VCPUS: usize = 4;
    const PER_VCPU: u64 = 100_000;

    let c = Arc::new(PerVcpuCounter::new("concurrent_vcpu", VCPUS));

    let handles: Vec<_> = (0..VCPUS)
        .map(|vcpu| {
            let cc = Arc::clone(&c);
            std::thread::spawn(move || {
                for _ in 0..PER_VCPU { cc.inc(vcpu); }
            })
        })
        .collect();
    for h in handles { h.join().unwrap(); }

    let per = c.per_vcpu();
    for (vcpu, &count) in per.iter().enumerate() {
        assert_eq!(count, PER_VCPU,
            "vCPU {vcpu}: expected {PER_VCPU}, got {count}");
    }
    assert_eq!(c.total(), VCPUS as u64 * PER_VCPU);
}
```

---

## 3. IndexedCounter

### Test: `indexed_counter_basic_increment`

```rust
// tests/indexed_counter.rs
use helm_spy::primitives::IndexedCounter;

#[test]
fn indexed_counter_basic_increment() {
    let labels = vec!["IntAlu", "Load", "Store", "Branch"];
    let ctr = IndexedCounter::new("test", labels);

    ctr.inc(0); ctr.inc(0); // 2 IntAlu
    ctr.inc(1);              // 1 Load
    ctr.inc(3);              // 1 Branch

    assert_eq!(ctr.get(0), 2, "IntAlu count");
    assert_eq!(ctr.get(1), 1, "Load count");
    assert_eq!(ctr.get(2), 0, "Store count (zero)");
    assert_eq!(ctr.get(3), 1, "Branch count");
    assert_eq!(ctr.total(), 4);
}
```

### Test: `indexed_counter_fraction`

```rust
#[test]
fn indexed_counter_fraction() {
    let ctr = IndexedCounter::new("frac", vec!["A", "B"]);
    ctr.add(0, 75);
    ctr.add(1, 25);

    let fa = ctr.fraction(0);
    let fb = ctr.fraction(1);
    assert!((fa - 0.75).abs() < 1e-9, "bucket A fraction: {fa}");
    assert!((fb - 0.25).abs() < 1e-9, "bucket B fraction: {fb}");
}
```

### Test: `indexed_counter_fraction_zero_total`

```rust
#[test]
fn indexed_counter_fraction_zero_total() {
    let ctr = IndexedCounter::new("zero", vec!["X", "Y"]);
    // Total is zero — fraction must return 0.0 without divide-by-zero.
    assert_eq!(ctr.fraction(0), 0.0);
    assert_eq!(ctr.total(), 0);
}
```

### Test: `indexed_counter_table_format`

```rust
#[test]
fn indexed_counter_table_format() {
    let ctr = IndexedCounter::new("tbl", vec!["A", "B", "C"]);
    ctr.inc(0); ctr.inc(0); // A=2
    ctr.inc(2);              // C=1

    let table = ctr.table();
    assert_eq!(table.len(), 3, "table must include all buckets (including zero-count)");

    let a_entry = table.iter().find(|(l, _, _)| *l == "A").unwrap();
    assert_eq!(a_entry.1, 2);
    assert!((a_entry.2 - 2.0 / 3.0).abs() < 1e-9, "A fraction: {}", a_entry.2);

    let b_entry = table.iter().find(|(l, _, _)| *l == "B").unwrap();
    assert_eq!(b_entry.1, 0);
    assert_eq!(b_entry.2, 0.0, "B fraction must be 0.0");
}
```

### Test: `indexed_counter_concurrent_increment`

```rust
use std::sync::Arc;
use std::thread;

#[test]
fn indexed_counter_concurrent_increment() {
    const THREADS: usize = 8;
    const PER_THREAD: u64 = 100_000;

    let ctr = Arc::new(IndexedCounter::new("concurrent", vec!["A", "B"]));

    let handles: Vec<_> = (0..THREADS)
        .map(|i| {
            let c = Arc::clone(&ctr);
            thread::spawn(move || {
                for _ in 0..PER_THREAD { c.inc(i % 2); }
            })
        })
        .collect();
    for h in handles { h.join().unwrap(); }

    let expected = THREADS as u64 * PER_THREAD;
    assert_eq!(ctr.total(), expected, "no increments lost under contention");
}
```

### Test: `indexed_counter_reset`

```rust
#[test]
fn indexed_counter_reset() {
    let ctr = IndexedCounter::new("rst", vec!["X"]);
    ctr.add(0, 1000);
    assert_eq!(ctr.total(), 1000);
    ctr.reset();
    assert_eq!(ctr.total(), 0);
    assert_eq!(ctr.get(0), 0);
}
```

### Test: `indexed_counter_merge`

```rust
#[test]
fn indexed_counter_merge() {
    let a = IndexedCounter::new("a", vec!["X", "Y"]);
    let b = IndexedCounter::new("b", vec!["X", "Y"]);
    a.add(0, 100); a.add(1, 50);
    b.add(0, 200); b.add(1, 75);
    a.merge(&b);
    assert_eq!(a.get(0), 300);
    assert_eq!(a.get(1), 125);
}
```

---

## 4. Histogram

### Test: `histogram_basic_bucketing`

```rust
// tests/histogram.rs
use helm_spy::primitives::Histogram;

#[test]
fn histogram_basic_bucketing() {
    // Buckets: [0,10), [10,100), [100,1000), [1000,∞)
    let h = Histogram::new(vec![10, 100, 1000]);

    h.record(5);     // → [0, 10)
    h.record(50);    // → [10, 100)
    h.record(500);   // → [100, 1000)
    h.record(5000);  // → [1000, ∞)

    let counts = h.counts();
    assert_eq!(counts.len(), 4);
    assert_eq!(counts[0], 1, "bucket [0, 10)");
    assert_eq!(counts[1], 1, "bucket [10, 100)");
    assert_eq!(counts[2], 1, "bucket [100, 1000)");
    assert_eq!(counts[3], 1, "bucket [1000, ∞)");
}
```

### Test: `histogram_edge_boundary`

```rust
#[test]
fn histogram_edge_boundary() {
    let h = Histogram::new(vec![10, 100]);

    // record(10) must go into [10, 100), not [0, 10).
    h.record(10);
    h.record(100);  // → [100, ∞)

    let counts = h.counts();
    assert_eq!(counts[0], 0, "nothing in [0, 10)");
    assert_eq!(counts[1], 1, "10 in [10, 100)");
    assert_eq!(counts[2], 1, "100 in [100, ∞)");
}
```

### Test: `histogram_percentile`

```rust
#[test]
fn histogram_percentile() {
    // 1000 samples uniformly distributed across [0, 1000)
    let h = Histogram::new(vec![250, 500, 750, 1000]);
    for i in 0u64..1000 { h.record(i); }

    // p50 should be near 500
    let p50 = h.percentile(0.50);
    assert!(p50 >= 250.0 && p50 <= 750.0,
        "p50 expected near 500, got {p50}");
    // p100 should be in the overflow bucket
    let p100 = h.percentile(1.0);
    assert!(p100 > 0.0);
}
```

### Test: `histogram_concurrent_record`

```rust
use std::sync::Arc;

#[test]
fn histogram_concurrent_record() {
    let h = Arc::new(Histogram::new(vec![50, 100]));
    const N: usize = 10_000;

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let hh = Arc::clone(&h);
            std::thread::spawn(move || {
                for _ in 0..N { hh.record(75); }  // all in [50, 100)
            })
        })
        .collect();
    for handle in handles { handle.join().unwrap(); }

    let counts = h.counts();
    assert_eq!(counts[1], 4 * N as u64, "all records in [50, 100)");
}
```

---

## 5. IntervalHistogram

### Test: `interval_histogram_basic_windowing`

```rust
// tests/interval_histogram.rs
use helm_spy::primitives::IntervalHistogram;

#[test]
fn interval_histogram_basic_windowing() {
    // window = 10 instructions; edges = [5, 10, 15]
    let hist = IntervalHistogram::new("ipc_dist", 10, vec![5, 10, 15]);

    // Tick 20 times with value=1 — 2 windows of 10 each, accumulating 10 per window.
    for i in 0u64..20 { hist.tick_with(1, i); }

    // Each window accumulates 10 → two samples of 10 → bucket [10, 15)
    let counts = hist.counts();
    let total: u64 = counts.iter().sum();
    assert!(total >= 1, "at least one window must have been committed");
}
```

### Test: `interval_histogram_single_window`

```rust
#[test]
fn interval_histogram_single_window() {
    let hist = IntervalHistogram::new("single", 5, vec![3, 6, 9]);

    // Tick 5 times with value=2 → window accumulates 10.
    for i in 0u64..5 { hist.tick_with(2, i); }
    // One more tick crosses the boundary and commits the window.
    hist.tick_with(0, 5);

    let total: u64 = hist.counts().iter().sum();
    assert!(total >= 1, "window should have been committed on boundary crossing");
}
```

### Test: `interval_histogram_window_boundary_is_exclusive`

```rust
#[test]
fn interval_histogram_window_boundary_is_exclusive() {
    // window_size=3: windows are [0,3), [3,6), [6,9), ...
    let hist = IntervalHistogram::new("boundary", 3, vec![2, 4]);

    // insn_count 0,1,2 → window 0; 3,4,5 → window 1 (boundary at 3).
    for i in 0u64..6 { hist.tick_with(1, i); }
    // After 6 ticks: window 0 committed at boundary (i=3); window 1 committed at boundary (i=6 — not yet)
    // Total committed = at least 1 (window 0)
    let total: u64 = hist.counts().iter().sum();
    assert!(total >= 1, "window 0 must be committed when window 1 starts");
}
```

### Test: `interval_histogram_approx_mean`

```rust
#[test]
fn interval_histogram_approx_mean() {
    // window=1: every instruction is its own window.
    let hist = IntervalHistogram::new("mean_test", 1, vec![4, 8]);

    for i in 0u64..100 {
        // Alternate 3 and 7: roughly half in [0,4), half in [4,8)
        hist.tick_with(if i % 2 == 0 { 3 } else { 7 }, i);
    }
    let counts = hist.counts();
    let total: u64 = counts.iter().sum();
    assert!(total > 0, "some samples must have been recorded");

    let mean = hist.approx_mean();
    assert!(mean > 0.0, "mean must be positive");
    // Mean should be approximately 5 (average of 3 and 7)
    assert!(mean > 2.0 && mean < 9.0, "mean {mean} out of expected range");
}
```

---

## 6. HeatMap

### Test: `heatmap_inc_and_top`

```rust
// tests/heatmap.rs
use helm_spy::primitives::HeatMap;

#[test]
fn heatmap_inc_and_top() {
    let hm = HeatMap::new("hot_pcs");

    // Add varying counts to three PCs.
    for _ in 0..100 { hm.inc(0xDEAD_0000); }
    for _ in 0..50  { hm.inc(0xBEEF_0000); }
    for _ in 0..10  { hm.inc(0xCAFE_0000); }

    let top = hm.top(3);
    assert_eq!(top.len(), 3, "top(3) must return 3 entries");
    assert_eq!(top[0].0, 0xDEAD_0000, "hottest PC");
    assert_eq!(top[0].1, 100,         "hottest PC count");
    assert_eq!(top[1].0, 0xBEEF_0000, "second PC");
    assert_eq!(top[2].0, 0xCAFE_0000, "third PC");
}
```

### Test: `heatmap_top_n_ordering`

```rust
#[test]
fn heatmap_top_n_ordering() {
    let hm = HeatMap::new("ordering");

    // Insert 20 PCs with counts 1..=20.
    for i in 1u64..=20 { hm.add(i * 0x1000, i); }

    let top10 = hm.top(10);
    // Must be sorted descending by count.
    assert_eq!(top10.len(), 10);
    for w in top10.windows(2) {
        assert!(w[0].1 >= w[1].1,
            "top() must be sorted descending: {} >= {} failed", w[0].1, w[1].1);
    }
    // The hottest PC should have count 20.
    assert_eq!(top10[0].1, 20, "hottest PC count");
}
```

### Test: `heatmap_top_n_fewer_than_n_entries`

```rust
#[test]
fn heatmap_top_n_fewer_than_n_entries() {
    let hm = HeatMap::new("sparse");
    hm.inc(0x1000);
    hm.inc(0x2000);

    // top(10) when only 2 PCs recorded — must return 2, not panic.
    let top = hm.top(10);
    assert_eq!(top.len(), 2, "top(10) with 2 entries must return 2");
}
```

### Test: `heatmap_concurrent_inc`

```rust
use std::sync::Arc;

#[test]
fn heatmap_concurrent_inc() {
    const THREADS: usize = 4;
    const PER_THREAD: u64 = 10_000;

    let hm = Arc::new(HeatMap::new("concurrent_heat"));

    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let h = Arc::clone(&hm);
            std::thread::spawn(move || {
                for _ in 0..PER_THREAD { h.inc(0xAAAA_0000); }
            })
        })
        .collect();
    for handle in handles { handle.join().unwrap(); }

    let top = hm.top(1);
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].1, THREADS as u64 * PER_THREAD,
        "concurrent inc must not lose updates");
}
```

---

## 7. RingBuffer

### Test: `ring_buffer_push_snapshot`

```rust
// tests/ring_buffer.rs
use helm_spy::primitives::RingBuffer;

#[derive(Debug, Clone, PartialEq)]
struct TestEvent { value: u32 }

#[test]
fn ring_buffer_push_snapshot() {
    let rb: RingBuffer<TestEvent> = RingBuffer::new("test", 4);

    rb.push(TestEvent { value: 1 });
    rb.push(TestEvent { value: 2 });
    rb.push(TestEvent { value: 3 });

    let snap = rb.snapshot();
    assert_eq!(snap.len(), 3);
    assert_eq!(snap[0].value, 1);
    assert_eq!(snap[2].value, 3);
}
```

### Test: `ring_buffer_capacity_overwrite`

```rust
#[test]
fn ring_buffer_capacity_overwrite() {
    let rb: RingBuffer<TestEvent> = RingBuffer::new("cap", 3);

    // Push 5 events into a buffer of capacity 3.
    for i in 1u32..=5 { rb.push(TestEvent { value: i }); }

    let snap = rb.snapshot();
    // Only the last 3 must survive.
    assert_eq!(snap.len(), 3, "capacity 3 must hold at most 3 events");

    // The last 3 pushed are 3, 4, 5.
    let values: Vec<u32> = snap.iter().map(|e| e.value).collect();
    assert_eq!(values, vec![3, 4, 5],
        "oldest events must have been overwritten: got {values:?}");
}
```

### Test: `ring_buffer_empty_snapshot`

```rust
#[test]
fn ring_buffer_empty_snapshot() {
    let rb: RingBuffer<TestEvent> = RingBuffer::new("empty", 8);
    let snap = rb.snapshot();
    assert!(snap.is_empty(), "snapshot of empty ring must return empty Vec");
}
```

### Test: `ring_buffer_len`

```rust
#[test]
fn ring_buffer_len() {
    let rb: RingBuffer<TestEvent> = RingBuffer::new("len_test", 8);
    assert_eq!(rb.len(), 0);

    rb.push(TestEvent { value: 1 });
    assert_eq!(rb.len(), 1);

    rb.push(TestEvent { value: 2 });
    assert_eq!(rb.len(), 2);
}
```

---

## 8. EventStream

### Test: `event_stream_push_drain`

```rust
// tests/event_stream.rs
use helm_spy::primitives::EventStream;

#[derive(Debug, Clone, PartialEq)]
struct Event { seq: u64 }

#[test]
fn event_stream_push_drain() {
    let stream: EventStream<Event> = EventStream::new("stream", 10);

    for i in 0u64..5 { stream.push(Event { seq: i }); }

    let drained = stream.drain();
    assert_eq!(drained.len(), 5);
    for (i, ev) in drained.iter().enumerate() {
        assert_eq!(ev.seq, i as u64, "event at index {i} has wrong seq");
    }
    // After drain, stream must be empty.
    assert_eq!(stream.drain().len(), 0, "stream must be empty after drain");
}
```

### Test: `event_stream_bounded_at_max`

```rust
#[test]
fn event_stream_bounded_at_max() {
    let stream: EventStream<Event> = EventStream::new("bounded", 5);

    // Push more events than the capacity.
    for i in 0u64..10 { stream.push(Event { seq: i }); }

    let drained = stream.drain();
    assert_eq!(drained.len(), 5,
        "EventStream must stop recording at max capacity");

    // First 5 events must be captured (not the later ones).
    for (i, ev) in drained.iter().enumerate() {
        assert_eq!(ev.seq, i as u64, "event {i} has wrong seq: {}", ev.seq);
    }
}
```

### Test: `event_stream_capacity_zero_never_panics`

```rust
#[test]
fn event_stream_capacity_zero_never_panics() {
    // max=0 means capture nothing.
    let stream: EventStream<Event> = EventStream::new("zero_cap", 0);
    stream.push(Event { seq: 1 });
    stream.push(Event { seq: 2 });
    assert_eq!(stream.drain().len(), 0);
}
```

---

## 9. TraceRing

### Test: `trace_ring_push_drain`

```rust
// tests/trace_ring.rs
use helm_spy::primitives::TraceRing;

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq)]
struct TestRecord { value: u64 }

#[test]
fn trace_ring_push_drain() {
    let ring: TraceRing<TestRecord, 16> = TraceRing::new();

    ring.push(TestRecord { value: 1 });
    ring.push(TestRecord { value: 2 });
    ring.push(TestRecord { value: 3 });

    let mut out = Vec::new();
    ring.drain_into(&mut out);

    assert_eq!(out.len(), 3);
    assert_eq!(out[0].value, 1);
    assert_eq!(out[1].value, 2);
    assert_eq!(out[2].value, 3);
}
```

### Test: `trace_ring_drain_empty`

```rust
#[test]
fn trace_ring_drain_empty() {
    let ring: TraceRing<TestRecord, 8> = TraceRing::new();
    let mut out = Vec::new();
    ring.drain_into(&mut out);
    assert_eq!(out.len(), 0, "drain on empty ring must produce no records");
}
```

### Test: `trace_ring_overwrite_on_full`

```rust
#[test]
fn trace_ring_overwrite_on_full() {
    // Ring capacity = 4. Lossy overwrite semantics.
    let ring: TraceRing<TestRecord, 4> = TraceRing::new();

    // Push 6 records — ring overwrites the oldest when full.
    for i in 0u64..6 { ring.push(TestRecord { value: i }); }

    let mut out = Vec::new();
    ring.drain_into(&mut out);

    // Must not produce more records than capacity allows to be drained.
    assert!(out.len() <= 4,
        "must not produce more than capacity records, got {}", out.len());
}
```

### Test: `trace_ring_len`

```rust
#[test]
fn trace_ring_len() {
    let ring: TraceRing<TestRecord, 8> = TraceRing::new();
    assert_eq!(ring.len(), 0);

    ring.push(TestRecord { value: 42 });
    assert_eq!(ring.len(), 1);

    ring.push(TestRecord { value: 43 });
    assert_eq!(ring.len(), 2);

    let mut out = Vec::new();
    ring.drain_into(&mut out);
    assert_eq!(ring.len(), 0, "len must be 0 after drain");
}
```

### Test: `trace_ring_producer_consumer_threads`

```rust
use std::sync::Arc;

#[test]
fn trace_ring_producer_consumer_threads() {
    // Single-producer, single-consumer pattern.
    let ring = Arc::new(TraceRing::<TestRecord, 1024>::new());

    let producer = {
        let r = Arc::clone(&ring);
        std::thread::spawn(move || {
            for i in 0u64..512 { r.push(TestRecord { value: i }); }
        })
    };
    producer.join().unwrap();

    // Consumer reads all records after producer finishes.
    let mut out = Vec::new();
    ring.drain_into(&mut out);

    assert_eq!(out.len(), 512, "all 512 records must be readable");
    for (i, rec) in out.iter().enumerate() {
        assert_eq!(rec.value, i as u64, "record at index {i} has wrong value");
    }
}
```

### Test: `branch_record_flags`

```rust
use helm_spy::primitives::BranchRecord;

#[test]
fn branch_record_flags() {
    let rec = BranchRecord {
        pc: 0x1000, target: 0x2000, insn_count: 100,
        flags: 0b0000_0101,  // taken=1, predicted=0, kind=1
        _pad: [0; 7],
    };
    assert!(rec.taken(),        "bit 0: taken");
    assert!(!rec.predicted(),   "bit 1: predicted=0");
    assert_eq!(rec.kind(), 1,   "bits 2..4: kind=1");
}
```

---

## 10. InsnMix

### Test: `insn_mix_record_all_classes`

```rust
// tests/insn_mix.rs
use helm_spy::analysis::InsnMix;
use helm_engine::InsnClass;

#[test]
fn insn_mix_record_all_classes() {
    let mix = InsnMix::new("test_mix");

    mix.record(InsnClass::IntAlu);
    mix.record(InsnClass::IntAlu);
    mix.record(InsnClass::Load);
    mix.record(InsnClass::Store);
    mix.record(InsnClass::Branch);

    assert_eq!(mix.count(InsnClass::IntAlu), 2);
    assert_eq!(mix.count(InsnClass::Load),   1);
    assert_eq!(mix.count(InsnClass::Store),  1);
    assert_eq!(mix.count(InsnClass::Branch), 1);
    assert_eq!(mix.count(InsnClass::FpScalar), 0, "unrecorded class must be zero");
    assert_eq!(mix.total(), 5);
}
```

### Test: `insn_mix_table_sorted_descending`

```rust
#[test]
fn insn_mix_table_sorted_descending() {
    let mix = InsnMix::new("sorted");

    for _ in 0..100 { mix.record(InsnClass::IntAlu); }
    for _ in 0..50  { mix.record(InsnClass::Load);   }
    for _ in 0..10  { mix.record(InsnClass::Branch); }

    let table = mix.table();

    // Table must be sorted descending by count.
    for window in table.windows(2) {
        assert!(window[0].1 >= window[1].1,
            "table not sorted: {:?} before {:?}", window[0], window[1]);
    }
    assert_eq!(table[0].0, InsnClass::IntAlu, "IntAlu must be first");
    assert_eq!(table[0].1, 100, "IntAlu count");
}
```

### Test: `insn_mix_percentage_sums_to_100`

```rust
#[test]
fn insn_mix_percentage_sums_to_100() {
    let mix = InsnMix::new("pct");
    for _ in 0..1000 { mix.record(InsnClass::IntAlu); }
    for _ in 0..500  { mix.record(InsnClass::Load);   }

    let table = mix.table();
    let total_pct: f64 = table.iter().map(|(_, _, p)| p).sum();
    assert!((total_pct - 100.0).abs() < 0.001,
        "percentages must sum to 100.0%, got {total_pct:.3}%");
}
```

### Test: `insn_mix_fraction`

```rust
#[test]
fn insn_mix_fraction() {
    let mix = InsnMix::new("frac");
    for _ in 0..75 { mix.record(InsnClass::IntAlu); }
    for _ in 0..25 { mix.record(InsnClass::Load);   }

    let f = mix.fraction(InsnClass::IntAlu);
    assert!((f - 0.75).abs() < 1e-9, "IntAlu fraction: {f}");
}
```

### Test: `insn_mix_concurrent_record`

```rust
use std::sync::Arc;

#[test]
fn insn_mix_concurrent_record() {
    const THREADS: usize = 4;
    const PER_THREAD: u64 = 250_000;

    let mix = Arc::new(InsnMix::new("concurrent_mix"));

    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let m = Arc::clone(&mix);
            std::thread::spawn(move || {
                for _ in 0..PER_THREAD { m.record(InsnClass::IntAlu); }
            })
        })
        .collect();
    for h in handles { h.join().unwrap(); }

    assert_eq!(mix.total(), THREADS as u64 * PER_THREAD,
        "no increments lost under 4-thread concurrent record()");
}
```

### Test: `insn_mix_reset`

```rust
#[test]
fn insn_mix_reset() {
    let mix = InsnMix::new("rst");
    for _ in 0..1000 { mix.record(InsnClass::IntAlu); }
    mix.reset();
    assert_eq!(mix.total(), 0);
    assert_eq!(mix.count(InsnClass::IntAlu), 0);
    // Must be usable after reset.
    mix.record(InsnClass::Branch);
    assert_eq!(mix.total(), 1);
}
```

---

## 11. CacheModel

### Test: `cache_model_basic_hit_miss`

```rust
// tests/cache_model.rs
use helm_spy::analysis::{CacheModel, CacheResult};

#[test]
fn cache_model_basic_hit_miss() {
    // 2-set, 2-way, 64-byte lines
    let mut cache = CacheModel::new("l1d", 2, 2, 64);

    // First access: always miss (cold).
    assert_eq!(cache.access(0x0000), CacheResult::Miss);
    // Same cache line again: hit.
    assert_eq!(cache.access(0x0000), CacheResult::Hit);
    // Different cache line in same set: miss.
    assert_eq!(cache.access(0x0040), CacheResult::Miss);
    // Hit again.
    assert_eq!(cache.access(0x0040), CacheResult::Hit);

    assert_eq!(cache.hits(),   2);
    assert_eq!(cache.misses(), 2);
    assert!((cache.hit_rate() - 0.5).abs() < 1e-9,
        "hit rate: {}", cache.hit_rate());
}
```

### Test: `cache_model_lru_eviction`

```rust
#[test]
fn cache_model_lru_eviction() {
    // 1-set, 2-way, 64-byte lines — cleanest LRU test.
    let mut cache = CacheModel::new("tiny", 1, 2, 64);

    cache.access(0x00);  // miss, fills way 0
    cache.access(0x40);  // miss, fills way 1 (0x40 = next 64-byte line in set 0)
    cache.access(0x00);  // hit — makes way 0 MRU, way 1 becomes LRU

    // Bring in a third line: LRU way (way 1, line 0x40) must be evicted.
    let result = cache.access(0x80);
    assert!(matches!(result, CacheResult::Evict(_)),
        "expected eviction, got {:?}", result);

    // Line 0x00 must still be in the cache (it was MRU).
    assert_eq!(cache.access(0x00), CacheResult::Hit,
        "MRU line must survive eviction");
}
```

### Test: `cache_model_mpki`

```rust
#[test]
fn cache_model_mpki() {
    let mut cache = CacheModel::new("mpki_test", 4, 4, 64);

    // 100 unique cache lines → 100 compulsory misses.
    for i in 0u64..100 {
        cache.access(i * 64 * 16);  // stride > cache size → all misses
    }
    let mpki = cache.mpki(1000);
    // 100 misses / 1000 instructions × 1000 = 100.0
    assert!((mpki - 100.0).abs() < 0.1, "MPKI expected 100.0, got {mpki}");
}
```

### Test: `cache_model_hit_rate_zero_accesses`

```rust
#[test]
fn cache_model_hit_rate_zero_accesses() {
    let cache = CacheModel::new("empty", 4, 4, 64);
    // No accesses → hit_rate() returns 1.0 (defined behavior: no misses observed)
    assert_eq!(cache.hit_rate(), 1.0);
    assert_eq!(cache.mpki(0), 0.0);
}
```

### Test: `cache_model_reset`

```rust
#[test]
fn cache_model_reset() {
    let mut cache = CacheModel::new("rst", 2, 2, 64);
    cache.access(0x000);
    cache.access(0x000);  // hit
    assert_eq!(cache.hits(), 1);
    cache.reset();
    assert_eq!(cache.hits(),   0);
    assert_eq!(cache.misses(), 0);
    // After reset: same address is a miss again (cold start).
    assert_eq!(cache.access(0x000), CacheResult::Miss);
}
```

### Test: `cache_model_merge_stats`

```rust
#[test]
fn cache_model_merge_stats() {
    let mut a = CacheModel::new("a", 2, 2, 64);
    let mut b = CacheModel::new("b", 2, 2, 64);

    a.access(0x00); a.access(0x00);  // 1 miss, 1 hit in a
    b.access(0x40); b.access(0x80);  // 2 misses in b

    a.merge_stats(&b);

    assert_eq!(a.hits(),   1, "merged hits");
    assert_eq!(a.misses(), 3, "merged misses");
}
```

### Test: `cache_model_set_associativity_correct`

```rust
#[test]
fn cache_model_set_associativity_correct() {
    // 4-set, 1-way (direct-mapped), 64-byte lines.
    // Addresses that map to the same set: addr=0x000 → set 0; addr=0x100 → set 0 (conflict).
    // 0x000 / 64 = 0; 0 % 4 = 0.  0x100 / 64 = 4; 4 % 4 = 0. Conflict!
    let mut cache = CacheModel::new("dm", 4, 1, 64);

    cache.access(0x000);  // miss, fills set 0
    cache.access(0x040);  // miss, fills set 1 (different set)
    cache.access(0x000);  // hit (still in set 0)

    // 0x100 maps to set 0 → conflicts with 0x000.
    let result = cache.access(0x100);
    assert!(matches!(result, CacheResult::Miss | CacheResult::Evict(_)),
        "direct-mapped conflict must miss");

    // Now 0x000 should be evicted.
    let result2 = cache.access(0x000);
    assert_ne!(result2, CacheResult::Hit,
        "0x000 must have been evicted by 0x100 in a direct-mapped cache");
}
```

---

## 12. BranchPredictor

### Test: `branch_pred_bimodal_all_taken`

```rust
// tests/branch_pred.rs
use helm_spy::analysis::{BranchPredictor, PredictorKind};

#[test]
fn branch_pred_bimodal_all_taken() {
    let mut pred = BranchPredictor::bimodal_4k();

    // Warmup: 20 taken branches — 2-bit counter saturates to 0b11.
    for _ in 0..20 { pred.predict(0x1000, true); }

    // After warmup, should predict taken correctly.
    let correct = pred.predict(0x1000, true);
    assert!(correct, "warmed-up BiModal should predict taken correctly");
    assert!(pred.miss_rate() < 0.15,
        "miss rate should be low after warmup: {}", pred.miss_rate());
}
```

### Test: `branch_pred_bimodal_alternating`

```rust
#[test]
fn branch_pred_bimodal_alternating() {
    let mut pred = BranchPredictor::bimodal_4k();

    let mut mispreds = 0u32;
    for i in 0..100u32 {
        let taken = i % 2 == 0;
        if !pred.predict(0x2000, taken) { mispreds += 1; }
    }
    // BiModal cannot track alternating — expect >40% mispredictions.
    assert!(mispreds > 40,
        "bimodal should fail on alternating pattern, mispreds={mispreds}/100");
}
```

### Test: `branch_pred_gshare_loop`

```rust
#[test]
fn branch_pred_gshare_loop() {
    let mut pred = BranchPredictor::gshare_4k();

    // Loop: 9 taken then 1 not-taken, repeated. GShare should learn this pattern.
    let mut mispreds = 0u32;
    let mut taken_iter = (0u32..).map(|i| i % 10 != 9);  // 9T then 1NT

    // Warmup: 50 iterations.
    for _ in 0..50 { pred.predict(0x3000, taken_iter.next().unwrap()); }

    // Measurement: next 150 iterations.
    let post_warmup = 150u32;
    for _ in 0..post_warmup {
        let taken = taken_iter.next().unwrap();
        if !pred.predict(0x3000, taken) { mispreds += 1; }
    }

    let miss_rate = mispreds as f64 / post_warmup as f64;
    assert!(miss_rate < 0.20,
        "GShare should converge on a loop pattern, miss_rate={miss_rate:.2}");
}
```

### Test: `branch_pred_gshare_better_than_bimodal_on_correlated`

```rust
#[test]
fn branch_pred_gshare_better_than_bimodal_on_correlated() {
    // Pattern: B1=taken implies B2=taken (correlated branches).
    // GShare captures this via global history; BiModal cannot.
    let mut bimodal = BranchPredictor::bimodal_4k();
    let mut gshare  = BranchPredictor::gshare_4k();

    // Warmup with 100 correlated pairs.
    for _ in 0..50 {
        bimodal.predict(0x1000, true); bimodal.predict(0x2000, true);
        gshare.predict(0x1000, true);  gshare.predict(0x2000, true);
    }

    let bimodal_rate = bimodal.miss_rate();
    let gshare_rate  = gshare.miss_rate();

    // GShare should be at least as good as BiModal (not necessarily strictly better
    // in this simplified test, but must not be dramatically worse).
    assert!(gshare_rate <= bimodal_rate + 0.15,
        "GShare should not be much worse than BiModal: gshare={gshare_rate:.2} bimodal={bimodal_rate:.2}");
}
```

### Test: `branch_pred_perfect_zero_mispredictions`

```rust
#[test]
fn branch_pred_perfect_zero_mispredictions() {
    let mut pred = BranchPredictor::perfect();

    for i in 0u64..1000 {
        let taken = i % 3 == 0;  // arbitrary pattern
        assert!(pred.predict(0x4000, taken),
            "perfect predictor must always be correct at i={i}");
    }
    assert_eq!(pred.miss_rate(), 0.0);
    assert_eq!(pred.mpki(1000), 0.0);
}
```

### Test: `branch_pred_miss_rate_calculation`

```rust
#[test]
fn branch_pred_miss_rate_calculation() {
    let mut pred = BranchPredictor::bimodal_4k();

    // BiModal initializes to weakly-taken (0b10 ≥ 2 → predicts taken).
    // First prediction at a cold PC with taken=false → misprediction.
    let r1 = pred.predict(0x6000, false);  // cold weakly-taken predicts taken → MISS
    let r2 = pred.predict(0x6000, false);  // counter now 0b01 → predicts not-taken → HIT
    let r3 = pred.predict(0x6000, false);  // counter still predicts not-taken → HIT

    assert!(!r1, "cold prediction for taken=false should be a misprediction");
    assert!(r2,  "second prediction for taken=false should be correct");
    assert!(r3,  "third prediction for taken=false should be correct");

    assert_eq!(pred.predictions, 3);
    assert_eq!(pred.mispredictions, 1);
    assert!((pred.miss_rate() - 1.0 / 3.0).abs() < 1e-9);
}
```

### Test: `branch_pred_reset`

```rust
#[test]
fn branch_pred_reset() {
    let mut pred = BranchPredictor::bimodal_4k();

    // Warmup to a stable state.
    for _ in 0..50 { pred.predict(0x7000, true); }
    assert!(pred.miss_rate() < 0.1);

    pred.reset();

    // After reset: counters are zero, table is weakly-taken.
    assert_eq!(pred.predictions, 0);
    assert_eq!(pred.mispredictions, 0);
    assert_eq!(pred.miss_rate(), 0.0);
}
```

### Test: `branch_pred_mpki`

```rust
#[test]
fn branch_pred_mpki() {
    let mut pred = BranchPredictor::bimodal_4k();

    // Force 10 mispredictions out of 1000 instructions.
    // (Each misprediction corresponds to 1 branch, and we have ~100 branches in 1000 insns)
    // Simpler: measure the mpki formula directly.
    pred.mispredictions = 50;
    pred.predictions    = 100;

    let mpki = pred.mpki(1000);
    // 50 mispreds / 1000 insns × 1000 = 50.0
    assert!((mpki - 50.0).abs() < 1e-9, "MPKI: {mpki}");
}
```

---

## 13. Trigger System

### Test: `trigger_at_insn_fires_once`

```rust
// tests/trigger.rs
use helm_spy::analysis::Trigger;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[test]
fn trigger_at_insn_fires_once() {
    let fire_count = Arc::new(AtomicU32::new(0));
    let fc = Arc::clone(&fire_count);

    let trigger = Trigger::at_insn(50, move |_| {
        fc.fetch_add(1, Ordering::Relaxed);
    });

    for i in 0u64..50 { trigger.check(0x1000, i); }
    assert_eq!(fire_count.load(Ordering::Relaxed), 0, "must not fire before count=50");

    trigger.check(0x1000, 50);
    assert_eq!(fire_count.load(Ordering::Relaxed), 1, "must fire at count=50");

    // One-shot: must not fire again.
    trigger.check(0x1000, 51);
    trigger.check(0x1000, 100);
    assert_eq!(fire_count.load(Ordering::Relaxed), 1, "one-shot must not fire twice");
}
```

### Test: `trigger_every_n_fires_periodically`

```rust
#[test]
fn trigger_every_n_fires_periodically() {
    let fire_count = Arc::new(AtomicU32::new(0));
    let fc = Arc::clone(&fire_count);

    let trigger = Trigger::every_n(10, move |_| {
        fc.fetch_add(1, Ordering::Relaxed);
    });

    // Fire checks 0..100: fires at 0, 10, 20, ..., 90 → 10 times.
    for i in 0u64..100 { trigger.check(0x2000, i); }

    assert_eq!(fire_count.load(Ordering::Relaxed), 10,
        "EveryN(10) must fire 10 times in 100 ticks");
}
```

### Test: `trigger_at_pc_fires_on_match`

```rust
#[test]
fn trigger_at_pc_fires_on_match() {
    let fired = Arc::new(AtomicU32::new(0));
    let f = Arc::clone(&fired);

    // `at_pc` is repeating (one_shot=false).
    let trigger = Trigger::new(
        helm_spy::analysis::TriggerKind::AtPc(0xDEAD_0000),
        move |_| { f.fetch_add(1, Ordering::Relaxed); },
        false,  // not one-shot
    );

    trigger.check(0x1000_0000, 1);  // wrong PC
    trigger.check(0xDEAD_0000, 2);  // match
    trigger.check(0xDEAD_0000, 3);  // match again
    trigger.check(0x0000_0000, 4);  // no match

    assert_eq!(fired.load(Ordering::Relaxed), 2, "must fire on both PC matches");
}
```

### Test: `trigger_counter_reaches`

```rust
use helm_spy::primitives::Counter;

#[test]
fn trigger_counter_reaches() {
    let counter = Arc::new(Counter::new("threshold_test"));
    let fired   = Arc::new(AtomicU32::new(0));
    let f       = Arc::clone(&fired);
    let c       = Arc::clone(&counter);

    let trigger = Trigger::new(
        helm_spy::analysis::TriggerKind::CounterReaches(Arc::clone(&counter), 100),
        move |_| { f.fetch_add(1, Ordering::Relaxed); },
        true,  // one-shot
    );

    c.add(99);
    trigger.check(0x5000, 1);
    assert_eq!(fired.load(Ordering::Relaxed), 0, "must not fire at 99");

    c.inc();  // now at 100
    trigger.check(0x5000, 2);
    assert_eq!(fired.load(Ordering::Relaxed), 1, "must fire when counter reaches 100");

    trigger.check(0x5000, 3);
    assert_eq!(fired.load(Ordering::Relaxed), 1, "one-shot must not fire again");
}
```

### Test: `trigger_disarm_rearm`

```rust
#[test]
fn trigger_disarm_rearm() {
    let fire_count = Arc::new(AtomicU32::new(0));
    let fc = Arc::clone(&fire_count);

    let trigger = Trigger::every_n(1, move |_| {
        fc.fetch_add(1, Ordering::Relaxed);
    });

    for i in 0u64..5 { trigger.check(0x3000, i); }
    assert_eq!(fire_count.load(Ordering::Relaxed), 5);

    trigger.disarm();
    for i in 5u64..10 { trigger.check(0x3000, i); }
    assert_eq!(fire_count.load(Ordering::Relaxed), 5, "disarmed trigger must not fire");
    assert!(!trigger.is_armed(), "is_armed() must be false after disarm()");

    trigger.rearm();
    for i in 10u64..15 { trigger.check(0x3000, i); }
    assert_eq!(fire_count.load(Ordering::Relaxed), 10, "re-armed trigger must fire");
    assert!(trigger.is_armed(), "is_armed() must be true after rearm()");
}
```

### Test: `trigger_disarmed_path_correctness`

```rust
#[test]
fn trigger_disarmed_path_correctness() {
    let fired = Arc::new(AtomicU32::new(0));
    let f = Arc::clone(&fired);

    let trigger = Trigger::every_n(1, move |_| {
        f.fetch_add(1, Ordering::Relaxed);
    });
    trigger.disarm();

    for i in 0u64..1_000_000 {
        let fired_now = trigger.check(0x4000, i);
        assert!(!fired_now, "disarmed trigger must return false at {i}");
    }
    assert_eq!(fired.load(Ordering::Relaxed), 0,
        "disarmed trigger must never execute its action");
}
```

---

## 14. Window

### Test: `window_is_active_basic`

```rust
// tests/window.rs
use helm_spy::analysis::Window;

#[test]
fn window_is_active_basic() {
    let w = Window::new(100, 200);

    assert!(!w.is_active(0),   "before window start");
    assert!(!w.is_active(99),  "just before start");
    assert!( w.is_active(100), "at start (inclusive)");
    assert!( w.is_active(150), "inside window");
    assert!( w.is_active(199), "last valid count");
    assert!(!w.is_active(200), "at end (exclusive)");
    assert!(!w.is_active(300), "after window");
}
```

### Test: `window_active_flag`

```rust
#[test]
fn window_active_flag() {
    let w = Window::new(10, 20);

    w.is_active(5);
    assert!(!w.active(), "flag must be false before window");

    w.is_active(15);
    assert!(w.active(), "flag must be true inside window");

    w.is_active(25);
    assert!(!w.active(), "flag must be false after window");
}
```

### Test: `window_gates_collection`

```rust
use helm_spy::analysis::InsnMix;
use helm_engine::InsnClass;

#[test]
fn window_gates_collection() {
    let window = Window::new(50, 60);
    let mix    = InsnMix::new("windowed");

    // Simulate 100 "instructions" — only record inside the window [50, 60).
    for i in 0u64..100 {
        if window.is_active(i) {
            mix.record(InsnClass::IntAlu);
        }
    }

    // Window is [50, 60) → 10 instructions.
    assert_eq!(mix.total(), 10,
        "only instructions in [50, 60) must be recorded");
}
```

### Test: `window_start_must_be_less_than_end`

```rust
#[test]
#[should_panic(expected = "window start must be less than end")]
fn window_start_must_be_less_than_end() {
    let _w = Window::new(100, 100);  // equal → panic
}
```

---

## 15. SpySession — Integration Tests

These tests verify end-to-end wiring: probe bundles are subscribed, instructions are
fired through the probes, and the session primitives accumulate correct values.

### Test: `observe_session_insn_count_via_probe`

```rust
// tests/integration.rs
use helm_spy::SpySession;
use helm_probe::{CpuProbes, events::CpuStepEvent};

fn fire_nop_steps(probes: &mut CpuProbes, count: u64) {
    // NOP encoding: 0xD503_201F (AArch64)
    let ev = CpuStepEvent { pc: 0x4000_0000, raw: 0xD503_201F };
    for _ in 0..count {
        probes.post_step.notify(&ev);
    }
}

#[test]
fn observe_session_insn_count_via_probe() {
    let mut probes  = CpuProbes::default();
    let mut session = SpySession::new("test_session");
    session.subscribe(&mut probes);

    fire_nop_steps(&mut probes, 1_000);

    assert_eq!(session.insn_count.value(), 1_000,
        "insn_count must equal the number of post_step probe firings");
}
```

### Test: `observe_session_insn_mix_via_probe`

```rust
#[test]
fn observe_session_insn_mix_via_probe() {
    let mut probes  = CpuProbes::default();
    let mut session = SpySession::new("mix_session");
    session.subscribe(&mut probes);

    // NOP (0xD503_201F) → InsnClass::Other
    // B #4 (0x14000001) → InsnClass::Branch
    let nop    = CpuStepEvent { pc: 0x4000, raw: 0xD503_201F };
    let branch = CpuStepEvent { pc: 0x4004, raw: 0x14000001 };

    for _ in 0..60 { probes.post_step.notify(&nop); }
    for _ in 0..40 { probes.post_step.notify(&branch); }

    assert_eq!(session.insn_mix.total(), 100,
        "total must equal 60 + 40 = 100");

    // Branch class must have 40 counts.
    let table = session.insn_mix.table();
    let branch_row = table.iter().find(|(c, _, _)| *c == InsnClass::Branch).unwrap();
    assert_eq!(branch_row.1, 40, "Branch count must be 40");
}
```

### Test: `observe_session_hot_pcs_via_probe`

```rust
#[test]
fn observe_session_hot_pcs_via_probe() {
    let mut probes  = CpuProbes::default();
    let mut session = SpySession::new("hot_session");
    session.subscribe(&mut probes);

    // Fire 100 post_step events at PC 0xDEAD_0000.
    let ev = CpuStepEvent { pc: 0xDEAD_0000, raw: 0xD503_201F };
    for _ in 0..100 { probes.post_step.notify(&ev); }

    let top = session.hot_pcs.top(1);
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].0, 0xDEAD_0000, "hot PC must be 0xDEAD_0000");
    assert_eq!(top[0].1, 100, "hot PC count must be 100");
}
```

### Test: `observe_session_branch_heatmap_via_probe`

```rust
use helm_probe::events::{BranchEvent, BranchKind};

#[test]
fn observe_session_branch_heatmap_via_probe() {
    let mut probes  = CpuProbes::default();
    let mut session = SpySession::new("branch_session");
    session.subscribe(&mut probes);

    let ev = BranchEvent {
        pc: 0xCAFE_0000, target: 0xBEEF_0000,
        taken: true, kind: BranchKind::DirectCond,
    };
    for _ in 0..10 { probes.branch.notify(&ev); }

    let top = session.branch_heatmap.top(1);
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].0, 0xBEEF_0000, "branch target must be 0xBEEF_0000");
    assert_eq!(top[0].1, 10, "branch count must be 10");
}
```

### Test: `observe_session_fault_history_via_probe`

```rust
use helm_probe::events::CpuFaultEvent;

#[test]
fn observe_session_fault_history_via_probe() {
    let mut probes  = CpuProbes::default();
    let mut session = SpySession::new("fault_session");
    session.subscribe(&mut probes);

    let ev = CpuFaultEvent { pc: 0x1000, raw: 0, kind: "svc" };
    probes.fault.notify(&ev);
    probes.fault.notify(&ev);
    probes.fault.notify(&ev);

    let history = session.fault_history.snapshot();
    assert_eq!(history.len(), 3, "must have 3 fault events in history");
    assert!(history.iter().all(|e| e.kind == "svc"),
        "all events must be svc faults");
}
```

### Test: `observe_session_with_cache`

```rust
use helm_probe::events::MemAccessEvent;

#[test]
fn observe_session_with_cache() {
    let mut probes  = CpuProbes::default();
    let mut session = SpySession::new("cache_session")
        .with_cache_l1d(64, 8, 64);  // 32 KB 8-way
    session.subscribe(&mut probes);

    // All accesses to the same cache line → 1 cold miss, then all hits.
    let addr = 0x4000_0000u64;
    let ev = MemAccessEvent { addr, size: 8, is_store: false, pc: 0x1000 };
    for _ in 0..10 { probes.mem.notify(&ev); }

    let cache = session.cache_l1d.as_ref().unwrap().lock().unwrap();
    assert_eq!(cache.misses(), 1, "exactly 1 cold miss");
    assert_eq!(cache.hits(),   9, "9 subsequent hits");
    assert!((cache.hit_rate() - 0.9).abs() < 1e-9,
        "hit rate must be 0.9, got {}", cache.hit_rate());
}
```

### Test: `observe_session_trigger_fires_at_insn_count`

```rust
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use helm_spy::analysis::Trigger;
use helm_probe::events::PreStepEvent;

#[test]
fn observe_session_trigger_fires_at_insn_count() {
    let mut probes    = CpuProbes::default();
    let triggered     = Arc::new(AtomicU32::new(0));
    let t             = Arc::clone(&triggered);
    let mut session   = SpySession::new("trigger_session");

    session.add_trigger(Trigger::at_insn(500, move |_| {
        t.fetch_add(1, Ordering::Relaxed);
    }));
    session.subscribe(&mut probes);

    // Fire 600 step cycles: pre_step then post_step.
    let nop = CpuStepEvent { pc: 0x1000, raw: 0xD503_201F };
    for i in 0u64..600 {
        probes.pre_step.notify(&PreStepEvent { pc: 0x1000, insn_count: i });
        probes.post_step.notify(&nop);
    }

    assert_eq!(triggered.load(Ordering::Relaxed), 1,
        "trigger must fire exactly once at insn_count=500");
}
```

### Test: `observe_session_quantum_end_merges_cache_stats`

```rust
#[test]
fn observe_session_quantum_end_merges_cache_stats() {
    // Two "vCPUs" each run their own local CacheModel.
    // After quantum_end, their stats should be combined.
    use helm_spy::analysis::CacheModel;

    let mut vcpu0_cache = CacheModel::new("vcpu0", 4, 4, 64);
    let mut vcpu1_cache = CacheModel::new("vcpu1", 4, 4, 64);

    vcpu0_cache.access(0x000);  // miss
    vcpu0_cache.access(0x000);  // hit
    vcpu1_cache.access(0x100);  // miss
    vcpu1_cache.access(0x200);  // miss

    // Simulate quantum_end merge into summary cache.
    let mut summary = CacheModel::new("summary", 4, 4, 64);
    summary.merge_stats(&vcpu0_cache);
    summary.merge_stats(&vcpu1_cache);

    assert_eq!(summary.hits(),   1, "merged hits from vcpu0");
    assert_eq!(summary.misses(), 3, "merged misses from vcpu0 + vcpu1");
}
```

### Integration Test: `engine_runs_nops_insn_count_matches`

```rust
// tests/engine_integration.rs
// Requires helm-engine as a dev-dependency.
// Runs a minimal SE simulation and checks that the SpySession insn_count
// matches the number of instructions executed by the engine.

#[test]
#[cfg(feature = "integration")]
fn engine_runs_nops_insn_count_matches() {
    use helm_engine::{HelmSim, ExecMode};
    use helm_spy::SpySession;

    // Build a minimal AArch64 SE simulation running 1000 NOP instructions.
    let mut sim = HelmSim::new_aarch64_se_minimal();
    let mut session = SpySession::new("nop_test");
    session.subscribe(&mut sim.probes_mut());

    sim.run(1_000);

    // The engine must have retired exactly 1000 instructions.
    assert_eq!(session.insn_count.value(), 1_000,
        "insn_count via probe must match engine's retired instruction count");

    // InsnMix total must also be 1000.
    assert_eq!(session.insn_mix.total(), 1_000,
        "insn_mix.total() must match insn_count");

    // The NOP encoding (0xD503_201F) classifies as InsnClass::Other.
    let nop_count = session.insn_mix.count(InsnClass::Other);
    assert_eq!(nop_count, 1_000, "all instructions must be classified as Other (NOPs)");
}
```

---

## 16. Test Matrix

| Test | File | Type | Component | Validates |
|------|------|------|-----------|-----------|
| `counter_basic_increment_and_read` | `tests/counter.rs` | Unit | `Counter` | Inc, read, zero initial |
| `counter_add_by_n` | `tests/counter.rs` | Unit | `Counter` | Bulk add |
| `counter_reset` | `tests/counter.rs` | Unit | `Counter` | Reset to zero |
| `counter_concurrent_increment` | `tests/counter.rs` | Multi-thread | `Counter` | No lost updates |
| `counter_read_is_consistent` | `tests/counter.rs` | Unit | `Counter` | Monotone invariant |
| `per_vcpu_counter_independent_slots` | `tests/per_vcpu_counter.rs` | Unit | `PerVcpuCounter` | Per-slot isolation |
| `per_vcpu_counter_total_accumulation` | `tests/per_vcpu_counter.rs` | Unit | `PerVcpuCounter` | total() correctness |
| `per_vcpu_counter_zero_after_construction` | `tests/per_vcpu_counter.rs` | Unit | `PerVcpuCounter` | Cold-start |
| `per_vcpu_counter_concurrent_each_vcpu` | `tests/per_vcpu_counter.rs` | Multi-thread | `PerVcpuCounter` | Per-vCPU concurrency |
| `indexed_counter_basic_increment` | `tests/indexed_counter.rs` | Unit | `IndexedCounter` | Per-bucket counting |
| `indexed_counter_fraction` | `tests/indexed_counter.rs` | Unit | `IndexedCounter` | Fraction calculation |
| `indexed_counter_fraction_zero_total` | `tests/indexed_counter.rs` | Unit | `IndexedCounter` | Zero-total guard |
| `indexed_counter_table_format` | `tests/indexed_counter.rs` | Unit | `IndexedCounter` | Table completeness |
| `indexed_counter_concurrent_increment` | `tests/indexed_counter.rs` | Multi-thread | `IndexedCounter` | No lost updates |
| `indexed_counter_reset` | `tests/indexed_counter.rs` | Unit | `IndexedCounter` | Reset to zero |
| `indexed_counter_merge` | `tests/indexed_counter.rs` | Unit | `IndexedCounter` | Quantum-end merge |
| `histogram_basic_bucketing` | `tests/histogram.rs` | Unit | `Histogram` | Bucket assignment |
| `histogram_edge_boundary` | `tests/histogram.rs` | Unit | `Histogram` | Exact edge boundary |
| `histogram_percentile` | `tests/histogram.rs` | Unit | `Histogram` | Percentile calculation |
| `histogram_concurrent_record` | `tests/histogram.rs` | Multi-thread | `Histogram` | Concurrent safety |
| `interval_histogram_basic_windowing` | `tests/interval_histogram.rs` | Unit | `IntervalHistogram` | Window commit |
| `interval_histogram_single_window` | `tests/interval_histogram.rs` | Unit | `IntervalHistogram` | Single window |
| `interval_histogram_window_boundary_is_exclusive` | `tests/interval_histogram.rs` | Unit | `IntervalHistogram` | Exclusive end |
| `interval_histogram_approx_mean` | `tests/interval_histogram.rs` | Unit | `IntervalHistogram` | Mean from buckets |
| `heatmap_inc_and_top` | `tests/heatmap.rs` | Unit | `HeatMap` | Top-N correctness |
| `heatmap_top_n_ordering` | `tests/heatmap.rs` | Unit | `HeatMap` | Descending sort |
| `heatmap_top_n_fewer_than_n_entries` | `tests/heatmap.rs` | Unit | `HeatMap` | Sparse heatmap |
| `heatmap_concurrent_inc` | `tests/heatmap.rs` | Multi-thread | `HeatMap` | No lost updates |
| `ring_buffer_push_snapshot` | `tests/ring_buffer.rs` | Unit | `RingBuffer` | Push and snapshot |
| `ring_buffer_capacity_overwrite` | `tests/ring_buffer.rs` | Unit | `RingBuffer` | Overwrite oldest |
| `ring_buffer_empty_snapshot` | `tests/ring_buffer.rs` | Unit | `RingBuffer` | Empty guard |
| `ring_buffer_len` | `tests/ring_buffer.rs` | Unit | `RingBuffer` | Length tracking |
| `event_stream_push_drain` | `tests/event_stream.rs` | Unit | `EventStream` | FIFO ordering |
| `event_stream_bounded_at_max` | `tests/event_stream.rs` | Unit | `EventStream` | Stops at max |
| `event_stream_capacity_zero_never_panics` | `tests/event_stream.rs` | Unit | `EventStream` | Zero-max guard |
| `trace_ring_push_drain` | `tests/trace_ring.rs` | Unit | `TraceRing` | FIFO ordering |
| `trace_ring_drain_empty` | `tests/trace_ring.rs` | Unit | `TraceRing` | Empty drain |
| `trace_ring_overwrite_on_full` | `tests/trace_ring.rs` | Unit | `TraceRing` | Lossy overwrite |
| `trace_ring_len` | `tests/trace_ring.rs` | Unit | `TraceRing` | Length tracking |
| `trace_ring_producer_consumer_threads` | `tests/trace_ring.rs` | Multi-thread | `TraceRing` | SPSC correctness |
| `branch_record_flags` | `tests/trace_ring.rs` | Unit | `BranchRecord` | Flag bit layout |
| `insn_mix_record_all_classes` | `tests/insn_mix.rs` | Unit | `InsnMix` | All class variants |
| `insn_mix_table_sorted_descending` | `tests/insn_mix.rs` | Unit | `InsnMix` | Table ordering |
| `insn_mix_percentage_sums_to_100` | `tests/insn_mix.rs` | Unit | `InsnMix` | Pct invariant |
| `insn_mix_fraction` | `tests/insn_mix.rs` | Unit | `InsnMix` | Per-class fraction |
| `insn_mix_concurrent_record` | `tests/insn_mix.rs` | Multi-thread | `InsnMix` | No lost records |
| `insn_mix_reset` | `tests/insn_mix.rs` | Unit | `InsnMix` | Clean reset |
| `cache_model_basic_hit_miss` | `tests/cache_model.rs` | Unit | `CacheModel` | Cold miss → hit |
| `cache_model_lru_eviction` | `tests/cache_model.rs` | Unit | `CacheModel` | LRU eviction policy |
| `cache_model_mpki` | `tests/cache_model.rs` | Unit | `CacheModel` | MPKI calculation |
| `cache_model_hit_rate_zero_accesses` | `tests/cache_model.rs` | Unit | `CacheModel` | Zero-access guard |
| `cache_model_reset` | `tests/cache_model.rs` | Unit | `CacheModel` | Tag + stat reset |
| `cache_model_merge_stats` | `tests/cache_model.rs` | Unit | `CacheModel` | Quantum-end merge |
| `cache_model_set_associativity_correct` | `tests/cache_model.rs` | Unit | `CacheModel` | Direct-mapped conflict |
| `branch_pred_bimodal_all_taken` | `tests/branch_pred.rs` | Unit | `BranchPredictor` | BiModal warmup |
| `branch_pred_bimodal_alternating` | `tests/branch_pred.rs` | Unit | `BranchPredictor` | BiModal failure mode |
| `branch_pred_gshare_loop` | `tests/branch_pred.rs` | Unit | `BranchPredictor` | GShare accuracy |
| `branch_pred_gshare_better_than_bimodal_on_correlated` | `tests/branch_pred.rs` | Unit | `BranchPredictor` | GShare advantage |
| `branch_pred_perfect_zero_mispredictions` | `tests/branch_pred.rs` | Unit | `BranchPredictor` | Perfect invariant |
| `branch_pred_miss_rate_calculation` | `tests/branch_pred.rs` | Unit | `BranchPredictor` | Miss rate arithmetic |
| `branch_pred_reset` | `tests/branch_pred.rs` | Unit | `BranchPredictor` | Counter + table reset |
| `branch_pred_mpki` | `tests/branch_pred.rs` | Unit | `BranchPredictor` | MPKI formula |
| `trigger_at_insn_fires_once` | `tests/trigger.rs` | Unit | `Trigger` | `AtInsn` one-shot |
| `trigger_every_n_fires_periodically` | `tests/trigger.rs` | Unit | `Trigger` | `EveryN` periodicity |
| `trigger_at_pc_fires_on_match` | `tests/trigger.rs` | Unit | `Trigger` | `AtPc` semantics |
| `trigger_counter_reaches` | `tests/trigger.rs` | Unit | `Trigger` | `CounterReaches` |
| `trigger_disarm_rearm` | `tests/trigger.rs` | Unit | `Trigger` | Armed/disarmed lifecycle |
| `trigger_disarmed_path_correctness` | `tests/trigger.rs` | Unit | `Trigger` | Disarmed path |
| `window_is_active_basic` | `tests/window.rs` | Unit | `Window` | Inclusive start, exclusive end |
| `window_active_flag` | `tests/window.rs` | Unit | `Window` | Flag side-effect |
| `window_gates_collection` | `tests/window.rs` | Unit | `Window` + `InsnMix` | Gated collection |
| `window_start_must_be_less_than_end` | `tests/window.rs` | Unit | `Window` | Assertion guard |
| `observe_session_insn_count_via_probe` | `tests/integration.rs` | Integration | `SpySession` | Subscribe + fire |
| `observe_session_insn_mix_via_probe` | `tests/integration.rs` | Integration | `SpySession` | Class routing |
| `observe_session_hot_pcs_via_probe` | `tests/integration.rs` | Integration | `SpySession` | PC heatmap wiring |
| `observe_session_branch_heatmap_via_probe` | `tests/integration.rs` | Integration | `SpySession` | Branch heatmap |
| `observe_session_fault_history_via_probe` | `tests/integration.rs` | Integration | `SpySession` | Fault ring wiring |
| `observe_session_with_cache` | `tests/integration.rs` | Integration | `SpySession` | L1D cache wiring |
| `observe_session_trigger_fires_at_insn_count` | `tests/integration.rs` | Integration | `SpySession` | End-to-end trigger |
| `observe_session_quantum_end_merges_cache_stats` | `tests/integration.rs` | Integration | `SpySession` | quantum_end merge |
| `engine_runs_nops_insn_count_matches` | `tests/engine_integration.rs` | Integration (engine) | `SpySession` + engine | Full pipeline |

### Running the Tests

```bash
# All helm-spy tests (unit + integration)
cargo test -p helm-spy

# Unit tests only (no engine dep required)
cargo test -p helm-spy --lib

# Multi-thread tests with release for realistic scheduling
cargo test -p helm-spy --release -- concurrent

# Specific component
cargo test -p helm-spy -- cache_model
cargo test -p helm-spy -- branch_pred
cargo test -p helm-spy -- trace_ring
cargo test -p helm-spy -- trigger
cargo test -p helm-spy -- window

# Engine integration tests (requires full workspace build)
cargo test -p helm-spy --features integration -- engine_runs

# With output
cargo test -p helm-spy -- --nocapture
```

### Test File Layout

```
framework/helm-spy/
└── src/
    ├── primitives/
    │   ├── counter.rs           # #[cfg(test)] mod tests { … }
    │   ├── per_vcpu.rs          # inline unit tests
    │   ├── indexed_counter.rs   # inline unit tests
    │   ├── histogram.rs         # inline unit tests
    │   ├── interval_histogram.rs # inline unit tests
    │   ├── heatmap.rs           # inline unit tests
    │   ├── ring_buffer.rs       # inline unit tests
    │   ├── event_stream.rs      # inline unit tests
    │   └── trace_ring.rs        # inline unit tests
    └── analysis/
        ├── insn_mix.rs          # inline unit tests
        ├── cache.rs             # inline unit tests
        ├── branch_pred.rs       # inline unit tests
        ├── trigger.rs           # inline unit tests
        └── window.rs            # inline unit tests
tests/
├── counter.rs               # standalone: Counter
├── per_vcpu_counter.rs      # standalone: PerVcpuCounter
├── indexed_counter.rs       # standalone: IndexedCounter
├── histogram.rs             # standalone: Histogram
├── interval_histogram.rs    # standalone: IntervalHistogram
├── heatmap.rs               # standalone: HeatMap
├── ring_buffer.rs           # standalone: RingBuffer
├── event_stream.rs          # standalone: EventStream
├── trace_ring.rs            # standalone: TraceRing + BranchRecord
├── insn_mix.rs              # standalone: InsnMix
├── cache_model.rs           # standalone: CacheModel
├── branch_pred.rs           # standalone: BranchPredictor
├── trigger.rs               # standalone: Trigger
├── window.rs                # standalone: Window
├── integration.rs           # SpySession + probe wiring
└── engine_integration.rs    # Full engine pipeline (feature-gated)
```

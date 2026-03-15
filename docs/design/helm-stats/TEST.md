# helm-stats — Test Plan

> **Crate:** `helm-stats`
> **Test targets:** `PerfCounter` (concurrent), `PerfHistogram`, `PerfFormula`, `StatsRegistry`, dump output

---

## 1. PerfCounter — Concurrent Increment (Multi-Thread)

**Goal:** Verify that concurrent increments from multiple threads produce the correct final count with no lost updates.

### Test: `test_counter_concurrent_increment`

```rust
// tests/counter_concurrent.rs
use helm_stats::PerfCounter;
use std::sync::Arc;
use std::thread;

#[test]
fn test_counter_concurrent_increment() {
    const NUM_THREADS: usize = 8;
    const INCS_PER_THREAD: u64 = 1_000_000;

    let counter = Arc::new(PerfCounter::new("test.counter", "concurrent test counter"));

    let handles: Vec<_> = (0..NUM_THREADS)
        .map(|_| {
            let c = Arc::clone(&counter);
            thread::spawn(move || {
                for _ in 0..INCS_PER_THREAD {
                    c.inc();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }

    let expected = NUM_THREADS as u64 * INCS_PER_THREAD;
    assert_eq!(
        counter.get(),
        expected,
        "concurrent inc() must not lose updates: expected {expected}, got {}",
        counter.get()
    );
}
```

### Test: `test_counter_inc_by_concurrent`

```rust
#[test]
fn test_counter_inc_by_concurrent() {
    const NUM_THREADS: usize = 4;
    const BATCH: u64 = 1000;
    const ITERS: u64 = 100_000;

    let counter = Arc::new(PerfCounter::new("test.batch", "batch increment test"));

    let handles: Vec<_> = (0..NUM_THREADS)
        .map(|_| {
            let c = Arc::clone(&counter);
            thread::spawn(move || {
                for _ in 0..ITERS {
                    c.inc_by(BATCH);
                }
            })
        })
        .collect();

    for h in handles { h.join().unwrap(); }

    let expected = NUM_THREADS as u64 * ITERS * BATCH;
    assert_eq!(counter.get(), expected);
}
```

### Test: `test_counter_reset`

```rust
#[test]
fn test_counter_reset() {
    let counter = PerfCounter::new("test.reset", "reset test");
    for _ in 0..1000 { counter.inc(); }
    assert_eq!(counter.get(), 1000);
    counter.reset();
    assert_eq!(counter.get(), 0);
}
```

---

## 2. PerfHistogram — Unit Tests

### Test: `test_histogram_basic_bucketing`

```rust
use helm_stats::PerfHistogram;

#[test]
fn test_histogram_basic_bucketing() {
    // Buckets: [0,10), [10,100), [100,1000), [1000,∞)
    let hist = PerfHistogram::new("test.hist", "test histogram", vec![10, 100, 1000]);

    hist.record(5);    // → bucket 0: [0, 10)
    hist.record(50);   // → bucket 1: [10, 100)
    hist.record(500);  // → bucket 2: [100, 1000)
    hist.record(5000); // → bucket 3: [1000, ∞)

    let counts = hist.counts();
    assert_eq!(counts.len(), 4);
    assert_eq!(counts[0], 1, "underflow bucket");
    assert_eq!(counts[1], 1, "bucket [10, 100)");
    assert_eq!(counts[2], 1, "bucket [100, 1000)");
    assert_eq!(counts[3], 1, "overflow bucket");
}
```

### Test: `test_histogram_edge_boundary`

```rust
#[test]
fn test_histogram_edge_boundary() {
    let hist = PerfHistogram::new("test.boundary", "boundary test", vec![10, 100]);

    // Exactly at edge — record(10) should go into [10, 100), not [0, 10)
    hist.record(10);
    hist.record(100);  // → [100, ∞)

    let counts = hist.counts();
    assert_eq!(counts[0], 0, "nothing in [0, 10)");
    assert_eq!(counts[1], 1, "10 in [10, 100)");
    assert_eq!(counts[2], 1, "100 in [100, ∞)");
}
```

### Test: `test_histogram_concurrent_record`

```rust
#[test]
fn test_histogram_concurrent_record() {
    use std::sync::Arc;
    let hist = Arc::new(PerfHistogram::new("test.concurrent", "concurrent hist", vec![50, 100]));
    const N: usize = 10_000;

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let h = Arc::clone(&hist);
            std::thread::spawn(move || {
                for _ in 0..N { h.record(75); }  // all into [50, 100)
            })
        })
        .collect();
    for handle in handles { handle.join().unwrap(); }

    let counts = hist.counts();
    assert_eq!(counts[1], 4 * N as u64, "all records must land in bucket [50,100)");
}
```

---

## 3. PerfFormula — Evaluation Tests

### Test: `test_formula_simple_counter_ref`

```rust
use helm_stats::{PerfCounter, PerfFormula, StatsRegistry};
use std::sync::Arc;

fn make_registry() -> StatsRegistry {
    let mut reg = StatsRegistry::new();
    let hits   = reg.perf_counter("cpu.hits",   "hits");
    let misses = reg.perf_counter("cpu.misses", "misses");
    hits.inc_by(80);
    misses.inc_by(20);
    reg
}

#[test]
fn test_formula_simple_counter_ref() {
    let reg = make_registry();
    let f = PerfFormula::counter("cpu.hits");
    assert_eq!(f.eval(&reg), 80.0);
}
```

### Test: `test_formula_hit_rate`

```rust
#[test]
fn test_formula_hit_rate() {
    let reg = make_registry();  // hits=80, misses=20
    let hit_rate = PerfFormula::div(
        PerfFormula::counter("cpu.hits"),
        PerfFormula::add(
            PerfFormula::counter("cpu.hits"),
            PerfFormula::counter("cpu.misses"),
        ),
    );
    let val = hit_rate.eval(&reg);
    assert!((val - 0.8).abs() < 1e-9, "expected 0.8, got {val}");
}
```

### Test: `test_formula_division_by_zero`

```rust
#[test]
fn test_formula_division_by_zero() {
    let mut reg = StatsRegistry::new();
    reg.perf_counter("cpu.zero", "zero counter");  // value = 0
    let f = PerfFormula::div(
        PerfFormula::constant(1.0),
        PerfFormula::counter("cpu.zero"),
    );
    assert!(f.eval(&reg).is_nan(), "division by zero must yield NaN");
}
```

### Test: `test_formula_missing_counter`

```rust
#[test]
fn test_formula_missing_counter() {
    let reg = StatsRegistry::new();
    let f = PerfFormula::counter("nonexistent.path");
    assert!(f.eval(&reg).is_nan(), "missing counter must yield NaN");
}
```

### Test: `test_formula_composed`

```rust
#[test]
fn test_formula_composed() {
    let mut reg = StatsRegistry::new();
    let cycles = reg.perf_counter("cpu.cycles", "cycles");
    let insns  = reg.perf_counter("cpu.insns",  "insns");
    cycles.inc_by(1_230_000_000);
    insns.inc_by(1_000_000_000);

    // CPI = cycles / insns
    let cpi = PerfFormula::div(
        PerfFormula::counter("cpu.cycles"),
        PerfFormula::counter("cpu.insns"),
    );
    let val = cpi.eval(&reg);
    assert!((val - 1.23).abs() < 1e-6, "CPI expected 1.23, got {val}");
}
```

---

## 4. StatsRegistry — Integration Tests

### Test: `test_registry_path_conflict_panics`

```rust
#[test]
#[should_panic(expected = "already registered as a different stat type")]
fn test_registry_path_conflict_panics() {
    let mut reg = StatsRegistry::new();
    reg.perf_counter("cpu.hits", "counter");
    reg.perf_histogram("cpu.hits", "histogram", vec![10, 100]);  // must panic
}
```

### Test: `test_registry_same_path_same_type_returns_same_arc`

```rust
#[test]
fn test_registry_same_path_same_type_returns_same_arc() {
    let mut reg = StatsRegistry::new();
    let a = reg.perf_counter("cpu.hits", "first registration");
    let b = reg.perf_counter("cpu.hits", "second registration");
    // Must be the same Arc (same pointer).
    assert!(Arc::ptr_eq(&a, &b), "same path must return the same Arc<PerfCounter>");
}
```

### Test: `test_registry_dump_json_roundtrip`

```rust
#[test]
fn test_registry_dump_json_roundtrip() {
    let mut reg = StatsRegistry::new();
    let hits   = reg.perf_counter("cpu.hits",   "hits");
    let misses = reg.perf_counter("cpu.misses", "misses");
    hits.inc_by(100);
    misses.inc_by(5);
    reg.perf_formula(
        "cpu.hit_rate",
        "hit rate",
        PerfFormula::div(
            PerfFormula::counter("cpu.hits"),
            PerfFormula::add(
                PerfFormula::counter("cpu.hits"),
                PerfFormula::counter("cpu.misses"),
            ),
        ),
    );

    let tmp = tempfile::NamedTempFile::new().unwrap();
    reg.dump_json(tmp.path()).unwrap();

    let content = std::fs::read_to_string(tmp.path()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

    assert_eq!(parsed["cpu.hits"]["value"], 100);
    assert_eq!(parsed["cpu.misses"]["value"], 5);

    let hit_rate = parsed["cpu.hit_rate"]["value"].as_f64().unwrap();
    assert!((hit_rate - 100.0 / 105.0).abs() < 1e-6, "hit_rate mismatch: {hit_rate}");
}
```

### Test: `test_registry_invalid_path_panics`

```rust
#[test]
#[should_panic]
fn test_registry_invalid_path_panics() {
    let mut reg = StatsRegistry::new();
    reg.perf_counter("", "empty path should panic");
}

#[test]
#[should_panic]
fn test_registry_uppercase_path_panics() {
    let mut reg = StatsRegistry::new();
    reg.perf_counter("CPU.hits", "uppercase segment should panic");
}
```

---

## Test Matrix

| Test | Type | Component |
|------|------|-----------|
| Concurrent increment (8 threads × 1M) | Multi-thread correctness | `PerfCounter` |
| Concurrent `inc_by` (4 threads × 100K × 1K) | Multi-thread correctness | `PerfCounter` |
| Reset to zero | Unit | `PerfCounter` |
| Basic bucket assignment | Unit | `PerfHistogram` |
| Exact edge boundary | Unit | `PerfHistogram` |
| Concurrent `record()` (4 threads × 10K) | Multi-thread correctness | `PerfHistogram` |
| Counter reference evaluation | Unit | `PerfFormula` |
| Hit rate formula (div/add) | Unit | `PerfFormula` |
| Division by zero → NaN | Unit | `PerfFormula` |
| Missing counter → NaN | Unit | `PerfFormula` |
| CPI formula (composed) | Unit | `PerfFormula` |
| Path conflict panics | Unit | `StatsRegistry` |
| Same path returns same Arc | Unit | `StatsRegistry` |
| JSON dump round-trip | Integration | `StatsRegistry` |
| Empty path panics | Unit | `StatsRegistry` |
| Uppercase segment panics | Unit | `StatsRegistry` |

### Running the Tests

```bash
# All helm-stats tests
cargo test -p helm-stats

# Concurrent tests only (run with --release for realistic concurrency)
cargo test -p helm-stats --release test_counter_concurrent

# With test output
cargo test -p helm-stats -- --nocapture
```

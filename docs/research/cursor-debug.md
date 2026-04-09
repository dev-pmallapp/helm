# cursor-debug -- Codebase Audit

Date: 2026-04-07

## Summary

The `debug/` domain contains two crates: `helm-spy` (collection layer) and
`helm-report` (delivery layer). The separation of concerns is correct and
dependency direction is clean (`helm-report` → `helm-spy`, no cycle).
Test coverage is solid for both crates. The most serious issues are a CSV
column-order vs header mismatch in `helm-report`, release-build probe wiring
that silently no-ops, and several silent error-swallowing patterns in the
async sink and report scheduling layers.

---

## Design Issues

### D1. `cfg(debug_assertions)`-only probe wiring -- release silently no-ops

**Severity: High**

`HelmSpy::subscribe`, `subscribe_in_window`, and `add_trigger_live` are
compiled only in debug builds. Release builds retain the data structures but
have no way to wire them to probe events. Tests pass in debug; release users
silently get zero data.

```rust
// debug/helm-spy/src/session.rs:105-106
#[cfg(debug_assertions)]
pub fn subscribe(&self, probes: &mut helm_probe::CpuProbes) {
```

Same gate on `ProbePluginBridge::wire` (`bridge.rs:31`).

**Suggested fix:** Use a Cargo feature (`spy` or `instrumentation`) instead of
`cfg(debug_assertions)`. Enable by default in development profiles, let users
opt in for release profiling. Alternatively, provide an always-compiled
`subscribe` that is a no-op behind a runtime flag.

---

### D2. `ProbePluginBridge::wire` vs `HelmSpy::subscribe` duplication

**Severity: Medium**

Both `ProbePluginBridge::wire` (`bridge.rs:32-41`) and `HelmSpy::subscribe`
(`session.rs:106-120`) wire the same primitives to probes, but with different
coverage:

- `HelmSpy::subscribe` also wires triggers (`session.rs:117-119`).
- `ProbePluginBridge::wire` does not wire triggers.

Two entry points with different behavior is confusing. If a caller uses the
bridge path, triggers silently stop working.

**Suggested fix:** Remove one entry point. Preferably keep `HelmSpy::subscribe`
as the canonical wiring API and have `ProbePluginBridge` delegate to it, or
deprecate one in favor of the other.

---

### D3. `ReportFormatter` trait is a fat interface

**Severity: Low**

`ReportFormatter` (`helm-report/src/format/mod.rs:14-26`) requires
`format_session`, `format_counter`, and `format_histogram` even though not all
formatters meaningfully implement incremental methods. `format_counter` and
`format_histogram` are used for incremental delivery but callers often only use
`format_session`.

**Suggested fix:** Consider splitting into `SessionFormatter` +
`IncrementalFormatter` sub-traits, or provide default impls that delegate to
`format_session` where applicable.

---

### D4. `QuantumObserver` trait is documented but unintegrated

**Severity: Low**

`lib.rs:31` documents the `quantum` module with the `QuantumObserver` trait.
No engine or sim component implements or calls this trait. It is dead
abstraction surface.

**Suggested fix:** Either wire it into the engine's quantum-boundary flush path
or remove the module until needed.

---

## Correctness Issues

### C1. CSV column order contradicts the header (helm-report)

**Severity: Critical**

Header declares `timestamp_ns,metric,value` but data rows emit
`metric,timestamp_ns,value`:

```rust
// debug/helm-report/src/format/csv.rs:18
out.push_str("timestamp_ns,metric,value\n");

// csv.rs:20-33 -- row closure emits metric FIRST, then ts, then value
let mut row = |metric: &str, value: &str| {
    // ...
    out.push_str(metric);        // <-- column 1 is metric
    out.push(',');
    out.push_str(&ts.to_string()); // <-- column 2 is timestamp
    out.push(',');
    out.push_str(value);           // <-- column 3 is value
    out.push('\n');
};
```

Tests only assert three columns exist and that column 2 is numeric -- they do
not assert column semantics, so the mismatch passes CI.

**Suggested fix:** Change the header to `metric,timestamp_ns,value` to match
the emitted order, or reorder the row closure to emit `ts,metric,value`.
Add a test that asserts `rows[1][0]` matches the first header name.

---

### C2. `HelmSpy::snapshot()` drops fault history and ticks

**Severity: High**

The snapshot always sets `fault_history: None` and `tick_count: 0` despite
`HelmSpy` owning a populated `fault_history: Arc<RingBuffer<String>>`:

```rust
// debug/helm-spy/src/session.rs:94-95
fault_history: None,
tick_count: 0,
```

Any reporter consuming the snapshot never sees faults or tick counts. The
`fault_history` test (`session.rs:234-246`) validates the ring buffer in
isolation but no test checks whether `snapshot()` propagates it.

**Suggested fix:**
```rust
fault_history: Some(self.fault_history.snapshot()),
// tick_count should be fed from the engine's cycle counter
```

---

### C3. Branch predictor snapshot silently swallows poisoned mutex

**Severity: Medium**

If the `Mutex<BranchPredictor>` is poisoned, the branch predictor section is
silently omitted from the snapshot:

```rust
// debug/helm-spy/src/session.rs:85-93
branch_pred: self.branch_pred.as_ref().and_then(|pred| {
    pred.lock().ok().map(|guard| BranchPredSnapshot { ... })
}),
```

A poisoned mutex usually indicates a panic inside a lock guard -- silently
omitting that data hides the root cause.

**Suggested fix:** Log a warning or propagate the error via a `Result` return
from `snapshot()`.

---

### C4. `SimPointCollector::on_branch` hardcodes `+4` fall-through

**Severity: High**

The not-taken fall-through address is computed as `_pc + 4`, which assumes
fixed 32-bit instructions:

```rust
// debug/helm-spy/src/analysis/simpoint.rs:69
self.current_bb_start = if taken { target } else { _pc + 4 };
```

This is wrong for RISC-V compressed instructions (2 bytes) and for any future
variable-length ISA. BBV semantics will produce incorrect basic-block
boundaries for those targets.

**Suggested fix:** Accept instruction size as a parameter to `on_branch`, or
use the next-PC value from the probe event instead of computing it.

---

### C5. `PerVcpuCounter::inc` / `add` -- unbounded index

**Severity: Medium**

An out-of-range `vcpu` index causes an unchecked slice panic:

```rust
// debug/helm-spy/src/primitives/counter.rs:88-89
pub fn inc(&self, vcpu: usize) {
    self.slots[vcpu].fetch_add(1, Ordering::Relaxed);
}
```

Same for `add` and `value`.

**Suggested fix:** Add `debug_assert!(vcpu < self.slots.len())` or return
early with a warning. In release, `.get()` with a fallback is safer.

---

### C6. `IntervalHistogram::tick` has confusing dual semantics

**Severity: Medium**

When a window boundary is crossed, `value` seeds `window_accum` via
`swap`. Otherwise, the code does `fetch_add(1, ...)`, ignoring `value`
entirely:

```rust
// debug/helm-spy/src/primitives/histogram.rs:105-113
pub fn tick(&self, value: u64, insn_count: u64) {
    let window = insn_count / self.window_size;
    let prev = self.last_window.swap(window, Ordering::Relaxed);
    if window != prev && prev != u64::MAX {
        let sample = self.window_accum.swap(value, Ordering::Relaxed);
        self.hist.record(sample);
    } else {
        self.window_accum.fetch_add(1, Ordering::Relaxed);
    }
}
```

Callers passing a meaningful sample value in the non-boundary case get it
silently discarded. The `+1` increment suggests this counts events rather
than accumulating values -- but the name `value` implies otherwise.

**Suggested fix:** Rename `value` to `initial_accum` or document that `value`
is only used as the seed for the next window. Consider `fetch_add(value, ...)`
if actual accumulation is intended.

---

### C7. `ProbePluginBridge::step_to_insn_info` hardcodes `size: 4` and `vcpu_idx: 0`

**Severity: Medium**

```rust
// debug/helm-spy/src/bridge.rs:70-74
InsnInfo {
    vcpu_idx: 0,
    pc: event.pc,
    raw: event.raw,
    size: 4,
```

Wrong for RISC-V compressed (2-byte) instructions and misleading for
multi-vCPU simulations. Downstream consumers may trust these fields.

**Suggested fix:** Derive `size` from the probe event (add an `insn_size`
field to `CpuStepEvent`) and pass `vcpu_idx` as a parameter.

---

### C8. `AsyncFileSink` drain thread silently drops write/flush errors

**Severity: High**

```rust
// debug/helm-report/src/sink/async_file.rs:51-55
Ok(DrainMsg::Write(data)) => {
    let _ = writer.write_all(&data);
}
Ok(DrainMsg::Flush) => {
    let _ = writer.flush();
}
```

Disk-full, permission errors, or I/O failures are silently swallowed. The
caller's `Sink::write` returns `Ok` because the channel send succeeded, but
the data may never reach disk.

**Suggested fix:** Track an `Arc<AtomicBool>` error flag that the drain thread
sets on failure. `Sink::write` checks it and returns `Err` on subsequent calls.
Alternatively, use an error channel (`mpsc::Sender<io::Error>`).

---

### C9. `ReportSchedule::check` discards delivery errors

**Severity: Medium**

```rust
// debug/helm-report/src/schedule.rs:61
let _ = self.report.deliver();
```

A failed delivery (sink error) is silently ignored. The schedule will
re-attempt on the next trigger, but the failed data is lost.

**Suggested fix:** Log the error or accumulate a failure counter accessible
from the schedule's status API.

---

### C10. `BinaryTraceSink` record_count is `u32` -- overflow risk

**Severity: Low**

If more than `u32::MAX` records are written, the header count wraps. This is
unlikely in practice but violates the binary format contract.

**Suggested fix:** Use `u64` for the record count, or saturate at `u32::MAX`.

---

## Completeness Issues

### P1. `ReportTrigger::OnCounter` is never handled

The enum variant exists (`schedule.rs:13`) but `check()` matches it with
`_ => {}` (`schedule.rs:56`). The trigger type is dead specification -- a
user configuring it gets no delivery.

### P2. `fault_history` never serialized in any formatter

`JsonFormatter`, `TextFormatter`, `CsvFormatter`, and `GemstatsFormatter` all
skip the `fault_history` field of `HelmSpySnapshot` even when `Some`.

### P3. `QuantumObserver` trait has no implementors

See D4 above.

---

## Software Engineering Issues

### E1. Crate-level `#![allow(missing_docs, unsafe_code)]`

`helm-spy/src/lib.rs:4` suppresses both `missing_docs` and `unsafe_code`
warnings at the crate level. The `unsafe_code` allow is only needed for
`trace_ring.rs`. Targeted `#[allow(unsafe_code)]` on the specific module or
function would keep the rest of the crate auditable.

### E2. `PowerModel` / `EnergyTable` numbers are illustrative, not sourced

`cortex_a55()` in the energy model provides specific pJ/nJ numbers with no
reference to a data source. Consumers may trust them as authoritative.

**Suggested fix:** Add a doc comment citing the source or marking the values
as "illustrative estimates".

### E3. Test gap: snapshot does not round-trip fault history

`session.rs:234-246` tests `RingBuffer` directly but no test asserts that
`HelmSpy::snapshot()` includes populated faults.

### E4. Test gap: release vs debug probe wiring

No test validates that calling `HelmSpy::subscribe` in a release build
(where it doesn't exist) produces a compile error or an appropriate
runtime fallback.

---

## Architecture Issues

### A1. Dependency direction is correct

`helm-report` → `helm-spy` for snapshot types. No cycle. The cost is pulling
`dashmap` transitively through `helm-spy`, but this is acceptable.

### A2. Snapshot schema ownership is clean

`HelmSpySnapshot` is defined in `helm-spy/src/snapshot.rs` and re-exported
by `helm-report/src/snapshot.rs`. Single owner, no duplication.

---

## Idiomatic Rust Issues

### I1. Crate-wide `unsafe_code` allow instead of targeted

See E1. More idiomatic to `#[allow(unsafe_code)]` only on the SPSC ring
buffer module.

### I2. `JsonFormatter` silently returns `{}` on serialization failure

```rust
// helm-report/src/format/json.rs
to_vec_pretty(&obj).unwrap_or_else(|_| b"{}".to_vec())
```

Silent degradation. Prefer returning a `Result` or logging the error.

### I3. `AsyncFileSink::write` allocates per call

```rust
// helm-report/src/sink/async_file.rs:79
.send(DrainMsg::Write(data.to_vec()))
```

Each write allocates a `Vec`. Acceptable on the cold report path but
worth noting if reporting frequency increases.

---

## Recommendations

### Quick Wins (< 1 hour each)

1. **Fix CSV column order** (C1) -- swap header or swap row closure order
2. **Populate `fault_history` in `snapshot()`** (C2) -- one-line fix
3. **Add `debug_assert!` to `PerVcpuCounter` index** (C5)
4. **Document `PowerModel` numbers as estimates** (E2)
5. **Add `insn_size` to `CpuStepEvent`** and use it in bridge (C7)
6. **Narrow `#![allow(unsafe_code)]`** to `trace_ring` module only (E1, I1)

### Medium Effort (1-4 hours each)

7. **Replace `cfg(debug_assertions)` with a Cargo feature** for probe wiring (D1)
8. **Consolidate `ProbePluginBridge` and `HelmSpy::subscribe`** (D2)
9. **Add error tracking to `AsyncFileSink`** drain thread (C8)
10. **Implement `OnCounter` trigger** or remove the dead variant (P1)
11. **Add `fault_history` to all formatters** (P2)

### Structural (> 4 hours)

12. **Parameterize `SimPointCollector::on_branch`** fall-through address (C4)
13. **Redesign `IntervalHistogram::tick`** to have clearer accumulation semantics (C6)
14. **Wire `QuantumObserver`** into the engine or remove (D4)

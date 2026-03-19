# helm-report — High-Level Design

> **Status:** Draft — Phase 3 (Delivery layer)
> **Crate path:** `debug/helm-report/`
> **Depends on:** `helm-spy`
> **Provides to:** `helm-engine`, `helm-python`

---

## Table of Contents

1. [Purpose](#1-purpose)
2. [Scope Boundaries](#2-scope-boundaries)
3. [Crate Structure](#3-crate-structure)
4. [Dependency Graph](#4-dependency-graph)
5. [Sink Trait and Hierarchy](#5-sink-trait-and-hierarchy)
6. [ReportFormatter Catalog](#6-reportformatter-catalog)
7. [SpySpySnapshot](#7-observesessionsnapshot)
8. [Report](#8-report)
9. [ReportSchedule](#9-reportschedule)
10. [BinaryTraceSink — Typed Binary Traces](#10-binarytracesink--typed-binary-traces)
11. [URI-Based Sink Construction](#11-uri-based-sink-construction)
12. [Python API](#12-python-api)
13. [Phased Implementation](#13-phased-implementation)
14. [Design Decisions](#14-design-decisions)

---

## 1. Purpose

`helm-report` is the **delivery layer** of the Instrumentation-v2 redesign. It receives
already-collected analysis data from `helm-spy` (in the form of an
`SpySpySnapshot`) and delivers that data to one or more configured destinations
in a configured format.

**The crate has exactly one job: format bytes, write bytes.** It contains no analysis
logic, no probe subscriptions, and no hot-path code. Every function in `helm-report` runs
on the cold path — triggered explicitly by the user (`session.report(...)`) or by a
scheduled trigger fired at quantum boundaries.

The collection/delivery split that `helm-report` enforces:

```
┌─────────────────────────────────────────────────────┐
│  COLLECTION  (helm-spy — hot path)               │
│                                                      │
│  counter.inc()                ← AtomicU64 add        │
│  histogram.record(class)      ← array index + add   │
│  ring_buf.push(event)         ← fixed circular buf   │
│  heatmap.inc(pc)              ← per-thread local     │
└─────────────────────────────────────────────────────┘
                       ↓  snapshot()  (cold)
┌─────────────────────────────────────────────────────┐
│  DELIVERY  (helm-report — cold path, explicit)       │
│                                                      │
│  session.snapshot()   → SpySpySnapshot       │
│      ↓                                              │
│  ReportFormatter::format_session(snapshot)           │
│      ↓ Vec<u8>                                      │
│  Sink::write(bytes) × N sinks                        │
│      → File | Stderr | TCP | Python | Null | Binary  │
└─────────────────────────────────────────────────────┘
```

**Invariants (inviolable):**
1. Nothing in `helm-report` allocates on the per-instruction path.
2. Nothing in `helm-report` subscribes to probe events.
3. Delivery is always explicit: `Report::deliver()` or triggered by `ReportSchedule`.
4. The same collected data can be delivered to multiple sinks in one call.
5. `helm-spy` does not depend on `helm-report` — the dependency is one-way.

---

## 2. Scope Boundaries

### In scope

| Concern | Component |
|---------|-----------|
| Sink trait and all sink implementations | `src/sink/` |
| ReportFormatter trait and all formatter implementations | `src/format/` |
| SpySpySnapshot — immutable copy of SpySession | `src/snapshot.rs` |
| Report — pairs a snapshot with a formatter and sinks | `src/report.rs` |
| ReportSchedule — trigger-based periodic/conditional delivery | `src/schedule.rs` |
| URI-based sink construction | `src/sink/uri.rs` |
| BinaryTraceSink — typed binary file drain for TraceRing | `src/sink/binary.rs` |
| PythonSink — GIL-safe string buffer for Python polling | `src/sink/python.rs` |

### Out of scope

| Concern | Where it lives |
|---------|---------------|
| Analysis primitives (Counter, Histogram, HeatMap) | `helm-spy` |
| Probe subscriptions and collection logic | `helm-spy` |
| TraceRing construction and hot-path push | `helm-spy` |
| SpySession (live, with AtomicU64 fields) | `helm-spy` |
| GDB RSP stub | `helm-debug` |
| Checkpoint save/restore | `helm-debug` |
| Diagnostic macros (sim_stub!, sim_warn!) | `helm-diag` |

---

## 3. Crate Structure

```
debug/helm-report/
├── Cargo.toml
└── src/
    ├── lib.rs              — pub use re-exports
    ├── snapshot.rs         — SpySpySnapshot + session.snapshot()
    ├── report.rs           — Report struct + deliver() + deliver_to()
    ├── schedule.rs         — ReportTrigger enum + ReportSchedule + check()
    ├── error.rs            — SinkError, FormatError
    ├── sink/
    │   ├── mod.rs          — Sink trait definition + pub use
    │   ├── stderr.rs       — StderrSink
    │   ├── file.rs         — FileSink (sync, buffered)
    │   ├── async_file.rs   — AsyncFileSink (background drain thread)
    │   ├── tcp.rs          — TcpSink
    │   ├── null.rs         — NullSink
    │   ├── binary.rs       — BinaryTraceSink<T>
    │   ├── python.rs       — PythonSink
    │   └── uri.rs          — sink_from_uri()
    └── format/
        ├── mod.rs          — ReportFormatter trait + pub use
        ├── text.rs         — TextFormatter
        ├── json.rs         — JsonFormatter
        ├── csv.rs          — CsvFormatter
        └── gemstats.rs     — GemstatsFormatter
```

---

## 4. Dependency Graph

```
helm-core       (zero deps)
helm-probe      (zero deps)
    │
    └── helm-spy    (deps: helm-probe)
            │  ← SpySession, SpySpySnapshot, TraceRing
            │    (no dep on helm-report — collection ≠ delivery)
            │
    helm-report         (deps: helm-spy)
            │  ← Report, ReportSchedule, Sink, ReportFormatter
            │
    helm-debug          (deps: helm-probe; no dep on helm-spy/helm-report)
            │
    helm-engine         (deps: helm-probe, helm-spy, helm-report, helm-debug)
    helm-python         (deps: helm-engine; exposes Python API via PyO3)
```

`helm-report` does NOT depend on:
- `helm-probe` (no probe subscriptions — that is `helm-spy`'s job)
- `helm-debug` (no GDB, no checkpoint)
- `helm-core` directly (all arch types come through `helm-spy` types)

External crate dependencies:
- `serde_json` — JSON formatting
- `bytemuck` — safe Pod byte casting in `BinaryTraceSink`

---

## 5. Sink Trait and Hierarchy

### 5.1 The Sink trait

```rust
/// A delivery destination for report data.
///
/// Implementations must be Send + Sync — the engine may call write() from
/// a background thread (e.g., AsyncFileSink's drain thread fires a trigger
/// that calls Report::deliver(), which calls Sink::write()).
///
/// write() receives fully-formatted bytes. The sink is responsible for
/// buffering (or not). Implementations must be idempotent on flush().
pub trait Sink: Send + Sync {
    fn write(&self, data: &[u8]) -> std::io::Result<()>;
    fn flush(&self) -> std::io::Result<()> { Ok(()) }
    fn name(&self) -> &str;
}
```

### 5.2 Sink hierarchy diagram

```
Sink (trait)
├── StderrSink          — eprintln!() passthrough; no buffering; always available
├── FileSink            — Mutex<BufWriter<File>>; synchronous; flush on close
├── AsyncFileSink       — mpsc channel → background drain thread → BufWriter<File>
├── TcpSink             — Mutex<BufWriter<TcpStream>>; reconnect on drop
├── NullSink            — /dev/null; discards all writes; benchmarking baseline
├── BinaryTraceSink<T>  — drains TraceRing<T,N> to typed binary file with header
└── PythonSink          — Arc<Mutex<Vec<String>>> buffer; Python polls via py.read_lines()
```

### 5.3 Sink selection guide

| Sink | Use case | Thread safety | Buffered |
|------|----------|---------------|----------|
| `StderrSink` | Quick debug output; no file needed | Inherent (stderr) | No |
| `FileSink` | Persistent output; caller controls flush timing | `Mutex` | Yes (8 KB) |
| `AsyncFileSink` | High-throughput; do not block simulation thread | `SyncSender<_>` | Yes (channel) |
| `TcpSink` | Remote collection; dashboard integration | `Mutex` | Yes (4 KB) |
| `NullSink` | Benchmarking formatter overhead in isolation | Trivial | No |
| `BinaryTraceSink` | Typed event traces; Python mmap analysis | Background thread | Yes (ring) |
| `PythonSink` | Interactive Python session; sim.spy()… | `Arc<Mutex<_>>` | Vec<String> |

---

## 6. ReportFormatter Catalog

### 6.1 ReportFormatter trait

```rust
pub trait ReportFormatter: Send + Sync {
    /// Format the entire session snapshot into a byte buffer.
    fn format_session(&self, session: &SpySpySnapshot) -> Vec<u8>;

    /// Format a single named counter (for incremental delivery).
    fn format_counter(&self, name: &str, value: u64, unit: &str) -> Vec<u8>;

    /// Format a histogram (bins + counts).
    fn format_histogram(&self, name: &str, bins: &[(&str, u64)]) -> Vec<u8>;

    /// MIME type / content type string for this format.
    fn content_type(&self) -> &'static str;
}
```

### 6.2 Formatter catalog

| Formatter | Output | Use case |
|-----------|--------|----------|
| `TextFormatter` | Human-readable gem5-style text | Console, CI log output |
| `JsonFormatter` | Structured JSON object per metric | Machine consumption, dashboards |
| `CsvFormatter` | `timestamp,metric,value` lines | Time-series databases, Excel |
| `GemstatsFormatter` | gem5 `stats.txt` column-aligned text | Gem5 toolchain integration |

### 6.3 TextFormatter sample output

```
---------- Begin Simulation Statistics ----------
sim_insns                                    10000000  # Instructions retired
sim_ipc                                          1.23  # Instructions per cycle
insn_mix.IntAlu                               4523891   52.39%
insn_mix.Load                                 1823441   21.11%
insn_mix.Store                                 912233   10.56%
insn_mix.Branch                               1204122   13.94%
insn_mix.SIMD                                  536313    6.21%
cache_l1d.hits                                9823441
cache_l1d.misses                               176559
cache_l1d.hit_rate                               0.982
branch_pred.predictions                       1204122
branch_pred.mispredictions                      84288
branch_pred.miss_rate                            0.070
hot_pcs[0]                            0xffff800010012a4c  count=234812
hot_pcs[1]                            0xffff800010012abc  count=198234
---------- End Simulation Statistics ----------
```

### 6.4 JsonFormatter sample output

```json
{
  "helm_report_version": 1,
  "timestamp_ns": 1710849600000000000,
  "metrics": [
    { "name": "sim_insns",         "value": 10000000, "unit": "instructions" },
    { "name": "insn_mix.IntAlu",   "value": 4523891,  "unit": "instructions", "pct": 52.39 },
    { "name": "cache_l1d.hits",    "value": 9823441,  "unit": "accesses" },
    { "name": "cache_l1d.misses",  "value": 176559,   "unit": "accesses" },
    { "name": "cache_l1d.hit_rate","value": 0.982,    "unit": "ratio" }
  ],
  "hot_pcs": [
    { "pc": "0xffff800010012a4c", "count": 234812 },
    { "pc": "0xffff800010012abc", "count": 198234 }
  ]
}
```

### 6.5 GemstatsFormatter sample output

```
---------- Begin Simulation Statistics ----------
sim_insns                                    10000000                       # Instructions retired
sim_ticks                                     8130081                       # Ticks simulated
sim_freq                                  1000000000                       # Simulated frequency
system.cpu.committedInsts                    10000000                       # Committed instructions
system.cpu.ipc                                   1.23                       # IPC
---------- End Simulation Statistics ----------
```

---

## 7. SpySpySnapshot

`SpySpySnapshot` is an immutable, atomic copy of `SpySession` state taken at
a single point in time. It replaces live `AtomicU64` fields with plain `u64` values, making
it safe to format from any thread without re-ordering concerns.

```rust
/// Immutable snapshot of SpySession state.
///
/// Created by SpySession::snapshot() — a cheap copy of all atomic values.
/// Sent to helm-report for formatting; never modified after creation.
pub struct SpySpySnapshot {
    // ── Instruction mix ────────────────────────────────────────────────────
    pub insn_count:     u64,
    pub insn_mix:       Vec<(String, u64)>,     // (class_name, count) pairs

    // ── Hot PC heatmap ─────────────────────────────────────────────────────
    pub hot_pcs:        Vec<(u64, u64)>,         // (pc, count), sorted descending

    // ── Branch heatmap ─────────────────────────────────────────────────────
    pub branch_heatmap: Vec<(u64, u64)>,         // (pc, count), sorted descending

    // ── Cache model (optional) ─────────────────────────────────────────────
    pub cache_l1d:      Option<CacheSnapshot>,

    // ── Branch predictor (optional) ────────────────────────────────────────
    pub branch_pred:    Option<BranchPredSnapshot>,

    // ── Fault history (optional) ───────────────────────────────────────────
    pub fault_history:  Option<Vec<CpuFaultEvent>>,

    // ── Timing context ─────────────────────────────────────────────────────
    pub tick_count:     u64,
    pub snapshot_ns:    u64,    // wall-clock UNIX nanoseconds at snapshot time
}

pub struct CacheSnapshot {
    pub name:      String,
    pub hits:      u64,
    pub misses:    u64,
    pub hit_rate:  f64,
}

pub struct BranchPredSnapshot {
    pub name:          String,
    pub kind:          String,     // "BiModal" | "GShare" | "Perfect"
    pub predictions:   u64,
    pub mispredictions: u64,
    pub miss_rate:     f64,
}
```

`SpySession::snapshot()` copies all `AtomicU64` values with `Ordering::Relaxed` —
acceptable because the snapshot is a best-effort view, not a serialization point.

---

## 8. Report

`Report` pairs an `SpySpySnapshot` with one `ReportFormatter` and one or more
`Sink` instances. `deliver()` formats the snapshot once and writes the bytes to every sink.

```rust
pub struct Report {
    session:   Arc<SpySpySnapshot>,
    formatter: Box<dyn ReportFormatter>,
    sinks:     Vec<Box<dyn Sink>>,
}

impl Report {
    /// Format once, write to all sinks.
    pub fn deliver(&self) -> Result<(), SinkError>;

    /// Format once, write to a single additional sink (ad-hoc delivery).
    pub fn deliver_to(&self, sink: &dyn Sink) -> Result<(), SinkError>;

    /// Flush all sinks (call after deliver() when writing to buffered sinks).
    pub fn flush_all(&self) -> Result<(), SinkError>;
}
```

Design decisions:
- `formatter.format_session()` is called exactly once; the resulting `Vec<u8>` is cloned
  into each sink's `write()`. For large reports (>1 MB JSON), this is still faster than
  re-formatting for each sink. The clone cost is I/O-dominated for any real sink.
- `deliver()` continues writing to remaining sinks even if one sink's `write()` returns
  `Err`. All errors are collected and returned as `SinkError::MultipleErrors(Vec<io::Error>)`.
- `deliver_to()` enables ad-hoc delivery to a temporary sink (e.g., a one-off TCP write)
  without adding the sink to the permanent list.

---

## 9. ReportSchedule

`ReportSchedule` owns a `Report` and a list of `ReportTrigger` values. It is checked
on every instruction (via a pre_step probe subscriber in `helm-engine`) with negligible cost
when no trigger is active.

```rust
pub enum ReportTrigger {
    /// Deliver on process exit (registered with atexit-style hook).
    AtExit,
    /// Deliver every N instructions.
    EveryNInsns(u64),
    /// Deliver when the named counter reaches the given threshold.
    OnCounter { name: String, threshold: u64 },
    /// Deliver when PC == addr.
    OnPc(u64),
    /// Never fires automatically; user calls ReportSchedule::deliver() directly.
    Explicit,
}

pub struct ReportSchedule {
    triggers: Vec<ReportTrigger>,
    report:   Report,
    // Internal state for EveryNInsns: last delivery at this insn count.
    last_delivered_at: u64,
}

impl ReportSchedule {
    /// Called from the pre_step probe subscriber on every instruction.
    ///
    /// Hot-path friendly: for EveryNInsns with large N, the check is
    /// one integer modulo — ~1 ns. For OnPc, one equality comparison.
    /// AtExit and Explicit never fire here.
    pub fn check(&mut self, pc: u64, insn_count: u64);

    /// Called at process exit for AtExit triggers.
    pub fn flush_at_exit(&self);

    /// Deliver immediately, regardless of triggers.
    pub fn deliver(&self) -> Result<(), SinkError>;
}
```

`ReportSchedule` does not subscribe to probes itself. `helm-engine` calls
`schedule.check(pc, insn_count)` from its pre_step subscriber. This keeps the scheduling
logic in one place and avoids a second probe subscription layered inside `helm-report`.

---

## 10. BinaryTraceSink — Typed Binary Traces

`BinaryTraceSink<T>` drains a `TraceRing<T, N>` (from `helm-spy`) to a typed binary
file in the background. Python reads the file with `mmap + struct.unpack_from` — zero
text parsing.

### 10.1 File format

Every binary trace file begins with a fixed 80-byte header:

```c
/* trace_header.h — also published for Python consumers */
#define HELM_TRACE_MAGIC   0x484D4C54   /* 'HLMT' little-endian */
#define HELM_TRACE_VERSION 1

typedef struct {
    uint32_t magic;         /* 0x484D4C54 */
    uint32_t version;       /* 1 */
    uint32_t record_size;   /* sizeof(T) */
    uint32_t record_count;  /* total records written (filled on close) */
    char     type_name[64]; /* null-terminated, e.g. "BranchRecord" */
} TraceHeader;              /* sizeof = 80 bytes */
```

Records follow immediately after the header as a packed array of `T` values.

### 10.2 Canonical BranchRecord layout

```c
/* 32 bytes — two cache lines */
typedef struct {
    uint64_t pc;          /* instruction address */
    uint64_t target;      /* branch target */
    uint64_t insn_count;  /* instruction count at branch */
    uint8_t  flags;       /* bit 0: taken; bits 2..4: BranchKind */
    uint8_t  _pad[7];
} BranchRecord;
```

### 10.3 Python consumer

```python
import mmap, struct

HDR_FMT = "=IIII64s"      # little-endian: magic, version, rec_size, rec_count, type_name
HDR_SIZE = struct.calcsize(HDR_FMT)   # 80
REC_FMT  = "=QQQBxxxxxxx"  # pc, target, insn_count, flags, 7-pad (= 32 bytes)

with open("branch.trace", "rb") as f:
    mm = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
    magic, version, rec_size, rec_count, type_name = struct.unpack_from(HDR_FMT, mm, 0)
    assert magic == 0x484D4C54, "not a helm trace"
    assert rec_size == struct.calcsize(REC_FMT), "record size mismatch"
    records = list(struct.iter_unpack(REC_FMT, mm[HDR_SIZE:HDR_SIZE + rec_count * rec_size]))
```

### 10.4 BinaryTraceSink design

- Background drain thread: `ring.drain_into(&mut buf)` every 10 ms; writes accumulated
  records as raw bytes via `bytemuck::cast_slice`.
- On drop: signals drain thread to stop, flushes remaining records, writes final
  `record_count` into the header at offset 12, closes the file.
- Type parameter `T: Copy + Send + bytemuck::Pod + 'static` ensures safe byte casting with
  no `unsafe` in the drain loop itself.

---

## 11. URI-Based Sink Construction

`sink_from_uri(uri: &str) -> Result<Box<dyn Sink>, SinkError>` constructs a sink from a
URI string. This is the primary way the Python layer creates sinks.

| URI | Sink created |
|-----|-------------|
| `stderr:` | `StderrSink` |
| `file:/path/to/file` | `AsyncFileSink` (background drain) |
| `file+sync:/path/to/file` | `FileSink` (synchronous, for test reproducibility) |
| `tcp:host:port` | `TcpSink` (connects immediately) |
| `null:` | `NullSink` |

URI parsing errors (malformed TCP address, file path with no `/`, etc.) return
`Err(SinkError::InvalidUri(String))`. TCP connection failures return
`Err(SinkError::Io(io::Error))`.

---

## 12. Python API

```python
import helm_ng

sim = helm_ng.Simulation(platform="virt")
session = sim.spy()
session.track_insns()
session.track_branches()
session.track_memory(l1d_size=32768, l1d_assoc=8)

sim.run(50_000_000)

# Deliver to stderr in human-readable text (default)
session.report(sink="stderr:", format="text")

# Deliver to a JSON file
session.report(sink="file:/tmp/perf.json", format="json")

# Deliver to gem5-compatible stats.txt format over TCP (e.g., to a dashboard)
session.report(sink="tcp:localhost:9001", format="gemstats")

# Scheduled: deliver every 10M instructions to a rolling CSV
session.report_every(
    n_insns=10_000_000,
    sink="file:/tmp/rolling.csv",
    format="csv",
)

# Explicit snapshot query without delivery
snap = session.snapshot()
print(f"IPC: {snap.insn_count / snap.tick_count:.3f}")
print(f"L1D miss rate: {snap.cache_l1d.hit_rate:.1%}")
```

The Python `session.report(sink=..., format=...)` call is implemented in `helm-python` as:

```rust
// helm-python/src/observe.rs
#[pymethods]
impl PySpySession {
    fn report(&self, sink: &str, format: &str) -> PyResult<()> {
        let snap = Arc::new(self.inner.snapshot());
        let formatter: Box<dyn ReportFormatter> = match format {
            "text"     => Box::new(TextFormatter::default()),
            "json"     => Box::new(JsonFormatter::default()),
            "csv"      => Box::new(CsvFormatter::default()),
            "gemstats" => Box::new(GemstatsFormatter::default()),
            other      => return Err(PyValueError::new_err(format!("unknown format: {other}"))),
        };
        let sink = helm_report::sink_from_uri(sink)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let report = Report::new(snap, formatter, vec![sink]);
        report.deliver().map_err(|e| PyValueError::new_err(e.to_string()))
    }
}
```

---

## 13. Phased Implementation

### Phase 3 — Core delivery (prerequisite: helm-spy Phase 2)

1. `SpySpySnapshot` struct + `SpySession::snapshot()` impl
2. `Sink` trait + `StderrSink`, `FileSink`, `NullSink`
3. `TextFormatter` (replicates current atexit-style output)
4. `Report::deliver()`
5. `sink_from_uri()` for `stderr:`, `file:`, `null:` (TCP deferred)
6. `session.report(sink, format)` Python binding

### Phase 3.1 — Async and network sinks

1. `AsyncFileSink` with background drain thread
2. `TcpSink`
3. `sink_from_uri()` extended with `tcp:` and `file+sync:`
4. `ReportSchedule` + `ReportTrigger::EveryNInsns` + `AtExit`

### Phase 3.2 — Formatters and binary traces

1. `JsonFormatter`
2. `CsvFormatter`
3. `GemstatsFormatter`
4. `BinaryTraceSink<T>` — requires `bytemuck::Pod` bound on `TraceRing` record types
5. `PythonSink` — requires confirming GIL interaction pattern with PyO3

### Phase 5 — Advanced delivery

1. `ReportTrigger::OnCounter` + `OnPc`
2. Differential report: compare two `SpySpySnapshot` values (format deltas)
3. Protobuf formatter (for production data pipeline integration)

---

## 14. Design Decisions

| Decision | Rationale |
|----------|-----------|
| `helm-report` depends on `helm-spy` (one direction only) | Collection must not depend on delivery; otherwise a delivery failure breaks collection |
| `SpySpySnapshot` copies all values from `SpySession` | Avoids holding AtomicU64 references across a format pass; snapshot is immutable, thread-safe |
| `BinaryTraceSink` uses `bytemuck::Pod` instead of `unsafe` byte cast | `bytemuck` checks `repr(C)` alignment at compile time; avoids `unsafe` in the drain loop |
| `AsyncFileSink` per-sink background thread (not a global I/O pool) | Simpler ownership; avoids global state; one thread per sink is acceptable for the delivery rate |
| `sink_from_uri` returns `Result` (not `Box<dyn Sink>` directly) | TCP connect and file open can fail; callers must handle errors before the first write |
| `deliver()` continues on sink error (does not short-circuit) | A failing TCP sink must not prevent the file sink from receiving the report |
| `ReportSchedule::check()` called by helm-engine (not self-subscribed) | Keeps probe subscription logic in one place; report layer stays pure delivery |

---

*See [`LLD-sinks.md`](LLD-sinks.md) for full implementation of sinks, formatters, Report, and ReportSchedule.*
*See [`TEST.md`](TEST.md) for the complete test strategy.*

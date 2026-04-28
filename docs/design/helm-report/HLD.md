# helm-report — High-Level Design

> **Status:** Implemented — all sinks, formatters, Report, and ReportSchedule are complete (62 tests pass).
> **Crate path:** `debug/helm-report/`
> **Currently standalone:** `SpySpySnapshot` is defined locally in `src/snapshot.rs`. When `helm-spy` is
> implemented, that definition will move there and `helm-report` will add `helm-spy` as a dependency.
> **Provides to:** `helm-engine`, `helm-python` (future wiring)
>
> **Build duality (apr 2026 update):** `helm-report` is cold-path by
> design, but a perf binary should not link `serde_json` /
> `bytemuck` / the formatter / sink machinery at all. This revision
> adds Cargo features so the public `Sink` / `ReportFormatter` types
> become trait-bounded interfaces with optional concrete implementors:
> with `--no-default-features` the crate exports the trait shells and
> a `NullSink` only, and `Report::deliver()` is `#[inline] fn _ {}`.
> See § 13.

---

## Table of Contents

1. [Purpose](#1-purpose)
2. [Scope Boundaries](#2-scope-boundaries)
3. [Crate Structure](#3-crate-structure)
4. [Dependency Graph](#4-dependency-graph)
5. [Sink Trait and Hierarchy](#5-sink-trait-and-hierarchy)
6. [ReportFormatter Catalog](#6-reportformatter-catalog)
7. [SpySpySnapshot](#7-spyspysnapshot)
8. [Report](#8-report)
9. [ReportSchedule](#9-reportschedule)
10. [BinaryTraceSink — Typed Binary Traces](#10-binarytracesink--typed-binary-traces)
11. [URI-Based Sink Construction](#11-uri-based-sink-construction)
12. [Design Decisions](#12-design-decisions)

---

## 1. Purpose

`helm-report` is the **delivery layer** of the Instrumentation-v2 redesign. It receives
already-collected analysis data (in the form of a `SpySpySnapshot`) and delivers that data to one
or more configured destinations in a configured format.

**The crate has exactly one job: format bytes, write bytes.** It contains no analysis logic, no
probe subscriptions, and no hot-path code. Every function in `helm-report` runs on the cold path —
triggered explicitly by the caller (`Report::deliver()`) or by a scheduled trigger fired at
instruction-count boundaries.

The collection/delivery split that `helm-report` enforces:

```
+-----------------------------------------------------+
|  COLLECTION  (helm-spy -- hot path, not yet wired)  |
|                                                     |
|  counter.inc()             <- AtomicU64 add         |
|  histogram.record(class)   <- array index + add     |
|  heatmap.inc(pc)           <- per-thread local      |
+-----------------------------------------------------+
                     |  snapshot()  (cold)
                     v
+-----------------------------------------------------+
|  DELIVERY  (helm-report -- cold path, explicit)     |
|                                                     |
|  SpySpySnapshot (immutable copy of session state)   |
|      |                                              |
|      v                                              |
|  ReportFormatter::format_session(snapshot)          |
|      |  Vec<u8>                                     |
|      v                                              |
|  Sink::write(bytes) x N sinks                       |
|      -> File | Stderr | TCP | Python | Null | Binary|
+-----------------------------------------------------+
```

**Invariants (inviolable):**

1. Nothing in `helm-report` allocates on the per-instruction path.
2. Nothing in `helm-report` subscribes to probe events.
3. Delivery is always explicit: `Report::deliver()` or triggered by `ReportSchedule`.
4. The same snapshot can be delivered to multiple sinks in one call.
5. When `helm-spy` is implemented, `helm-spy` will NOT depend on `helm-report` — the
   dependency is one-way.
6. With `--no-default-features` the crate has zero runtime side
   effects: every formatter is absent, every sink except `NullSink` is
   absent, and `Report::deliver()` is a `Result::Ok` no-op.

---

## 2. Scope Boundaries

### In scope

| Concern | Component |
|---------|-----------|
| Sink trait and all 7 sink implementations | `src/sink/` |
| ReportFormatter trait and all 4 formatter implementations | `src/format/` |
| SpySpySnapshot — locally defined immutable snapshot struct | `src/snapshot.rs` |
| Report — pairs a snapshot with a formatter and sinks | `src/report.rs` |
| ReportSchedule — trigger-based periodic/conditional delivery | `src/schedule.rs` |
| URI-based sink construction | `src/sink/uri.rs` |
| BinaryTraceSink — typed binary file drain with background thread | `src/sink/binary.rs` |
| PythonSink — GIL-safe string buffer for Python polling | `src/sink/python.rs` |

### Out of scope

| Concern | Where it lives |
|---------|----------------|
| Analysis primitives (Counter, Histogram, HeatMap) | `helm-spy` (not yet implemented) |
| Probe subscriptions and collection logic | `helm-spy` |
| HelmSpy (live session with AtomicU64 fields) | `helm-spy` |
| GDB RSP stub | `helm-debug` |
| Checkpoint save/restore | `helm-debug` |

---

## 3. Crate Structure

```
debug/helm-report/
├── Cargo.toml
└── src/
    ├── lib.rs              -- pub use re-exports, shared test_snapshot() helper
    ├── snapshot.rs         -- SpySpySnapshot, CacheSnapshot, BranchPredSnapshot, CpuFaultEvent
    ├── report.rs           -- Report struct, deliver(), deliver_to(), flush_all()
    ├── schedule.rs         -- ReportTrigger enum, ReportSchedule, check(), flush_at_exit()
    ├── error.rs            -- SinkError enum
    ├── sink/
    │   ├── mod.rs          -- Sink trait definition, TestSink (cfg(test)), pub use
    │   ├── stderr.rs       -- StderrSink
    │   ├── file.rs         -- FileSink (synchronous, 8 KB BufWriter)
    │   ├── async_file.rs   -- AsyncFileSink (background drain thread, 64 KB BufWriter)
    │   ├── tcp.rs          -- TcpSink (4 KB BufWriter over TcpStream)
    │   ├── null.rs         -- NullSink (discards all writes)
    │   ├── binary.rs       -- BinaryTraceSink<T>, TraceFileHeader, HELM_TRACE_MAGIC/VERSION
    │   ├── python.rs       -- PythonSink (Arc<Mutex<Vec<String>>>)
    │   └── uri.rs          -- sink_from_uri()
    └── format/
        ├── mod.rs          -- ReportFormatter trait, pub use
        ├── text.rs         -- TextFormatter (gem5-style human-readable)
        ├── json.rs         -- JsonFormatter (serde_json pretty object)
        ├── csv.rs          -- CsvFormatter (metric,timestamp_ns,value rows)
        └── helmstats.rs     -- HelmstatsFormatter (gem5 stats.txt column-aligned)
```

---

## 4. Dependency Graph

```
helm-report  (standalone; no workspace crate deps)
    |
    |  [dependencies]
    +-- bytemuck  = "1"  (features = ["derive"])   -- Pod byte casting for BinaryTraceSink
    +-- serde_json = "1"                            -- JSON formatting in JsonFormatter
    |
    |  [dev-dependencies]
    +-- tempfile = "3"                              -- Temporary files in sink tests
```

`helm-report` currently has no dependency on `helm-spy`, `helm-core`, or any other workspace
crate. It defines `SpySpySnapshot` locally. This is intentional: the crate is fully usable and
testable in isolation today.

**Next step:** once `helm-spy` is implemented, add it as a dependency, remove `src/snapshot.rs`,
and re-export `SpySpySnapshot` from `helm-spy`.

No async runtime (`tokio`, `async-std`). `AsyncFileSink` and `BinaryTraceSink` use plain
`std::thread` drain loops with `SyncSender` back-pressure.

---

## 5. Sink Trait and Hierarchy

### 5.1 The Sink trait

```rust
pub trait Sink: Send + Sync {
    fn write(&self, data: &[u8]) -> std::io::Result<()>;
    fn flush(&self) -> std::io::Result<()> { Ok(()) }
    fn name(&self) -> &str;
}
```

`write()` receives fully-formatted bytes for one `Report::deliver()` call. Implementations are
responsible for internal buffering. `flush()` has a default no-op implementation; buffered sinks
override it. Both methods must be thread-safe (`Send + Sync`).

### 5.2 Sink hierarchy

```
Sink (trait)
+-- StderrSink          -- io::stderr().write_all(); no internal buffer
+-- FileSink            -- Mutex<BufWriter<File>>; 8 KB; flush on Drop
+-- AsyncFileSink       -- SyncSender<DrainMsg> to background thread; 64 KB BufWriter
+-- TcpSink             -- Mutex<BufWriter<TcpStream>>; 4 KB; no reconnect
+-- NullSink            -- discards all writes; O(1) time; benchmarking baseline
+-- BinaryTraceSink<T>  -- SyncSender<BinaryMsg<T>> to background thread; typed binary file
+-- PythonSink          -- Arc<Mutex<Vec<String>>>; Python polls via drain()
```

### 5.3 Sink selection guide

| Sink | Use case | Thread safety | Buffered |
|------|----------|---------------|----------|
| `StderrSink` | Quick debug output; always available | Inherent (OS-level) | No |
| `FileSink` | Persistent output; synchronous; test-reproducible | `Mutex` | Yes (8 KB) |
| `AsyncFileSink` | High-throughput; do not block calling thread | `SyncSender` (bound 1024) | Yes (64 KB) |
| `TcpSink` | Remote dashboard collection | `Mutex` | Yes (4 KB) |
| `NullSink` | Benchmarking formatter overhead | Trivial | No |
| `BinaryTraceSink<T>` | Typed event traces; Python mmap analysis | `SyncSender` (bound 512) | Yes (256 KB) |
| `PythonSink` | Interactive Python sessions; poll-based consumption | `Arc<Mutex<_>>` | Vec<String> |

---

## 6. ReportFormatter Catalog

### 6.1 ReportFormatter trait

```rust
pub trait ReportFormatter: Send + Sync {
    fn format_session(&self, session: &SpySpySnapshot) -> Vec<u8>;
    fn format_counter(&self, name: &str, value: u64, unit: &str) -> Vec<u8>;
    fn format_histogram(&self, name: &str, bins: &[(&str, u64)]) -> Vec<u8>;
    fn content_type(&self) -> &'static str;
}
```

### 6.2 Formatter catalog

| Formatter | Content-Type | Output style |
|-----------|-------------|--------------|
| `TextFormatter` | `text/plain; charset=utf-8` | gem5-style: name left-padded to 40, value right-justified in 20 |
| `JsonFormatter` | `application/json; charset=utf-8` | Single JSON object (pretty-printed via serde_json) |
| `CsvFormatter` | `text/csv; charset=utf-8` | Header row then one metric per row: `metric,timestamp_ns,value` |
| `HelmstatsFormatter` | `text/plain; charset=utf-8` | gem5 `stats.txt` column-aligned; gem5 metric names |

### 6.3 TextFormatter sample output

```
---------- Begin Simulation Statistics ----------
sim_insns                                    10000000  # Instructions retired
sim_ticks                                     8130081  # Ticks simulated
sim_ipc                                      1.230000  # Instructions per cycle
insn_mix.IntAlu                    5000000   50.00%
insn_mix.Load                      2000000   20.00%
cache_l1d.hits                                9823441
cache_l1d.misses                               176559
cache_l1d.hit_rate                           0.982153
branch_pred_bimodal.predictions               1500000
branch_pred_bimodal.mispredictions             105000
branch_pred_bimodal.miss_rate                0.070000
hot_pcs[0]            0xffff800010012a4c  count=234812
hot_pcs[1]            0xffff800010012abc  count=198234
----------  End Simulation Statistics  ----------
```

### 6.4 HelmstatsFormatter key metric names

```
sim_insns                       <count>   # Number of instructions simulated
sim_ticks                       <count>   # Number of ticks simulated
sim_freq                     1000000000   # Frequency of simulated ticks
system.cpu.committedInsts       <count>   # Committed instructions
system.cpu.ipc                  <float>   # Instructions per tick
system.cpu.op_class_0::<class>  <count>   # Instruction mix entry
system.cpu.dcache.overall_hits::total     <count>
system.cpu.dcache.overall_misses::total   <count>
system.cpu.dcache.overall_miss_rate::total <float>
system.cpu.branchPred.lookups   <count>
system.cpu.branchPred.mispredicts <count>
```

---

## 7. SpySpySnapshot

`SpySpySnapshot` is an immutable, point-in-time copy of observation session state. It replaces
live `AtomicU64` fields with plain `u64` values, making it safe to format from any thread without
ordering concerns.

**Currently defined in `src/snapshot.rs`.** When `helm-spy` is implemented this struct will
move there. The comment in source reads: *"When helm-spy is implemented, these definitions will
move there and helm-report will re-export them via the helm-spy dependency."*

```rust
pub struct SpySpySnapshot {
    pub insn_count:      u64,
    pub insn_mix:        Vec<(String, u64)>,     // (class_name, count); order stable
    pub hot_pcs:         Vec<(u64, u64)>,         // (pc, count); sorted descending by count
    pub branch_heatmap:  Vec<(u64, u64)>,         // (pc, count); sorted descending by count
    pub cache_l1d:       Option<CacheSnapshot>,
    pub branch_pred:     Option<BranchPredSnapshot>,
    pub fault_history:   Option<Vec<CpuFaultEvent>>,
    pub tick_count:      u64,
    pub snapshot_ns:     u64,   // UNIX nanoseconds at snapshot time
}

pub struct CacheSnapshot {
    pub name:     String,
    pub hits:     u64,
    pub misses:   u64,
    pub hit_rate: f64,          // hits / (hits + misses)
}

pub struct BranchPredSnapshot {
    pub name:           String,
    pub kind:           String,  // "BiModal" | "GShare" | "Perfect"
    pub predictions:    u64,
    pub mispredictions: u64,
    pub miss_rate:      f64,     // mispredictions / predictions
}

pub struct CpuFaultEvent {
    pub insn_count:  u64,
    pub pc:          u64,
    pub fault_code:  u32,
    pub description: String,
}
```

`SpySpySnapshot` also provides two computed methods:
- `ipc() -> f64` — `insn_count / tick_count`; returns `0.0` if `tick_count == 0`
- `insn_mix_total() -> u64` — sum of all mix class counts; should equal `insn_count`

---

## 8. Report

`Report` pairs a `SpySpySnapshot` with one `ReportFormatter` and one or more `Sink` instances.

```rust
pub struct Report {
    session:   Arc<SpySpySnapshot>,
    formatter: Box<dyn ReportFormatter>,
    sinks:     Vec<Box<dyn Sink>>,
}

impl Report {
    pub fn new(
        session:   Arc<SpySpySnapshot>,
        formatter: Box<dyn ReportFormatter>,
        sinks:     Vec<Box<dyn Sink>>,
    ) -> Self;

    /// Format once; write to all registered sinks.
    /// Errors from individual sinks are accumulated; a failure does NOT prevent
    /// delivery to subsequent sinks. Returns SinkError::MultipleErrors if >1 fail.
    pub fn deliver(&self) -> Result<(), SinkError>;

    /// Format once; write to a single ad-hoc sink. Does not touch the permanent sink list.
    pub fn deliver_to(&self, sink: &dyn Sink) -> Result<(), SinkError>;

    /// Flush all registered sinks.
    pub fn flush_all(&self) -> Result<(), SinkError>;
}
```

`deliver()` calls `flush_all_inner()` after writing to all sinks. Errors from both write and flush
passes are accumulated and returned together.

---

## 9. ReportSchedule

`ReportSchedule` wraps a `Report` with a list of triggers. The engine calls `check()` on every
instruction from a `pre_step` hook.

```rust
pub enum ReportTrigger {
    AtExit,
    EveryNInsns(u64),
    OnCounter { name: String, threshold: u64 },
    OnPc(u64),
    Explicit,
}

pub struct ReportSchedule {
    triggers:          Vec<ReportTrigger>,
    report:            Report,
    last_delivered_at: u64,   // insn_count at which EveryNInsns last fired
}

impl ReportSchedule {
    pub fn new(report: Report, triggers: Vec<ReportTrigger>) -> Self;

    /// Called per-instruction. Fires EveryNInsns and OnPc triggers.
    /// AtExit, Explicit, and OnCounter do NOT fire here.
    /// Cost when no trigger fires: one integer division per EveryNInsns trigger.
    pub fn check(&mut self, pc: u64, insn_count: u64);

    /// Call at process exit; delivers once for any AtExit trigger in the list.
    pub fn flush_at_exit(&self);

    /// Deliver immediately regardless of triggers.
    pub fn deliver(&self) -> Result<(), SinkError>;
}
```

`EveryNInsns(n)` fires when `(insn_count / n) > (last_delivered_at / n)`. This detects each
integer multiple of `n` being crossed, even if `check()` is not called on the exact boundary.

`OnCounter` is defined in the enum but `check()` currently passes it through to the `_ => {}`
catch-all — it does not fire automatically. It is reserved for future use.

---

## 10. BinaryTraceSink — Typed Binary Traces

`BinaryTraceSink<T>` writes typed binary records to a file via a background drain thread.
Python reads the file with `mmap + struct.unpack_from` — zero text parsing.

### 10.1 File format

Every binary trace file begins with a fixed 80-byte header (`TraceFileHeader`):

```
Offset  Size  Field
     0     4  magic        = 0x484D4C54  ('HLMT' little-endian)
     4     4  version      = 1
     8     4  record_size  = sizeof(T)
    12     4  record_count = 0 at open; patched on close
    16    64  type_name    = null-padded ASCII (e.g. "BranchRecord")
```

`HELM_TRACE_MAGIC = 0x484D_4C54` and `HELM_TRACE_VERSION = 1` are exported as public constants.
`TraceFileHeader` is `repr(C)`, `Copy`, `Pod`, `Zeroable`, and compile-time asserted to be 80 bytes.

After the header: a packed array of `T` values. `record_count` is patched at header offset 12
when the drain thread receives `Stop`.

### 10.2 Type constraints

`T: Copy + Send + bytemuck::Pod + 'static`

- `Copy`: records are moved out of the channel message
- `Pod`: safe byte cast via `bytemuck::cast_slice` (no `unsafe` in drain loop)
- `Send`: records cross thread boundary

### 10.3 API

```rust
impl<T: Copy + Send + Pod + 'static> BinaryTraceSink<T> {
    /// Open path, write header, spawn drain thread.
    /// Returns (sink, JoinHandle). Caller may store the handle and join at exit.
    pub fn open(path: impl AsRef<Path>, type_name: &str) -> io::Result<(Self, JoinHandle<()>)>;

    /// Push a slice of records to the drain queue.
    pub fn push_records(&self, records: &[T]) -> io::Result<()>;

    /// Return approximate record count flushed so far (updated on Stop).
    pub fn record_count(&self) -> u32;
}
```

`Sink::write()` on `BinaryTraceSink<T>` validates that `data.len()` is a multiple of
`sizeof(T)` before casting.

---

## 11. URI-Based Sink Construction

`sink_from_uri(uri: &str) -> Result<Box<dyn Sink>, SinkError>` constructs a sink from a URI
string. This is the intended entry point for Python-layer sink creation.

| URI | Sink | Notes |
|-----|------|-------|
| `stderr:` | `StderrSink` | Always succeeds |
| `null:` | `NullSink` | Always succeeds |
| `file:/absolute/path` | `AsyncFileSink` | JoinHandle is dropped (drain runs until sink is dropped) |
| `file+sync:/absolute/path` | `FileSink` | Synchronous; useful for test reproducibility |
| `tcp:host:port` | `TcpSink` | Connects immediately; returns `Io` error on failure |

Returns `Err(SinkError::InvalidUri)` for unrecognised schemes or malformed TCP addresses (no
colon separating host and port). Returns `Err(SinkError::Io)` if the sink cannot be opened.

---

## 12. Design Decisions

| Decision | Rationale |
|----------|-----------|
| `SpySpySnapshot` defined locally (no `helm-spy` dep yet) | Crate is fully testable in isolation; move happens when helm-spy is implemented |
| `AsyncFileSink::open()` returns `(Self, JoinHandle<()>)` | Caller decides when to join; sink itself only holds the sender side |
| `BinaryTraceSink` uses `bytemuck::Pod` instead of `unsafe` byte cast | Alignment checked at compile time; drain loop has zero `unsafe` |
| No async runtime | Background I/O uses plain `std::thread`; avoids executor complexity |
| `deliver()` continues past sink errors | A failing TCP sink must not prevent the file sink from receiving the report |
| `sink_from_uri("file:...")` discards `JoinHandle` | Drain thread runs until the `Box<dyn Sink>` is dropped; acceptable for delivery-rate sinks |
| `ReportSchedule::check()` is called by the engine (not self-subscribed) | Keeps probe subscription logic in one place; report layer stays pure delivery |
| `OnCounter` variant defined but inactive in `check()` | Reserves the variant for future use without breaking the enum |
| `Sink` / `ReportFormatter` are trait shells with feature-gated impls | Lets a perf build drop all formatters/sinks and the `serde_json` + `bytemuck` deps. Mirrors `helm-stats` and `helm-spy` ZST-when-off pattern. |
| `Report::deliver()` is no-op without `report` feature | Keeps callers (`helm-engine`, `helm-python`) source-stable across builds; cost in the perf build is zero (inlined empty function). |
| `HelmstatsFormatter` consumes `&dyn StatsRegistry` (planned) | Aligns gem5 stats.txt output with the `helm-stats` registry. `HelmSpySnapshot` remains the input for analysis-model summaries; raw counters dump from the registry directly. |
| Optional `helmstats` config emitter | Adds gem5-style `m5out/{config.ini,config.json}` writers, gated behind `helmstats`. Off in the perf build. |

---

## 13. Cargo Features and the no-link perf build

| Feature              | Default | Implies                  | Effect |
|----------------------|---------|--------------------------|--------|
| `report`             | on      | --                       | enables the `Sink` trait impls (`StderrSink`, `FileSink`, `AsyncFileSink`, `TcpSink`, `BinaryTraceSink`, `PythonSink`), the `ReportFormatter` impls (`TextFormatter`, `JsonFormatter`, `CsvFormatter`), `Report::deliver()`, and `ReportSchedule`. Without it, only the trait definitions and `NullSink` exist; `Report::deliver()` is an inlined `Ok(())`. |
| `helmstats`          | off     | `report`                 | enables `HelmstatsFormatter` and the `m5out/{config.ini,config.json,stats.txt}` writers. (Source file remains `src/format/helmstats.rs` since it emits the gem5 line format; the *feature* is named after helm.) |
| `binary-trace`       | on      | `report`, `bytemuck/derive` | enables `BinaryTraceSink<T>` and the trace-file header. |
| `python-sink`        | on      | `report`                 | enables `PythonSink`. |
| `tcp-sink`           | off     | `report`                 | enables `TcpSink`. |
| `serde-json`         | on      | `report`                 | enables `JsonFormatter` (and pulls in `serde_json`). |

Per the project policy that stats are dev/profiling-only, all of these
are **off by default**. A `cargo build --release` (no extra features)
yields a binary where:

- the crate has no `serde_json` / `bytemuck` link,
- the only concrete sink is `NullSink`,
- `Report::deliver()` and `ReportSchedule::check()` are inlined empty
  functions (`check()` returns immediately, no division per trigger),
- `HelmstatsFormatter` and the `config.ini`/`config.json` writers are
  absent.

Dev/profiling builds opt in via aggregate features on `helm-cli`
(`dev-instrumentation`, `profiling`); see
`docs/research/gem5-stats-helm-adaptation.md` § 3.2.1.

### Trait shells stay unconditional

```rust
// debug/helm-report/src/sink/mod.rs
pub trait Sink: Send + Sync {
    fn write(&self, data: &[u8]) -> std::io::Result<()>;
    fn flush(&self) -> std::io::Result<()> { Ok(()) }
    fn name(&self) -> &str;
}

pub struct NullSink;
impl Sink for NullSink {
    #[inline] fn write(&self, _: &[u8]) -> std::io::Result<()> { Ok(()) }
    fn name(&self) -> &str { "null" }
}

#[cfg(feature = "report")]
mod stderr;       #[cfg(feature = "report")] pub use stderr::StderrSink;
#[cfg(feature = "report")]
mod file;         #[cfg(feature = "report")] pub use file::{FileSink, AsyncFileSink};
#[cfg(feature = "tcp-sink")]
mod tcp;          #[cfg(feature = "tcp-sink")] pub use tcp::TcpSink;
#[cfg(feature = "python-sink")]
mod python;       #[cfg(feature = "python-sink")] pub use python::PythonSink;
#[cfg(feature = "binary-trace")]
mod binary;       #[cfg(feature = "binary-trace")] pub use binary::{BinaryTraceSink, TraceFileHeader};
```

`Report` itself follows the same shape:

```rust
// debug/helm-report/src/report.rs
pub struct Report {
    #[cfg(feature = "report")] inner: ReportInner,
}

impl Report {
    #[cfg(feature = "report")]
    pub fn deliver(&self) -> Result<(), SinkError> { self.inner.deliver() }

    #[cfg(not(feature = "report"))]
    #[inline(always)]
    pub fn deliver(&self) -> Result<(), SinkError> { Ok(()) }
}
```

Callers (`helm-engine::HelmSim::dump_stats`, `helm-python`) need not
change between builds; the `cfg`-selected body chooses whether work
actually happens.

### Verification

`debug/helm-report/tests/feature_gate_off.rs`:

```rust
#![cfg(not(feature = "report"))]
use helm_report::{NullSink, Report};

#[test]
fn perf_build_has_only_null_sink() {
    // These types must be absent from the perf build:
    //   - StderrSink, FileSink, AsyncFileSink, TcpSink, BinaryTraceSink<_>
    //   - TextFormatter, JsonFormatter, CsvFormatter, HelmstatsFormatter
    // Compile-fail tests (in tests/ui/) confirm absence.
    let s = NullSink;
    s.write(b"ignored").unwrap();
}
```

`tests/ui/*.rs` files use `compile_fail` doctests via `trybuild` to
lock in the absence of the gated symbols when the features are off.

### helm-stats output mapping (gem5-shaped)

With `helmstats` enabled, `helm-engine::HelmSim::dump_stats(out_dir)`
produces:

```
<out_dir>/
  config.ini       # gem5-shaped INI dump of the SimObject tree
  config.json      # same data, JSON
  stats.txt        # one block per Report::deliver() call
```

`stats.txt` is generated by `HelmstatsFormatter` consuming
`&dyn helm_stats::StatsRegistry` directly (raw counters/histograms/
formulas) plus `HelmSpySnapshot` for analysis-model summaries
(insn-mix percentages, branch predictor miss rate, cache hit rate).

Without `helmstats`, none of these files are produced and the writer
code is absent from the binary.
*See [`LLD-sinks.md`](LLD-sinks.md) for full implementation detail of sinks, formatters, Report, and ReportSchedule.*
*See [`TEST.md`](TEST.md) for the complete test listing.*

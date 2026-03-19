# helm-report — Low-Level Design: Sinks, Formatters, Report, ReportSchedule

> **Status:** Implemented — 62 tests pass.
> **Crate path:** `debug/helm-report/`
> **Companion:** [`HLD.md`](HLD.md)

---

## Table of Contents

1. [Cargo.toml](#1-cargotoml)
2. [Error Types](#2-error-types)
3. [Sink Trait](#3-sink-trait)
4. [StderrSink](#4-stderrsink)
5. [FileSink](#5-filesink)
6. [AsyncFileSink](#6-asyncfilesink)
7. [TcpSink](#7-tcpsink)
8. [NullSink](#8-nullsink)
9. [BinaryTraceSink](#9-binarytracesink)
10. [PythonSink](#10-pythonsink)
11. [sink_from_uri](#11-sink_from_uri)
12. [SpySpySnapshot](#12-spyspysnapshot)
13. [ReportFormatter Trait](#13-reportformatter-trait)
14. [TextFormatter](#14-textformatter)
15. [JsonFormatter](#15-jsonformatter)
16. [CsvFormatter](#16-csvformatter)
17. [GemstatsFormatter](#17-gemstatsformatter)
18. [Report](#18-report)
19. [ReportSchedule](#19-reportschedule)

---

## 1. Cargo.toml

```toml
[package]
name    = "helm-report"
version.workspace = true
edition.workspace = true

[lints]
workspace = true

[dependencies]
bytemuck   = { version = "1", features = ["derive"] }
serde_json = "1"

[dev-dependencies]
tempfile = "3"
```

No `tokio`, no async runtime. `AsyncFileSink` and `BinaryTraceSink` use plain `std::thread`
drain loops with bounded `SyncSender` channels for back-pressure.

---

## 2. Error Types

```rust
// src/error.rs

#[derive(Debug)]
pub enum SinkError {
    /// A single sink returned an I/O error.
    Io(io::Error),
    /// Multiple sinks returned errors; deliver() continues past each failure.
    MultipleErrors(Vec<io::Error>),
    /// URI string could not be parsed into a valid sink.
    InvalidUri(String),
}
```

`SinkError` implements `std::fmt::Display` and `std::error::Error`. `From<io::Error>` is
implemented, converting to `SinkError::Io`.

---

## 3. Sink Trait

```rust
// src/sink/mod.rs

pub trait Sink: Send + Sync {
    fn write(&self, data: &[u8]) -> io::Result<()>;
    fn flush(&self) -> io::Result<()> { Ok(()) }
    fn name(&self) -> &str;
}
```

`write()` receives the complete formatted output of one `Report::deliver()` call. Partial writes
(interrupted I/O) must be retried by the implementation or returned as `Err`. The engine may call
`write()` from a thread different from the one that constructed the sink; all implementations must
be `Send + Sync`.

`flush()` has a default no-op implementation. Buffered sinks (`FileSink`, `AsyncFileSink`,
`TcpSink`) override it.

### TestSink (cfg(test) only)

An in-memory sink used across all test modules. Defined in `src/sink/mod.rs` under `#[cfg(test)]`.

```rust
pub struct TestSink {
    pub written:   Arc<Mutex<Vec<u8>>>,
    pub flushes:   Arc<Mutex<u32>>,
    pub sink_name: &'static str,
}

impl TestSink {
    pub fn new(name: &'static str) -> Self;
    pub fn contents(&self) -> Vec<u8>;
    pub fn flush_count(&self) -> u32;
    pub fn contents_as_string(&self) -> String;
}
```

---

## 4. StderrSink

```rust
// src/sink/stderr.rs

pub struct StderrSink;
```

Writes to stderr using `io::stderr().write_all(data)`. No internal buffer. Thread safety comes
from the OS-level stderr handle. `flush()` calls `io::stderr().flush()`. `name()` returns
`"stderr"`.

---

## 5. FileSink

```rust
// src/sink/file.rs

pub struct FileSink {
    inner: Mutex<BufWriter<File>>,
    path:  String,
}

impl FileSink {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self>;
}
```

`open()` calls `File::create(path)` and wraps it in a `BufWriter::with_capacity(8 * 1024, file)`.
`write()` and `flush()` acquire the `Mutex` before delegating. `name()` returns the path string.

`Drop` impl performs a best-effort flush (ignores the error on drop).

---

## 6. AsyncFileSink

```rust
// src/sink/async_file.rs

pub struct AsyncFileSink {
    tx:   SyncSender<DrainMsg>,
    name: String,
}
```

The drain thread message enum:

```rust
enum DrainMsg {
    Write(Vec<u8>),
    Flush,
    Stop,
}
```

### Construction

```rust
impl AsyncFileSink {
    const CHANNEL_BOUND: usize = 1024;
    const DRAIN_TIMEOUT_MS: u64 = 100;

    /// Open path and spawn drain thread.
    /// Returns (sink, JoinHandle). The caller may join at simulation exit.
    pub fn open(path: impl AsRef<Path>) -> io::Result<(Self, JoinHandle<()>)>;
}
```

`open()` creates the file, allocates a `SyncSender<DrainMsg>` with bound 1024, spawns a drain
thread named `"helm-report-drain:<path>"`, and returns the sink plus the `JoinHandle`. The drain
thread uses a 64 KB `BufWriter` and calls `recv_timeout(100 ms)` — it flushes on timeout, on
`Flush` message, and on `Stop` or channel disconnect.

The bound of 1024 provides back-pressure: if the drain thread falls behind by more than 1024
messages, `write()` blocks until capacity is available.

### Sink impl

`write()` sends `DrainMsg::Write(data.to_vec())`. Returns `BrokenPipe` if the drain thread has
stopped. `flush()` sends `DrainMsg::Flush`.

### Drop

`Drop` sends `DrainMsg::Stop`. This signals the drain thread to flush and exit. The caller should
join the returned `JoinHandle` to ensure the file is fully written before the process exits.

---

## 7. TcpSink

```rust
// src/sink/tcp.rs

pub struct TcpSink {
    inner: Mutex<BufWriter<TcpStream>>,
    addr:  String,
}

impl TcpSink {
    pub fn connect(addr: &str) -> io::Result<Self>;
}
```

`connect()` calls `TcpStream::connect(addr)` and wraps it in `BufWriter::with_capacity(4 * 1024, stream)`.

Connects once at construction time. Does not attempt reconnection if the connection drops —
writes return `Err(BrokenPipe)`. Create a new `TcpSink` to reconnect.

`name()` returns the address string passed to `connect()`.

---

## 8. NullSink

```rust
// src/sink/null.rs

pub struct NullSink;
```

`write()` is `#[inline(always)]` and always returns `Ok(())`. No allocation, no I/O. Used for
benchmarking formatter overhead in isolation. `name()` returns `"null"`.

---

## 9. BinaryTraceSink

```rust
// src/sink/binary.rs

pub const HELM_TRACE_MAGIC:   u32 = 0x484D_4C54;  // 'HLMT' LE
pub const HELM_TRACE_VERSION: u32 = 1;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct TraceFileHeader {
    pub magic:        u32,       // offset  0
    pub version:      u32,       // offset  4
    pub record_size:  u32,       // offset  8
    pub record_count: u32,       // offset 12  -- patched on close
    pub type_name:    [u8; 64],  // offset 16  -- null-padded ASCII
}
// sizeof(TraceFileHeader) == 80  (compile-time asserted)
```

### BinaryTraceSink<T>

```rust
pub struct BinaryTraceSink<T: Copy + Send + Pod + 'static> {
    tx:           SyncSender<BinaryMsg<T>>,
    record_count: Arc<AtomicU32>,
    name:         String,
}
```

The drain thread message enum:

```rust
enum BinaryMsg<T: Pod + Send + 'static> {
    Records(Vec<T>),
    Stop,
}
```

### Construction

```rust
impl<T: Copy + Send + Pod + 'static> BinaryTraceSink<T> {
    const CHANNEL_BOUND:    usize = 512;
    const DRAIN_INTERVAL_MS: u64 = 10;

    /// Open path, write header (record_count = 0), spawn drain thread.
    pub fn open(
        path:      impl AsRef<Path>,
        type_name: &str,
    ) -> io::Result<(Self, JoinHandle<()>)>;

    /// Push a slice of records to the drain queue.
    pub fn push_records(&self, records: &[T]) -> io::Result<()>;

    /// Approximate record count flushed so far; updated on Stop.
    pub fn record_count(&self) -> u32;
}
```

`open()` creates the file using `OpenOptions::new().write(true).create(true).truncate(true)`,
writes the 80-byte header immediately (with `record_count = 0`), then spawns the drain thread.

On `Stop` (or channel disconnect), the drain thread flushes, seeks to offset 12 in the file,
writes the final `record_count` as little-endian `u32`, and stores the count in the
`Arc<AtomicU32>`.

### Sink impl

`Sink::write()` validates that `data.len() % sizeof(T) == 0`, casts via `bytemuck::cast_slice`,
and calls `push_records()`. Returns `InvalidInput` if the length is not aligned.

### Drop

Sends `BinaryMsg::Stop` on drop.

---

## 10. PythonSink

```rust
// src/sink/python.rs

#[derive(Clone)]
pub struct PythonSink {
    buf: Arc<Mutex<Vec<String>>>,
}

impl PythonSink {
    pub fn new() -> Self;

    /// Clone the inner Arc for sharing with a PyO3 wrapper.
    pub fn inner(&self) -> Arc<Mutex<Vec<String>>>;

    /// Drain all buffered strings; clears the buffer.
    pub fn drain(&self) -> Vec<String>;
}
```

`write()` converts bytes to a `String` via `String::from_utf8_lossy` and appends it to the
`Vec<String>`. Each call to `write()` appends one entry (the full formatted output of one
`deliver()` call). `name()` returns `"python"`.

`drain()` acquires the `Mutex`, takes the current `Vec<String>` via `std::mem::take`, and returns
it — the buffer is empty after a `drain()` call.

The `Arc` can be cloned and shared with a PyO3 wrapper without holding the GIL. The `Mutex`
(not the GIL) guards the data.

---

## 11. sink_from_uri

```rust
// src/sink/uri.rs

pub fn sink_from_uri(uri: &str) -> Result<Box<dyn Sink>, SinkError>;
```

| URI | Sink constructed | Notes |
|-----|-----------------|-------|
| `"stderr:"` | `StderrSink` | Exact string match |
| `"null:"` | `NullSink` | Exact string match |
| `"file+sync:<path>"` | `FileSink` | Strips prefix `"file+sync:"` |
| `"file:<path>"` | `AsyncFileSink` | Strips prefix `"file:"`; `JoinHandle` is dropped |
| `"tcp:<host>:<port>"` | `TcpSink` | Strips prefix `"tcp:"`; validates remaining string contains `:` |

For `"file:"`, `AsyncFileSink::open()` returns `(sink, _handle)` — the `JoinHandle` is bound to
`_handle` and immediately dropped. The drain thread runs until the returned `Box<dyn Sink>` is
dropped, at which point `Drop` sends `Stop`.

For `"tcp:"`, if the remaining string after the prefix does not contain `:`, the function returns
`Err(SinkError::InvalidUri(...))` without attempting a connection. Connection failures return
`Err(SinkError::Io(...))`.

Unrecognised schemes return `Err(SinkError::InvalidUri(...))`.

---

## 12. SpySpySnapshot

```rust
// src/snapshot.rs
// NOTE: Defined here because helm-spy does not yet exist.
//       Will move to helm-spy when that crate is implemented.

#[derive(Clone, Debug)]
pub struct SpySpySnapshot {
    pub insn_count:     u64,
    pub insn_mix:       Vec<(String, u64)>,    // (class_name, count); order stable
    pub hot_pcs:        Vec<(u64, u64)>,        // (pc, count); sorted descending by count
    pub branch_heatmap: Vec<(u64, u64)>,        // (pc, count); sorted descending by count
    pub cache_l1d:      Option<CacheSnapshot>,
    pub branch_pred:    Option<BranchPredSnapshot>,
    pub fault_history:  Option<Vec<CpuFaultEvent>>,
    pub tick_count:     u64,
    pub snapshot_ns:    u64,                    // UNIX nanoseconds at snapshot time
}

#[derive(Clone, Debug)]
pub struct CacheSnapshot {
    pub name:     String,
    pub hits:     u64,
    pub misses:   u64,
    pub hit_rate: f64,   // hits / (hits + misses)
}

#[derive(Clone, Debug)]
pub struct BranchPredSnapshot {
    pub name:           String,
    pub kind:           String,    // "BiModal" | "GShare" | "Perfect"
    pub predictions:    u64,
    pub mispredictions: u64,
    pub miss_rate:      f64,       // mispredictions / predictions
}

#[derive(Clone, Debug)]
pub struct CpuFaultEvent {
    pub insn_count:  u64,
    pub pc:          u64,
    pub fault_code:  u32,
    pub description: String,
}
```

Computed methods on `SpySpySnapshot`:

```rust
impl SpySpySnapshot {
    pub fn ipc(&self) -> f64;            // insn_count / tick_count; 0.0 if tick_count == 0
    pub fn insn_mix_total(&self) -> u64; // sum of all insn_mix counts
}
```

---

## 13. ReportFormatter Trait

```rust
// src/format/mod.rs

pub trait ReportFormatter: Send + Sync {
    /// Format the entire snapshot into a byte buffer.
    fn format_session(&self, session: &SpySpySnapshot) -> Vec<u8>;

    /// Format a single named counter value (for incremental delivery).
    fn format_counter(&self, name: &str, value: u64, unit: &str) -> Vec<u8>;

    /// Format a named histogram as (bin_label, count) pairs.
    fn format_histogram(&self, name: &str, bins: &[(&str, u64)]) -> Vec<u8>;

    /// MIME type for the output of this formatter.
    fn content_type(&self) -> &'static str;
}
```

All four methods are required. Formatters are `Send + Sync` — the engine may format from a
background thread. All four concrete formatters (`TextFormatter`, `JsonFormatter`,
`CsvFormatter`, `GemstatsFormatter`) implement `Default`.

---

## 14. TextFormatter

```rust
// src/format/text.rs

#[derive(Default)]
pub struct TextFormatter;
```

Output layout: metric name left-padded to 40 characters; value right-justified in 20 characters;
optional `  # comment` suffix. Delimited by `"---------- Begin Simulation Statistics ----------"`
and `"----------  End Simulation Statistics  ----------"` lines.

`format_session()` emits, in order:
1. `sim_insns` and `sim_ticks`
2. `sim_ipc` (only if `tick_count > 0`)
3. Instruction mix lines: `insn_mix.<class>` with count and percentage
4. Cache lines (if `cache_l1d` is `Some`): `cache_<name>.hits`, `.misses`, `.hit_rate`
5. Branch predictor lines (if `branch_pred` is `Some`): `branch_pred_<name>.predictions`,
   `.mispredictions`, `.miss_rate`
6. Hot PC entries (up to 10): `hot_pcs[i]` with hex address and count

`content_type()` returns `"text/plain; charset=utf-8"`.

---

## 15. JsonFormatter

```rust
// src/format/json.rs

#[derive(Default)]
pub struct JsonFormatter;
```

Output is a single pretty-printed JSON object via `serde_json::to_vec_pretty`. Top-level keys:

| Key | Type | Notes |
|-----|------|-------|
| `helm_report_version` | integer | Always `1` |
| `timestamp_ns` | integer | `snapshot_ns` from snapshot |
| `sim_insns` | integer | |
| `sim_ticks` | integer | |
| `sim_ipc` | float | |
| `insn_mix` | array | Objects with `name`, `value`, `unit`, `pct` |
| `hot_pcs` | array | Objects with `"pc"` (hex string) and `"count"`; up to 20 |
| `cache_l1d` | object or absent | `name`, `hits`, `misses`, `hit_rate` |
| `branch_pred` | object or absent | `name`, `kind`, `predictions`, `mispredictions`, `miss_rate` |

`format_counter()` returns a JSON object with `name`, `value`, `unit`.
`format_histogram()` returns a JSON object with `name` and a `bins` array of `{bin, count}` objects.

`content_type()` returns `"application/json; charset=utf-8"`.

---

## 16. CsvFormatter

```rust
// src/format/csv.rs

#[derive(Default)]
pub struct CsvFormatter;
```

The first line is always the header row:

```
timestamp_ns,metric,value
```

Each subsequent line is one metric (but note: the row data is written as `metric,timestamp_ns,value`
— the column order in data rows matches `metric` first, then `timestamp_ns`, then `value`, which
differs from the header label order). This is the behavior as implemented in source.

`format_session()` emits rows for: `sim_insns`, `sim_ticks`, `sim_ipc`, one row per instruction
mix class (plus a `<class>.pct` percentage row), cache metrics (if present), and branch predictor
metrics (if present).

Metric names containing commas are double-quoted. Floating-point values use 6 decimal places
(IPC, hit_rate, miss_rate); percentages use 2 decimal places.

`format_counter()` emits `<name>,0,<value>`. `format_histogram()` emits one line per bin:
`<name>.<label>,0,<count>`.

`content_type()` returns `"text/csv; charset=utf-8"`.

---

## 17. GemstatsFormatter

```rust
// src/format/gemstats.rs

#[derive(Default)]
pub struct GemstatsFormatter;
```

Emits gem5-compatible `stats.txt` output. Column layout: name in columns 0–39, value in columns
40–59, comment from column 60. Uses the gem5 metric naming scheme.

`format_session()` emits, in order:
1. Begin marker
2. `sim_insns`, `sim_ticks`, `sim_freq` (hardcoded `1000000000`)
3. `system.cpu.committedInsts`, `system.cpu.ipc`
4. `system.cpu.op_class_0::<class>` with count and percentage for each mix class
5. `system.cpu.dcache.overall_hits::total`, `.overall_misses::total`,
   `.overall_miss_rate::total` (if `cache_l1d` is `Some`; miss_rate = `1.0 - hit_rate`)
6. `system.cpu.branchPred.lookups`, `.mispredicts` (if `branch_pred` is `Some`)
7. End marker

`content_type()` returns `"text/plain; charset=utf-8"`.

---

## 18. Report

```rust
// src/report.rs

pub struct Report {
    session:   Arc<SpySpySnapshot>,
    formatter: Box<dyn ReportFormatter>,
    sinks:     Vec<Box<dyn Sink>>,
}
```

### Methods

```rust
impl Report {
    pub fn new(
        session:   Arc<SpySpySnapshot>,
        formatter: Box<dyn ReportFormatter>,
        sinks:     Vec<Box<dyn Sink>>,
    ) -> Self;

    pub fn deliver(&self) -> Result<(), SinkError>;

    pub fn deliver_to(&self, sink: &dyn Sink) -> Result<(), SinkError>;

    pub fn flush_all(&self) -> Result<(), SinkError>;
}
```

### deliver() implementation detail

```
1. Call formatter.format_session(&self.session) -> Vec<u8>  (exactly once)
2. For each sink in self.sinks:
     if sink.write(&data).is_err() { errors.push(e) }
3. Call flush_all_inner(&mut errors)  -- flush all sinks, accumulate errors
4. If errors.is_empty() -> Ok(())
   Else if errors.len() == 1 -> Err(SinkError::Io(errors.remove(0)))
   Else -> Err(SinkError::MultipleErrors(errors))
```

A failure from one sink does NOT short-circuit delivery to subsequent sinks.

`deliver_to()` formats once, calls `sink.write(&data)`, then `sink.flush()`. It does not touch
the permanent sink list.

`flush_all()` delegates to `flush_all_inner()` — iterates all sinks, calls `flush()`, accumulates
errors, and returns the same `Ok/Io/MultipleErrors` pattern.

---

## 19. ReportSchedule

```rust
// src/schedule.rs

#[derive(Debug, Clone)]
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
    last_delivered_at: u64,
}
```

### Methods

```rust
impl ReportSchedule {
    pub fn new(report: Report, triggers: Vec<ReportTrigger>) -> Self;
    pub fn check(&mut self, pc: u64, insn_count: u64);
    pub fn flush_at_exit(&self);
    pub fn deliver(&self) -> Result<(), SinkError>;
}
```

### check() logic

Iterates all triggers. For `EveryNInsns(n)`: fires if `n > 0 && insn_count > 0 && (insn_count / n) > (last_delivered_at / n)`. For `OnPc(addr)`: fires if `pc == addr`. `AtExit`, `Explicit`, and `OnCounter` do not fire in `check()` (handled by `_ => {}`).

When any trigger fires, `last_delivered_at` is set to `insn_count` and `self.report.deliver()` is called (errors ignored).

### flush_at_exit()

Checks if any trigger is `AtExit`. If so, calls `self.report.deliver()` once (error ignored).

### deliver()

Delegates to `self.report.deliver()`. Used for `Explicit` trigger invocation.

---

*See [`HLD.md`](HLD.md) for architecture overview and design decisions.*
*See [`TEST.md`](TEST.md) for the complete test listing.*

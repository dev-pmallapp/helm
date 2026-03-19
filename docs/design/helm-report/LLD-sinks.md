# helm-report — Low-Level Design: Sinks, Formatters, Report, ReportSchedule

> **Status:** Draft — Phase 3 (Delivery layer)
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
12. [SpySpySnapshot](#12-observesessionsnapshot)
13. [ReportFormatter Trait](#13-reportformatter-trait)
14. [TextFormatter](#14-textformatter)
15. [JsonFormatter](#15-jsonformatter)
16. [CsvFormatter](#16-csvformatter)
17. [GemstatsFormatter](#17-gemstatsformatter)
18. [Report](#18-report)
19. [ReportSchedule](#19-reportschedule)
20. [lib.rs Re-exports](#20-librs-re-exports)

---

## 1. Cargo.toml

```toml
[package]
name    = "helm-report"
version = "0.1.0"
edition = "2021"

[dependencies]
helm-spy = { path = "../helm-spy" }
bytemuck     = { version = "1", features = ["derive"] }
serde_json   = "1"

[dev-dependencies]
tempfile = "3"
```

No `tokio`, no async runtime. `AsyncFileSink` uses a plain `std::thread` drain loop.

---

## 2. Error Types

```rust
// src/error.rs

use std::io;

#[derive(Debug)]
pub enum SinkError {
    /// A single sink returned an I/O error.
    Io(io::Error),
    /// Multiple sinks returned errors; deliver() continues past each failure.
    MultipleErrors(Vec<io::Error>),
    /// URI string could not be parsed into a valid sink.
    InvalidUri(String),
}

impl std::fmt::Display for SinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SinkError::Io(e)              => write!(f, "sink I/O error: {e}"),
            SinkError::MultipleErrors(es) => {
                write!(f, "{} sink error(s):", es.len())?;
                for e in es { write!(f, "\n  - {e}")?; }
                Ok(())
            }
            SinkError::InvalidUri(uri)    => write!(f, "invalid sink URI: {uri:?}"),
        }
    }
}

impl std::error::Error for SinkError {}

impl From<io::Error> for SinkError {
    fn from(e: io::Error) -> Self { SinkError::Io(e) }
}
```

---

## 3. Sink Trait

```rust
// src/sink/mod.rs

use std::io;

/// A delivery destination for report data.
///
/// Implementations MUST be `Send + Sync` — the engine may deliver from
/// a background thread or from a different thread than the one that created
/// the sink.
///
/// `write()` receives fully-formatted bytes. The sink is responsible for
/// internal buffering. The engine calls `flush()` after a logical report
/// boundary; sinks that do not buffer may return `Ok(())` from `flush()`.
///
/// `write()` is called with the complete formatted output of one `Report::deliver()`.
/// Partial writes (interrupted I/O) MUST be retried or returned as `Err`.
pub trait Sink: Send + Sync {
    fn write(&self, data: &[u8]) -> io::Result<()>;
    fn flush(&self) -> io::Result<()> { Ok(()) }
    fn name(&self) -> &str;
}

pub use super::sink::{
    stderr::StderrSink,
    file::FileSink,
    async_file::AsyncFileSink,
    tcp::TcpSink,
    null::NullSink,
    binary::BinaryTraceSink,
    python::PythonSink,
    uri::sink_from_uri,
};
```

---

## 4. StderrSink

```rust
// src/sink/stderr.rs

use std::io::{self, Write};
use super::Sink;

/// Writes to stderr. No buffering. Always available.
///
/// Thread safety: `io::stderr()` is inherently synchronized on all major
/// platforms (POSIX: stderr is unbuffered). No Mutex needed.
pub struct StderrSink;

impl Sink for StderrSink {
    fn write(&self, data: &[u8]) -> io::Result<()> {
        io::stderr().write_all(data)
    }

    fn flush(&self) -> io::Result<()> {
        io::stderr().flush()
    }

    fn name(&self) -> &str { "stderr" }
}
```

---

## 5. FileSink

```rust
// src/sink/file.rs

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;
use super::Sink;

/// Synchronous buffered file sink.
///
/// Uses an 8 KB `BufWriter`. The `Mutex` makes `write()` safe to call from
/// multiple threads, though concurrent writes from unrelated reports are not
/// expected in practice.
///
/// Prefer `AsyncFileSink` when the caller cannot afford to block on I/O.
pub struct FileSink {
    inner: Mutex<BufWriter<File>>,
    path:  String,
}

impl FileSink {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let file = File::create(path)?;
        Ok(FileSink {
            inner: Mutex::new(BufWriter::with_capacity(8 * 1024, file)),
            path:  path.to_string_lossy().into_owned(),
        })
    }
}

impl Sink for FileSink {
    fn write(&self, data: &[u8]) -> io::Result<()> {
        let mut guard = self.inner.lock().unwrap();
        guard.write_all(data)
    }

    fn flush(&self) -> io::Result<()> {
        self.inner.lock().unwrap().flush()
    }

    fn name(&self) -> &str { &self.path }
}

impl Drop for FileSink {
    fn drop(&mut self) {
        // Best-effort flush; ignore error on drop.
        let _ = self.inner.lock().unwrap().flush();
    }
}
```

---

## 6. AsyncFileSink

`AsyncFileSink` sends formatted bytes to a background drain thread via a bounded `SyncSender`.
The simulation thread never blocks waiting for disk I/O. The drain thread owns the `BufWriter`
and does not hold any lock shared with the simulation path.

```rust
// src/sink/async_file.rs

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::mpsc::{self, SyncSender};
use std::thread::{self, JoinHandle};
use super::Sink;

/// Message sent from the sink to the drain thread.
enum DrainMsg {
    Write(Vec<u8>),
    Flush,
    Stop,
}

/// Asynchronous file sink. Writes are queued to a background drain thread.
///
/// The `SyncSender` with bound 1024 provides bounded memory: if the drain
/// thread falls behind by more than 1024 messages the `write()` call will
/// block — which is the correct back-pressure behavior.
///
/// Call `join()` to wait for the drain thread to finish flushing. If the sink
/// is dropped without calling `join()`, the drain thread is stopped and
/// remaining queued messages are written before the file is closed.
pub struct AsyncFileSink {
    tx:   SyncSender<DrainMsg>,
    name: String,
}

impl AsyncFileSink {
    const CHANNEL_BOUND: usize = 1024;
    const DRAIN_TIMEOUT_MS: u64 = 100;

    /// Open `path` and spawn a drain thread.
    ///
    /// Returns `(sink, join_handle)`. The caller may store the handle and call
    /// `join_handle.join()` at simulation exit to ensure the file is fully written.
    pub fn open(path: impl AsRef<Path>) -> io::Result<(Self, JoinHandle<()>)> {
        let path = path.as_ref();
        let file = File::create(path)?;
        let name = path.to_string_lossy().into_owned();
        let (tx, rx) = mpsc::sync_channel::<DrainMsg>(Self::CHANNEL_BOUND);
        let mut writer = BufWriter::with_capacity(64 * 1024, file);

        let handle = thread::Builder::new()
            .name(format!("helm-report-drain:{name}"))
            .spawn(move || {
                loop {
                    match rx.recv_timeout(
                        std::time::Duration::from_millis(Self::DRAIN_TIMEOUT_MS),
                    ) {
                        Ok(DrainMsg::Write(data)) => {
                            // Ignore write errors in the drain thread; they surface via flush.
                            let _ = writer.write_all(&data);
                        }
                        Ok(DrainMsg::Flush) => {
                            let _ = writer.flush();
                        }
                        Ok(DrainMsg::Stop) => {
                            let _ = writer.flush();
                            break;
                        }
                        // Timeout: flush what we have so far.
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let _ = writer.flush();
                        }
                        // Sender disconnected: drain remaining messages, then stop.
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            let _ = writer.flush();
                            break;
                        }
                    }
                }
            })
            .expect("failed to spawn drain thread");

        Ok((AsyncFileSink { tx, name }, handle))
    }
}

impl Sink for AsyncFileSink {
    fn write(&self, data: &[u8]) -> io::Result<()> {
        self.tx
            .send(DrainMsg::Write(data.to_vec()))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "drain thread stopped"))
    }

    fn flush(&self) -> io::Result<()> {
        self.tx
            .send(DrainMsg::Flush)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "drain thread stopped"))
    }

    fn name(&self) -> &str { &self.name }
}

impl Drop for AsyncFileSink {
    fn drop(&mut self) {
        // Signal drain thread to stop and flush remaining data.
        // The drain thread will write all queued messages before exiting.
        let _ = self.tx.send(DrainMsg::Stop);
    }
}
```

**Drain thread invariants:**

- `CHANNEL_BOUND = 1024` means at most 1024 × (average message size) bytes are buffered
  in the channel at any time before `write()` blocks.
- `DRAIN_TIMEOUT_MS = 100`: the drain thread flushes the `BufWriter` every 100 ms even
  if no `Flush` message arrives. This prevents stale data from sitting in the kernel buffer.
- On `Stop`: the drain thread processes all remaining messages before breaking. The OS
  closes the file descriptor cleanly when `BufWriter<File>` is dropped at end of `drain()`.
- If the drain thread panics, subsequent `write()` calls return
  `Err(BrokenPipe)`. The caller's `deliver()` collects this as a `SinkError`.

---

## 7. TcpSink

```rust
// src/sink/tcp.rs

use std::io::{self, BufWriter, Write};
use std::net::TcpStream;
use std::sync::Mutex;
use super::Sink;

/// Buffered TCP stream sink.
///
/// Connects once at construction time. If the connection drops, writes return
/// `Err(BrokenPipe)` — the sink does NOT attempt to reconnect. The caller
/// should create a new `TcpSink` if reconnection is desired.
///
/// Buffer size: 4 KB. Flushed after every `flush()` call (which `Report::deliver()`
/// calls after writing to all sinks).
pub struct TcpSink {
    inner: Mutex<BufWriter<TcpStream>>,
    addr:  String,
}

impl TcpSink {
    pub fn connect(addr: &str) -> io::Result<Self> {
        let stream = TcpStream::connect(addr)?;
        Ok(TcpSink {
            inner: Mutex::new(BufWriter::with_capacity(4 * 1024, stream)),
            addr:  addr.to_owned(),
        })
    }
}

impl Sink for TcpSink {
    fn write(&self, data: &[u8]) -> io::Result<()> {
        self.inner.lock().unwrap().write_all(data)
    }

    fn flush(&self) -> io::Result<()> {
        self.inner.lock().unwrap().flush()
    }

    fn name(&self) -> &str { &self.addr }
}
```

---

## 8. NullSink

```rust
// src/sink/null.rs

use std::io;
use super::Sink;

/// Discards all writes. Used for benchmarking formatter overhead in isolation.
///
/// `write()` and `flush()` are always `Ok(())`. No allocation, no I/O.
pub struct NullSink;

impl Sink for NullSink {
    #[inline(always)]
    fn write(&self, _data: &[u8]) -> io::Result<()> { Ok(()) }

    fn name(&self) -> &str { "null" }
}
```

---

## 9. BinaryTraceSink

`BinaryTraceSink<T>` writes a typed binary trace file with a fixed 80-byte header followed
by packed records of type `T`. The drain loop runs in a background thread at a 10 ms interval.

### 9.1 Header layout (Rust)

```rust
// src/sink/binary.rs  — header definition

use bytemuck::{Pod, Zeroable};

pub const HELM_TRACE_MAGIC:   u32 = 0x484D_4C54;  // 'HLMT' LE
pub const HELM_TRACE_VERSION: u32 = 1;

/// 80-byte file header written at offset 0.
/// `record_count` is zero at open time; written with the final count on close.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct TraceFileHeader {
    pub magic:        u32,        //  0
    pub version:      u32,        //  4
    pub record_size:  u32,        //  8
    pub record_count: u32,        // 12  — patched on close
    pub type_name:    [u8; 64],   // 16  — null-padded ASCII
}                                 // total: 80 bytes

static_assert_size!(TraceFileHeader, 80);

impl TraceFileHeader {
    pub fn new<T>(type_name: &str) -> Self {
        let mut hdr = TraceFileHeader::zeroed();
        hdr.magic       = HELM_TRACE_MAGIC;
        hdr.version     = HELM_TRACE_VERSION;
        hdr.record_size = std::mem::size_of::<T>() as u32;
        let bytes = type_name.as_bytes();
        let len   = bytes.len().min(63);
        hdr.type_name[..len].copy_from_slice(&bytes[..len]);
        hdr
    }
}
```

### 9.2 BinaryTraceSink implementation

```rust
// src/sink/binary.rs  (continued)

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    mpsc::{self, SyncSender},
    Arc,
};
use std::thread::{self, JoinHandle};
use bytemuck::Pod;

enum BinaryMsg<T: Pod + Send + 'static> {
    Records(Vec<T>),
    Stop,
}

/// Typed binary trace sink. Drains records to a binary file in a background thread.
///
/// # Type constraints
/// `T: Copy + Send + Pod + 'static`
/// - `Copy`: drain loop copies records out of the channel message.
/// - `Pod`: safe byte cast via `bytemuck::cast_slice` (no `unsafe` in drain loop).
/// - `Send`: records are moved across thread boundary.
pub struct BinaryTraceSink<T: Copy + Send + Pod + 'static> {
    tx:           SyncSender<BinaryMsg<T>>,
    record_count: Arc<AtomicU32>,
    name:         String,
}

impl<T: Copy + Send + Pod + 'static> BinaryTraceSink<T> {
    const CHANNEL_BOUND: usize = 512;
    const DRAIN_INTERVAL_MS: u64 = 10;

    /// Open `path`, write the header, and spawn the drain thread.
    pub fn open(
        path:      impl AsRef<Path>,
        type_name: &str,
    ) -> io::Result<(Self, JoinHandle<()>)> {
        let path = path.as_ref();
        let name = path.to_string_lossy().into_owned();

        // Write header (record_count = 0; will be patched on Stop).
        let mut file = OpenOptions::new().write(true).create(true).truncate(true).open(path)?;
        let hdr = TraceFileHeader::new::<T>(type_name);
        file.write_all(bytemuck::bytes_of(&hdr))?;

        let (tx, rx) = mpsc::sync_channel::<BinaryMsg<T>>(Self::CHANNEL_BOUND);
        let record_count = Arc::new(AtomicU32::new(0));
        let rc_clone = Arc::clone(&record_count);
        let path_clone = path.to_path_buf();

        let handle = thread::Builder::new()
            .name(format!("helm-report-binary:{name}"))
            .spawn(move || {
                let mut writer = BufWriter::with_capacity(256 * 1024, file);
                let mut count: u32 = 0;

                loop {
                    match rx.recv_timeout(
                        std::time::Duration::from_millis(Self::DRAIN_INTERVAL_MS),
                    ) {
                        Ok(BinaryMsg::Records(recs)) => {
                            let bytes = bytemuck::cast_slice(&recs);
                            let _ = writer.write_all(bytes);
                            count += recs.len() as u32;
                        }
                        Ok(BinaryMsg::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            let _ = writer.flush();
                            // Patch record_count at header offset 12.
                            if let Ok(mut raw) = OpenOptions::new().write(true).open(&path_clone) {
                                let _ = raw.seek(SeekFrom::Start(12));
                                let _ = raw.write_all(&count.to_le_bytes());
                            }
                            rc_clone.store(count, Ordering::Relaxed);
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let _ = writer.flush();
                        }
                    }
                }
            })
            .expect("failed to spawn binary drain thread");

        Ok((BinaryTraceSink { tx, record_count, name }, handle))
    }

    /// Push a slice of records to the drain queue.
    pub fn push_records(&self, records: &[T]) -> io::Result<()> {
        self.tx
            .send(BinaryMsg::Records(records.to_vec()))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "binary drain stopped"))
    }

    /// Return the number of records flushed so far (approximate; updated on Stop).
    pub fn record_count(&self) -> u32 {
        self.record_count.load(Ordering::Relaxed)
    }
}

impl<T: Copy + Send + Pod + 'static> Sink for BinaryTraceSink<T> {
    /// For use as a generic `Sink`: treat `data` as raw bytes and write directly.
    /// Normally callers use `push_records()` instead.
    fn write(&self, data: &[u8]) -> io::Result<()> {
        // Wrap raw bytes as a record slice if alignment matches; otherwise reject.
        let record_size = std::mem::size_of::<T>();
        if data.len() % record_size != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "data length not a multiple of record size",
            ));
        }
        // Safety: checked alignment above; T: Pod guarantees any bit pattern is valid.
        let records: &[T] = bytemuck::cast_slice(data);
        self.push_records(records)
    }

    fn name(&self) -> &str { &self.name }
}

impl<T: Copy + Send + Pod + 'static> Drop for BinaryTraceSink<T> {
    fn drop(&mut self) {
        let _ = self.tx.send(BinaryMsg::Stop);
    }
}
```

### 9.3 Canonical BranchRecord

```rust
/// 32-byte branch event record (repr(C), Pod).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BranchRecord {
    pub pc:         u64,   //  0  instruction address
    pub target:     u64,   //  8  branch target
    pub insn_count: u64,   // 16  instruction count at branch
    pub flags:      u8,    // 24  bit 0: taken; bits 2..4: BranchKind
    pub _pad:       [u8; 7], // 25
}                           // total: 32 bytes
```

---

## 10. PythonSink

`PythonSink` buffers formatted lines in a `Vec<String>` protected by a `Mutex`. Python
polls `read_lines()` from PyO3. **The GIL is NOT held** when the Rust simulation writes
to the sink — the `Mutex` is the only synchronization primitive.

```rust
// src/sink/python.rs

use std::io;
use std::sync::{Arc, Mutex};
use super::Sink;

/// GIL-safe sink that buffers lines for Python consumption.
///
/// Pattern:
/// - Rust side: `write()` appends the formatted string to a `Vec<String>`.
/// - Python side: `PyPythonSink::read_lines()` acquires the Mutex, drains the Vec,
///   and returns the lines as a `Vec<String>` to Python.
///
/// The `Arc<Mutex<Vec<String>>>` can be cloned and shared with the PyO3 wrapper
/// without holding the GIL. This is safe because `Mutex` (not the GIL) guards the data.
#[derive(Clone)]
pub struct PythonSink {
    buf: Arc<Mutex<Vec<String>>>,
}

impl PythonSink {
    pub fn new() -> Self {
        PythonSink { buf: Arc::new(Mutex::new(Vec::new())) }
    }

    /// Take a reference to the inner buffer for sharing with a PyO3 wrapper.
    pub fn inner(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.buf)
    }

    /// Drain all buffered lines; returns them to the caller and clears the buffer.
    pub fn drain(&self) -> Vec<String> {
        let mut guard = self.buf.lock().unwrap();
        std::mem::take(&mut *guard)
    }
}

impl Default for PythonSink {
    fn default() -> Self { PythonSink::new() }
}

impl Sink for PythonSink {
    fn write(&self, data: &[u8]) -> io::Result<()> {
        let s = String::from_utf8_lossy(data).into_owned();
        self.buf.lock().unwrap().push(s);
        Ok(())
    }

    fn name(&self) -> &str { "python" }
}
```

**PyO3 binding pattern:**

```rust
// helm-python/src/observe.rs

#[pyclass]
pub struct PyPythonSink {
    inner: Arc<Mutex<Vec<String>>>,
}

#[pymethods]
impl PyPythonSink {
    /// Return and clear all buffered lines. Call after sim.run().
    fn read_lines(&self) -> Vec<String> {
        let mut guard = self.inner.lock().unwrap();
        std::mem::take(&mut *guard)
    }
}
```

---

## 11. sink_from_uri

```rust
// src/sink/uri.rs

use std::io;
use super::{
    AsyncFileSink, FileSink, NullSink, Sink, StderrSink, TcpSink,
};
use crate::error::SinkError;

/// Construct a `Box<dyn Sink>` from a URI string.
///
/// Supported URI schemes:
///
/// | URI                        | Sink             |
/// |----------------------------|------------------|
/// | `stderr:`                  | `StderrSink`     |
/// | `null:`                    | `NullSink`       |
/// | `file:/absolute/path`      | `AsyncFileSink`  |
/// | `file+sync:/absolute/path` | `FileSink`       |
/// | `tcp:host:port`            | `TcpSink`        |
///
/// Returns `Err(SinkError::InvalidUri)` for unrecognised or malformed URIs.
/// Returns `Err(SinkError::Io)` if the sink cannot be opened (file create
/// failure, TCP connect failure).
pub fn sink_from_uri(uri: &str) -> Result<Box<dyn Sink>, SinkError> {
    if uri == "stderr:" {
        return Ok(Box::new(StderrSink));
    }
    if uri == "null:" {
        return Ok(Box::new(NullSink));
    }
    if let Some(path) = uri.strip_prefix("file+sync:/") {
        let path = format!("/{path}");
        let sink = FileSink::open(&path).map_err(SinkError::Io)?;
        return Ok(Box::new(sink));
    }
    if let Some(path) = uri.strip_prefix("file:/") {
        let path = format!("/{path}");
        let (sink, _handle) = AsyncFileSink::open(&path).map_err(SinkError::Io)?;
        return Ok(Box::new(sink));
    }
    if let Some(addr) = uri.strip_prefix("tcp:") {
        // Validate: must contain at least one colon separating host and port.
        if !addr.contains(':') {
            return Err(SinkError::InvalidUri(format!(
                "tcp: URI must be tcp:host:port, got {uri:?}"
            )));
        }
        let sink = TcpSink::connect(addr).map_err(SinkError::Io)?;
        return Ok(Box::new(sink));
    }
    Err(SinkError::InvalidUri(format!("unrecognised URI scheme in {uri:?}")))
}
```

---

## 12. SpySpySnapshot

```rust
// src/snapshot.rs

/// Immutable point-in-time copy of SpySession state.
///
/// Created by `SpySession::snapshot()` on the cold path.
/// All `AtomicU64` fields are copied with `Ordering::Relaxed` — acceptable
/// because the snapshot is a best-effort view, not a strict serialization point.
///
/// Owned by `Report` via `Arc<SpySpySnapshot>`; shared cheaply across
/// multiple sinks within one `deliver()` call.
#[derive(Clone, Debug)]
pub struct SpySpySnapshot {
    // Instruction mix
    pub insn_count: u64,
    pub insn_mix:   Vec<(String, u64)>,  // (class_name, count), order stable

    // Hot PC heatmap (top-N PCs by visit count)
    pub hot_pcs:        Vec<(u64, u64)>,  // (pc, count), sorted descending by count

    // Branch heatmap (top-N branch sites)
    pub branch_heatmap: Vec<(u64, u64)>,  // (pc, count), sorted descending by count

    // Optional subsystems
    pub cache_l1d:    Option<CacheSnapshot>,
    pub branch_pred:  Option<BranchPredSnapshot>,
    pub fault_history: Option<Vec<CpuFaultEvent>>,

    // Timing
    pub tick_count:   u64,
    pub snapshot_ns:  u64,  // UNIX nanoseconds (wall clock) at snapshot time
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
    pub kind:           String,  // "BiModal" | "GShare" | "Perfect"
    pub predictions:    u64,
    pub mispredictions: u64,
    pub miss_rate:      f64,     // mispredictions / predictions
}

#[derive(Clone, Debug)]
pub struct CpuFaultEvent {
    pub insn_count: u64,
    pub pc:         u64,
    pub fault_code: u32,
    pub description: String,
}

impl SpySpySnapshot {
    /// Compute IPC from the snapshot fields. Returns 0.0 if tick_count == 0.
    pub fn ipc(&self) -> f64 {
        if self.tick_count == 0 { 0.0 }
        else { self.insn_count as f64 / self.tick_count as f64 }
    }

    /// Total instruction count across all mix classes. Should equal insn_count.
    pub fn insn_mix_total(&self) -> u64 {
        self.insn_mix.iter().map(|(_, c)| c).sum()
    }
}
```

---

## 13. ReportFormatter Trait

```rust
// src/format/mod.rs

use crate::snapshot::SpySpySnapshot;

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

pub use super::format::{
    text::TextFormatter,
    json::JsonFormatter,
    csv::CsvFormatter,
    gemstats::GemstatsFormatter,
};
```

---

## 14. TextFormatter

```rust
// src/format/text.rs

use std::fmt::Write;
use super::ReportFormatter;
use crate::snapshot::SpySpySnapshot;

/// Human-readable gem5-style text output.
///
/// Column width: metric name left-padded to 40 chars, value right-justified in 20 chars.
/// Percentages appended for instruction mix lines.
#[derive(Default)]
pub struct TextFormatter;

const SEP: &str = "---------- Begin Simulation Statistics ----------";
const SEP_END: &str = "----------  End Simulation Statistics  ----------";

impl TextFormatter {
    fn format_metric(out: &mut String, name: &str, value: impl std::fmt::Display, comment: &str) {
        let comment_part = if comment.is_empty() {
            String::new()
        } else {
            format!("  # {comment}")
        };
        let _ = writeln!(out, "{name:<40}{value:>20}{comment_part}");
    }
}

impl ReportFormatter for TextFormatter {
    fn format_session(&self, s: &SpySpySnapshot) -> Vec<u8> {
        let mut out = String::with_capacity(2048);
        out.push_str(SEP);
        out.push('\n');

        Self::format_metric(&mut out, "sim_insns",  s.insn_count, "Instructions retired");
        Self::format_metric(&mut out, "sim_ticks",  s.tick_count, "Ticks simulated");
        if s.tick_count > 0 {
            Self::format_metric(
                &mut out, "sim_ipc",
                format!("{:.6}", s.ipc()),
                "Instructions per cycle",
            );
        }

        // Instruction mix
        let total = s.insn_mix_total().max(1);
        for (class, count) in &s.insn_mix {
            let pct = 100.0 * (*count as f64) / (total as f64);
            let name = format!("insn_mix.{class}");
            let val  = format!("{count:>12}  {pct:>6.2}%");
            let _ = writeln!(out, "{name:<40}{val}");
        }

        // Cache
        if let Some(ref c) = s.cache_l1d {
            let prefix = format!("cache_{}", c.name);
            Self::format_metric(&mut out, &format!("{prefix}.hits"),     c.hits,   "");
            Self::format_metric(&mut out, &format!("{prefix}.misses"),   c.misses, "");
            Self::format_metric(&mut out, &format!("{prefix}.hit_rate"),
                format!("{:.6}", c.hit_rate), "");
        }

        // Branch predictor
        if let Some(ref bp) = s.branch_pred {
            let prefix = format!("branch_pred_{}", bp.name);
            Self::format_metric(&mut out, &format!("{prefix}.predictions"),    bp.predictions,    "");
            Self::format_metric(&mut out, &format!("{prefix}.mispredictions"), bp.mispredictions, "");
            Self::format_metric(&mut out, &format!("{prefix}.miss_rate"),
                format!("{:.6}", bp.miss_rate), "");
        }

        // Hot PCs (top 10)
        for (i, (pc, count)) in s.hot_pcs.iter().take(10).enumerate() {
            let name = format!("hot_pcs[{i}]");
            let val  = format!("{pc:#018x}  count={count}");
            let _ = writeln!(out, "{name:<40}{val}");
        }

        out.push_str(SEP_END);
        out.push('\n');
        out.into_bytes()
    }

    fn format_counter(&self, name: &str, value: u64, unit: &str) -> Vec<u8> {
        format!("{name:<40}{value:>20}  # {unit}\n").into_bytes()
    }

    fn format_histogram(&self, name: &str, bins: &[(&str, u64)]) -> Vec<u8> {
        let mut out = String::new();
        for (label, count) in bins {
            let key = format!("{name}.{label}");
            let _ = writeln!(out, "{key:<40}{count:>20}");
        }
        out.into_bytes()
    }

    fn content_type(&self) -> &'static str { "text/plain; charset=utf-8" }
}
```

---

## 15. JsonFormatter

```rust
// src/format/json.rs

use super::ReportFormatter;
use crate::snapshot::SpySpySnapshot;
use serde_json::{json, to_vec_pretty};

/// Structured JSON formatter.
///
/// Output is a single JSON object. All integer values are JSON numbers.
/// Floating-point fields (ipc, hit_rate, miss_rate) are JSON numbers (f64).
#[derive(Default)]
pub struct JsonFormatter;

impl ReportFormatter for JsonFormatter {
    fn format_session(&self, s: &SpySpySnapshot) -> Vec<u8> {
        let total = s.insn_mix_total().max(1);
        let mix: Vec<_> = s.insn_mix.iter().map(|(class, count)| {
            json!({
                "name":  format!("insn_mix.{class}"),
                "value": count,
                "unit":  "instructions",
                "pct":   (100.0 * (*count as f64) / (total as f64))
            })
        }).collect();

        let hot_pcs: Vec<_> = s.hot_pcs.iter().take(20).map(|(pc, count)| {
            json!({ "pc": format!("{pc:#x}"), "count": count })
        }).collect();

        let mut obj = json!({
            "helm_report_version": 1,
            "timestamp_ns": s.snapshot_ns,
            "sim_insns": s.insn_count,
            "sim_ticks": s.tick_count,
            "sim_ipc":   s.ipc(),
            "insn_mix":  mix,
            "hot_pcs":   hot_pcs,
        });

        if let Some(ref c) = s.cache_l1d {
            obj["cache_l1d"] = json!({
                "name":     c.name,
                "hits":     c.hits,
                "misses":   c.misses,
                "hit_rate": c.hit_rate,
            });
        }

        if let Some(ref bp) = s.branch_pred {
            obj["branch_pred"] = json!({
                "name":           bp.name,
                "kind":           bp.kind,
                "predictions":    bp.predictions,
                "mispredictions": bp.mispredictions,
                "miss_rate":      bp.miss_rate,
            });
        }

        to_vec_pretty(&obj).unwrap_or_else(|_| b"{}".to_vec())
    }

    fn format_counter(&self, name: &str, value: u64, unit: &str) -> Vec<u8> {
        let obj = json!({ "name": name, "value": value, "unit": unit });
        to_vec_pretty(&obj).unwrap_or_default()
    }

    fn format_histogram(&self, name: &str, bins: &[(&str, u64)]) -> Vec<u8> {
        let bins: Vec<_> = bins.iter().map(|(l, c)| json!({ "bin": l, "count": c })).collect();
        let obj = json!({ "name": name, "bins": bins });
        to_vec_pretty(&obj).unwrap_or_default()
    }

    fn content_type(&self) -> &'static str { "application/json; charset=utf-8" }
}
```

---

## 16. CsvFormatter

```rust
// src/format/csv.rs

use super::ReportFormatter;
use crate::snapshot::SpySpySnapshot;

/// CSV formatter: `timestamp_ns,metric,value` lines.
///
/// The first line is always the header row. Subsequent lines are one metric per row.
/// Floating-point values are formatted with 6 decimal places.
/// All values are plain text; no quoting unless the metric name contains a comma.
#[derive(Default)]
pub struct CsvFormatter;

impl ReportFormatter for CsvFormatter {
    fn format_session(&self, s: &SpySpySnapshot) -> Vec<u8> {
        let mut out = String::with_capacity(1024);
        let ts = s.snapshot_ns;

        out.push_str("timestamp_ns,metric,value\n");

        let mut row = |metric: &str, value: &str| {
            // Quote metric name if it contains a comma.
            if metric.contains(',') {
                out.push('"');
                out.push_str(metric);
                out.push('"');
            } else {
                out.push_str(metric);
            }
            out.push(',');
            out.push_str(&ts.to_string());
            out.push(',');
            out.push_str(value);
            out.push('\n');
        };

        row("sim_insns", &s.insn_count.to_string());
        row("sim_ticks", &s.tick_count.to_string());
        row("sim_ipc",   &format!("{:.6}", s.ipc()));

        let total = s.insn_mix_total().max(1);
        for (class, count) in &s.insn_mix {
            row(&format!("insn_mix.{class}"), &count.to_string());
            let pct = 100.0 * (*count as f64) / (total as f64);
            row(&format!("insn_mix.{class}.pct"), &format!("{pct:.2}"));
        }

        if let Some(ref c) = s.cache_l1d {
            row(&format!("cache_{}.hits",     c.name), &c.hits.to_string());
            row(&format!("cache_{}.misses",   c.name), &c.misses.to_string());
            row(&format!("cache_{}.hit_rate", c.name), &format!("{:.6}", c.hit_rate));
        }

        if let Some(ref bp) = s.branch_pred {
            row(&format!("branch_pred_{}.predictions",    bp.name), &bp.predictions.to_string());
            row(&format!("branch_pred_{}.mispredictions", bp.name), &bp.mispredictions.to_string());
            row(&format!("branch_pred_{}.miss_rate",      bp.name), &format!("{:.6}", bp.miss_rate));
        }

        out.into_bytes()
    }

    fn format_counter(&self, name: &str, value: u64, _unit: &str) -> Vec<u8> {
        format!("{name},0,{value}\n").into_bytes()
    }

    fn format_histogram(&self, name: &str, bins: &[(&str, u64)]) -> Vec<u8> {
        let mut out = String::new();
        for (label, count) in bins {
            out.push_str(&format!("{name}.{label},0,{count}\n"));
        }
        out.into_bytes()
    }

    fn content_type(&self) -> &'static str { "text/csv; charset=utf-8" }
}
```

---

## 17. GemstatsFormatter

```rust
// src/format/gemstats.rs

use std::fmt::Write;
use super::ReportFormatter;
use crate::snapshot::SpySpySnapshot;

/// gem5-compatible `stats.txt` formatter.
///
/// Column alignment: name in column 0..39, value in column 40..59,
/// comment starting at column 60. Matches gem5 output exactly for
/// known metric names (`system.cpu.committedInsts`, etc.).
#[derive(Default)]
pub struct GemstatsFormatter;

impl GemstatsFormatter {
    fn line(out: &mut String, name: &str, val: &str, comment: &str) {
        let _ = writeln!(out, "{name:<40}{val:<20}# {comment}");
    }
}

impl ReportFormatter for GemstatsFormatter {
    fn format_session(&self, s: &SpySpySnapshot) -> Vec<u8> {
        let mut out = String::with_capacity(2048);
        out.push_str("---------- Begin Simulation Statistics ----------\n");

        Self::line(&mut out, "sim_insns",
            &s.insn_count.to_string(),    "Number of instructions simulated");
        Self::line(&mut out, "sim_ticks",
            &s.tick_count.to_string(),    "Number of ticks simulated");
        Self::line(&mut out, "sim_freq",
            "1000000000",                  "Frequency of simulated ticks");
        Self::line(&mut out, "system.cpu.committedInsts",
            &s.insn_count.to_string(),    "Committed instructions");
        Self::line(&mut out, "system.cpu.ipc",
            &format!("{:.6}", s.ipc()),   "Instructions per tick");

        let total = s.insn_mix_total().max(1);
        for (class, count) in &s.insn_mix {
            let name = format!("system.cpu.op_class_0::{class}");
            let pct  = 100.0 * (*count as f64) / (total as f64);
            Self::line(&mut out, &name,
                &format!("{count}  {pct:.4}%"), "");
        }

        if let Some(ref c) = s.cache_l1d {
            Self::line(&mut out, &format!("system.cpu.dcache.overall_hits::total"),
                &c.hits.to_string(),   "");
            Self::line(&mut out, &format!("system.cpu.dcache.overall_misses::total"),
                &c.misses.to_string(), "");
            Self::line(&mut out, &format!("system.cpu.dcache.overall_miss_rate::total"),
                &format!("{:.6}", 1.0 - c.hit_rate), "");
        }

        if let Some(ref bp) = s.branch_pred {
            Self::line(&mut out, "system.cpu.branchPred.lookups",
                &bp.predictions.to_string(), "");
            Self::line(&mut out, "system.cpu.branchPred.mispredicts",
                &bp.mispredictions.to_string(), "");
        }

        out.push_str("----------  End Simulation Statistics  ----------\n");
        out.into_bytes()
    }

    fn format_counter(&self, name: &str, value: u64, comment: &str) -> Vec<u8> {
        let mut out = String::new();
        Self::line(&mut out, name, &value.to_string(), comment);
        out.into_bytes()
    }

    fn format_histogram(&self, name: &str, bins: &[(&str, u64)]) -> Vec<u8> {
        let mut out = String::new();
        for (label, count) in bins {
            Self::line(&mut out, &format!("{name}::{label}"), &count.to_string(), "");
        }
        out.into_bytes()
    }

    fn content_type(&self) -> &'static str { "text/plain; charset=utf-8" }
}
```

---

## 18. Report

```rust
// src/report.rs

use std::sync::Arc;
use crate::{
    error::SinkError,
    format::ReportFormatter,
    sink::Sink,
    snapshot::SpySpySnapshot,
};

/// Pairs an immutable session snapshot with a formatter and one or more sinks.
///
/// `deliver()` formats the snapshot exactly once, then writes the resulting
/// bytes to each sink in order. Errors from individual sinks are accumulated;
/// a failure from one sink does NOT prevent delivery to subsequent sinks.
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
    ) -> Self {
        Report { session, formatter, sinks }
    }

    /// Format once; write to all sinks. Collects all errors.
    pub fn deliver(&self) -> Result<(), SinkError> {
        let data = self.formatter.format_session(&self.session);
        let mut errors = Vec::new();
        for sink in &self.sinks {
            if let Err(e) = sink.write(&data) {
                errors.push(e);
            }
        }
        self.flush_all_inner(&mut errors);
        if errors.is_empty() {
            Ok(())
        } else if errors.len() == 1 {
            Err(SinkError::Io(errors.remove(0)))
        } else {
            Err(SinkError::MultipleErrors(errors))
        }
    }

    /// Format once; write to a single additional sink (ad-hoc delivery).
    /// Does not affect the permanent sink list.
    pub fn deliver_to(&self, sink: &dyn Sink) -> Result<(), SinkError> {
        let data = self.formatter.format_session(&self.session);
        sink.write(&data).map_err(SinkError::Io)?;
        sink.flush().map_err(SinkError::Io)
    }

    /// Flush all registered sinks.
    pub fn flush_all(&self) -> Result<(), SinkError> {
        let mut errors = Vec::new();
        self.flush_all_inner(&mut errors);
        if errors.is_empty() { Ok(()) }
        else if errors.len() == 1 { Err(SinkError::Io(errors.remove(0))) }
        else { Err(SinkError::MultipleErrors(errors)) }
    }

    fn flush_all_inner(&self, errors: &mut Vec<std::io::Error>) {
        for sink in &self.sinks {
            if let Err(e) = sink.flush() {
                errors.push(e);
            }
        }
    }
}
```

---

## 19. ReportSchedule

```rust
// src/schedule.rs

use crate::{error::SinkError, report::Report};

/// Trigger that fires a report delivery.
#[derive(Debug, Clone)]
pub enum ReportTrigger {
    /// Deliver when the process exits (called from `flush_at_exit()`).
    AtExit,
    /// Deliver every N instructions.
    EveryNInsns(u64),
    /// Deliver when the named counter exceeds a threshold.
    OnCounter { name: String, threshold: u64 },
    /// Deliver when PC equals the given address.
    OnPc(u64),
    /// Never fires automatically; caller invokes `deliver()` directly.
    Explicit,
}

/// Wraps a `Report` with a list of triggers. The engine calls `check()` from
/// the `pre_step` probe subscriber on every instruction.
pub struct ReportSchedule {
    triggers:          Vec<ReportTrigger>,
    report:            Report,
    last_delivered_at: u64,  // insn_count at which we last fired EveryNInsns
}

impl ReportSchedule {
    pub fn new(report: Report, triggers: Vec<ReportTrigger>) -> Self {
        ReportSchedule { triggers, report, last_delivered_at: 0 }
    }

    /// Called on every instruction from the engine's pre_step hook.
    ///
    /// Cost when no trigger fires: one integer division per `EveryNInsns` trigger,
    /// one equality compare per `OnPc` trigger. Typically < 2 ns.
    pub fn check(&mut self, pc: u64, insn_count: u64) {
        let mut should_deliver = false;
        for trigger in &self.triggers {
            match trigger {
                ReportTrigger::EveryNInsns(n) => {
                    if *n > 0 && insn_count > 0
                        && (insn_count / n) > (self.last_delivered_at / n)
                    {
                        should_deliver = true;
                    }
                }
                ReportTrigger::OnPc(addr) => {
                    if pc == *addr { should_deliver = true; }
                }
                // AtExit / Explicit / OnCounter do not fire in check().
                _ => {}
            }
        }
        if should_deliver {
            self.last_delivered_at = insn_count;
            // Ignore delivery errors from scheduled reports.
            let _ = self.report.deliver();
        }
    }

    /// Call at process exit. Fires all `AtExit` triggers.
    pub fn flush_at_exit(&self) {
        let has_at_exit = self.triggers.iter().any(|t| matches!(t, ReportTrigger::AtExit));
        if has_at_exit {
            let _ = self.report.deliver();
        }
    }

    /// Deliver immediately, regardless of triggers.
    pub fn deliver(&self) -> Result<(), SinkError> {
        self.report.deliver()
    }
}
```

**Integration in helm-engine:**

```rust
// helm-engine/src/engine.rs  (sketch)

fn pre_step(&mut self, pc: u64) {
    // Called before every instruction decode.
    if let Some(sched) = &mut self.report_schedule {
        sched.check(pc, self.insn_count);
    }
    // probe notifications, etc.
}
```

`ReportSchedule` is stored as `Option<ReportSchedule>` in the engine. When the Python layer
configures `session.report_every(n_insns=..., ...)`, the Python binding constructs the
`ReportSchedule` and installs it via `HelmSim::set_report_schedule(schedule)`.

---

## 20. lib.rs Re-exports

```rust
// src/lib.rs

pub mod error;
pub mod snapshot;
pub mod report;
pub mod schedule;
pub mod sink;
pub mod format;

pub use error::SinkError;
pub use snapshot::{
    BranchPredSnapshot, CacheSnapshot, CpuFaultEvent, SpySpySnapshot,
};
pub use report::Report;
pub use schedule::{ReportSchedule, ReportTrigger};
pub use sink::{
    sink_from_uri, AsyncFileSink, BinaryTraceSink, FileSink, NullSink,
    PythonSink, Sink, StderrSink, TcpSink,
};
pub use format::{
    CsvFormatter, GemstatsFormatter, JsonFormatter, ReportFormatter, TextFormatter,
};
```

---

*See [`HLD.md`](HLD.md) for purpose, architecture, and phased plan.*
*See [`TEST.md`](TEST.md) for the complete test strategy.*

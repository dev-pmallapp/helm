# helm-diag — LLD: Diagnostic Channel

> **Crate:** `helm-diag`
> **Location:** `framework/helm-diag/`
> **Phase:** Phase 0
> **Dependencies:** none (mandatory); `log` (optional feature `log-fallback`)

---

## Table of Contents

1. [Crate Structure](#1-crate-structure)
2. [Cargo.toml](#2-cargotoml)
3. [DiagLevel — src/entry.rs](#3-diaglevel--srcentryrs)
4. [DiagEntry — src/entry.rs](#4-diagentry--srcentryrs)
5. [SimContext and Thread-locals — src/lib.rs](#5-simcontext-and-thread-locals--srclibrs)
6. [DiagMonitor and emit() — src/lib.rs](#6-diagmonitor-and-emit--srclibrs)
7. [DiagSink — src/sink.rs](#7-diagsink--srsinkrs)
8. [URI Backend Parser — src/sink.rs](#8-uri-backend-parser--srcsinkrs)
9. [Macros — src/macros.rs](#9-macros--srcmacrosrs)
10. [How helm-debug Uses helm-diag](#10-how-helm-debug-uses-helm-diag)

---

## 1. Crate Structure

```
framework/helm-diag/
├── Cargo.toml
└── src/
    ├── lib.rs          # Public re-exports, thread-locals, install_monitor, update_sim_ctx, emit
    ├── entry.rs        # DiagLevel, DiagEntry, DiagEntry::format()
    ├── sink.rs         # Backend enum, open_backend(), DiagSink, drop/join
    └── macros.rs       # sim_stub!, sim_warn!, sim_info!
```

`lib.rs` is the top-level module. It declares `mod entry`, `mod sink`, `mod macros`
and re-exports `DiagLevel`, `DiagEntry`, `DiagMonitor`, `DiagSink`, `SimContext`,
`install_monitor`, `update_sim_ctx`, and `emit`.

The macros (`sim_stub!`, `sim_warn!`, `sim_info!`) are `#[macro_export]`-annotated in
`macros.rs` and automatically available at the crate root via `helm_diag::sim_stub!`
without a use statement.

---

## 2. Cargo.toml

```toml
[package]
name    = "helm-diag"
version = "0.1.0"
edition = "2021"
description = "Async structured diagnostic channel for helm-ng simulator."
license = "MIT OR Apache-2.0"

# No mandatory dependencies.
[dependencies]
# Optional: route emit() through the `log` facade when no DiagMonitor is installed.
log = { version = "0.4", optional = true }

[features]
default = []
# Enable `log` crate integration as a fallback when no DiagMonitor is installed.
# Without this feature, emit() falls back to eprintln! for non-Branch levels.
log-fallback = ["dep:log"]
```

No `[dev-dependencies]` beyond what is needed for unit tests (no external test crates
are required; `tempfile` is pulled transitively if needed).

---

## 3. DiagLevel — src/entry.rs

```rust
// src/entry.rs

/// Severity level of a diagnostic entry.
///
/// Ordered from lowest to highest severity:
/// `Info < Stub < Warn < Error`
///
/// Note: `Branch` is intentionally absent. Branch events are emitted through
/// `probe!(probes.branch, BranchEvent { ... })` at Layer 1 (helm-probe), not
/// through the diagnostic channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagLevel {
    /// Normal informational message (loader, boot progress).
    Info,
    /// Unimplemented feature — stub was executed, returned a default value.
    Stub,
    /// Something unexpected but recoverable.
    Warn,
    /// Fatal or hard error (unhandled trap, assertion failure, unrecoverable state).
    Error,
}

impl DiagLevel {
    /// Four-character tag used in formatted output. Space-padded to 4 chars.
    pub fn as_tag(self) -> &'static str {
        match self {
            DiagLevel::Info  => "INFO",
            DiagLevel::Stub  => "STUB",
            DiagLevel::Warn  => "WARN",
            DiagLevel::Error => "ERR ",
        }
    }
}
```

### Ordering

`DiagLevel` derives `PartialOrd` + `Ord` in declaration order. The resulting ordering is:

```
Info (0) < Stub (1) < Warn (2) < Error (3)
```

This ordering is used by `DiagSink::with_min_level(level)` to discard entries below
the configured minimum. The sentinel value for "pass all" is `DiagLevel::Info`.

---

## 4. DiagEntry — src/entry.rs

```rust
// src/entry.rs (continued)

use std::fmt::Write as FmtWrite;

/// One structured diagnostic record emitted by the simulator.
///
/// # Wire format
/// ```text
/// [STUB] sim_ns=000001234 insns=000025750 gicv2-dist       pc=0x0000000040201234 | MRS ID_AA64MMFR4_EL1 → 0
/// ```
#[derive(Debug, Clone)]
pub struct DiagEntry {
    /// Simulated nanoseconds elapsed (derived from `insns × 1_000_000_000 / freq_hz`).
    pub sim_ns:    u64,
    /// Total instructions retired at the time of the message.
    pub sim_insns: u64,
    /// Short, stable identifier for the emitting component (e.g. `"gicv2-dist"`).
    /// Must be `'static` so no allocation is required at the call site.
    pub component: &'static str,
    /// Severity level.
    pub level: DiagLevel,
    /// Guest program counter, if known at the call site.
    pub pc: Option<u64>,
    /// Free-form human-readable message string.
    pub message: String,
}

impl DiagEntry {
    /// Format into the canonical single-line representation.
    ///
    /// The format is stable — downstream log parsers may rely on it.
    ///
    /// Column layout:
    /// - `[LEVL]`              — 6 chars (bracket + 4-char tag + bracket)
    /// - `sim_ns=NNNNNNNNNNNN` — 18 chars (label + 12-digit number)
    /// - `insns=NNNNNNNNNNNN`  — 18 chars
    /// - `<component>`         — left-justified, padded to 16 chars
    /// - `pc=0xHHHHHHHHHHHHHHHH` or `pc=?                 ` — 20 chars
    /// - `| <message>`
    pub fn format(&self) -> String {
        let mut s = String::with_capacity(128);
        let pc_str = match self.pc {
            Some(p) => format!("{p:#018x}"),
            None    => "?                 ".to_string(),
        };
        let _ = write!(
            s,
            "[{}] sim_ns={:012} insns={:012} {:<16} pc={} | {}",
            self.level.as_tag(),
            self.sim_ns,
            self.sim_insns,
            self.component,
            pc_str,
            self.message,
        );
        s
    }
}
```

---

## 5. SimContext and Thread-locals — src/lib.rs

```rust
// src/lib.rs

pub mod entry;
pub mod sink;
#[macro_use]
pub mod macros;

pub use entry::{DiagEntry, DiagLevel};
pub use sink::DiagSink;

use std::cell::RefCell;
use std::sync::mpsc::SyncSender;

// ── Thread-local simulation context ───────────────────────────────────────────

/// Per-thread simulation context updated by the engine before each instruction step.
///
/// The engine calls [`update_sim_ctx`] once per step (or once per quantum for
/// bulk-step modes) to keep the context approximately accurate.  Hot-path code
/// reads this via [`emit`]; no argument passing is required at every call site.
#[derive(Clone, Copy, Default)]
pub struct SimContext {
    /// Simulated nanoseconds since simulation start.
    pub sim_ns: u64,
    /// Total instructions retired since simulation start.
    pub sim_insns: u64,
}

thread_local! {
    /// Active diagnostic sender for this thread.
    ///
    /// Set by [`install_monitor`] at engine startup. Absent on threads that are
    /// not simulation threads (e.g. the GDB server thread, the Python thread).
    pub static DIAG_MONITOR: RefCell<Option<DiagMonitor>> =
        RefCell::new(None);

    /// Current simulation context for this thread.
    ///
    /// Updated by the engine via [`update_sim_ctx`] before each step.
    /// Reads in [`emit`] are non-blocking.
    pub static SIM_CTX: RefCell<SimContext> =
        const { RefCell::new(SimContext { sim_ns: 0, sim_insns: 0 }) };
}

/// Register a [`DiagMonitor`] on the calling thread.
///
/// After this call, [`emit`] will route entries through the monitor rather than
/// falling back to `eprintln!`. Replaces any previously installed monitor.
///
/// Call this once per simulation thread during engine startup, after
/// [`DiagSink::open`] has returned a `(DiagSink, DiagMonitor)` pair.
pub fn install_monitor(m: DiagMonitor) {
    DIAG_MONITOR.with(|cell| *cell.borrow_mut() = Some(m));
}

/// Unregister the current thread's monitor.
///
/// After this call, [`emit`] falls back to `eprintln!` for Stub/Warn/Info/Error
/// levels. Call during engine teardown if the `DiagSink` must be dropped before
/// the simulation thread exits.
pub fn uninstall_monitor() {
    DIAG_MONITOR.with(|cell| *cell.borrow_mut() = None);
}

/// Update the thread-local simulation context.
///
/// The engine calls this before each instruction step (or at quantum boundaries
/// in bulk-step mode).  The `freq_hz` parameter converts instruction counts to
/// nanoseconds.  Pass `freq_hz = 0` to store raw instruction counts in `sim_ns`
/// (useful before a frequency is known, e.g. during ELF loading).
///
/// # Arguments
/// - `insns` — total instructions retired so far on this thread
/// - `freq_hz` — simulated CPU frequency in Hz (e.g. `1_000_000_000` for 1 GHz)
pub fn update_sim_ctx(insns: u64, freq_hz: u64) {
    SIM_CTX.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.sim_insns = insns;
        ctx.sim_ns = if freq_hz > 0 {
            insns * 1_000_000_000 / freq_hz
        } else {
            insns
        };
    });
}
```

---

## 6. DiagMonitor and emit() — src/lib.rs

```rust
// src/lib.rs (continued)

// ── DiagMonitor ───────────────────────────────────────────────────────────────

/// Cheap, clonable sender handle.
///
/// `DiagMonitor` is the only type that hot-path code holds. It is a thin wrapper
/// around a `SyncSender<DiagEntry>`. Cloning it is O(1) (reference-counted
/// under the hood by `SyncSender`). Calling [`try_send`](DiagMonitor::try_send)
/// never blocks — if the bounded queue is full, the entry is silently dropped.
#[derive(Clone)]
pub struct DiagMonitor {
    tx: SyncSender<DiagEntry>,
}

impl DiagMonitor {
    /// Non-blocking send. Drops the entry silently if the queue is full.
    ///
    /// This is the only method hot-path code should call. It performs a single
    /// `SyncSender::try_send`, which on x86 compiles to a handful of
    /// compare-exchange instructions and a conditional branch.
    #[inline]
    pub fn try_send(&self, entry: DiagEntry) {
        let _ = self.tx.try_send(entry);
    }
}

// ── emit() ────────────────────────────────────────────────────────────────────

/// Emit a diagnostic entry from the calling thread.
///
/// Reads the thread-local `SIM_CTX` to stamp `sim_ns` and `sim_insns`.
/// Attempts a non-blocking send via the thread-local `DIAG_MONITOR`.
///
/// **Fallback behavior when no monitor is installed:**
/// - `Stub`, `Warn`, `Info`, `Error` levels: `eprintln!` the formatted entry.
///   This ensures diagnostics are always visible even without a configured backend.
/// - If the `log-fallback` feature is enabled, routes through the `log` crate
///   instead of `eprintln!`.
///
/// This function is the sole dispatch point. All macros (`sim_stub!`, `sim_warn!`,
/// `sim_info!`) call it. Never call this from a `Drop` impl — the thread-local
/// may not be accessible during unwinding.
pub fn emit(level: DiagLevel, component: &'static str, pc: Option<u64>, message: String) {
    // Read timestamps from thread-local — borrow is always brief.
    let (sim_ns, sim_insns) = SIM_CTX.with(|c| {
        let ctx = c.borrow();
        (ctx.sim_ns, ctx.sim_insns)
    });

    let entry = DiagEntry { sim_ns, sim_insns, component, level, pc, message };

    // Attempt to route through the installed monitor.
    let sent = DIAG_MONITOR.with(|cell| {
        if let Some(ref m) = *cell.borrow() {
            m.try_send(entry.clone());
            true
        } else {
            false
        }
    });

    // Fallback: write directly to stderr (or `log`) if no monitor is installed.
    if !sent {
        #[cfg(feature = "log-fallback")]
        {
            let line = entry.format();
            match level {
                DiagLevel::Error => log::error!("{line}"),
                DiagLevel::Warn  => log::warn!("{line}"),
                DiagLevel::Stub  => log::debug!("{line}"),
                DiagLevel::Info  => log::info!("{line}"),
            }
        }
        #[cfg(not(feature = "log-fallback"))]
        {
            eprintln!("{}", entry.format());
        }
    }
}
```

---

## 7. DiagSink — src/sink.rs

```rust
// src/sink.rs

use crate::entry::DiagEntry;
use crate::DiagMonitor;

use std::io::{self, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Bounded channel depth. At 4096 entries, a full queue of STUB messages at
/// 1 GHz would represent ~4 µs of simulation time — plenty of headroom.
const QUEUE_DEPTH: usize = 4096;

/// How long the drain thread waits for new entries before flushing the backend.
/// A 50 ms timeout means the file backend flushes at most 20 times per second
/// when the queue is idle.
const DRAIN_TIMEOUT: Duration = Duration::from_millis(50);

// ── Backend ───────────────────────────────────────────────────────────────────

/// Write destination for the drain thread.
///
/// `Backend` is not public — callers interact only through the URI string passed
/// to [`DiagSink::open`].
pub(crate) enum Backend {
    Stderr,
    File(std::fs::File),
    Tcp(Arc<Mutex<TcpStream>>),
    Null,
}

impl Backend {
    pub(crate) fn write_line(&mut self, line: &str) {
        match self {
            Backend::Stderr => {
                eprintln!("{line}");
            }
            Backend::File(f) => {
                let _ = writeln!(f, "{line}");
            }
            Backend::Tcp(stream) => {
                if let Ok(mut s) = stream.lock() {
                    let _ = writeln!(s, "{line}");
                }
            }
            Backend::Null => {}
        }
    }

    pub(crate) fn flush(&mut self) {
        match self {
            Backend::File(f) => {
                let _ = f.flush();
            }
            Backend::Tcp(s) => {
                if let Ok(mut s) = s.lock() {
                    let _ = s.flush();
                }
            }
            Backend::Stderr | Backend::Null => {}
        }
    }
}

// ── DiagSink ──────────────────────────────────────────────────────────────────

/// Owns the background drain thread.
///
/// Dropping `DiagSink` joins the drain thread, ensuring all pending entries are
/// written before the sink is destroyed. This guarantee is critical for the
/// `file:` and `tcp:` backends — the last diagnostic messages are never silently
/// lost.
///
/// # Lifecycle
///
/// ```rust
/// // At engine startup:
/// let (sink, monitor) = DiagSink::open("file:/tmp/sim.log")?;
/// helm_diag::install_monitor(monitor);
///
/// // Run simulation...
/// sim.run(1_000_000);
///
/// // At engine teardown:
/// helm_diag::uninstall_monitor();  // optional — safe to let monitor live until drop
/// drop(sink);                       // join drain thread; flush file
/// ```
pub struct DiagSink {
    /// `None` after `drop` has joined the thread.
    handle: Option<thread::JoinHandle<()>>,
}

impl DiagSink {
    /// Open a sink draining to the given URI.
    ///
    /// Returns `(DiagSink, DiagMonitor)`. The `DiagMonitor` is the cheap sender
    /// that the simulation thread holds. Register it on the simulation thread via
    /// [`install_monitor`](crate::install_monitor).
    ///
    /// # URI formats
    /// - `stderr:` or `stderr`         — write to stderr (always available)
    /// - `null:` or `null`             — discard all entries
    /// - `file:/path/to/file`          — append to file; create if absent
    /// - `tcp:host:port`               — connect to TCP listener; stream lines
    ///
    /// # Errors
    /// Returns `Err` if the URI is malformed, the file cannot be created, or the
    /// TCP connection is refused.
    pub fn open(uri: &str) -> io::Result<(Self, DiagMonitor)> {
        let backend = open_backend(uri)?;
        let (tx, rx) = mpsc::sync_channel::<DiagEntry>(QUEUE_DEPTH);
        let monitor = DiagMonitor { tx };

        let handle = thread::Builder::new()
            .name("helm-diag".into())
            .spawn(move || {
                let mut backend = backend;
                loop {
                    match rx.recv_timeout(DRAIN_TIMEOUT) {
                        Ok(entry) => {
                            backend.write_line(&entry.format());
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            // Periodic flush — keeps file buffering from
                            // introducing long latencies in file/TCP backends.
                            backend.flush();
                        }
                        Err(RecvTimeoutError::Disconnected) => {
                            // All senders (monitors) have been dropped.
                            // Drain any remaining messages then exit.
                            break;
                        }
                    }
                }
                // Final flush before exiting the drain thread.
                backend.flush();
            })?;

        Ok((Self { handle: Some(handle) }, monitor))
    }

    /// Open a sink, falling back to `stderr:` if the URI fails.
    ///
    /// Logs a warning to `eprintln!` when the primary URI fails.
    /// Never panics. Use this at engine startup when a misconfigured URI should
    /// not abort the simulation.
    pub fn open_or_stderr(uri: Option<&str>) -> (Self, DiagMonitor) {
        let effective_uri = uri.unwrap_or("stderr:");
        match Self::open(effective_uri) {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!(
                    "[helm-diag] failed to open backend {:?}: {}; falling back to stderr",
                    effective_uri, e
                );
                // stderr always succeeds.
                Self::open("stderr:").expect("stderr always works")
            }
        }
    }
}

impl Drop for DiagSink {
    /// Join the drain thread on drop.
    ///
    /// Dropping `DiagSink` causes the `SyncSender` side of the channel to close
    /// (because the monitor clones inside the drain thread don't hold `tx`),
    /// which unblocks the drain thread's `recv_timeout` with `Disconnected`, at
    /// which point it flushes and exits.  This `drop` waits for that exit.
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
```

---

## 8. URI Backend Parser — src/sink.rs

```rust
// src/sink.rs (continued)

/// Parse a sim-trace URI string into an open [`Backend`].
///
/// Accepted forms (see [`DiagSink::open`] doc for full table).
pub(crate) fn open_backend(uri: &str) -> io::Result<Backend> {
    // Empty string and bare "stderr" both map to Stderr.
    if uri.is_empty() || uri == "stderr" || uri == "stderr:" {
        return Ok(Backend::Stderr);
    }

    if uri == "null" || uri == "null:" {
        return Ok(Backend::Null);
    }

    if let Some(path) = uri.strip_prefix("file:") {
        // path may start with / (absolute) or not (relative to CWD).
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        return Ok(Backend::File(f));
    }

    if let Some(rest) = uri.strip_prefix("tcp:") {
        // rest = "host:port"
        let stream = TcpStream::connect(rest)
            .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;
        // TCP_NODELAY prevents buffering from introducing multi-second delays
        // on lines that don't fill an MSS.
        stream.set_nodelay(true).ok();
        return Ok(Backend::Tcp(Arc::new(Mutex::new(stream))));
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "unknown helm-diag URI: {uri:?}  \
             (expected: stderr:, null:, file:/path, tcp:host:port)"
        ),
    ))
}
```

### URI parser edge cases

| Input | Result |
|-------|--------|
| `""` | `Backend::Stderr` |
| `"stderr"` | `Backend::Stderr` |
| `"stderr:"` | `Backend::Stderr` |
| `"null"` | `Backend::Null` |
| `"null:"` | `Backend::Null` |
| `"file:/tmp/sim.log"` | `Backend::File` (created/appended) |
| `"file:relative/path"` | `Backend::File` (relative to CWD) |
| `"tcp:127.0.0.1:9000"` | `Backend::Tcp` (connect) |
| `"tcp:localhost:9000"` | `Backend::Tcp` (connect) |
| `"tcp:bogus"` | `Err(ConnectionRefused)` |
| `"bogus:"` | `Err(InvalidInput)` |

---

## 9. Macros — src/macros.rs

```rust
// src/macros.rs

/// Emit a `Stub`-level diagnostic message.
///
/// Use for unimplemented features that return a default value and should not
/// abort the simulation. Common in device register stubs and unimplemented
/// sysreg handlers.
///
/// # Call forms
///
/// ```rust
/// // With PC:
/// sim_stub!(component = "gicv2-dist", pc = state.pc, "GICD_TYPER read → 0");
/// sim_stub!(component = "pl011",      pc = pc, "write to read-only FCR: {:#x}", val);
///
/// // Without PC:
/// sim_stub!(component = "aarch64-fp", "FPCR feature not implemented");
/// ```
#[macro_export]
macro_rules! sim_stub {
    (component=$comp:expr, pc=$pc:expr, $($arg:tt)*) => {
        $crate::emit(
            $crate::DiagLevel::Stub,
            $comp,
            Some($pc),
            ::std::format!($($arg)*),
        )
    };
    (component=$comp:expr, $($arg:tt)*) => {
        $crate::emit(
            $crate::DiagLevel::Stub,
            $comp,
            None,
            ::std::format!($($arg)*),
        )
    };
}

/// Emit a `Warn`-level diagnostic message.
///
/// Use for unexpected but recoverable conditions: a write to a reserved
/// register, an unsupported combination of flags, a device reset while active.
///
/// # Call forms
///
/// ```rust
/// // With PC:
/// sim_warn!(component = "mmu", pc = state.pc, "unmapped VA {:#010x} — returning 0", va);
///
/// // Without PC:
/// sim_warn!(component = "helm-loader", "ELF has PT_LOAD with zero filesz");
/// ```
#[macro_export]
macro_rules! sim_warn {
    (component=$comp:expr, pc=$pc:expr, $($arg:tt)*) => {
        $crate::emit(
            $crate::DiagLevel::Warn,
            $comp,
            Some($pc),
            ::std::format!($($arg)*),
        )
    };
    (component=$comp:expr, $($arg:tt)*) => {
        $crate::emit(
            $crate::DiagLevel::Warn,
            $comp,
            None,
            ::std::format!($($arg)*),
        )
    };
}

/// Emit an `Info`-level diagnostic message.
///
/// Use for normal operational events: loader progress, device initialization,
/// boot stage transitions. Prefer `Info` over `Warn` for expected events that
/// the user may want to observe.
///
/// `sim_info!` does not accept a `pc=` argument — informational messages
/// are typically not associated with a specific guest instruction.
///
/// # Call form
///
/// ```rust
/// sim_info!(component = "helm-loader", "ELF loaded: entry={:#018x}", entry);
/// sim_info!(component = "arm-virt",    "GICv2 mapped at {:#010x}", base);
/// ```
#[macro_export]
macro_rules! sim_info {
    (component=$comp:expr, $($arg:tt)*) => {
        $crate::emit(
            $crate::DiagLevel::Info,
            $comp,
            None,
            ::std::format!($($arg)*),
        )
    };
}
```

### Macro design notes

- All three macros delegate to `emit()` without any conditional logic — the non-blocking
  check lives in `emit()`.
- `component` must be a `&'static str` expression. Using a runtime string here would
  require an allocation per call; the `&'static str` constraint forces call sites to use
  string literals or static constants.
- `pc` is `Option<u64>` in `emit()`. The macro forms that accept `pc=` pass `Some(pc)`;
  forms without `pc=` pass `None`. This avoids a runtime `if let Some(pc) = ...` at
  the call site.
- `format!($($arg)*)` is evaluated unconditionally. If the call site has a very expensive
  format argument (e.g. a hex dump), callers can guard with an `if` before the macro.
  The diagnostic channel is not a zero-cost probe system — that role belongs to `helm-probe`.

---

## 10. How helm-debug Uses helm-diag

After the extraction, `helm-debug` gains `helm-diag` as a dependency and uses it as follows:

### 10.1 Opening the DiagSink (engine startup)

In `helm-engine/src/lib.rs` (or `helm-cli/src/main.rs`) during `build_simulator()`:

```rust
use helm_diag::{DiagSink, install_monitor};

// The Python config or CLI --sim-trace flag provides the URI.
let uri = config.sim_trace_uri.as_deref();  // e.g. Some("file:/tmp/sim.log")

let (diag_sink, diag_monitor) = DiagSink::open_or_stderr(uri);

// Register the monitor on the simulation thread (this call is on the sim thread).
install_monitor(diag_monitor.clone());

// The engine holds `diag_sink` — it is dropped when the engine is destroyed,
// joining the background thread and flushing the file.
engine.diag_sink = Some(diag_sink);
```

### 10.2 Updating SimContext on the Hot Path

In `helm-engine/src/fs.rs` (the FS-mode step loop):

```rust
use helm_diag::update_sim_ctx;

fn step_aarch64_fs(state: &mut ArchState, ..., insn_count: u64, freq_hz: u64) -> StepResult {
    // Update the diag context at the top of every quantum (not every instruction —
    // the per-instruction cost would dominate for STUB-free runs).
    update_sim_ctx(insn_count, freq_hz);

    // ... decode and execute ...
}
```

For maximum accuracy, `update_sim_ctx` can be called once per instruction. In practice,
calling it once per quantum (typically 1000–10000 instructions) is accurate enough for
log timestamps, and has negligible overhead.

### 10.3 Emitting Diagnostics from helm-arch

In `helm-arch/src/aarch64/execute/sysreg.rs`:

```rust
use helm_diag::sim_stub;

fn read_sysreg(state: &mut ArchState, reg: SysReg) -> u64 {
    match reg {
        SysReg::ID_AA64MMFR4_EL1 => {
            sim_stub!(component = "aarch64-sysreg", pc = state.pc,
                      "MRS {:?} → 0  (unimplemented)", reg);
            0
        }
        // ...
    }
}
```

### 10.4 Emitting Diagnostics from helm-devices

In `hw/helm-hw-char/src/pl011.rs` (example device):

```rust
use helm_diag::sim_warn;

fn mmio_write(&mut self, offset: u32, val: u32) {
    match offset {
        0x00 => self.data_reg = val,
        0x44 => {
            // FBRD (fractional baud rate divisor) — not modeled
            sim_warn!(component = "pl011", "write to unmodeled FBRD: {val:#010x}");
        }
        _ => {
            sim_warn!(component = "pl011", "write to unknown offset {offset:#x}: {val:#010x}");
        }
    }
}
```

### 10.5 Re-exports in helm-debug (Compatibility)

To avoid breaking any existing code that imported from `helm_debug::sim_trace`,
`helm-debug` re-exports the `helm-diag` types at the old path during a transition
period:

```rust
// runtime/helm-debug/src/sim_trace.rs (transition shim)
//
// This module is deprecated. Import from `helm_diag` directly.
// Will be removed in Phase 1.

pub use helm_diag::{
    DiagEntry    as MonitorEntry,
    DiagLevel    as Level,
    DiagMonitor  as Monitor,
    DiagSink     as MonitorSink,
    SimContext,
    install_monitor,
    update_sim_ctx,
    emit,
};

// Re-export macros — these require explicit re-export because #[macro_export]
// macros land at the crate root, not in submodules.
pub use helm_diag::{sim_stub, sim_warn, sim_info};
```

The `sim_branch!` macro is **not re-exported** — call sites that used it must be
migrated to `probe!(probes.branch, BranchEvent { ... })`. This is intentional:
the missing re-export produces a compile error that guides the migration.

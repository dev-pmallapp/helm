# helm-diag — LLD: Diagnostic Channel

> **Crate:** `helm-diag`
> **Location:** `framework/helm-diag/`
> **Phase:** Phase 0
> **Dependencies:** none (mandatory); `log` (optional feature `log-fallback`)

---

## Table of Contents

1. [Crate Structure](#1-crate-structure)
2. [Cargo.toml](#2-cargotoml)
3. [DiagLevel and DiagContext — src/entry.rs](#3-diaglevel-and-simcontext--srcentryrs)
4. [DiagEntry — src/entry.rs](#4-diagentry--srcentryrs)
5. [Thread-locals, install_monitor, update_sim_ctx — src/lib.rs](#5-thread-locals-install_monitor-update_sim_ctx--srclibrs)
6. [DiagMonitor and emit() — src/lib.rs](#6-diagmonitor-and-emit--srclibrs)
7. [DiagSink — src/sink.rs](#7-diagsink--srcsinkrs)
8. [URI Backend Parser — src/sink.rs](#8-uri-backend-parser--srcsinkrs)
9. [Macros — src/macros.rs](#9-macros--srcmacrosrs)
10. [How Runtime Crates Use helm-diag](#10-how-runtime-crates-use-helm-diag)

---

## 1. Crate Structure

```
framework/helm-diag/
├── Cargo.toml
├── src/
│   ├── lib.rs       # Public re-exports, thread-locals, install_monitor,
│   │                #   uninstall_monitor, is_monitor_active, update_sim_ctx, emit
│   ├── entry.rs     # DiagLevel, DiagContext, DiagEntry, DiagEntry::format()
│   ├── sink.rs      # Backend enum, open_backend(), DiagSink, Drop impl
│   └── macros.rs    # sim_stub!, sim_warn!, sim_info!
└── tests/
    ├── macros.rs    # Integration: macro call sites
    └── multi_entry.rs  # Integration: ordering and overflow
```

`lib.rs` is the top-level module. It declares `mod entry`, `mod sink`, `#[macro_use] mod macros`
and re-exports `DiagLevel`, `DiagEntry`, `DiagContext`, `DiagSink` (from their defining
modules) and `DiagMonitor` (defined inline in `lib.rs`).

The macros (`sim_stub!`, `sim_warn!`, `sim_info!`) are `#[macro_export]`-annotated in
`macros.rs` and available at the crate root via `helm_diag::sim_stub!` without a use statement.

---

## 2. Cargo.toml

```toml
[package]
name    = "helm-diag"
version.workspace = true
edition.workspace = true

[lints]
workspace = true

[features]
default      = []
log-fallback = ["dep:log"]

[dependencies]
log = { version = "0.4", optional = true }
```

No `[dev-dependencies]`. All tests use only `std` and the crate itself.

---

## 3. DiagLevel and DiagContext — src/entry.rs

### DiagLevel

```rust
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
    Info,   // Normal informational message (loader, boot progress)
    Stub,   // Unimplemented feature -- stub executed, returned default value
    Warn,   // Something unexpected but recoverable
    Error,  // Fatal or hard error (unhandled trap, assertion failure)
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

Ordering is derived in declaration order: `Info (0) < Stub (1) < Warn (2) < Error (3)`.
`DiagLevel::Info` is the sentinel for "pass all". There are exactly four variants; no
`Branch` variant exists.

### DiagContext

```rust
/// Per-thread simulation context updated by the engine before each instruction step.
#[derive(Clone, Copy, Default)]
pub struct DiagContext {
    /// Simulated nanoseconds since simulation start.
    pub sim_ns: u64,
    /// Total instructions retired since simulation start.
    pub sim_insns: u64,
}
```

`DiagContext` lives in `entry.rs` and is re-exported from `lib.rs` via
`pub use entry::{DiagEntry, DiagLevel, DiagContext}`.

---

## 4. DiagEntry — src/entry.rs

```rust
/// One structured diagnostic record emitted by the simulator.
#[derive(Debug, Clone)]
pub struct DiagEntry {
    /// Simulated nanoseconds elapsed (derived from `insns * 1_000_000_000 / freq_hz`).
    pub sim_ns: u64,
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
```

### `DiagEntry::format()`

```rust
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
```

The output format is stable; downstream log parsers may rely on it.

Column layout:

| Column | Format spec | Example |
|--------|-------------|---------|
| `[LEVL]` | `[{}]` with `as_tag()` | `[STUB]`, `[ERR ]` |
| `sim_ns=NNNNNNNNNNNN` | `{:012}` | `sim_ns=000001234000` |
| `insns=NNNNNNNNNNNN` | `{:012}` | `insns=000025750000` |
| `<component>` | `{:<16}` (left, pad to 16) | `gicv2-dist      ` |
| `pc=0xHHHHHHHHHHHHHHHH` | `{p:#018x}` (0x + 16 hex digits) | `pc=0x0000000040201234` |
| `pc=?` (when None) | literal `"?                 "` | `pc=?` |
| `| <message>` | ` \| {}` | `| MRS ID_AA64MMFR4_EL1 -> 0` |

Example output lines:

```
[STUB] sim_ns=000001234 insns=000025750 gicv2-dist       pc=0x0000000040201234 | MRS ID_AA64MMFR4_EL1 -> 0
[WARN] sim_ns=000012300 insns=000025600 pl011-uart        pc=?                  | write to read-only reg 0x18
[INFO] sim_ns=000000000 insns=000000000 helm-loader       pc=?                  | ELF loaded: entry=0x4000_0000
[ERR ] sim_ns=000012500 insns=000025800 aarch64-execute   pc=0x000000004020ffff | unhandled exception ESR=0x96000004
```

---

## 5. Thread-locals, install_monitor, update_sim_ctx — src/lib.rs

### Thread-locals

```rust
thread_local! {
    /// Active diagnostic sender for this thread.
    ///
    /// Set by [`install_monitor`] at engine startup. Absent on threads that are
    /// not simulation threads (e.g. the GDB server thread, the Python thread).
    pub static DIAG_MONITOR: RefCell<Option<DiagMonitor>> =
        const { RefCell::new(None) };

    /// Current simulation context for this thread.
    ///
    /// Updated by the engine via [`update_sim_ctx`] before each step.
    /// Reads in [`emit`] are non-blocking.
    pub static SIM_CTX: RefCell<DiagContext> =
        const { RefCell::new(DiagContext { sim_ns: 0, sim_insns: 0 }) };
}
```

Both use `const { ... }` initializers (stable Rust 1.76+) which ensures zero-cost
initialization: the thread-local is `const`-initialized and the `RefCell` is never
heap-allocated.

### `install_monitor` and `uninstall_monitor`

```rust
pub fn install_monitor(m: DiagMonitor) {
    DIAG_MONITOR.with(|cell| *cell.borrow_mut() = Some(m));
}

pub fn uninstall_monitor() {
    DIAG_MONITOR.with(|cell| *cell.borrow_mut() = None);
}
```

`install_monitor` replaces any previously installed monitor. Call it once per simulation
thread after `DiagSink::open` returns a `(DiagSink, DiagMonitor)` pair.

`uninstall_monitor` is optional at teardown; dropping the `DiagSink` also terminates
the drain thread.

### `is_monitor_active`

```rust
#[inline]
pub fn is_monitor_active() -> bool {
    DIAG_MONITOR.with(|cell| cell.borrow().is_some())
}
```

The engine calls this before `update_sim_ctx` to skip the RefCell borrow when no
diagnostic backend is active. This is a measurable optimization at simulation speed.

### `update_sim_ctx`

```rust
pub fn update_sim_ctx(insns: u64, freq_hz: u64) {
    SIM_CTX.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.sim_insns = insns;
        ctx.sim_ns = if freq_hz > 0 {
            insns * 1_000_000_000 / freq_hz
        } else {
            insns   // zero freq: store raw insn count in sim_ns
        };
    });
}
```

Arguments:
- `insns` — total instructions retired so far on this thread
- `freq_hz` — simulated CPU frequency in Hz (e.g. `1_000_000_000` for 1 GHz)

Pass `freq_hz = 0` before a frequency is known (e.g. during ELF loading) to store raw
instruction counts in `sim_ns`. In `helm-engine`, the current call is:

```rust
if helm_diag::is_monitor_active() {
    helm_diag::update_sim_ctx(self.insns_retired, 1_000_000_000);
}
```

---

## 6. DiagMonitor and emit() — src/lib.rs

### DiagMonitor

```rust
/// Cheap, clonable sender handle.
///
/// `DiagMonitor` is the only type that hot-path code holds. It is a thin wrapper
/// around a `SyncSender<DiagEntry>`. Cloning it is O(1). Calling try_send()
/// never blocks -- if the bounded queue is full, the entry is silently dropped.
#[derive(Clone)]
pub struct DiagMonitor {
    pub(crate) tx: SyncSender<DiagEntry>,
}

impl DiagMonitor {
    /// Non-blocking send. Drops the entry silently if the queue is full.
    #[inline]
    pub fn try_send(&self, entry: DiagEntry) {
        let _ = self.tx.try_send(entry);
    }
}
```

`DiagMonitor` is constructed only by `DiagSink::open` (which sets `tx` to the send
half of a `sync_channel`). It is not constructable by external code.

### `emit()`

```rust
pub fn emit(level: DiagLevel, component: &'static str, pc: Option<u64>, message: String) {
    // Read timestamps from thread-local -- borrow is always brief.
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

Behavior:
1. Snapshots `SIM_CTX` into `sim_ns` and `sim_insns` (brief RefCell borrow).
2. Constructs a `DiagEntry`.
3. Checks `DIAG_MONITOR`: if a monitor is installed, calls `try_send` (non-blocking).
4. If no monitor is installed: falls back to `eprintln!` (or `log` if the feature is enabled).

The `entry.clone()` before `try_send` is required because `emit` continues to hold `entry`
for the fallback path. This clone is only performed on the hot path when a monitor is
installed, and it amounts to one heap allocation for the `message` String.

Do not call `emit()` from a `Drop` impl — thread-locals may not be accessible during
unwinding.

---

## 7. DiagSink — src/sink.rs

### Constants

```rust
const QUEUE_DEPTH: usize = 4096;
const DRAIN_TIMEOUT: Duration = Duration::from_millis(50);
```

At 1 GHz, a full queue of STUB messages represents ~4 us of simulation time — plenty of
headroom for any burst. The 50 ms drain timeout means the file backend flushes at most
20 times per second when the queue is idle.

### Backend (internal)

```rust
pub(crate) enum Backend {
    Stderr,
    File(std::fs::File),
    Tcp(Arc<Mutex<TcpStream>>),
    Null,
}
```

`Backend` is not public. Callers interact only through the URI string passed to
`DiagSink::open`. The drain thread owns the `Backend` and calls `write_line` and `flush`
on it.

- `Stderr`: `eprintln!("{line}")` — no explicit flush needed
- `File`: `writeln!(f, "{line}")` then periodic `f.flush()` on timeout
- `Tcp`: `writeln!(s, "{line}")` then periodic flush; `TCP_NODELAY` set on connect
- `Null`: no-op for both `write_line` and `flush`

### DiagSink

```rust
pub struct DiagSink {
    handle: Option<thread::JoinHandle<()>>,
}
```

#### `DiagSink::open(uri)`

```rust
pub fn open(uri: &str) -> io::Result<(Self, DiagMonitor)>
```

1. Calls `open_backend(uri)` to parse the URI and open the write destination.
2. Creates a `sync_channel::<DiagEntry>(QUEUE_DEPTH)`.
3. Wraps the send half in a `DiagMonitor`.
4. Spawns the drain thread named `"helm-diag"`.
5. Returns `(DiagSink, DiagMonitor)`.

The drain thread loop:

```
loop {
    match rx.recv_timeout(DRAIN_TIMEOUT) {
        Ok(entry)                         => write_line(entry.format())
        Err(RecvTimeoutError::Timeout)    => flush()
        Err(RecvTimeoutError::Disconnected) => break
    }
}
flush()  // final flush before exit
```

#### `DiagSink::open_or_stderr(uri)`

```rust
pub fn open_or_stderr(uri: Option<&str>) -> (Self, DiagMonitor)
```

Infallible constructor. `None` maps to `"stderr:"`. On failure, logs a warning to
`eprintln!` and falls back to `DiagSink::open("stderr:")` (which always succeeds).

#### `Drop for DiagSink`

```rust
impl Drop for DiagSink {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
```

Dropping `DiagSink` joins the drain thread. The drain thread exits when all `SyncSender`
clones are dropped (i.e., when the `DiagMonitor` is dropped or `uninstall_monitor` is
called). The join ensures all pending entries are written and the final flush completes
before `drop` returns.

Correct teardown order:

```rust
helm_diag::uninstall_monitor();  // or just let monitor drop
drop(monitor);                   // closes the channel sender side
drop(sink);                      // joins drain thread; all entries flushed
```

---

## 8. URI Backend Parser — src/sink.rs

```rust
pub(crate) fn open_backend(uri: &str) -> io::Result<Backend>
```

| Input | Result |
|-------|--------|
| `""` | `Backend::Stderr` |
| `"stderr"` | `Backend::Stderr` |
| `"stderr:"` | `Backend::Stderr` |
| `"null"` | `Backend::Null` |
| `"null:"` | `Backend::Null` |
| `"file:/path/to/file"` | `Backend::File` (create+append) |
| `"file:relative/path"` | `Backend::File` (relative to CWD) |
| `"tcp:127.0.0.1:9000"` | `Backend::Tcp` (connect, set_nodelay) |
| `"tcp:localhost:9000"` | `Backend::Tcp` (connect, set_nodelay) |
| `"tcp:host:port"` where port is bad | `Err(ConnectionRefused)` |
| `"bogus:something"` | `Err(InvalidInput)` — message contains `"helm-diag"` |

The `file:` prefix strips exactly one `:` character; the remainder is the path passed
directly to `OpenOptions`. Absolute paths (`/tmp/sim.log`) and relative paths both work.

The `tcp:` prefix strips exactly `"tcp:"` and passes the remainder to `TcpStream::connect`
as a `host:port` address string. `set_nodelay(true)` is called to prevent buffering
from introducing multi-second latency on lines shorter than an MSS.

---

## 9. Macros — src/macros.rs

All three macros are `#[macro_export]` and available at the crate root.

### `sim_stub!`

```rust
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
```

### `sim_warn!`

```rust
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
```

### `sim_info!`

```rust
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

`sim_info!` has only one form (no `pc=`). Informational messages are typically not
associated with a specific guest instruction.

### Macro Design Notes

- All three macros delegate to `emit()` without any conditional logic. The non-blocking
  check lives inside `emit()`.
- `component` must be a `&'static str` expression. Using a runtime string would require
  an allocation per call; the `&'static str` constraint forces call sites to use string
  literals or static constants.
- `pc` values are passed as `Some($pc)` by the with-pc arms and `None` by the without-pc
  arms. No runtime `if let` at the call site.
- `format!($($arg)*)` is evaluated unconditionally regardless of whether a monitor is
  installed. If the format expression is expensive, callers can guard with an explicit
  `if helm_diag::is_monitor_active()` before the macro. The diagnostic channel is not
  a zero-cost probe system — that role belongs to `helm-probe`.

---

## 10. How Runtime Crates Use helm-diag

### helm-arch (every execute sub-module)

```rust
// runtime/helm-arch/src/aarch64/execute/sysreg.rs
use helm_diag::{sim_stub, sim_warn};

fn read_sysreg(state: &mut ArchState, reg: SysReg) -> u64 {
    match reg {
        SysReg::ID_AA64MMFR4_EL1 => {
            sim_stub!(component = "aarch64-sysreg", pc = state.pc,
                      "MRS {:?} -> 0  (unimplemented)", reg);
            0
        }
        // ...
    }
}
```

All execute sub-modules (`fp.rs`, `simd.rs`, `branch.rs`, `dp.rs`, `ldst.rs`,
`mul_div.rs`, `helpers.rs`) import `sim_stub` and `sim_warn` from `helm_diag`.

### helm-engine (hot loop guard)

```rust
// runtime/helm-engine/src/lib.rs
use helm_diag;

// Inside the step loop:
if helm_diag::is_monitor_active() {
    helm_diag::update_sim_ctx(self.insns_retired, 1_000_000_000);
}
```

The `is_monitor_active()` guard prevents a RefCell borrow on every instruction when
no diagnostic backend is configured.

### helm-debug (compatibility re-export)

```rust
// runtime/helm-debug/src/lib.rs
//
// Diagnostic macros have moved to helm-diag. Re-export them so that
// `use helm_debug::{sim_stub, ...}` import paths continue to compile.
pub use helm_diag::{sim_stub, sim_warn, sim_info};
```

`sim_branch!` is not re-exported. Any call site that used it gets a compile error
directing migration to `probe!(probes.branch, BranchEvent { ... })`.

### helm-python (DiagSink from Python config)

```rust
// runtime/helm-python/src/lib.rs
use helm_diag::{DiagSink, install_monitor};

// Called from Python: sim.set_sim_trace(uri)
let (sink, monitor) = DiagSink::open(uri)
    .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
install_monitor(monitor);
// sink stored in HelmSim to keep drain thread alive
```

### helm-cli (startup)

```rust
// runtime/helm-cli/src/lib.rs
use helm_diag::{DiagSink, install_monitor};

let (sink, monitor) = DiagSink::open_or_stderr(Some(uri));
install_monitor(monitor);
// sink held until CLI exits
```

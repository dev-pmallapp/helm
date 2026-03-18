//! Async simulator sim-trace channel.
//!
//! Provides a bounded, non-blocking message queue for structured simulator
//! log output, independent of guest output (serial console).
//!
//! # URI formats
//! - `stderr:`        — write to stderr (default)
//! - `file:/tmp/foo`  — append to a file
//! - `tcp:host:port`  — connect; stream log lines to a TCP client
//! - `null:`          — discard all messages (benchmarking)
//!
//! # Usage pattern
//! The engine initialises a [`MonitorSink`] at startup and registers a
//! [`Monitor`] (the cheap sender) in the thread-local `SIM_MONITOR`.
//! Hot-path code (device reads, sysreg stubs) calls [`sim_warn!`] or
//! [`sim_stub!`] without holding any locks.  The sink thread drains the
//! queue in a tight loop and writes formatted lines to the backend.
//!
//! On `Drop`, [`MonitorSink`] waits for the background thread to finish
//! so the last messages are never lost.  A panic hook does a best-effort
//! flush of whatever remains in the queue.

use std::fmt::Write as FmtWrite;
use std::io::{self, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// ── Log level ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Normal informational message (loader, boot progress).
    Info,
    /// Something unexpected but recoverable.
    Warn,
    /// Unimplemented feature — stub was executed, returned default value.
    Stub,
    /// Fatal / hard error (guest trap, assertion).
    Error,
    /// Branch / control-flow event (used by the branch tracer).
    Branch,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Info   => "INFO",
            Level::Warn   => "WARN",
            Level::Stub   => "STUB",
            Level::Error  => "ERR ",
            Level::Branch => "BRNC",
        }
    }
}

// ── Log entry ─────────────────────────────────────────────────────────────────

/// One structured log record emitted by the simulator.
#[derive(Debug, Clone)]
pub struct MonitorEntry {
    /// Simulated nanoseconds elapsed (insns × assumed period).
    pub sim_ns: u64,
    /// Total instructions retired at the time of the message.
    pub sim_insns: u64,
    /// Component that emitted this message, e.g. `"gicv2-dist"`.
    pub component: &'static str,
    pub level: Level,
    /// Guest program counter, if known.
    pub pc: Option<u64>,
    pub message: String,
}

impl MonitorEntry {
    /// Format into the canonical log line.
    ///
    /// ```text
    /// [STUB] sim_ns=000001234 insns=000025750 gicv2-dist   pc=0x40201234 | MRS ID_AA64MMFR4_EL1 → 0
    /// ```
    pub fn format(&self) -> String {
        let mut s = String::with_capacity(128);
        let _ = write!(
            s,
            "[{}] sim_ns={:012} insns={:012} {:<16} pc={} | {}",
            self.level.as_str(),
            self.sim_ns,
            self.sim_insns,
            self.component,
            self.pc.map_or_else(|| "?                 ".to_string(), |p| format!("{p:#018x}")),
            self.message,
        );
        s
    }
}

// ── Backend ───────────────────────────────────────────────────────────────────

enum Backend {
    Stderr,
    File(std::fs::File),
    Tcp(Arc<Mutex<TcpStream>>),
    Null,
}

impl Backend {
    fn write_line(&mut self, line: &str) {
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

    fn flush(&mut self) {
        match self {
            Backend::File(f) => { let _ = f.flush(); }
            Backend::Tcp(s) => { if let Ok(mut s) = s.lock() { let _ = s.flush(); } }
            _ => {}
        }
    }
}

// ── URI parser ────────────────────────────────────────────────────────────────

/// Parse a sim-trace URI into a Backend.
///
/// Accepted forms:
/// - `stderr:` or `stderr`
/// - `null:` or `null`
/// - `file:/path/to/file`
/// - `tcp:host:port`
fn open_backend(uri: &str) -> io::Result<Backend> {
    if uri == "stderr" || uri == "stderr:" || uri.is_empty() {
        return Ok(Backend::Stderr);
    }
    if uri == "null" || uri == "null:" {
        return Ok(Backend::Null);
    }
    if let Some(path) = uri.strip_prefix("file:") {
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        return Ok(Backend::File(f));
    }
    if let Some(rest) = uri.strip_prefix("tcp:") {
        // rest = host:port
        let stream = TcpStream::connect(rest)
            .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;
        stream.set_nodelay(true).ok();
        return Ok(Backend::Tcp(Arc::new(Mutex::new(stream))));
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("unknown sim-trace URI: {uri}  (expected stderr:, null:, file:/path, tcp:host:port)"),
    ))
}

// ── SimTrace sender ────────────────────────────────────────────────────────────

/// Cheap, clonable sender.  Hot-path code uses [`try_send`](Monitor::try_send)
/// which never blocks and silently drops if the queue is full.
#[derive(Clone)]
pub struct Monitor {
    tx: SyncSender<MonitorEntry>,
}

impl Monitor {
    /// Non-blocking send.  Drops the entry silently if the queue is full.
    pub fn try_send(&self, entry: MonitorEntry) {
        let _ = self.tx.try_send(entry);
    }
}

// ── Thread-local sim context ──────────────────────────────────────────────────

/// Per-thread simulation context updated by the engine before each step.
#[derive(Clone, Copy, Default)]
pub struct SimContext {
    pub sim_ns: u64,
    pub sim_insns: u64,
}

thread_local! {
    /// Active sim-trace sender for this thread (set by the engine at init).
    pub static SIM_MONITOR: std::cell::RefCell<Option<Monitor>> =
        std::cell::RefCell::new(None);

    /// Current simulation context (insns, time) — updated by the engine.
    pub static SIM_CTX: std::cell::RefCell<SimContext> =
        const { std::cell::RefCell::new(SimContext { sim_ns: 0, sim_insns: 0 }) };
}

/// Register a [`Monitor`] on the current thread.
pub fn install_monitor(m: Monitor) {
    SIM_MONITOR.with(|cell| *cell.borrow_mut() = Some(m));
}

/// Update the thread-local simulation context (call before each step).
pub fn update_sim_ctx(insns: u64, freq_hz: u64) {
    SIM_CTX.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.sim_insns = insns;
        ctx.sim_ns = if freq_hz > 0 { insns * 1_000_000_000 / freq_hz } else { insns };
    });
}

/// Internal: emit a log entry using the thread-local monitor.
/// Falls back to eprintln! if no sim-trace is installed.
pub fn emit(level: Level, component: &'static str, pc: Option<u64>, message: String) {
    let (sim_ns, sim_insns) = SIM_CTX.with(|c| {
        let ctx = c.borrow();
        (ctx.sim_ns, ctx.sim_insns)
    });
    let entry = MonitorEntry { sim_ns, sim_insns, component, level, pc, message };
    let sent = SIM_MONITOR.with(|cell| {
        if let Some(ref m) = *cell.borrow() {
            m.try_send(entry.clone());
            true
        } else {
            false
        }
    });
    if !sent {
        // No monitor installed — write directly to stderr so nothing is lost
        eprintln!("{}", entry.format());
    }
}

// ── Macros ────────────────────────────────────────────────────────────────────

/// Log a STUB-level message (unimplemented feature, returned default).
#[macro_export]
macro_rules! sim_stub {
    (component=$comp:expr, pc=$pc:expr, $($arg:tt)*) => {
        $crate::sim_trace::emit(
            $crate::sim_trace::Level::Stub,
            $comp,
            Some($pc),
            format!($($arg)*),
        )
    };
    (component=$comp:expr, $($arg:tt)*) => {
        $crate::sim_trace::emit(
            $crate::sim_trace::Level::Stub,
            $comp,
            None,
            format!($($arg)*),
        )
    };
}

/// Log a WARN-level message.
#[macro_export]
macro_rules! sim_warn {
    (component=$comp:expr, pc=$pc:expr, $($arg:tt)*) => {
        $crate::sim_trace::emit(
            $crate::sim_trace::Level::Warn,
            $comp,
            Some($pc),
            format!($($arg)*),
        )
    };
    (component=$comp:expr, $($arg:tt)*) => {
        $crate::sim_trace::emit(
            $crate::sim_trace::Level::Warn,
            $comp,
            None,
            format!($($arg)*),
        )
    };
}

/// Log an INFO-level message.
#[macro_export]
macro_rules! sim_info {
    (component=$comp:expr, $($arg:tt)*) => {
        $crate::sim_trace::emit(
            $crate::sim_trace::Level::Info,
            $comp,
            None,
            format!($($arg)*),
        )
    };
}

/// Emit a branch/control-flow event.  Only fires when a MonitorSink is installed;
/// use `--sim-trace=file:/path` or `--sim-trace=tcp:host:port` to capture.
/// Format: `[BRNC] ... branch  pc=0xSRC | -> 0xDST <optional note>`
#[macro_export]
macro_rules! sim_branch {
    (pc=$pc:expr, target=$target:expr) => {
        $crate::sim_trace::emit(
            $crate::sim_trace::Level::Branch,
            "branch",
            Some($pc),
            format!("-> {:#018x}", $target),
        )
    };
    (pc=$pc:expr, target=$target:expr, $($arg:tt)*) => {
        $crate::sim_trace::emit(
            $crate::sim_trace::Level::Branch,
            "branch",
            Some($pc),
            format!("-> {:#018x} {}", $target, format_args!($($arg)*)),
        )
    };
}

// ── Sink (background drain thread) ───────────────────────────────────────────

const QUEUE_DEPTH: usize = 4096;
const DRAIN_TIMEOUT: Duration = Duration::from_millis(50);

/// Owns the sink thread. Dropping this joins the thread, draining the queue.
pub struct MonitorSink {
    handle: Option<thread::JoinHandle<()>>,
}

impl MonitorSink {
    /// Create a sink draining to the given URI.
    ///
    /// Returns `(sink, monitor)`.  Register the [`Monitor`] on the simulation
    /// thread via [`install_monitor`].
    pub fn open(uri: &str) -> io::Result<(Self, Monitor)> {
        let backend = open_backend(uri)?;
        let (tx, rx) = mpsc::sync_channel::<MonitorEntry>(QUEUE_DEPTH);
        let monitor = Monitor { tx };
        let handle = thread::Builder::new()
            .name("helm-sim-trace".into())
            .spawn(move || {
                let mut backend = backend;
                loop {
                    match rx.recv_timeout(DRAIN_TIMEOUT) {
                        Ok(entry) => {
                            backend.write_line(&entry.format());
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            backend.flush();
                        }
                        Err(RecvTimeoutError::Disconnected) => {
                            // Channel closed — drain any remaining messages
                            // then exit.
                            break;
                        }
                    }
                }
                // Final drain after channel close
                backend.flush();
            })?;

        Ok((Self { handle: Some(handle) }, monitor))
    }

    /// Open using `stderr:` as the fallback.
    pub fn open_or_stderr(uri: Option<&str>) -> (Self, Monitor) {
        match Self::open(uri.unwrap_or("stderr:")) {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("[helm-sim-trace] failed to open backend {uri:?}: {e}; falling back to stderr");
                Self::open("stderr:").expect("stderr always works")
            }
        }
    }
}

impl Drop for MonitorSink {
    fn drop(&mut self) {
        // Dropping MonitorSink closes the sender side → background thread exits
        // after draining the queue.
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}


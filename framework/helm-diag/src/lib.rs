// helm-diag — Async structured diagnostic channel for helm-ng simulator.
//
// Zero mandatory dependencies. Any crate in the project can depend on this
// without risk of creating a dependency cycle.

#![allow(missing_docs)]

pub mod entry;
pub mod sink;
#[macro_use]
pub mod macros;

pub use entry::{DiagEntry, DiagLevel, DiagContext};
pub use sink::DiagSink;

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Mutex;

static GLOBAL_MONITOR: Mutex<Option<DiagMonitor>> = Mutex::new(None);
static GLOBAL_MONITOR_ACTIVE: AtomicBool = AtomicBool::new(false);

// -- Thread-local simulation context -----------------------------------------

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

/// Register a [`DiagMonitor`] on the calling thread.
///
/// After this call, [`emit`] will route entries through the monitor rather than
/// falling back to `eprintln!`. Replaces any previously installed monitor.
pub fn install_monitor(m: DiagMonitor) {
    DIAG_MONITOR.with(|cell| *cell.borrow_mut() = Some(m.clone()));
    *GLOBAL_MONITOR.lock().unwrap() = Some(m);
    GLOBAL_MONITOR_ACTIVE.store(true, Ordering::Release);
}

/// Unregister the current thread's monitor.
///
/// After this call, [`emit`] falls back to `eprintln!` for non-Info levels.
pub fn uninstall_monitor() {
    DIAG_MONITOR.with(|cell| *cell.borrow_mut() = None);
    *GLOBAL_MONITOR.lock().unwrap() = None;
    GLOBAL_MONITOR_ACTIVE.store(false, Ordering::Release);
}

/// Returns `true` if a [`DiagMonitor`] is installed on the calling thread.
///
/// Used by the engine to skip the `update_sim_ctx` RefCell borrow when no
/// diagnostic backend is active (measurable overhead at simulation speed).
#[inline]
pub fn is_monitor_active() -> bool {
    GLOBAL_MONITOR_ACTIVE.load(Ordering::Acquire)
}

/// Update the thread-local simulation context.
///
/// The engine calls this before each instruction step (or at quantum boundaries
/// in bulk-step mode). The `freq_hz` parameter converts instruction counts to
/// nanoseconds. Pass `freq_hz = 0` to store raw instruction counts in `sim_ns`
/// (useful before a frequency is known, e.g. during ELF loading).
///
/// # Arguments
/// - `insns` -- total instructions retired so far on this thread
/// - `freq_hz` -- simulated CPU frequency in Hz (e.g. `1_000_000_000` for 1 GHz)
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

// -- DiagMonitor -------------------------------------------------------------

/// Cheap, clonable sender handle.
///
/// `DiagMonitor` is the only type that hot-path code holds. It is a thin wrapper
/// around a `SyncSender<DiagEntry>`. Cloning it is O(1) (reference-counted
/// under the hood by `SyncSender`). Calling [`try_send`](DiagMonitor::try_send)
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

// -- emit() ------------------------------------------------------------------

/// Emit a diagnostic entry from the calling thread.
///
/// Reads the thread-local `SIM_CTX` to stamp `sim_ns` and `sim_insns`.
/// Attempts a non-blocking send via the thread-local `DIAG_MONITOR`.
///
/// **Fallback behavior when no monitor is installed:**
/// - All levels: `eprintln!` the formatted entry.
///   This ensures diagnostics are always visible even without a configured backend.
/// - If the `log-fallback` feature is enabled, routes through the `log` crate
///   instead of `eprintln!`.
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
    }) || {
        if let Some(ref m) = *GLOBAL_MONITOR.lock().unwrap() {
            m.try_send(entry.clone());
            true
        } else {
            false
        }
    };

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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod threadlocal_tests {
    use super::{install_monitor, uninstall_monitor, DIAG_MONITOR, SIM_CTX, update_sim_ctx};
    use crate::sink::DiagSink;

    // T-TL-01
    #[test]
    fn install_sets_monitor() {
        let (_sink, monitor) = DiagSink::open("null:").unwrap();
        install_monitor(monitor);
        let is_some = DIAG_MONITOR.with(|c| c.borrow().is_some());
        assert!(is_some, "DIAG_MONITOR must be Some after install_monitor");
        uninstall_monitor();
    }

    // T-TL-02
    #[test]
    fn uninstall_clears_monitor() {
        let (_sink, monitor) = DiagSink::open("null:").unwrap();
        install_monitor(monitor);
        uninstall_monitor();
        let is_none = DIAG_MONITOR.with(|c| c.borrow().is_none());
        assert!(is_none, "DIAG_MONITOR must be None after uninstall_monitor");
    }

    // T-TL-03
    #[test]
    fn update_sim_ctx_computes_ns() {
        update_sim_ctx(1_000_000_000, 1_000_000_000);
        let (ns, insns) = SIM_CTX.with(|c| {
            let ctx = c.borrow();
            (ctx.sim_ns, ctx.sim_insns)
        });
        assert_eq!(insns, 1_000_000_000);
        assert_eq!(ns,    1_000_000_000);
    }

    // T-TL-04
    #[test]
    fn update_sim_ctx_zero_freq_stores_insns_in_ns() {
        update_sim_ctx(42, 0);
        let ns = SIM_CTX.with(|c| c.borrow().sim_ns);
        assert_eq!(ns, 42, "zero freq must store insns directly in sim_ns");
    }

    // T-TL-05
    #[test]
    fn install_monitor_replaces_previous() {
        let (_s1, m1) = DiagSink::open("null:").unwrap();
        let (_s2, m2) = DiagSink::open("null:").unwrap();
        install_monitor(m1);
        install_monitor(m2);
        uninstall_monitor();
    }

    // T-TL-06
    #[test]
    fn new_thread_has_no_monitor() {
        let handle = std::thread::spawn(|| {
            DIAG_MONITOR.with(|c| c.borrow().is_none())
        });
        assert!(handle.join().unwrap(), "new thread must start with no monitor");
    }
}

#[cfg(test)]
mod emit_tests {
    use super::{emit, install_monitor, uninstall_monitor, DIAG_MONITOR};
    use crate::DiagLevel;
    use crate::sink::DiagSink;

    // T-EMIT-01
    #[test]
    fn emit_without_monitor_does_not_panic() {
        uninstall_monitor();
        emit(DiagLevel::Stub, "test", None, "no monitor".to_string());
    }

    // T-EMIT-02
    #[test]
    fn emit_with_null_monitor_is_nonblocking() {
        let (sink, monitor) = DiagSink::open("null:").unwrap();
        install_monitor(monitor);
        emit(DiagLevel::Info, "test", None, "null backend".to_string());
        uninstall_monitor();
        drop(sink);
    }

    // T-EMIT-03
    #[test]
    fn emit_with_file_monitor_writes_to_file() {
        use std::io::BufRead;
        let path = std::env::temp_dir().join("helm-diag-emit-test.log");
        std::fs::remove_file(&path).ok();
        let uri = format!("file:{}", path.display());
        {
            let (sink, monitor) = DiagSink::open(&uri).unwrap();
            install_monitor(monitor);
            emit(DiagLevel::Warn, "emit-test", Some(0xDEAD_BEEF), "via emit".to_string());
            uninstall_monitor();
            drop(sink);
        }
        let f = std::fs::File::open(&path).unwrap();
        let lines: Vec<_> = std::io::BufReader::new(f)
            .lines()
            .map(|l| l.unwrap())
            .collect();
        assert!(!lines.is_empty());
        assert!(lines[0].contains("[WARN]"),       "must contain level: {:?}", lines[0]);
        assert!(lines[0].contains("emit-test"),    "must contain component: {:?}", lines[0]);
        assert!(lines[0].contains("via emit"),     "must contain message: {:?}", lines[0]);
        assert!(lines[0].contains("0x00000000deadbeef"),
             "must contain pc: {:?}", lines[0]);
        std::fs::remove_file(&path).ok();
    }

    // T-EMIT-04
    #[test]
    fn emit_all_levels_no_panic() {
        let (sink, monitor) = DiagSink::open("null:").unwrap();
        install_monitor(monitor);
        for &level in &[DiagLevel::Info, DiagLevel::Stub, DiagLevel::Warn, DiagLevel::Error] {
            emit(level, "test", None, format!("{level:?} message"));
        }
        uninstall_monitor();
        drop(sink);
    }

    #[test]
    fn emit_uses_global_monitor_when_thread_local_is_missing() {
        use std::io::BufRead;

        let path = std::env::temp_dir().join("helm-diag-global-fallback-test.log");
        std::fs::remove_file(&path).ok();
        let uri = format!("file:{}", path.display());
        {
            let (sink, monitor) = DiagSink::open(&uri).unwrap();
            install_monitor(monitor);
            DIAG_MONITOR.with(|cell| *cell.borrow_mut() = None);
            emit(
                DiagLevel::Info,
                "emit-test",
                Some(0x1234),
                "global fallback".to_string(),
            );
            uninstall_monitor();
            drop(sink);
        }
        let f = std::fs::File::open(&path).unwrap();
        let lines: Vec<_> = std::io::BufReader::new(f)
            .lines()
            .map(|l| l.unwrap())
            .collect();
        assert!(!lines.is_empty());
        assert!(lines[0].contains("global fallback"), "got: {:?}", lines[0]);
        assert!(lines[0].contains("pc=0x0000000000001234"), "got: {:?}", lines[0]);
        std::fs::remove_file(&path).ok();
    }
}

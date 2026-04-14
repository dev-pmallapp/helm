// src/sink.rs — DiagSink (background drain thread) and Backend enum

use crate::entry::DiagEntry;
use crate::DiagMonitor;

use std::io::{self, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// -- Constants ---------------------------------------------------------------

/// Bounded channel depth. At 4096 entries, a full queue of STUB messages at
/// 1 GHz would represent ~4 us of simulation time -- plenty of headroom.
const QUEUE_DEPTH: usize = 4096;

/// How long the drain thread waits for new entries before flushing the backend.
/// A 50 ms timeout means the file backend flushes at most 20 times per second
/// when the queue is idle.
const DRAIN_TIMEOUT: Duration = Duration::from_millis(50);

// -- Backend -----------------------------------------------------------------

/// Write destination for the drain thread.
///
/// `Backend` is not public -- callers interact only through the URI string passed
/// to [`DiagSink::open`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagMonitorKind {
    Stderr,
    File,
    Tcp,
    Null,
}

#[derive(Debug)]
pub(crate) enum Backend {
    Stderr,
    File(std::fs::File),
    Tcp(Arc<Mutex<TcpStream>>),
    Null,
}

impl Backend {
    pub(crate) fn kind(&self) -> DiagMonitorKind {
        match self {
            Backend::Stderr => DiagMonitorKind::Stderr,
            Backend::File(_) => DiagMonitorKind::File,
            Backend::Tcp(_) => DiagMonitorKind::Tcp,
            Backend::Null => DiagMonitorKind::Null,
        }
    }

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

// -- DiagSink ----------------------------------------------------------------

/// Owns the background drain thread.
///
/// Dropping `DiagSink` joins the drain thread, ensuring all pending entries are
/// written before the sink is destroyed. This guarantee is critical for the
/// `file:` and `tcp:` backends -- the last diagnostic messages are never silently
/// lost.
pub struct DiagSink {
    /// `None` after `drop` has joined the thread.
    handle: Option<thread::JoinHandle<()>>,
}

impl DiagSink {
    /// Open a sink draining to the given URI.
    ///
    /// Returns `(DiagSink, DiagMonitor)`. The `DiagMonitor` is the cheap sender
    /// that the simulation thread holds.
    ///
    /// # URI formats
    /// - `stderr:` or `stderr` or `""` -- write to stderr (always available)
    /// - `null:` or `null`             -- discard all entries
    /// - `file:/path/to/file`          -- append to file; create if absent
    /// - `tcp:host:port`               -- connect to TCP listener; stream lines
    ///
    /// # Errors
    /// Returns `Err` if the URI is malformed, the file cannot be created, or the
    /// TCP connection is refused.
    pub fn open(uri: &str) -> io::Result<(Self, DiagMonitor)> {
        let backend = open_backend(uri)?;
        let (tx, rx) = mpsc::sync_channel::<DiagEntry>(QUEUE_DEPTH);
        let monitor = DiagMonitor {
            tx,
            kind: backend.kind(),
        };

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
                            // Periodic flush -- keeps file buffering from
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

        Ok((
            Self {
                handle: Some(handle),
            },
            monitor,
        ))
    }

    /// Open a sink, falling back to `stderr:` if the URI fails.
    ///
    /// Logs a warning to `eprintln!` when the primary URI fails.
    /// Never panics.
    pub fn open_or_stderr(uri: Option<&str>) -> (Self, DiagMonitor) {
        let effective_uri = uri.unwrap_or("stderr:");
        match Self::open(effective_uri) {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!(
                    "[helm-diag] failed to open backend {effective_uri:?}: {e}; falling back to stderr"
                );
                // stderr always succeeds.
                Self::open("stderr:").expect("stderr always works")
            }
        }
    }
}

impl Drop for DiagSink {
    /// Join the drain thread on drop.
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// -- URI Backend Parser ------------------------------------------------------

/// Parse a sim-trace URI string into an open [`Backend`].
pub(crate) fn open_backend(uri: &str) -> io::Result<Backend> {
    // Empty string and bare "stderr" both map to Stderr.
    if uri.is_empty() || uri == "stderr" || uri == "stderr:" {
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
        let stream = TcpStream::connect(rest)
            .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod uri_tests {
    use super::open_backend;

    // T-URI-01
    #[test]
    fn empty_string_is_stderr() {
        assert!(open_backend("").is_ok());
    }

    // T-URI-02
    #[test]
    fn bare_stderr_is_stderr() {
        assert!(open_backend("stderr").is_ok());
    }

    // T-URI-03
    #[test]
    fn stderr_colon_is_stderr() {
        assert!(open_backend("stderr:").is_ok());
    }

    // T-URI-04
    #[test]
    fn bare_null_is_null() {
        assert!(open_backend("null").is_ok());
    }

    // T-URI-05
    #[test]
    fn null_colon_is_null() {
        assert!(open_backend("null:").is_ok());
    }

    // T-URI-06
    #[test]
    fn file_uri_opens_file() {
        let path = std::env::temp_dir().join("helm-diag-uri-test.log");
        let uri = format!("file:{}", path.display());
        let result = open_backend(&uri);
        assert!(result.is_ok(), "file URI must succeed: {result:?}");
        std::fs::remove_file(&path).ok();
    }

    // T-URI-07
    #[test]
    fn unknown_scheme_returns_err() {
        let result = open_backend("bogus:something");
        assert!(result.is_err(), "unknown scheme must return Err");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("helm-diag"),
            "error must mention helm-diag: {msg}"
        );
    }

    // T-URI-08
    #[test]
    fn tcp_unreachable_returns_err() {
        let result = open_backend("tcp:127.0.0.1:1");
        assert!(result.is_err(), "unreachable TCP must return Err");
    }
}

#[cfg(test)]
mod sink_tests {
    use super::DiagSink;
    use crate::{DiagEntry, DiagLevel};

    fn make_entry(level: DiagLevel, msg: &str) -> DiagEntry {
        DiagEntry {
            sim_ns: 0,
            sim_insns: 0,
            component: "test",
            level,
            pc: None,
            message: msg.to_string(),
        }
    }

    // T-SINK-01
    #[test]
    fn null_backend_is_nonblocking() {
        let (sink, monitor) = DiagSink::open("null:").unwrap();
        for i in 0..10_000u64 {
            monitor.try_send(DiagEntry {
                sim_ns: i,
                sim_insns: i,
                component: "test",
                level: DiagLevel::Info,
                pc: None,
                message: format!("msg {i}"),
            });
        }
        drop(monitor); // must drop sender before sink so drain thread exits
        drop(sink);
    }

    // T-SINK-02
    #[test]
    fn stderr_backend_accepts_entry() {
        let (sink, monitor) = DiagSink::open("stderr:").unwrap();
        monitor.try_send(make_entry(DiagLevel::Info, "sink test from stderr backend"));
        drop(monitor);
        drop(sink);
    }

    // T-SINK-03
    #[test]
    fn file_backend_writes_and_reads() {
        use std::io::BufRead;
        let path = std::env::temp_dir().join("helm-diag-sink-test.log");
        // Clean up any leftover from prior runs.
        std::fs::remove_file(&path).ok();
        let uri = format!("file:{}", path.display());
        {
            let (sink, monitor) = DiagSink::open(&uri).unwrap();
            monitor.try_send(make_entry(DiagLevel::Stub, "written by sink test"));
            drop(monitor); // must drop sender before sink
            drop(sink);
        }
        let f = std::fs::File::open(&path).unwrap();
        let lines: Vec<_> = std::io::BufReader::new(f)
            .lines()
            .map(|l| l.unwrap())
            .collect();
        assert!(!lines.is_empty(), "file must contain at least one line");
        assert!(
            lines[0].contains("written by sink test"),
            "line must contain the message: {:?}",
            lines[0]
        );
        std::fs::remove_file(&path).ok();
    }

    // T-SINK-04
    #[test]
    fn drop_joins_drain_thread() {
        let (sink, monitor) = DiagSink::open("null:").unwrap();
        monitor.try_send(make_entry(DiagLevel::Info, "pre-drop"));
        // drop must return; if drain thread hangs, this test times out.
        drop(monitor);
        drop(sink);
    }

    // T-SINK-05
    #[test]
    fn open_or_stderr_with_none_returns_stderr() {
        let (sink, monitor) = DiagSink::open_or_stderr(None);
        monitor.try_send(make_entry(DiagLevel::Info, "open_or_stderr test"));
        drop(monitor);
        drop(sink);
    }

    // T-SINK-06
    #[test]
    fn open_or_stderr_falls_back_on_bad_uri() {
        let (sink, monitor) = DiagSink::open_or_stderr(Some("bogus:uri"));
        monitor.try_send(make_entry(DiagLevel::Warn, "fallback test"));
        drop(monitor);
        drop(sink);
    }

    // T-SINK-07
    #[test]
    fn try_send_does_not_block_when_queue_full() {
        let (sink, monitor) = DiagSink::open("null:").unwrap();
        for _ in 0..4097 {
            monitor.try_send(make_entry(DiagLevel::Info, "overflow"));
        }
        drop(monitor);
        drop(sink);
    }
}

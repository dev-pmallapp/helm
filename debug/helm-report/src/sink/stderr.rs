// src/sink/stderr.rs -- StderrSink: writes to stderr, no buffering.

use super::Sink;
use std::io::{self, Write};

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

    fn name(&self) -> &str {
        "stderr"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::Sink;

    #[test]
    fn stderr_sink_write_does_not_panic() {
        let sink = StderrSink;
        assert!(sink.write(b"helm-report StderrSink test\n").is_ok());
    }

    #[test]
    fn stderr_sink_flush_ok() {
        let sink = StderrSink;
        assert!(sink.flush().is_ok());
    }

    #[test]
    fn stderr_sink_name() {
        assert_eq!(StderrSink.name(), "stderr");
    }
}

// src/sink/stderr.rs -- StderrSink: writes to stderr, no buffering.
//
// Dual-impl per `docs/design/helm-report/HLD.md` § 13: with `report`
// enabled, the type drains to `io::stderr()`. Without it, the type is
// a ZST whose `write()` returns `Ok(())` (no allocation, no I/O).

#[cfg(feature = "report")]
pub use live::StderrSink;
#[cfg(not(feature = "report"))]
pub use noop::StderrSink;

#[cfg(feature = "report")]
mod live {
    use crate::sink::Sink;
    use std::io::{self, Write};

    /// Writes to stderr. No buffering. Always available.
    ///
    /// Thread safety: `io::stderr()` is inherently synchronized on
    /// all major platforms (POSIX: stderr is unbuffered). No Mutex
    /// needed.
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
}

#[cfg(not(feature = "report"))]
mod noop {
    use crate::sink::Sink;
    use std::io;

    /// ZST shell when `report` is off. `write()` is an inlined no-op.
    pub struct StderrSink;

    impl Sink for StderrSink {
        #[inline(always)]
        fn write(&self, _data: &[u8]) -> io::Result<()> {
            Ok(())
        }

        #[inline(always)]
        fn name(&self) -> &str {
            "stderr"
        }
    }
}

#[cfg(all(test, feature = "report"))]
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

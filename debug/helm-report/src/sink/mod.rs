// src/sink/mod.rs -- Sink trait definition and submodule re-exports.

pub mod async_file;
pub mod binary;
pub mod file;
pub mod null;
pub mod python;
pub mod stderr;
pub mod tcp;
pub mod uri;

use std::io;

/// A delivery destination for report data.
///
/// Implementations MUST be `Send + Sync` -- the engine may deliver from
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
    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
    fn name(&self) -> &str;
}

pub use self::async_file::AsyncFileSink;
pub use self::binary::{BinaryTraceSink, TraceFileHeader, HELM_TRACE_MAGIC, HELM_TRACE_VERSION};
pub use self::file::FileSink;
pub use self::null::NullSink;
pub use self::python::PythonSink;
pub use self::stderr::StderrSink;
pub use self::tcp::TcpSink;
pub use self::uri::sink_from_uri;

/// In-memory sink for testing. Captures all written bytes and flush counts.
#[cfg(test)]
pub struct TestSink {
    pub written: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    pub flushes: std::sync::Arc<std::sync::Mutex<u32>>,
    pub sink_name: &'static str,
}

#[cfg(test)]
impl TestSink {
    pub fn new(name: &'static str) -> Self {
        TestSink {
            written: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            flushes: std::sync::Arc::new(std::sync::Mutex::new(0)),
            sink_name: name,
        }
    }

    pub fn contents(&self) -> Vec<u8> {
        self.written.lock().unwrap().clone()
    }

    pub fn flush_count(&self) -> u32 {
        *self.flushes.lock().unwrap()
    }

    pub fn contents_as_string(&self) -> String {
        String::from_utf8_lossy(&self.contents()).into_owned()
    }
}

#[cfg(test)]
impl Sink for TestSink {
    fn write(&self, data: &[u8]) -> io::Result<()> {
        self.written.lock().unwrap().extend_from_slice(data);
        Ok(())
    }

    fn flush(&self) -> io::Result<()> {
        *self.flushes.lock().unwrap() += 1;
        Ok(())
    }

    fn name(&self) -> &str {
        self.sink_name
    }
}

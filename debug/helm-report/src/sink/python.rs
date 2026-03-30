// src/sink/python.rs -- PythonSink: GIL-safe string buffer for Python polling.

use super::Sink;
use std::io;
use std::sync::{Arc, Mutex};

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
        PythonSink {
            buf: Arc::new(Mutex::new(Vec::new())),
        }
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
    fn default() -> Self {
        PythonSink::new()
    }
}

impl Sink for PythonSink {
    fn write(&self, data: &[u8]) -> io::Result<()> {
        let s = String::from_utf8_lossy(data).into_owned();
        self.buf.lock().unwrap().push(s);
        Ok(())
    }

    fn name(&self) -> &str {
        "python"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::Sink;

    #[test]
    fn python_sink_write_and_drain() {
        let sink = PythonSink::new();
        sink.write(b"first line\n").unwrap();
        sink.write(b"second line\n").unwrap();

        let lines = sink.drain();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "first line\n");
        assert_eq!(lines[1], "second line\n");

        // After drain, buffer is empty.
        let lines2 = sink.drain();
        assert!(lines2.is_empty());
    }

    #[test]
    fn python_sink_name() {
        assert_eq!(PythonSink::new().name(), "python");
    }

    #[test]
    fn python_sink_inner_shares_state() {
        let sink = PythonSink::new();
        let inner = sink.inner();
        sink.write(b"test\n").unwrap();
        let guard = inner.lock().unwrap();
        assert_eq!(guard.len(), 1);
    }
}

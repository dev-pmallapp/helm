// src/sink/null.rs -- NullSink: discards all writes.
//
// Identical impl in both feature modes -- `NullSink` is the canonical
// "no I/O, no allocation" sink and stays a ZST always. Keeping it
// available without `report` lets perf builds construct a
// `Box<dyn Sink>` for trait-object call sites without pulling in any
// formatter machinery.

use super::Sink;
use std::io;

/// Discards all writes. Used for benchmarking formatter overhead in
/// isolation and as the sole concrete `Sink` in perf builds.
///
/// `write()` and `flush()` are always `Ok(())`. No allocation, no I/O.
pub struct NullSink;

impl Sink for NullSink {
    #[inline(always)]
    fn write(&self, _data: &[u8]) -> io::Result<()> {
        Ok(())
    }

    #[inline(always)]
    fn name(&self) -> &str {
        "null"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::Sink;

    #[test]
    fn null_sink_write_always_ok() {
        let sink = NullSink;
        assert!(sink.write(&[0u8; 1024 * 1024]).is_ok());
    }

    #[test]
    fn null_sink_flush_always_ok() {
        let sink = NullSink;
        assert!(sink.flush().is_ok());
    }

    #[test]
    fn null_sink_name() {
        assert_eq!(NullSink.name(), "null");
    }

    #[test]
    fn null_sink_is_non_blocking() {
        use std::time::Instant;
        let sink = NullSink;
        let big = vec![0u8; 64 * 1024 * 1024];
        let t0 = Instant::now();
        for _ in 0..100 {
            sink.write(&big).unwrap();
        }
        assert!(
            t0.elapsed().as_millis() < 100,
            "NullSink::write is not O(1)"
        );
    }
}

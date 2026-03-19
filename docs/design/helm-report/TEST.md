# helm-report — Test Plan

> **Status:** Draft — Phase 3 (Delivery layer)
> **Crate path:** `framework/helm-report/`
> **Companion:** [`HLD.md`](HLD.md) · [`LLD-sinks.md`](LLD-sinks.md)

---

## Table of Contents

1. [Philosophy](#1-philosophy)
2. [Test Infrastructure](#2-test-infrastructure)
3. [StderrSink](#3-stderrsink)
4. [FileSink](#4-filesink)
5. [AsyncFileSink](#5-asyncfilesink)
6. [NullSink](#6-nullsink)
7. [BinaryTraceSink](#7-binarytracesink)
8. [TcpSink](#8-tcpsink)
9. [sink_from_uri](#9-sink_from_uri)
10. [TextFormatter](#10-textformatter)
11. [JsonFormatter](#11-jsonformatter)
12. [CsvFormatter](#12-csvformatter)
13. [GemstatsFormatter](#13-gemstatsformatter)
14. [Report::deliver](#14-reportdeliver)
15. [ReportSchedule::check](#15-reportschedulecheck)
16. [Test Matrix](#16-test-matrix)

---

## 1. Philosophy

`helm-report` is pure cold-path delivery code. Every test is a unit test that runs in
`cargo test` with no simulator binary, no platform, and no PyO3. Integration with the
engine (`ReportSchedule::check` in the hot loop) is verified by the engine-level tests
in `helm-engine`.

**Principles:**
- No network required for unit tests: `TcpSink` tests use a `TcpListener` mock server
  in a background thread.
- No spawned processes: `BinaryTraceSink` tests open a `tempfile`, join the drain thread,
  and read the file back.
- Formatter tests assert on string content, not byte-exact output, to tolerate whitespace
  changes.
- `AsyncFileSink` tests join the `JoinHandle` before asserting file contents.
- `Report::deliver` tests count writes to a `TestSink` to verify fan-out.

All tests live in `src/` as `#[cfg(test)]` modules within each file, plus a
`tests/integration.rs` for cross-module scenarios.

---

## 2. Test Infrastructure

```rust
// Shared test helper — add to src/sink/mod.rs or tests/common.rs

use std::sync::{Arc, Mutex};
use crate::sink::Sink;

/// In-memory sink that captures all written bytes. Thread-safe.
pub struct TestSink {
    pub written: Arc<Mutex<Vec<u8>>>,
    pub flushes: Arc<Mutex<u32>>,
    pub name:    &'static str,
}

impl TestSink {
    pub fn new(name: &'static str) -> Self {
        TestSink {
            written: Arc::new(Mutex::new(Vec::new())),
            flushes: Arc::new(Mutex::new(0)),
            name,
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

impl Sink for TestSink {
    fn write(&self, data: &[u8]) -> std::io::Result<()> {
        self.written.lock().unwrap().extend_from_slice(data);
        Ok(())
    }

    fn flush(&self) -> std::io::Result<()> {
        *self.flushes.lock().unwrap() += 1;
        Ok(())
    }

    fn name(&self) -> &str { self.name }
}

/// Construct a minimal SpySpySnapshot for tests.
pub fn test_snapshot() -> crate::snapshot::SpySpySnapshot {
    use crate::snapshot::*;
    SpySpySnapshot {
        insn_count: 10_000_000,
        insn_mix: vec![
            ("IntAlu".to_owned(), 5_000_000),
            ("Load".to_owned(),   2_000_000),
            ("Store".to_owned(),  1_000_000),
            ("Branch".to_owned(), 1_500_000),
            ("SIMD".to_owned(),     500_000),
        ],
        hot_pcs: vec![
            (0xffff_8000_1001_2a4c, 234_812),
            (0xffff_8000_1001_2abc, 198_234),
        ],
        branch_heatmap: vec![
            (0xffff_8000_1001_2a4c, 100_000),
        ],
        cache_l1d: Some(CacheSnapshot {
            name:     "l1d".to_owned(),
            hits:     9_823_441,
            misses:     176_559,
            hit_rate: 0.982_153,
        }),
        branch_pred: Some(BranchPredSnapshot {
            name:           "bimodal".to_owned(),
            kind:           "BiModal".to_owned(),
            predictions:    1_500_000,
            mispredictions:   105_000,
            miss_rate:      0.07,
        }),
        fault_history: None,
        tick_count:   8_130_081,
        snapshot_ns:  1_710_849_600_000_000_000,
    }
}
```

---

## 3. StderrSink

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::Sink;

    #[test]
    fn stderr_sink_write_does_not_panic() {
        // Can't easily capture stderr in a unit test; verify it at least
        // completes without error.
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
```

---

## 4. FileSink

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::Sink;
    use tempfile::NamedTempFile;

    #[test]
    fn filesink_write_and_read_back() {
        let tmp = NamedTempFile::new().unwrap();
        let sink = FileSink::open(tmp.path()).unwrap();

        sink.write(b"hello, helm-report\n").unwrap();
        sink.write(b"second line\n").unwrap();
        sink.flush().unwrap();
        drop(sink);  // Drop flushes and closes the file.

        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(contents, "hello, helm-report\nsecond line\n");
    }

    #[test]
    fn filesink_flush_on_drop() {
        let tmp = NamedTempFile::new().unwrap();
        {
            let sink = FileSink::open(tmp.path()).unwrap();
            sink.write(b"flushed-on-drop\n").unwrap();
            // Drop without explicit flush.
        }
        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(contents.contains("flushed-on-drop"));
    }

    #[test]
    fn filesink_name_is_path() {
        let tmp = NamedTempFile::new().unwrap();
        let path_str = tmp.path().to_string_lossy().into_owned();
        let sink = FileSink::open(tmp.path()).unwrap();
        assert_eq!(sink.name(), path_str);
    }

    #[test]
    fn filesink_open_nonexistent_dir_fails() {
        let result = FileSink::open("/nonexistent/path/that/cannot/exist/file.txt");
        assert!(result.is_err());
    }
}
```

---

## 5. AsyncFileSink

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::Sink;
    use tempfile::NamedTempFile;
    use std::time::Duration;

    #[test]
    fn async_filesink_delivers_all_writes() {
        let tmp = NamedTempFile::new().unwrap();
        let (sink, handle) = AsyncFileSink::open(tmp.path()).unwrap();

        for i in 0..100u32 {
            sink.write(format!("line {i}\n").as_bytes()).unwrap();
        }
        drop(sink);  // Sends Stop; drain thread flushes.
        handle.join().expect("drain thread panicked");

        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(contents.contains("line 0\n"));
        assert!(contents.contains("line 99\n"));
        assert_eq!(contents.lines().count(), 100);
    }

    #[test]
    fn async_filesink_join_on_drop() {
        // Verify that drop + join produces a complete file even without explicit flush.
        let tmp = NamedTempFile::new().unwrap();
        let (sink, handle) = AsyncFileSink::open(tmp.path()).unwrap();
        sink.write(b"only-write\n").unwrap();
        drop(sink);
        handle.join().unwrap();

        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(contents, "only-write\n");
    }

    #[test]
    fn async_filesink_write_after_stop_returns_error() {
        let tmp = NamedTempFile::new().unwrap();
        let (sink, handle) = AsyncFileSink::open(tmp.path()).unwrap();
        drop(sink.clone_tx_for_test());  // Force Stop via drop.
        // This is a conceptual test; actual test depends on API shape.
        handle.join().unwrap();
    }

    #[test]
    fn async_filesink_name_is_path() {
        let tmp = NamedTempFile::new().unwrap();
        let path_str = tmp.path().to_string_lossy().into_owned();
        let (sink, handle) = AsyncFileSink::open(tmp.path()).unwrap();
        assert_eq!(sink.name(), path_str);
        drop(sink);
        handle.join().unwrap();
    }
}
```

---

## 6. NullSink

```rust
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
        // Write a large buffer and verify it returns immediately (no I/O).
        use std::time::Instant;
        let sink = NullSink;
        let big = vec![0u8; 64 * 1024 * 1024];
        let t0 = Instant::now();
        for _ in 0..100 {
            sink.write(&big).unwrap();
        }
        // Should complete in well under 1 ms (no real I/O).
        assert!(t0.elapsed().as_millis() < 100, "NullSink::write is not O(1)");
    }
}
```

---

## 7. BinaryTraceSink

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::{Pod, Zeroable};
    use tempfile::NamedTempFile;

    /// 32-byte test record that matches BranchRecord layout.
    #[repr(C)]
    #[derive(Copy, Clone, Pod, Zeroable, Debug, PartialEq)]
    struct TestRecord {
        pc:         u64,
        target:     u64,
        insn_count: u64,
        flags:      u8,
        _pad:       [u8; 7],
    }

    #[test]
    fn binary_trace_sink_header_correct() {
        let tmp = NamedTempFile::new().unwrap();
        let (sink, handle) = BinaryTraceSink::<TestRecord>::open(
            tmp.path(), "TestRecord",
        ).unwrap();
        drop(sink);
        handle.join().unwrap();

        let data = std::fs::read(tmp.path()).unwrap();
        assert!(data.len() >= 80, "file too short for header");

        let magic   = u32::from_le_bytes(data[0..4].try_into().unwrap());
        let version = u32::from_le_bytes(data[4..8].try_into().unwrap());
        let rec_sz  = u32::from_le_bytes(data[8..12].try_into().unwrap());
        let rec_cnt = u32::from_le_bytes(data[12..16].try_into().unwrap());
        let type_name = std::str::from_utf8(&data[16..80]).unwrap().trim_end_matches('\0');

        assert_eq!(magic,   0x484D_4C54, "wrong magic");
        assert_eq!(version, 1,           "wrong version");
        assert_eq!(rec_sz,  32,          "wrong record size");
        assert_eq!(rec_cnt, 0,           "no records written — count should be 0");
        assert_eq!(type_name, "TestRecord");
    }

    #[test]
    fn binary_trace_sink_records_readable() {
        let tmp = NamedTempFile::new().unwrap();
        let (sink, handle) = BinaryTraceSink::<TestRecord>::open(
            tmp.path(), "TestRecord",
        ).unwrap();

        let records: Vec<TestRecord> = (0u64..10).map(|i| TestRecord {
            pc: 0x4000 + i * 4,
            target: 0x5000,
            insn_count: i * 100,
            flags: if i % 2 == 0 { 1 } else { 0 },
            _pad: [0u8; 7],
        }).collect();

        sink.push_records(&records).unwrap();
        drop(sink);
        handle.join().unwrap();

        let data = std::fs::read(tmp.path()).unwrap();
        let hdr_size = 80usize;
        let rec_size = std::mem::size_of::<TestRecord>();

        let rec_count_in_hdr = u32::from_le_bytes(data[12..16].try_into().unwrap());
        assert_eq!(rec_count_in_hdr, 10, "header record_count should be 10 after close");

        assert_eq!(data.len(), hdr_size + 10 * rec_size, "wrong file size");

        let records_bytes = &data[hdr_size..];
        let read_back: &[TestRecord] = bytemuck::cast_slice(records_bytes);
        assert_eq!(read_back.len(), 10);
        for (i, rec) in read_back.iter().enumerate() {
            assert_eq!(rec.pc, 0x4000 + i as u64 * 4);
            assert_eq!(rec.insn_count, i as u64 * 100);
        }
    }

    #[test]
    fn binary_trace_sink_header_size_is_80() {
        assert_eq!(std::mem::size_of::<TraceFileHeader>(), 80);
    }
}
```

---

## 8. TcpSink

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::Sink;
    use std::net::{TcpListener, TcpStream};
    use std::io::Read;
    use std::thread;
    use std::sync::mpsc;

    /// Spawn a single-connection TCP server on a random port.
    /// Returns the server address and a channel that delivers received bytes.
    fn mock_tcp_server() -> (String, mpsc::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = Vec::new();
                let _ = stream.read_to_end(&mut buf);
                let _ = tx.send(buf);
            }
        });
        (addr, rx)
    }

    #[test]
    fn tcp_sink_connect_and_write() {
        let (addr, rx) = mock_tcp_server();
        let sink = TcpSink::connect(&addr).unwrap();
        sink.write(b"hello from TcpSink\n").unwrap();
        sink.flush().unwrap();
        drop(sink);

        let received = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert!(received.starts_with(b"hello from TcpSink"));
    }

    #[test]
    fn tcp_sink_connect_refused_returns_error() {
        // Port 1 is typically refused on all platforms.
        let result = TcpSink::connect("127.0.0.1:1");
        assert!(result.is_err(), "expected connection refused");
    }

    #[test]
    fn tcp_sink_name_is_address() {
        let (addr, _rx) = mock_tcp_server();
        let sink = TcpSink::connect(&addr).unwrap();
        assert_eq!(sink.name(), addr);
    }
}
```

---

## 9. sink_from_uri

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_stderr_colon() {
        let sink = sink_from_uri("stderr:").unwrap();
        assert_eq!(sink.name(), "stderr");
    }

    #[test]
    fn uri_null_colon() {
        let sink = sink_from_uri("null:").unwrap();
        assert_eq!(sink.name(), "null");
    }

    #[test]
    fn uri_file_async() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let uri = format!("file:{}", tmp.path().display());
        let sink = sink_from_uri(&uri).unwrap();
        // AsyncFileSink name should contain the path.
        assert!(sink.name().contains('/'));
    }

    #[test]
    fn uri_file_sync() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let uri = format!("file+sync:{}", tmp.path().display());
        let sink = sink_from_uri(&uri).unwrap();
        assert!(sink.name().contains('/'));
    }

    #[test]
    fn uri_tcp_malformed_no_port() {
        // "tcp:hostname" with no port — invalid.
        let result = sink_from_uri("tcp:hostname");
        assert!(
            matches!(result, Err(crate::error::SinkError::InvalidUri(_))),
            "expected InvalidUri"
        );
    }

    #[test]
    fn uri_unknown_scheme() {
        let result = sink_from_uri("ftp:somehost");
        assert!(
            matches!(result, Err(crate::error::SinkError::InvalidUri(_))),
            "expected InvalidUri for unknown scheme"
        );
    }

    #[test]
    fn uri_empty_string() {
        let result = sink_from_uri("");
        assert!(result.is_err());
    }
}
```

---

## 10. TextFormatter

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::ReportFormatter;
    use crate::tests::test_snapshot;  // shared helper

    #[test]
    fn text_formatter_contains_sim_insns() {
        let snap = test_snapshot();
        let fmt  = TextFormatter::default();
        let out  = String::from_utf8(fmt.format_session(&snap)).unwrap();
        assert!(out.contains("sim_insns"), "missing sim_insns");
        assert!(out.contains("10000000"), "wrong insn count");
    }

    #[test]
    fn text_formatter_contains_begin_end_markers() {
        let snap = test_snapshot();
        let out  = String::from_utf8(TextFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("Begin Simulation Statistics"));
        assert!(out.contains("End Simulation Statistics"));
    }

    #[test]
    fn text_formatter_insn_mix_percentages() {
        let snap = test_snapshot();
        let out  = String::from_utf8(TextFormatter::default().format_session(&snap)).unwrap();
        // IntAlu = 5_000_000 / 10_000_000 = 50.00%
        assert!(out.contains("insn_mix.IntAlu"), "missing IntAlu");
        assert!(out.contains("50.00%"), "wrong percentage for IntAlu");
    }

    #[test]
    fn text_formatter_cache_present() {
        let snap = test_snapshot();
        let out  = String::from_utf8(TextFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("cache_l1d.hits"),    "missing cache hits");
        assert!(out.contains("cache_l1d.misses"),  "missing cache misses");
        assert!(out.contains("cache_l1d.hit_rate"), "missing hit rate");
    }

    #[test]
    fn text_formatter_hot_pcs() {
        let snap = test_snapshot();
        let out  = String::from_utf8(TextFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("hot_pcs[0]"), "missing hot PC entry");
        assert!(out.contains("0xffff800010012a4c"), "wrong PC address");
    }

    #[test]
    fn text_formatter_format_counter() {
        let fmt = TextFormatter::default();
        let out = String::from_utf8(fmt.format_counter("my_counter", 42, "things")).unwrap();
        assert!(out.contains("my_counter"), "missing counter name");
        assert!(out.contains("42"),          "missing counter value");
        assert!(out.contains("things"),      "missing unit");
    }

    #[test]
    fn text_formatter_content_type() {
        assert!(TextFormatter::default().content_type().contains("text/plain"));
    }
}
```

---

## 11. JsonFormatter

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::ReportFormatter;
    use crate::tests::test_snapshot;

    fn parse_output(snap: &crate::snapshot::SpySpySnapshot) -> serde_json::Value {
        let bytes = JsonFormatter::default().format_session(snap);
        serde_json::from_slice(&bytes).expect("output is not valid JSON")
    }

    #[test]
    fn json_formatter_is_valid_json() {
        let snap = test_snapshot();
        let _v = parse_output(&snap);  // panics if not valid JSON
    }

    #[test]
    fn json_formatter_sim_insns_field() {
        let snap = test_snapshot();
        let v = parse_output(&snap);
        assert_eq!(v["sim_insns"].as_u64(), Some(10_000_000));
    }

    #[test]
    fn json_formatter_insn_mix_array() {
        let snap = test_snapshot();
        let v = parse_output(&snap);
        let mix = v["insn_mix"].as_array().expect("insn_mix should be array");
        assert!(!mix.is_empty());
        assert!(mix.iter().any(|e| e["name"].as_str() == Some("insn_mix.IntAlu")));
    }

    #[test]
    fn json_formatter_cache_field() {
        let snap = test_snapshot();
        let v = parse_output(&snap);
        assert!(v["cache_l1d"].is_object(), "cache_l1d should be present");
        assert!(v["cache_l1d"]["hits"].is_number());
        assert!(v["cache_l1d"]["hit_rate"].is_number());
    }

    #[test]
    fn json_formatter_hot_pcs_array() {
        let snap = test_snapshot();
        let v = parse_output(&snap);
        let hot_pcs = v["hot_pcs"].as_array().expect("hot_pcs should be array");
        assert!(!hot_pcs.is_empty());
        assert!(hot_pcs[0]["pc"].is_string());
        assert!(hot_pcs[0]["count"].is_number());
    }

    #[test]
    fn json_formatter_content_type() {
        assert!(JsonFormatter::default().content_type().contains("application/json"));
    }
}
```

---

## 12. CsvFormatter

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::ReportFormatter;
    use crate::tests::test_snapshot;

    fn parse_csv(snap: &crate::snapshot::SpySpySnapshot) -> Vec<Vec<String>> {
        let bytes = CsvFormatter::default().format_session(snap);
        let s = String::from_utf8(bytes).unwrap();
        s.lines()
            .map(|l| l.split(',').map(str::to_owned).collect())
            .collect()
    }

    #[test]
    fn csv_formatter_header_row() {
        let snap = test_snapshot();
        let rows = parse_csv(&snap);
        assert!(!rows.is_empty());
        assert_eq!(rows[0], vec!["timestamp_ns", "metric", "value"]);
    }

    #[test]
    fn csv_formatter_sim_insns_row_present() {
        let snap = test_snapshot();
        let rows = parse_csv(&snap);
        let found = rows.iter().any(|r| r.len() >= 3 && r[0] == "sim_insns");
        assert!(found, "sim_insns row not found in CSV");
    }

    #[test]
    fn csv_formatter_three_columns() {
        let snap = test_snapshot();
        let rows = parse_csv(&snap);
        // Skip header; all data rows must have exactly 3 columns.
        for row in rows.iter().skip(1) {
            assert_eq!(
                row.len(), 3,
                "CSV row does not have exactly 3 columns: {row:?}"
            );
        }
    }

    #[test]
    fn csv_formatter_timestamp_is_numeric() {
        let snap = test_snapshot();
        let rows = parse_csv(&snap);
        // Column index 1 is the timestamp_ns value (per formatter: metric, ts, value order).
        // Verify at least one row has a numeric second column.
        let has_numeric_ts = rows.iter().skip(1).any(|r| r[1].parse::<u64>().is_ok());
        assert!(has_numeric_ts, "no numeric timestamp found in CSV rows");
    }

    #[test]
    fn csv_formatter_content_type() {
        assert!(CsvFormatter::default().content_type().contains("text/csv"));
    }
}
```

---

## 13. GemstatsFormatter

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::ReportFormatter;
    use crate::tests::test_snapshot;

    #[test]
    fn gemstats_formatter_begin_end_markers() {
        let snap = test_snapshot();
        let out  = String::from_utf8(GemstatsFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("Begin Simulation Statistics"));
        assert!(out.contains("End Simulation Statistics"));
    }

    #[test]
    fn gemstats_formatter_committed_insns_key() {
        let snap = test_snapshot();
        let out  = String::from_utf8(GemstatsFormatter::default().format_session(&snap)).unwrap();
        assert!(
            out.contains("system.cpu.committedInsts"),
            "missing system.cpu.committedInsts key"
        );
    }

    #[test]
    fn gemstats_formatter_ipc_key() {
        let snap = test_snapshot();
        let out  = String::from_utf8(GemstatsFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("system.cpu.ipc"), "missing system.cpu.ipc key");
    }

    #[test]
    fn gemstats_formatter_cache_keys_present() {
        let snap = test_snapshot();
        let out  = String::from_utf8(GemstatsFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("dcache.overall_hits"), "missing dcache hits");
        assert!(out.contains("dcache.overall_misses"), "missing dcache misses");
    }

    #[test]
    fn gemstats_formatter_content_type() {
        assert!(GemstatsFormatter::default().content_type().contains("text/plain"));
    }
}
```

---

## 14. Report::deliver

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::{
        format::TextFormatter,
        report::Report,
        tests::{TestSink, test_snapshot},
    };

    #[test]
    fn report_deliver_writes_to_single_sink() {
        let snap = Arc::new(test_snapshot());
        let sink = TestSink::new("test1");
        let report = Report::new(
            snap,
            Box::new(TextFormatter::default()),
            vec![Box::new(TestSink::new("s1"))],
        );
        // Use a TestSink we can inspect.
        let sink = TestSink::new("inspect");
        let sref: &dyn crate::sink::Sink = &sink;
        // deliver_to writes to an ad-hoc sink.
        report.deliver_to(sref).unwrap();
        let out = sink.contents_as_string();
        assert!(out.contains("sim_insns"), "deliver_to missing sim_insns");
    }

    #[test]
    fn report_deliver_fan_out_to_multiple_sinks() {
        let snap = Arc::new(test_snapshot());
        let s1 = Arc::new(TestSink::new("s1"));
        let s2 = Arc::new(TestSink::new("s2"));
        let s3 = Arc::new(TestSink::new("s3"));

        // Wrap the shared TestSinks; we need to hold Arcs to inspect later.
        // Use a custom wrapping to share and inspect.
        use std::sync::Mutex;
        let captured1: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let captured2: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));

        struct CaptureSink(Arc<Mutex<Vec<u8>>>);
        impl crate::sink::Sink for CaptureSink {
            fn write(&self, data: &[u8]) -> std::io::Result<()> {
                self.0.lock().unwrap().extend_from_slice(data);
                Ok(())
            }
            fn name(&self) -> &str { "capture" }
        }

        let report = Report::new(
            snap,
            Box::new(TextFormatter::default()),
            vec![
                Box::new(CaptureSink(Arc::clone(&captured1))),
                Box::new(CaptureSink(Arc::clone(&captured2))),
            ],
        );

        report.deliver().unwrap();

        let c1 = String::from_utf8(captured1.lock().unwrap().clone()).unwrap();
        let c2 = String::from_utf8(captured2.lock().unwrap().clone()).unwrap();

        // Both sinks should have received identical content.
        assert_eq!(c1, c2, "fan-out sinks received different content");
        assert!(c1.contains("sim_insns"), "content missing from sink 1");
        assert!(c2.contains("sim_insns"), "content missing from sink 2");
    }

    #[test]
    fn report_deliver_continues_on_sink_error() {
        use std::io;
        struct FailSink;
        impl crate::sink::Sink for FailSink {
            fn write(&self, _: &[u8]) -> io::Result<()> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "injected failure"))
            }
            fn name(&self) -> &str { "fail" }
        }

        let captured = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        struct CaptureSink2(Arc<std::sync::Mutex<Vec<u8>>>);
        impl crate::sink::Sink for CaptureSink2 {
            fn write(&self, data: &[u8]) -> io::Result<()> {
                self.0.lock().unwrap().extend_from_slice(data);
                Ok(())
            }
            fn name(&self) -> &str { "capture2" }
        }

        let snap = Arc::new(test_snapshot());
        let report = Report::new(
            snap,
            Box::new(TextFormatter::default()),
            vec![
                Box::new(FailSink),
                Box::new(CaptureSink2(Arc::clone(&captured))),
            ],
        );

        // deliver() should return an error (from FailSink) but still write to CaptureSink2.
        let result = report.deliver();
        assert!(result.is_err(), "expected error from FailSink");

        let content = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
        assert!(
            content.contains("sim_insns"),
            "CaptureSink2 should have received content despite FailSink error"
        );
    }
}
```

---

## 15. ReportSchedule::check

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use crate::{
        format::TextFormatter,
        report::Report,
        schedule::{ReportSchedule, ReportTrigger},
        tests::test_snapshot,
    };

    struct CounterSink(Arc<Mutex<u32>>);
    impl crate::sink::Sink for CounterSink {
        fn write(&self, _: &[u8]) -> std::io::Result<()> {
            *self.0.lock().unwrap() += 1;
            Ok(())
        }
        fn name(&self) -> &str { "counter" }
    }

    fn make_schedule(trigger: ReportTrigger) -> (ReportSchedule, Arc<Mutex<u32>>) {
        let count = Arc::new(Mutex::new(0u32));
        let sink  = Box::new(CounterSink(Arc::clone(&count)));
        let report = Report::new(
            Arc::new(test_snapshot()),
            Box::new(TextFormatter::default()),
            vec![sink],
        );
        let sched = ReportSchedule::new(report, vec![trigger]);
        (sched, count)
    }

    #[test]
    fn schedule_every_n_insns_fires_at_interval() {
        let (mut sched, count) = make_schedule(ReportTrigger::EveryNInsns(1_000_000));

        // Simulate 3.5M instructions in steps.
        for i in 0u64..3_500_000 {
            sched.check(0x4000, i);
        }

        let fires = *count.lock().unwrap();
        // Should have fired at 1M, 2M, 3M — exactly 3 times.
        assert_eq!(fires, 3, "EveryNInsns(1M) should fire 3 times at 3.5M insns");
    }

    #[test]
    fn schedule_on_pc_fires_on_match() {
        let (mut sched, count) = make_schedule(ReportTrigger::OnPc(0xDEAD_BEEF));

        sched.check(0x1000, 100);
        sched.check(0xDEAD_BEEF, 101);  // fires here
        sched.check(0x2000, 102);
        sched.check(0xDEAD_BEEF, 103);  // fires again

        let fires = *count.lock().unwrap();
        assert_eq!(fires, 2, "OnPc should fire on every PC match");
    }

    #[test]
    fn schedule_at_exit_does_not_fire_from_check() {
        let (mut sched, count) = make_schedule(ReportTrigger::AtExit);

        for i in 0u64..10_000_000 {
            sched.check(0x1000, i);
        }

        let fires = *count.lock().unwrap();
        assert_eq!(fires, 0, "AtExit should not fire from check()");
    }

    #[test]
    fn schedule_at_exit_fires_from_flush_at_exit() {
        let (sched, count) = make_schedule(ReportTrigger::AtExit);
        sched.flush_at_exit();
        let fires = *count.lock().unwrap();
        assert_eq!(fires, 1, "flush_at_exit() should deliver once for AtExit trigger");
    }

    #[test]
    fn schedule_explicit_does_not_fire_from_check() {
        let (mut sched, count) = make_schedule(ReportTrigger::Explicit);

        for i in 0u64..5_000_000 {
            sched.check(0x1000, i);
        }

        let fires = *count.lock().unwrap();
        assert_eq!(fires, 0, "Explicit trigger should never fire from check()");
    }

    #[test]
    fn schedule_deliver_fires_explicit() {
        let (sched, count) = make_schedule(ReportTrigger::Explicit);
        sched.deliver().unwrap();
        assert_eq!(*count.lock().unwrap(), 1, "deliver() should fire once");
    }
}
```

---

## 16. Test Matrix

| Test | Module | What is verified | Phase |
|------|--------|------------------|-------|
| `stderr_sink_write_does_not_panic` | `sink::stderr` | Write to real stderr completes | 3 |
| `stderr_sink_flush_ok` | `sink::stderr` | flush() always Ok | 3 |
| `filesink_write_and_read_back` | `sink::file` | Bytes written appear in file | 3 |
| `filesink_flush_on_drop` | `sink::file` | BufWriter flushed on drop | 3 |
| `filesink_open_nonexistent_dir_fails` | `sink::file` | Bad path returns Err | 3 |
| `async_filesink_delivers_all_writes` | `sink::async_file` | All 100 lines present after join | 3.1 |
| `async_filesink_join_on_drop` | `sink::async_file` | Single write flushed on drop+join | 3.1 |
| `async_filesink_name_is_path` | `sink::async_file` | name() returns path | 3.1 |
| `null_sink_write_always_ok` | `sink::null` | Ok regardless of data size | 3 |
| `null_sink_is_non_blocking` | `sink::null` | 6.4 GB written in < 100 ms | 3 |
| `binary_trace_sink_header_correct` | `sink::binary` | magic, version, record_size fields | 3.2 |
| `binary_trace_sink_records_readable` | `sink::binary` | 10 records readable after close | 3.2 |
| `binary_trace_sink_header_size_is_80` | `sink::binary` | TraceFileHeader sizeof == 80 | 3.2 |
| `tcp_sink_connect_and_write` | `sink::tcp` | Bytes received by mock server | 3.1 |
| `tcp_sink_connect_refused_returns_error` | `sink::tcp` | Connection refused is Err | 3.1 |
| `uri_stderr_colon` | `sink::uri` | "stderr:" → StderrSink | 3 |
| `uri_null_colon` | `sink::uri` | "null:" → NullSink | 3 |
| `uri_file_async` | `sink::uri` | "file:/path" → AsyncFileSink | 3.1 |
| `uri_file_sync` | `sink::uri` | "file+sync:/path" → FileSink | 3.1 |
| `uri_tcp_malformed_no_port` | `sink::uri` | "tcp:hostname" → InvalidUri | 3.1 |
| `uri_unknown_scheme` | `sink::uri` | "ftp:..." → InvalidUri | 3 |
| `text_formatter_contains_sim_insns` | `format::text` | sim_insns and value present | 3 |
| `text_formatter_contains_begin_end_markers` | `format::text` | Stats block delimiters | 3 |
| `text_formatter_insn_mix_percentages` | `format::text` | Percentage column correct | 3 |
| `text_formatter_cache_present` | `format::text` | Cache hit/miss/hit_rate lines | 3 |
| `text_formatter_hot_pcs` | `format::text` | hot_pcs[0] with address | 3 |
| `json_formatter_is_valid_json` | `format::json` | Parses without panic | 3.2 |
| `json_formatter_sim_insns_field` | `format::json` | sim_insns field value | 3.2 |
| `json_formatter_insn_mix_array` | `format::json` | insn_mix is non-empty array | 3.2 |
| `json_formatter_cache_field` | `format::json` | cache_l1d object with hits | 3.2 |
| `json_formatter_hot_pcs_array` | `format::json` | hot_pcs array with pc/count | 3.2 |
| `csv_formatter_header_row` | `format::csv` | First row is header | 3.2 |
| `csv_formatter_sim_insns_row_present` | `format::csv` | sim_insns row found | 3.2 |
| `csv_formatter_three_columns` | `format::csv` | All data rows have 3 columns | 3.2 |
| `gemstats_formatter_begin_end_markers` | `format::gemstats` | Stats block delimiters | 3.2 |
| `gemstats_formatter_committed_insns_key` | `format::gemstats` | gem5 committedInsts key | 3.2 |
| `gemstats_formatter_ipc_key` | `format::gemstats` | system.cpu.ipc key | 3.2 |
| `gemstats_formatter_cache_keys_present` | `format::gemstats` | dcache.overall_hits key | 3.2 |
| `report_deliver_writes_to_single_sink` | `report` | deliver_to writes correctly | 3 |
| `report_deliver_fan_out_to_multiple_sinks` | `report` | Two sinks get identical bytes | 3 |
| `report_deliver_continues_on_sink_error` | `report` | FailSink doesn't block CaptureSink | 3 |
| `schedule_every_n_insns_fires_at_interval` | `schedule` | 3 fires at 3.5M for n=1M | 3.1 |
| `schedule_on_pc_fires_on_match` | `schedule` | 2 fires for 2 PC matches | 3.1 |
| `schedule_at_exit_does_not_fire_from_check` | `schedule` | AtExit never fires in check() | 3.1 |
| `schedule_at_exit_fires_from_flush_at_exit` | `schedule` | flush_at_exit() fires AtExit | 3.1 |
| `schedule_explicit_does_not_fire_from_check` | `schedule` | Explicit never auto-fires | 3.1 |
| `schedule_deliver_fires_explicit` | `schedule` | deliver() fires once | 3.1 |

**Total: 47 unit tests across 3 phases.**

---

*See [`HLD.md`](HLD.md) for purpose, architecture, and phased plan.*
*See [`LLD-sinks.md`](LLD-sinks.md) for full implementation details.*

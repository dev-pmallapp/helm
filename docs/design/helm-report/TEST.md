# helm-report — Test Plan

> **Status:** 62 tests implemented and passing.
> **Crate path:** `debug/helm-report/`
> **Companion:** [`HLD.md`](HLD.md) · [`LLD-sinks.md`](LLD-sinks.md)

---

## Table of Contents

1. [Philosophy](#1-philosophy)
2. [Test Infrastructure](#2-test-infrastructure)
3. [Test Listing by Module](#3-test-listing-by-module)
4. [Test Matrix](#4-test-matrix)

---

## 1. Philosophy

`helm-report` is pure cold-path delivery code. Every test is a `#[cfg(test)]` unit test that
runs under `cargo test` with no simulator binary, no platform, and no PyO3 dependency.

Principles:
- No network required for unit tests: `TcpSink` tests spin up a `TcpListener` mock server in a
  background thread on a random port.
- No spawned processes: `BinaryTraceSink` and `AsyncFileSink` tests use `tempfile::NamedTempFile`,
  join the drain thread, and read the file back.
- Formatter tests assert on string content, not byte-exact output, to tolerate minor whitespace
  or alignment changes.
- `Report::deliver()` tests use inline `CaptureSink` and `FailSink` types defined inside the test.
- `ReportSchedule` tests use a `CounterSink` that increments an `Arc<Mutex<u32>>` on each write.
- All tests share a `test_snapshot()` function defined in `src/lib.rs` under `pub(crate) mod tests`.

---

## 2. Test Infrastructure

The shared test helper is in `src/lib.rs`:

```rust
#[cfg(test)]
pub(crate) mod tests {
    pub fn test_snapshot() -> SpySpySnapshot { ... }
}
```

`test_snapshot()` constructs a `SpySpySnapshot` with:
- `insn_count = 10_000_000`, `tick_count = 8_130_081`
- 5-class `insn_mix`: IntAlu(5M), Load(2M), Store(1M), Branch(1.5M), SIMD(500K)
- 2 `hot_pcs` entries
- 1 `branch_heatmap` entry
- `cache_l1d = Some(...)` with `name = "l1d"`, hits, misses, hit_rate
- `branch_pred = Some(...)` with `name = "bimodal"`, `kind = "BiModal"`
- `fault_history = None`
- `snapshot_ns = 1_710_849_600_000_000_000`

The `TestSink` struct is in `src/sink/mod.rs` under `#[cfg(test)]` and is used by
`report.rs` tests. Sink-level tests mostly construct the concrete sink directly.

---

## 3. Test Listing by Module

### format::csv (5 tests)

Module: `src/format/csv.rs`

| Test name | What it verifies |
|-----------|-----------------|
| `csv_formatter_header_row` | First row is `["timestamp_ns", "metric", "value"]` |
| `csv_formatter_sim_insns_row_present` | A row with `r[0] == "sim_insns"` exists |
| `csv_formatter_three_columns` | All data rows (after header) have exactly 3 columns |
| `csv_formatter_timestamp_is_numeric` | Column index 1 parses as `u64` in at least one row |
| `csv_formatter_content_type` | `content_type()` contains `"text/csv"` |

### format::helmstats (5 tests)

Module: `src/format/helmstats.rs`

| Test name | What it verifies |
|-----------|-----------------|
| `helmstats_formatter_begin_end_markers` | Output contains both `"Begin Simulation Statistics"` and `"End Simulation Statistics"` |
| `helmstats_formatter_committed_insns_key` | Output contains `"system.cpu.committedInsts"` |
| `helmstats_formatter_ipc_key` | Output contains `"system.cpu.ipc"` |
| `helmstats_formatter_cache_keys_present` | Output contains `"dcache.overall_hits"` and `"dcache.overall_misses"` |
| `helmstats_formatter_content_type` | `content_type()` contains `"text/plain"` |

### format::json (6 tests)

Module: `src/format/json.rs`

| Test name | What it verifies |
|-----------|-----------------|
| `json_formatter_is_valid_json` | Output parses as valid JSON without panic |
| `json_formatter_sim_insns_field` | `v["sim_insns"].as_u64() == Some(10_000_000)` |
| `json_formatter_insn_mix_array` | `v["insn_mix"]` is a non-empty array containing `"insn_mix.IntAlu"` |
| `json_formatter_cache_field` | `v["cache_l1d"]` is an object with numeric `hits` and `hit_rate` |
| `json_formatter_hot_pcs_array` | `v["hot_pcs"]` is a non-empty array with string `pc` and numeric `count` |
| `json_formatter_content_type` | `content_type()` contains `"application/json"` |

### format::text (7 tests)

Module: `src/format/text.rs`

| Test name | What it verifies |
|-----------|-----------------|
| `text_formatter_contains_sim_insns` | Output contains `"sim_insns"` and `"10000000"` |
| `text_formatter_contains_begin_end_markers` | Output contains both stats block delimiters |
| `text_formatter_insn_mix_percentages` | Output contains `"insn_mix.IntAlu"` and `"50.00%"` |
| `text_formatter_cache_present` | Output contains `"cache_l1d.hits"`, `"cache_l1d.misses"`, `"cache_l1d.hit_rate"` |
| `text_formatter_hot_pcs` | Output contains `"hot_pcs[0]"` and `"0xffff800010012a4c"` |
| `text_formatter_format_counter` | `format_counter("my_counter", 42, "things")` contains name, value, unit |
| `text_formatter_content_type` | `content_type()` contains `"text/plain"` |

### report (3 tests)

Module: `src/report.rs`

| Test name | What it verifies |
|-----------|-----------------|
| `report_deliver_writes_to_single_sink` | `deliver_to()` writes to an ad-hoc `TestSink`; output contains `"sim_insns"` |
| `report_deliver_fan_out_to_multiple_sinks` | Two `CaptureSink` instances receive identical content from `deliver()` |
| `report_deliver_continues_on_sink_error` | `FailSink` returns error but subsequent `CaptureSink2` still receives content |

### schedule (6 tests)

Module: `src/schedule.rs`

| Test name | What it verifies |
|-----------|-----------------|
| `schedule_every_n_insns_fires_at_interval` | `EveryNInsns(1_000_000)` fires exactly 3 times across 3.5M instructions |
| `schedule_on_pc_fires_on_match` | `OnPc(0xDEAD_BEEF)` fires on each matching PC (2 matches = 2 fires) |
| `schedule_at_exit_does_not_fire_from_check` | `AtExit` trigger does not fire from `check()` across 10K calls |
| `schedule_at_exit_fires_from_flush_at_exit` | `flush_at_exit()` fires exactly once for an `AtExit` trigger |
| `schedule_explicit_does_not_fire_from_check` | `Explicit` trigger fires 0 times across 5K `check()` calls |
| `schedule_deliver_fires_explicit` | `ReportSchedule::deliver()` fires the report exactly once |

### sink::async_file (3 tests)

Module: `src/sink/async_file.rs`

| Test name | What it verifies |
|-----------|-----------------|
| `async_filesink_delivers_all_writes` | 100 writes followed by drop+join produces a file with 100 lines |
| `async_filesink_join_on_drop` | Single write followed by drop+join produces the exact content |
| `async_filesink_name_is_path` | `name()` returns the path string |

### sink::binary (3 tests)

Module: `src/sink/binary.rs`

| Test name | What it verifies |
|-----------|-----------------|
| `binary_trace_sink_header_correct` | magic, version, record_size, record_count, type_name fields correct after empty sink |
| `binary_trace_sink_records_readable` | 10 pushed `TestRecord` values are readable back from file with correct fields |
| `binary_trace_sink_header_size_is_80` | `sizeof(TraceFileHeader) == 80` |

### sink::file (4 tests)

Module: `src/sink/file.rs`

| Test name | What it verifies |
|-----------|-----------------|
| `filesink_write_and_read_back` | Two writes + explicit flush + drop produce the exact file content |
| `filesink_flush_on_drop` | Write without flush; drop causes content to appear in file |
| `filesink_name_is_path` | `name()` returns the path string |
| `filesink_open_nonexistent_dir_fails` | `open("/nonexistent/...")` returns `Err` |

### sink::null (4 tests)

Module: `src/sink/null.rs`

| Test name | What it verifies |
|-----------|-----------------|
| `null_sink_write_always_ok` | `write(&[0u8; 1 MB])` returns `Ok(())` |
| `null_sink_flush_always_ok` | `flush()` returns `Ok(())` |
| `null_sink_name` | `name()` returns `"null"` |
| `null_sink_is_non_blocking` | 100 writes of 64 MB each complete in < 100 ms |

### sink::python (3 tests)

Module: `src/sink/python.rs`

| Test name | What it verifies |
|-----------|-----------------|
| `python_sink_write_and_drain` | Two writes; drain returns 2 strings; second drain is empty |
| `python_sink_name` | `name()` returns `"python"` |
| `python_sink_inner_shares_state` | `inner()` Arc sees write made through the sink |

### sink::stderr (3 tests)

Module: `src/sink/stderr.rs`

| Test name | What it verifies |
|-----------|-----------------|
| `stderr_sink_write_does_not_panic` | `write(b"...")` completes with `Ok(())` |
| `stderr_sink_flush_ok` | `flush()` returns `Ok(())` |
| `stderr_sink_name` | `name()` returns `"stderr"` |

### sink::tcp (3 tests)

Module: `src/sink/tcp.rs`

| Test name | What it verifies |
|-----------|-----------------|
| `tcp_sink_connect_and_write` | Bytes received by mock `TcpListener` after write+flush+drop |
| `tcp_sink_connect_refused_returns_error` | `connect("127.0.0.1:1")` returns `Err` |
| `tcp_sink_name_is_address` | `name()` returns the address string |

### sink::uri (7 tests)

Module: `src/sink/uri.rs`

| Test name | What it verifies |
|-----------|-----------------|
| `uri_stderr_colon` | `sink_from_uri("stderr:")` returns a sink with `name() == "stderr"` |
| `uri_null_colon` | `sink_from_uri("null:")` returns a sink with `name() == "null"` |
| `uri_file_sync` | `sink_from_uri("file+sync:<path>")` returns a sink whose name contains `/` |
| `uri_file_async` | `sink_from_uri("file:<path>")` returns a sink whose name contains `/` |
| `uri_tcp_malformed_no_port` | `sink_from_uri("tcp:hostname")` returns `Err(InvalidUri(...))` |
| `uri_unknown_scheme` | `sink_from_uri("ftp:somehost")` returns `Err(InvalidUri(...))` |
| `uri_empty_string` | `sink_from_uri("")` returns `Err` |

---

## 4. Test Matrix

| Module | Test count |
|--------|-----------|
| `format::csv` | 5 |
| `format::helmstats` | 5 |
| `format::json` | 6 |
| `format::text` | 7 |
| `report` | 3 |
| `schedule` | 6 |
| `sink::async_file` | 3 |
| `sink::binary` | 3 |
| `sink::file` | 4 |
| `sink::null` | 4 |
| `sink::python` | 3 |
| `sink::stderr` | 3 |
| `sink::tcp` | 3 |
| `sink::uri` | 7 |
| **Total** | **62** |

### Run command

```bash
cargo test --package helm-report
```

All 62 tests are `#[cfg(test)]` unit tests within the source files. No integration test binary.
No network infrastructure required beyond a loopback interface (used by `TcpSink` tests).

---

*See [`HLD.md`](HLD.md) for purpose and architecture.*
*See [`LLD-sinks.md`](LLD-sinks.md) for implementation detail.*

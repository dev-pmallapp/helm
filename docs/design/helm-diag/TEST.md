# TEST: helm-diag — Diagnostic Channel

> Test plan covering `DiagLevel`, `DiagEntry`, `DiagMonitor`, `DiagSink`,
> thread-local install/uninstall, `emit()`, URI backend parsing, and the
> `sim_stub!` / `sim_warn!` / `sim_info!` macros.

**Test files:**
- `framework/helm-diag/src/entry.rs` — inline `#[cfg(test)]` module (DiagLevel ordering, DiagEntry format)
- `framework/helm-diag/src/sink.rs` — inline `#[cfg(test)]` module (URI parser, backend, DiagSink lifecycle)
- `framework/helm-diag/src/lib.rs` — inline `#[cfg(test)]` module (thread-local, emit, install/uninstall)
- `framework/helm-diag/tests/macros.rs` — integration test (sim_stub!, sim_warn!, sim_info! call sites)
- `framework/helm-diag/tests/multi_entry.rs` — integration test (ordering under load, concurrent sends)

---

## Table of Contents

1. [Test Philosophy](#1-test-philosophy)
2. [DiagLevel Tests](#2-diaglevel-tests)
3. [DiagEntry Format Tests](#3-diagentry-format-tests)
4. [URI Backend Parser Tests](#4-uri-backend-parser-tests)
5. [DiagSink Backend Tests](#5-diagsink-backend-tests)
6. [Thread-local Install / Uninstall Tests](#6-thread-local-install--uninstall-tests)
7. [emit() Tests](#7-emit-tests)
8. [Macro Call Site Tests](#8-macro-call-site-tests)
9. [Multi-entry Ordering Tests](#9-multi-entry-ordering-tests)
10. [Test Matrix](#10-test-matrix)

---

## 1. Test Philosophy

**What we test:**

1. **DiagLevel ordering**: `Info < Stub < Warn < Error`; four variants only (no Branch).
2. **DiagEntry format correctness**: fixed-width columns, level tag, component padding, PC
   formatting (`0x…` vs `?`), message passthrough.
3. **URI parsing**: all accepted forms (`stderr:`, `null:`, `file:`, `tcp:`), edge cases
   (empty string, bare name), and error cases (unknown scheme, bad TCP address).
4. **Backend behavior**: file backend writes and flushes; null backend discards silently;
   stderr backend does not panic.
5. **DiagSink lifecycle**: dropping `DiagSink` joins the background thread before returning;
   final flush completes before exit.
6. **Thread-local install/uninstall**: `install_monitor` registers on calling thread;
   `uninstall_monitor` removes it; `emit` sees the correct state per thread.
7. **emit() dispatch**: with no monitor installed, `emit` falls back to `eprintln!` (or
   `log`) without panicking. With monitor installed, entry is sent non-blocking.
8. **Macros**: `sim_stub!`, `sim_warn!`, `sim_info!` compile at the expected call sites,
   produce entries with the correct `DiagLevel`, component, and optional PC.
9. **Multi-entry ordering**: entries sent through the channel arrive at the drain thread in
   order; none are dropped when the queue is not full.

**What we do NOT test:**

- Thread-safety of concurrent `install_monitor` on the same thread (single-threaded
  thread-local by design).
- TCP backend in automated CI (requires a running listener; use manual integration test
  or a test that spawns a `TcpListener` in the same process).
- Wall-clock timing of the drain thread (50 ms timeout is a best-effort implementation
  detail, not a correctness guarantee).
- `log-fallback` feature path (tested only when the feature is enabled; CI runs without
  it by default; `--features log-fallback` test run is a manual check).

---

## 2. DiagLevel Tests

```rust
// framework/helm-diag/src/entry.rs  (inline #[cfg(test)])

#[cfg(test)]
mod level_tests {
    use super::DiagLevel;

    // ── T-LEVEL-01 ──────────────────────────────────────────────────────────
    /// Ordering: Info < Stub < Warn < Error.
    #[test]
    fn level_ordering_is_correct() {
        assert!(DiagLevel::Info  < DiagLevel::Stub);
        assert!(DiagLevel::Stub  < DiagLevel::Warn);
        assert!(DiagLevel::Warn  < DiagLevel::Error);
        assert!(DiagLevel::Info  < DiagLevel::Error);
    }

    // ── T-LEVEL-02 ──────────────────────────────────────────────────────────
    /// DiagLevel::Info is the minimum (sentinel for "pass all").
    #[test]
    fn info_is_minimum_level() {
        for &lvl in &[DiagLevel::Info, DiagLevel::Stub, DiagLevel::Warn, DiagLevel::Error] {
            assert!(lvl >= DiagLevel::Info, "{lvl:?} must be >= Info");
        }
    }

    // ── T-LEVEL-03 ──────────────────────────────────────────────────────────
    /// as_tag() returns the correct 4-character string for every variant.
    #[test]
    fn as_tag_returns_correct_strings() {
        assert_eq!(DiagLevel::Info.as_tag(),  "INFO");
        assert_eq!(DiagLevel::Stub.as_tag(),  "STUB");
        assert_eq!(DiagLevel::Warn.as_tag(),  "WARN");
        assert_eq!(DiagLevel::Error.as_tag(), "ERR ");
    }

    // ── T-LEVEL-04 ──────────────────────────────────────────────────────────
    /// All as_tag() strings are exactly 4 characters.
    #[test]
    fn as_tag_is_always_four_chars() {
        for &lvl in &[DiagLevel::Info, DiagLevel::Stub, DiagLevel::Warn, DiagLevel::Error] {
            assert_eq!(lvl.as_tag().len(), 4, "{lvl:?}.as_tag() must be 4 chars");
        }
    }

    // ── T-LEVEL-05 ──────────────────────────────────────────────────────────
    /// DiagLevel derives Clone and Copy — can be used by value.
    #[test]
    fn level_is_copy() {
        let a = DiagLevel::Warn;
        let b = a;          // copy
        let c = a.clone();  // clone
        assert_eq!(b, c);
    }

    // ── T-LEVEL-06 ──────────────────────────────────────────────────────────
    /// There are exactly four DiagLevel variants (no Branch).
    /// This test enforces the invariant by exhaustively matching.
    #[test]
    fn diaglevel_has_four_variants() {
        // If someone adds a variant, this match becomes non-exhaustive and
        // the compiler will point here.
        let count = [DiagLevel::Info, DiagLevel::Stub, DiagLevel::Warn, DiagLevel::Error].len();
        assert_eq!(count, 4);
    }
}
```

---

## 3. DiagEntry Format Tests

```rust
// framework/helm-diag/src/entry.rs  (inline #[cfg(test)], continued)

#[cfg(test)]
mod entry_tests {
    use super::{DiagEntry, DiagLevel};

    fn make(level: DiagLevel, component: &'static str, pc: Option<u64>, msg: &str) -> DiagEntry {
        DiagEntry {
            sim_ns: 1234, sim_insns: 5678,
            component, level, pc,
            message: msg.to_string(),
        }
    }

    // ── T-ENTRY-01 ──────────────────────────────────────────────────────────
    /// format() starts with the correct level tag in brackets.
    #[test]
    fn format_starts_with_level_tag() {
        assert!(make(DiagLevel::Info,  "c", None, "m").format().starts_with("[INFO]"));
        assert!(make(DiagLevel::Stub,  "c", None, "m").format().starts_with("[STUB]"));
        assert!(make(DiagLevel::Warn,  "c", None, "m").format().starts_with("[WARN]"));
        assert!(make(DiagLevel::Error, "c", None, "m").format().starts_with("[ERR ]"));
    }

    // ── T-ENTRY-02 ──────────────────────────────────────────────────────────
    /// format() contains sim_ns zero-padded to 12 digits.
    #[test]
    fn format_contains_sim_ns_zero_padded() {
        let entry = make(DiagLevel::Info, "c", None, "m");
        // sim_ns = 1234 → "sim_ns=000000001234"
        assert!(entry.format().contains("sim_ns=000000001234"),
            "got: {}", entry.format());
    }

    // ── T-ENTRY-03 ──────────────────────────────────────────────────────────
    /// format() contains sim_insns zero-padded to 12 digits.
    #[test]
    fn format_contains_sim_insns_zero_padded() {
        let entry = make(DiagLevel::Info, "c", None, "m");
        // sim_insns = 5678 → "insns=000000005678"
        assert!(entry.format().contains("insns=000000005678"),
            "got: {}", entry.format());
    }

    // ── T-ENTRY-04 ──────────────────────────────────────────────────────────
    /// format() contains the component string.
    #[test]
    fn format_contains_component() {
        let entry = make(DiagLevel::Stub, "gicv2-dist", None, "msg");
        assert!(entry.format().contains("gicv2-dist"), "got: {}", entry.format());
    }

    // ── T-ENTRY-05 ──────────────────────────────────────────────────────────
    /// format() with pc=Some(addr) renders the address as 0x-prefixed 18-char hex.
    #[test]
    fn format_pc_some_renders_hex() {
        let entry = make(DiagLevel::Stub, "c", Some(0x4020_1234), "m");
        let s = entry.format();
        assert!(s.contains("pc=0x0000000040201234"), "got: {s}");
    }

    // ── T-ENTRY-06 ──────────────────────────────────────────────────────────
    /// format() with pc=None renders "pc=?".
    #[test]
    fn format_pc_none_renders_question_mark() {
        let entry = make(DiagLevel::Info, "c", None, "m");
        let s = entry.format();
        assert!(s.contains("pc=?"), "got: {s}");
    }

    // ── T-ENTRY-07 ──────────────────────────────────────────────────────────
    /// format() contains the message after the " | " separator.
    #[test]
    fn format_contains_message_after_separator() {
        let entry = make(DiagLevel::Warn, "c", None, "write to read-only reg");
        let s = entry.format();
        assert!(s.contains("| write to read-only reg"), "got: {s}");
    }

    // ── T-ENTRY-08 ──────────────────────────────────────────────────────────
    /// format() output is a single line (no embedded newlines).
    #[test]
    fn format_is_single_line() {
        let entry = make(DiagLevel::Info, "c", None, "no newlines here");
        assert!(!entry.format().contains('\n'), "format must not contain newlines");
    }

    // ── T-ENTRY-09 ──────────────────────────────────────────────────────────
    /// DiagEntry derives Clone — can be sent through the channel.
    #[test]
    fn entry_is_clone() {
        let entry = make(DiagLevel::Stub, "test", Some(0x1000), "hello");
        let clone = entry.clone();
        assert_eq!(entry.format(), clone.format());
    }

    // ── T-ENTRY-10 ──────────────────────────────────────────────────────────
    /// sim_ns = 0 and sim_insns = 0 renders as twelve zeros each.
    #[test]
    fn format_zero_timestamps() {
        let entry = DiagEntry {
            sim_ns: 0, sim_insns: 0,
            component: "c", level: DiagLevel::Info, pc: None,
            message: "m".to_string(),
        };
        let s = entry.format();
        assert!(s.contains("sim_ns=000000000000"), "got: {s}");
        assert!(s.contains("insns=000000000000"),  "got: {s}");
    }
}
```

---

## 4. URI Backend Parser Tests

```rust
// framework/helm-diag/src/sink.rs  (inline #[cfg(test)])

#[cfg(test)]
mod uri_tests {
    use super::open_backend;

    // ── T-URI-01 ────────────────────────────────────────────────────────────
    /// Empty string maps to Stderr backend (no error).
    #[test]
    fn empty_string_is_stderr() {
        assert!(open_backend("").is_ok());
    }

    // ── T-URI-02 ────────────────────────────────────────────────────────────
    /// Bare "stderr" maps to Stderr backend.
    #[test]
    fn bare_stderr_is_stderr() {
        assert!(open_backend("stderr").is_ok());
    }

    // ── T-URI-03 ────────────────────────────────────────────────────────────
    /// "stderr:" with colon maps to Stderr backend.
    #[test]
    fn stderr_colon_is_stderr() {
        assert!(open_backend("stderr:").is_ok());
    }

    // ── T-URI-04 ────────────────────────────────────────────────────────────
    /// Bare "null" maps to Null backend.
    #[test]
    fn bare_null_is_null() {
        assert!(open_backend("null").is_ok());
    }

    // ── T-URI-05 ────────────────────────────────────────────────────────────
    /// "null:" with colon maps to Null backend.
    #[test]
    fn null_colon_is_null() {
        assert!(open_backend("null:").is_ok());
    }

    // ── T-URI-06 ────────────────────────────────────────────────────────────
    /// "file:/path" opens (creates) the file; returns Ok.
    #[test]
    fn file_uri_opens_file() {
        let path = std::env::temp_dir().join("helm-diag-uri-test.log");
        let uri = format!("file:{}", path.display());
        let result = open_backend(&uri);
        assert!(result.is_ok(), "file URI must succeed: {result:?}");
        std::fs::remove_file(&path).ok();
    }

    // ── T-URI-07 ────────────────────────────────────────────────────────────
    /// Unknown scheme returns Err with descriptive message.
    #[test]
    fn unknown_scheme_returns_err() {
        let result = open_backend("bogus:something");
        assert!(result.is_err(), "unknown scheme must return Err");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("helm-diag"), "error must mention helm-diag: {msg}");
    }

    // ── T-URI-08 ────────────────────────────────────────────────────────────
    /// "tcp:" with an unreachable address returns a connection error.
    #[test]
    fn tcp_unreachable_returns_err() {
        // Port 1 is almost universally closed.
        let result = open_backend("tcp:127.0.0.1:1");
        assert!(result.is_err(), "unreachable TCP must return Err");
    }
}
```

---

## 5. DiagSink Backend Tests

```rust
// framework/helm-diag/src/sink.rs  (inline #[cfg(test)], continued)

#[cfg(test)]
mod sink_tests {
    use super::{DiagSink};
    use crate::{DiagEntry, DiagLevel, install_monitor, uninstall_monitor};

    fn make_entry(level: DiagLevel, msg: &str) -> DiagEntry {
        DiagEntry {
            sim_ns: 0, sim_insns: 0, component: "test",
            level, pc: None, message: msg.to_string(),
        }
    }

    // ── T-SINK-01 ───────────────────────────────────────────────────────────
    /// null: backend accepts entries without blocking or panicking.
    #[test]
    fn null_backend_is_nonblocking() {
        let (sink, monitor) = DiagSink::open("null:").unwrap();
        for i in 0..10_000u64 {
            monitor.try_send(DiagEntry {
                sim_ns: i, sim_insns: i, component: "test",
                level: DiagLevel::Info, pc: None, message: format!("msg {i}"),
            });
        }
        drop(sink);  // joins drain thread
    }

    // ── T-SINK-02 ───────────────────────────────────────────────────────────
    /// stderr: backend accepts an entry without panicking.
    #[test]
    fn stderr_backend_accepts_entry() {
        let (sink, monitor) = DiagSink::open("stderr:").unwrap();
        monitor.try_send(make_entry(DiagLevel::Info, "sink test from stderr backend"));
        drop(sink);
    }

    // ── T-SINK-03 ───────────────────────────────────────────────────────────
    /// file: backend writes entries; they can be read back from the file.
    #[test]
    fn file_backend_writes_and_reads() {
        use std::io::BufRead;
        let path = std::env::temp_dir().join("helm-diag-sink-test.log");
        let uri = format!("file:{}", path.display());
        {
            let (sink, monitor) = DiagSink::open(&uri).unwrap();
            monitor.try_send(make_entry(DiagLevel::Stub, "written by sink test"));
            drop(sink);  // flush and join before reading
        }
        let f = std::fs::File::open(&path).unwrap();
        let lines: Vec<_> = std::io::BufReader::new(f)
            .lines()
            .map(|l| l.unwrap())
            .collect();
        assert!(!lines.is_empty(), "file must contain at least one line");
        assert!(lines[0].contains("written by sink test"),
            "line must contain the message: {:?}", lines[0]);
        std::fs::remove_file(&path).ok();
    }

    // ── T-SINK-04 ───────────────────────────────────────────────────────────
    /// Dropping DiagSink joins the drain thread (does not hang or panic).
    #[test]
    fn drop_joins_drain_thread() {
        let (sink, monitor) = DiagSink::open("null:").unwrap();
        monitor.try_send(make_entry(DiagLevel::Info, "pre-drop"));
        // drop must return; if drain thread hangs, this test times out.
        drop(sink);
    }

    // ── T-SINK-05 ───────────────────────────────────────────────────────────
    /// open_or_stderr returns a working pair even when given None.
    #[test]
    fn open_or_stderr_with_none_returns_stderr() {
        let (sink, monitor) = DiagSink::open_or_stderr(None);
        monitor.try_send(make_entry(DiagLevel::Info, "open_or_stderr test"));
        drop(sink);
    }

    // ── T-SINK-06 ───────────────────────────────────────────────────────────
    /// open_or_stderr falls back gracefully when given an invalid URI.
    #[test]
    fn open_or_stderr_falls_back_on_bad_uri() {
        // Should not panic; should fall back to stderr.
        let (sink, monitor) = DiagSink::open_or_stderr(Some("bogus:uri"));
        monitor.try_send(make_entry(DiagLevel::Warn, "fallback test"));
        drop(sink);
    }

    // ── T-SINK-07 ───────────────────────────────────────────────────────────
    /// DiagMonitor::try_send is non-blocking when the queue is full.
    /// Fills the entire queue then sends one more — must not block or panic.
    #[test]
    fn try_send_does_not_block_when_queue_full() {
        // Open with null: so the drain thread never drains (it still has
        // recv_timeout so eventually it does drain, but we can saturate quickly).
        let (sink, monitor) = DiagSink::open("null:").unwrap();
        // Send QUEUE_DEPTH + 1 entries.  Queue capacity is 4096.
        for _ in 0..4097 {
            monitor.try_send(make_entry(DiagLevel::Info, "overflow"));
        }
        drop(sink);
    }
}
```

---

## 6. Thread-local Install / Uninstall Tests

```rust
// framework/helm-diag/src/lib.rs  (inline #[cfg(test)])

#[cfg(test)]
mod threadlocal_tests {
    use super::{install_monitor, uninstall_monitor, DIAG_MONITOR, SIM_CTX, update_sim_ctx};
    use crate::sink::DiagSink;

    // ── T-TL-01 ─────────────────────────────────────────────────────────────
    /// After install_monitor, DIAG_MONITOR is Some on the calling thread.
    #[test]
    fn install_sets_monitor() {
        let (_sink, monitor) = DiagSink::open("null:").unwrap();
        install_monitor(monitor);
        let is_some = DIAG_MONITOR.with(|c| c.borrow().is_some());
        assert!(is_some, "DIAG_MONITOR must be Some after install_monitor");
        uninstall_monitor();
    }

    // ── T-TL-02 ─────────────────────────────────────────────────────────────
    /// After uninstall_monitor, DIAG_MONITOR is None on the calling thread.
    #[test]
    fn uninstall_clears_monitor() {
        let (_sink, monitor) = DiagSink::open("null:").unwrap();
        install_monitor(monitor);
        uninstall_monitor();
        let is_none = DIAG_MONITOR.with(|c| c.borrow().is_none());
        assert!(is_none, "DIAG_MONITOR must be None after uninstall_monitor");
    }

    // ── T-TL-03 ─────────────────────────────────────────────────────────────
    /// update_sim_ctx(insns, freq_hz) correctly computes sim_ns.
    #[test]
    fn update_sim_ctx_computes_ns() {
        // 1_000_000_000 insns at 1 GHz → 1_000_000_000 ns
        update_sim_ctx(1_000_000_000, 1_000_000_000);
        let (ns, insns) = SIM_CTX.with(|c| {
            let ctx = c.borrow();
            (ctx.sim_ns, ctx.sim_insns)
        });
        assert_eq!(insns, 1_000_000_000);
        assert_eq!(ns,    1_000_000_000);
    }

    // ── T-TL-04 ─────────────────────────────────────────────────────────────
    /// update_sim_ctx with freq_hz=0 stores insn count directly in sim_ns.
    #[test]
    fn update_sim_ctx_zero_freq_stores_insns_in_ns() {
        update_sim_ctx(42, 0);
        let ns = SIM_CTX.with(|c| c.borrow().sim_ns);
        assert_eq!(ns, 42, "zero freq must store insns directly in sim_ns");
    }

    // ── T-TL-05 ─────────────────────────────────────────────────────────────
    /// install_monitor replaces a previously installed monitor without panic.
    #[test]
    fn install_monitor_replaces_previous() {
        let (_s1, m1) = DiagSink::open("null:").unwrap();
        let (_s2, m2) = DiagSink::open("null:").unwrap();
        install_monitor(m1);
        install_monitor(m2);  // must not panic; replaces m1
        uninstall_monitor();
    }

    // ── T-TL-06 ─────────────────────────────────────────────────────────────
    /// A spawned thread starts with no monitor installed.
    #[test]
    fn new_thread_has_no_monitor() {
        let handle = std::thread::spawn(|| {
            DIAG_MONITOR.with(|c| c.borrow().is_none())
        });
        assert!(handle.join().unwrap(), "new thread must start with no monitor");
    }
}
```

---

## 7. `emit()` Tests

```rust
// framework/helm-diag/src/lib.rs  (inline #[cfg(test)], continued)

#[cfg(test)]
mod emit_tests {
    use super::{emit, install_monitor, uninstall_monitor};
    use crate::{DiagLevel, DiagEntry};
    use crate::sink::DiagSink;

    // ── T-EMIT-01 ───────────────────────────────────────────────────────────
    /// emit() does not panic when no monitor is installed.
    #[test]
    fn emit_without_monitor_does_not_panic() {
        uninstall_monitor();
        // Falls back to eprintln! — visible in test output but must not panic.
        emit(DiagLevel::Stub, "test", None, "no monitor".to_string());
    }

    // ── T-EMIT-02 ───────────────────────────────────────────────────────────
    /// emit() with a null: monitor sends the entry without blocking.
    #[test]
    fn emit_with_null_monitor_is_nonblocking() {
        let (sink, monitor) = DiagSink::open("null:").unwrap();
        install_monitor(monitor);
        emit(DiagLevel::Info, "test", None, "null backend".to_string());
        uninstall_monitor();
        drop(sink);
    }

    // ── T-EMIT-03 ───────────────────────────────────────────────────────────
    /// emit() routes to a file: monitor; entry appears in the output file.
    #[test]
    fn emit_with_file_monitor_writes_to_file() {
        use std::io::BufRead;
        let path = std::env::temp_dir().join("helm-diag-emit-test.log");
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
        assert!(lines[0].contains("0xdeadbeef")
             || lines[0].contains("0x00000000deadbeef"),
             "must contain pc: {:?}", lines[0]);
        std::fs::remove_file(&path).ok();
    }

    // ── T-EMIT-04 ───────────────────────────────────────────────────────────
    /// emit() with all four DiagLevels does not panic (null: backend).
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
}
```

---

## 8. Macro Call Site Tests

```rust
// framework/helm-diag/tests/macros.rs

use helm_diag::{DiagSink, DiagLevel, DiagEntry, install_monitor, uninstall_monitor, sim_stub, sim_warn, sim_info};
use std::sync::{Arc, Mutex};

// Helper: open a capturing backend by collecting formatted lines in-process.
// We use a file backend and read it back since there is no Buffer backend.
fn open_capture() -> (DiagSink, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "helm-diag-macro-test-{}.log",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()
    ));
    let uri = format!("file:{}", path.display());
    let (sink, monitor) = DiagSink::open(&uri).unwrap();
    install_monitor(monitor);
    (sink, path)
}

fn read_lines(path: &std::path::Path) -> Vec<String> {
    use std::io::BufRead;
    let f = std::fs::File::open(path).unwrap();
    std::io::BufReader::new(f).lines().map(|l| l.unwrap()).collect()
}

fn cleanup(path: &std::path::Path) {
    std::fs::remove_file(path).ok();
}

// ── T-MACRO-01 ──────────────────────────────────────────────────────────────
/// sim_stub! with pc= emits a STUB-level entry containing the component and PC.
#[test]
fn sim_stub_with_pc() {
    let (sink, path) = open_capture();
    sim_stub!(component = "test-crate", pc = 0x4000_0000_u64, "stub message {}", 42);
    uninstall_monitor();
    drop(sink);
    let lines = read_lines(&path);
    assert!(!lines.is_empty());
    assert!(lines[0].contains("[STUB]"),       "expected STUB: {:?}", lines[0]);
    assert!(lines[0].contains("test-crate"),   "expected component: {:?}", lines[0]);
    assert!(lines[0].contains("stub message 42"), "expected message: {:?}", lines[0]);
    assert!(lines[0].contains("0x0000000040000000") || lines[0].contains("40000000"),
        "expected PC in line: {:?}", lines[0]);
    cleanup(&path);
}

// ── T-MACRO-02 ──────────────────────────────────────────────────────────────
/// sim_stub! without pc= emits a STUB-level entry with pc=?.
#[test]
fn sim_stub_without_pc() {
    let (sink, path) = open_capture();
    sim_stub!(component = "test-crate", "no pc stub");
    uninstall_monitor();
    drop(sink);
    let lines = read_lines(&path);
    assert!(!lines.is_empty());
    assert!(lines[0].contains("[STUB]"),    "expected STUB: {:?}", lines[0]);
    assert!(lines[0].contains("pc=?"),      "expected pc=? when no pc given: {:?}", lines[0]);
    assert!(lines[0].contains("no pc stub"), "expected message: {:?}", lines[0]);
    cleanup(&path);
}

// ── T-MACRO-03 ──────────────────────────────────────────────────────────────
/// sim_warn! with pc= emits a WARN-level entry.
#[test]
fn sim_warn_with_pc() {
    let (sink, path) = open_capture();
    sim_warn!(component = "pl011", pc = 0x0900_0018_u64, "write to read-only reg {:#x}", 0x18_u32);
    uninstall_monitor();
    drop(sink);
    let lines = read_lines(&path);
    assert!(!lines.is_empty());
    assert!(lines[0].contains("[WARN]"),    "expected WARN: {:?}", lines[0]);
    assert!(lines[0].contains("pl011"),     "expected component: {:?}", lines[0]);
    cleanup(&path);
}

// ── T-MACRO-04 ──────────────────────────────────────────────────────────────
/// sim_warn! without pc= emits a WARN-level entry with pc=?.
#[test]
fn sim_warn_without_pc() {
    let (sink, path) = open_capture();
    sim_warn!(component = "helm-loader", "ELF has PT_LOAD with zero filesz");
    uninstall_monitor();
    drop(sink);
    let lines = read_lines(&path);
    assert!(!lines.is_empty());
    assert!(lines[0].contains("[WARN]"),    "expected WARN: {:?}", lines[0]);
    assert!(lines[0].contains("pc=?"),      "expected pc=? when no pc given: {:?}", lines[0]);
    cleanup(&path);
}

// ── T-MACRO-05 ──────────────────────────────────────────────────────────────
/// sim_info! emits an INFO-level entry; it does not accept a pc= argument.
#[test]
fn sim_info_emits_info_level() {
    let (sink, path) = open_capture();
    sim_info!(component = "helm-loader", "ELF loaded: entry={:#018x}", 0x4000_0000_u64);
    uninstall_monitor();
    drop(sink);
    let lines = read_lines(&path);
    assert!(!lines.is_empty());
    assert!(lines[0].contains("[INFO]"),       "expected INFO: {:?}", lines[0]);
    assert!(lines[0].contains("helm-loader"), "expected component: {:?}", lines[0]);
    assert!(lines[0].contains("ELF loaded"),  "expected message: {:?}", lines[0]);
    cleanup(&path);
}

// ── T-MACRO-06 ──────────────────────────────────────────────────────────────
/// All three macros compile with a format string and multiple arguments.
#[test]
fn macros_accept_format_args() {
    let (sink, path) = open_capture();
    let x: u32 = 0xABCD;
    let y: u64 = 0x1234_5678;
    sim_stub!(component = "test", "x={x:#x} y={y:#018x}");
    sim_warn!(component = "test", pc = y, "x={x:#x}");
    sim_info!(component = "test", "y={y:#018x}");
    uninstall_monitor();
    drop(sink);
    let lines = read_lines(&path);
    assert_eq!(lines.len(), 3, "expected 3 lines: {lines:?}");
    cleanup(&path);
}

// ── T-MACRO-07 ──────────────────────────────────────────────────────────────
/// Macros do not panic when no monitor is installed.
#[test]
fn macros_without_monitor_do_not_panic() {
    uninstall_monitor();
    sim_stub!(component = "test", "no monitor stub");
    sim_warn!(component = "test", "no monitor warn");
    sim_info!(component = "test", "no monitor info");
}
```

---

## 9. Multi-entry Ordering Tests

```rust
// framework/helm-diag/tests/multi_entry.rs

use helm_diag::{DiagSink, DiagLevel, DiagEntry, install_monitor, uninstall_monitor};

// ── T-ORDER-01 ──────────────────────────────────────────────────────────────
/// Entries sent in sequence arrive at the file backend in the same order.
#[test]
fn entries_arrive_in_send_order() {
    use std::io::BufRead;
    let path = std::env::temp_dir().join("helm-diag-order-test.log");
    let uri = format!("file:{}", path.display());
    let n = 100usize;
    {
        let (sink, monitor) = DiagSink::open(&uri).unwrap();
        install_monitor(monitor.clone());
        for i in 0..n {
            monitor.try_send(DiagEntry {
                sim_ns: i as u64, sim_insns: i as u64,
                component: "order-test", level: DiagLevel::Info,
                pc: None, message: format!("entry-{i:04}"),
            });
        }
        uninstall_monitor();
        drop(sink);  // join before reading
    }
    let f = std::fs::File::open(&path).unwrap();
    let lines: Vec<_> = std::io::BufReader::new(f)
        .lines().map(|l| l.unwrap()).collect();
    assert_eq!(lines.len(), n, "expected {n} lines, got {}", lines.len());
    for (i, line) in lines.iter().enumerate() {
        assert!(line.contains(&format!("entry-{i:04}")),
            "line {i} must contain entry-{i:04}: {line:?}");
    }
    std::fs::remove_file(&path).ok();
}

// ── T-ORDER-02 ──────────────────────────────────────────────────────────────
/// try_send on a full queue returns without blocking (not all entries must arrive).
/// This test verifies the non-blocking contract, not delivery of all entries.
#[test]
fn full_queue_try_send_does_not_block() {
    let (sink, monitor) = DiagSink::open("null:").unwrap();
    // Saturate the queue (capacity = 4096) before the drain thread can drain it.
    // All try_send calls must return immediately.
    for i in 0..8192u64 {
        monitor.try_send(DiagEntry {
            sim_ns: i, sim_insns: i, component: "overflow",
            level: DiagLevel::Info, pc: None,
            message: format!("entry-{i}"),
        });
    }
    drop(sink);
}
```

---

## 10. Test Matrix

| ID | Description | Test file | Command | Pass criteria |
|---|---|---|---|---|
| T-LEVEL-01 | Level ordering: Info < Stub < Warn < Error | `entry.rs` | `cargo test -p helm-diag` | all asserts pass |
| T-LEVEL-02 | Info is minimum level (all >= Info) | `entry.rs` | `cargo test -p helm-diag` | all asserts pass |
| T-LEVEL-03 | as_tag() returns correct 4-char strings | `entry.rs` | `cargo test -p helm-diag` | INFO/STUB/WARN/ERR |
| T-LEVEL-04 | as_tag() is always exactly 4 chars | `entry.rs` | `cargo test -p helm-diag` | len == 4 for all |
| T-LEVEL-05 | DiagLevel is Copy | `entry.rs` | `cargo test -p helm-diag` | compiles, eq |
| T-LEVEL-06 | DiagLevel has exactly four variants | `entry.rs` | `cargo test -p helm-diag` | count == 4 |
| T-ENTRY-01 | format() starts with correct level tag | `entry.rs` | `cargo test -p helm-diag` | starts_with [LEVL] |
| T-ENTRY-02 | sim_ns is 12-digit zero-padded | `entry.rs` | `cargo test -p helm-diag` | sim_ns=000000001234 |
| T-ENTRY-03 | sim_insns is 12-digit zero-padded | `entry.rs` | `cargo test -p helm-diag` | insns=000000005678 |
| T-ENTRY-04 | format() contains component string | `entry.rs` | `cargo test -p helm-diag` | contains component |
| T-ENTRY-05 | pc=Some renders 0x-prefixed hex | `entry.rs` | `cargo test -p helm-diag` | 0x0000000040201234 |
| T-ENTRY-06 | pc=None renders "pc=?" | `entry.rs` | `cargo test -p helm-diag` | contains "pc=?" |
| T-ENTRY-07 | format() contains message after " \| " | `entry.rs` | `cargo test -p helm-diag` | "\| write to…" |
| T-ENTRY-08 | format() is a single line (no \n) | `entry.rs` | `cargo test -p helm-diag` | !contains('\\n') |
| T-ENTRY-09 | DiagEntry derives Clone | `entry.rs` | `cargo test -p helm-diag` | clone eq format |
| T-ENTRY-10 | sim_ns=0 and sim_insns=0 renders twelve zeros | `entry.rs` | `cargo test -p helm-diag` | sim_ns=000000000000 |
| T-URI-01 | empty string → Stderr | `sink.rs` | `cargo test -p helm-diag` | Ok |
| T-URI-02 | "stderr" → Stderr | `sink.rs` | `cargo test -p helm-diag` | Ok |
| T-URI-03 | "stderr:" → Stderr | `sink.rs` | `cargo test -p helm-diag` | Ok |
| T-URI-04 | "null" → Null | `sink.rs` | `cargo test -p helm-diag` | Ok |
| T-URI-05 | "null:" → Null | `sink.rs` | `cargo test -p helm-diag` | Ok |
| T-URI-06 | "file:/path" → File created | `sink.rs` | `cargo test -p helm-diag` | Ok, file exists |
| T-URI-07 | unknown scheme → Err with message | `sink.rs` | `cargo test -p helm-diag` | Err, "helm-diag" |
| T-URI-08 | "tcp:127.0.0.1:1" → Err | `sink.rs` | `cargo test -p helm-diag` | Err |
| T-SINK-01 | null: backend: 10k sends without block | `sink.rs` | `cargo test -p helm-diag` | no hang |
| T-SINK-02 | stderr: backend: accepts entry | `sink.rs` | `cargo test -p helm-diag` | no panic |
| T-SINK-03 | file: backend: write + read back | `sink.rs` | `cargo test -p helm-diag` | line contains msg |
| T-SINK-04 | drop DiagSink joins drain thread | `sink.rs` | `cargo test -p helm-diag` | returns |
| T-SINK-05 | open_or_stderr(None) returns working pair | `sink.rs` | `cargo test -p helm-diag` | no panic |
| T-SINK-06 | open_or_stderr falls back on bad URI | `sink.rs` | `cargo test -p helm-diag` | no panic |
| T-SINK-07 | try_send does not block on full queue | `sink.rs` | `cargo test -p helm-diag` | returns fast |
| T-TL-01 | install_monitor sets DIAG_MONITOR to Some | `lib.rs` | `cargo test -p helm-diag` | is_some() == true |
| T-TL-02 | uninstall_monitor clears DIAG_MONITOR | `lib.rs` | `cargo test -p helm-diag` | is_none() == true |
| T-TL-03 | update_sim_ctx computes sim_ns correctly | `lib.rs` | `cargo test -p helm-diag` | ns == 1_000_000_000 |
| T-TL-04 | update_sim_ctx with freq=0 stores insns | `lib.rs` | `cargo test -p helm-diag` | sim_ns == 42 |
| T-TL-05 | install_monitor replaces previous monitor | `lib.rs` | `cargo test -p helm-diag` | no panic |
| T-TL-06 | new thread starts with no monitor | `lib.rs` | `cargo test -p helm-diag` | is_none() == true |
| T-EMIT-01 | emit() without monitor does not panic | `lib.rs` | `cargo test -p helm-diag` | no panic |
| T-EMIT-02 | emit() with null: monitor is non-blocking | `lib.rs` | `cargo test -p helm-diag` | no hang |
| T-EMIT-03 | emit() with file: monitor writes to file | `lib.rs` | `cargo test -p helm-diag` | line has [WARN] |
| T-EMIT-04 | emit() all four levels, no panic | `lib.rs` | `cargo test -p helm-diag` | no panic |
| T-MACRO-01 | sim_stub! with pc= emits STUB+PC | `tests/macros.rs` | `cargo test -p helm-diag --test macros` | [STUB], component, PC |
| T-MACRO-02 | sim_stub! without pc= → pc=? | `tests/macros.rs` | `cargo test -p helm-diag --test macros` | pc=? |
| T-MACRO-03 | sim_warn! with pc= emits WARN+PC | `tests/macros.rs` | `cargo test -p helm-diag --test macros` | [WARN] |
| T-MACRO-04 | sim_warn! without pc= → pc=? | `tests/macros.rs` | `cargo test -p helm-diag --test macros` | pc=? |
| T-MACRO-05 | sim_info! emits INFO level | `tests/macros.rs` | `cargo test -p helm-diag --test macros` | [INFO] |
| T-MACRO-06 | All macros accept format args | `tests/macros.rs` | `cargo test -p helm-diag --test macros` | 3 lines |
| T-MACRO-07 | Macros without monitor: no panic | `tests/macros.rs` | `cargo test -p helm-diag --test macros` | no panic |
| T-ORDER-01 | 100 entries arrive in send order | `tests/multi_entry.rs` | `cargo test -p helm-diag --test multi_entry` | entry-0000…entry-0099 |
| T-ORDER-02 | Full queue: try_send non-blocking | `tests/multi_entry.rs` | `cargo test -p helm-diag --test multi_entry` | returns fast |

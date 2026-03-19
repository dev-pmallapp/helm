# TEST: helm-diag — Diagnostic Channel

> Test plan covering `DiagLevel`, `DiagEntry`, `DiagMonitor`, `DiagSink`,
> thread-local install/uninstall, `emit()`, URI backend parsing, and the
> `sim_stub!` / `sim_warn!` / `sim_info!` macros.

**Run command:** `cargo test -p helm-diag`

**Pass count:** 50 tests total (all passing)

**Test files:**

| File | Module | Tests |
|------|--------|-------|
| `framework/helm-diag/src/entry.rs` | `level_tests` | 6 |
| `framework/helm-diag/src/entry.rs` | `entry_tests` | 10 |
| `framework/helm-diag/src/sink.rs` | `uri_tests` | 8 |
| `framework/helm-diag/src/sink.rs` | `sink_tests` | 7 |
| `framework/helm-diag/src/lib.rs` | `threadlocal_tests` | 6 |
| `framework/helm-diag/src/lib.rs` | `emit_tests` | 4 |
| `framework/helm-diag/tests/macros.rs` | (integration) | 7 |
| `framework/helm-diag/tests/multi_entry.rs` | (integration) | 2 |

---

## Table of Contents

1. [Test Philosophy](#1-test-philosophy)
2. [DiagLevel Tests — `level_tests`](#2-diaglevel-tests--level_tests)
3. [DiagEntry Format Tests — `entry_tests`](#3-diagentry-format-tests--entry_tests)
4. [URI Backend Parser Tests — `uri_tests`](#4-uri-backend-parser-tests--uri_tests)
5. [DiagSink Backend Tests — `sink_tests`](#5-diagsink-backend-tests--sink_tests)
6. [Thread-local Tests — `threadlocal_tests`](#6-thread-local-tests--threadlocal_tests)
7. [emit() Tests — `emit_tests`](#7-emit-tests--emit_tests)
8. [Macro Call Site Tests — `tests/macros.rs`](#8-macro-call-site-tests--testsmacrosrs)
9. [Multi-entry Tests — `tests/multi_entry.rs`](#9-multi-entry-tests--testsmulti_entryrs)
10. [Test Matrix](#10-test-matrix)

---

## 1. Test Philosophy

**What we test:**

1. **DiagLevel ordering** — `Info < Stub < Warn < Error`; exactly four variants (no Branch).
2. **DiagEntry format correctness** — fixed-width columns, level tag, component padding, PC
   formatting (`0x…` vs `?`), message passthrough, single-line output.
3. **URI parsing** — all accepted forms (`stderr:`, `null:`, `file:`, `tcp:`), edge cases
   (empty string, bare name without colon), and error cases (unknown scheme, bad TCP address).
4. **Backend behavior** — file backend writes and flushes; null backend discards silently;
   stderr backend does not panic.
5. **DiagSink lifecycle** — dropping `DiagSink` joins the background drain thread before
   returning; final flush completes before exit.
6. **Thread-local install/uninstall** — `install_monitor` registers on calling thread;
   `uninstall_monitor` removes it; new threads start with no monitor; `update_sim_ctx`
   computes `sim_ns` correctly including the zero-frequency edge case.
7. **emit() dispatch** — with no monitor installed, `emit` falls back to `eprintln!`
   without panicking. With monitor installed, entry is sent non-blocking.
8. **Macros** — `sim_stub!`, `sim_warn!`, `sim_info!` produce entries with correct
   `DiagLevel`, component, and optional PC; format args work; no panic without monitor.
9. **Multi-entry ordering** — entries sent through the channel arrive at the drain thread
   in send order; `try_send` does not block when the queue is full.

**What we do NOT test:**

- TCP backend in automated CI (requires a running listener; port 1 is used only to verify
  that a refused connection returns `Err`).
- Wall-clock timing of the drain thread (50 ms timeout is a best-effort implementation
  detail, not a correctness guarantee).
- `log-fallback` feature path (only active when the feature is enabled; CI runs without
  it by default).
- Concurrent `install_monitor` calls on the same thread (thread-locals are single-threaded
  by design).

---

## 2. DiagLevel Tests — `level_tests`

Location: `framework/helm-diag/src/entry.rs`, `#[cfg(test)] mod level_tests`

| Test ID | Function | What it checks |
|---------|----------|----------------|
| T-LEVEL-01 | `level_ordering_is_correct` | `Info < Stub`, `Stub < Warn`, `Warn < Error`, `Info < Error` |
| T-LEVEL-02 | `info_is_minimum_level` | All four variants are `>= Info` |
| T-LEVEL-03 | `as_tag_returns_correct_strings` | `"INFO"`, `"STUB"`, `"WARN"`, `"ERR "` |
| T-LEVEL-04 | `as_tag_is_always_four_chars` | `as_tag().len() == 4` for all variants |
| T-LEVEL-05 | `level_is_copy` | `DiagLevel` is `Copy` and `Clone`; copy == clone |
| T-LEVEL-06 | `diaglevel_has_four_variants` | Array `[Info, Stub, Warn, Error].len() == 4` |

---

## 3. DiagEntry Format Tests — `entry_tests`

Location: `framework/helm-diag/src/entry.rs`, `#[cfg(test)] mod entry_tests`

All tests construct entries with `sim_ns=1234`, `sim_insns=5678` unless otherwise noted.

| Test ID | Function | What it checks |
|---------|----------|----------------|
| T-ENTRY-01 | `format_starts_with_level_tag` | Output starts with `[INFO]`, `[STUB]`, `[WARN]`, `[ERR ]` |
| T-ENTRY-02 | `format_contains_sim_ns_zero_padded` | Contains `"sim_ns=000000001234"` (12-digit) |
| T-ENTRY-03 | `format_contains_sim_insns_zero_padded` | Contains `"insns=000000005678"` (12-digit) |
| T-ENTRY-04 | `format_contains_component` | Contains the component string `"gicv2-dist"` |
| T-ENTRY-05 | `format_pc_some_renders_hex` | `pc=Some(0x4020_1234)` renders as `"pc=0x0000000040201234"` |
| T-ENTRY-06 | `format_pc_none_renders_question_mark` | `pc=None` renders as `"pc=?"` |
| T-ENTRY-07 | `format_contains_message_after_separator` | Contains `"| write to read-only reg"` |
| T-ENTRY-08 | `format_is_single_line` | Output does not contain `'\n'` |
| T-ENTRY-09 | `entry_is_clone` | `entry.clone().format() == entry.format()` |
| T-ENTRY-10 | `format_zero_timestamps` | `sim_ns=0, sim_insns=0` renders as 12 zeros each |

---

## 4. URI Backend Parser Tests — `uri_tests`

Location: `framework/helm-diag/src/sink.rs`, `#[cfg(test)] mod uri_tests`

Tests call `open_backend(uri)` directly (the `pub(crate)` function).

| Test ID | Function | Input | Expected |
|---------|----------|-------|----------|
| T-URI-01 | `empty_string_is_stderr` | `""` | `Ok` |
| T-URI-02 | `bare_stderr_is_stderr` | `"stderr"` | `Ok` |
| T-URI-03 | `stderr_colon_is_stderr` | `"stderr:"` | `Ok` |
| T-URI-04 | `bare_null_is_null` | `"null"` | `Ok` |
| T-URI-05 | `null_colon_is_null` | `"null:"` | `Ok` |
| T-URI-06 | `file_uri_opens_file` | `"file:/tmp/helm-diag-uri-test.log"` | `Ok`, file created |
| T-URI-07 | `unknown_scheme_returns_err` | `"bogus:something"` | `Err`; message contains `"helm-diag"` |
| T-URI-08 | `tcp_unreachable_returns_err` | `"tcp:127.0.0.1:1"` | `Err` (connection refused) |

---

## 5. DiagSink Backend Tests — `sink_tests`

Location: `framework/helm-diag/src/sink.rs`, `#[cfg(test)] mod sink_tests`

| Test ID | Function | What it checks |
|---------|----------|----------------|
| T-SINK-01 | `null_backend_is_nonblocking` | 10,000 entries sent to `null:` without hang |
| T-SINK-02 | `stderr_backend_accepts_entry` | Single entry to `stderr:` does not panic |
| T-SINK-03 | `file_backend_writes_and_reads` | Entry written to `file:` backend, read back and verified |
| T-SINK-04 | `drop_joins_drain_thread` | `drop(sink)` returns (drain thread exits cleanly) |
| T-SINK-05 | `open_or_stderr_with_none_returns_stderr` | `open_or_stderr(None)` returns working pair |
| T-SINK-06 | `open_or_stderr_falls_back_on_bad_uri` | `open_or_stderr(Some("bogus:uri"))` does not panic |
| T-SINK-07 | `try_send_does_not_block_when_queue_full` | 4097 entries sent (queue depth 4096); no block |

Note: T-SINK-01 sends 10,000 entries but the queue depth is 4096; many are silently
dropped by `try_send`. This is the intended non-blocking behavior.

---

## 6. Thread-local Tests — `threadlocal_tests`

Location: `framework/helm-diag/src/lib.rs`, `#[cfg(test)] mod threadlocal_tests`

| Test ID | Function | What it checks |
|---------|----------|----------------|
| T-TL-01 | `install_sets_monitor` | `DIAG_MONITOR` is `Some` after `install_monitor` |
| T-TL-02 | `uninstall_clears_monitor` | `DIAG_MONITOR` is `None` after `uninstall_monitor` |
| T-TL-03 | `update_sim_ctx_computes_ns` | 1B insns at 1 GHz yields `sim_ns = 1_000_000_000` |
| T-TL-04 | `update_sim_ctx_zero_freq_stores_insns_in_ns` | `freq_hz=0` stores insns directly in `sim_ns` |
| T-TL-05 | `install_monitor_replaces_previous` | Second `install_monitor` replaces first without panic |
| T-TL-06 | `new_thread_has_no_monitor` | Spawned thread starts with `DIAG_MONITOR = None` |

---

## 7. emit() Tests — `emit_tests`

Location: `framework/helm-diag/src/lib.rs`, `#[cfg(test)] mod emit_tests`

| Test ID | Function | What it checks |
|---------|----------|----------------|
| T-EMIT-01 | `emit_without_monitor_does_not_panic` | `emit(Stub, ...)` with no monitor falls back to `eprintln!`, no panic |
| T-EMIT-02 | `emit_with_null_monitor_is_nonblocking` | `emit(Info, ...)` with `null:` monitor does not block |
| T-EMIT-03 | `emit_with_file_monitor_writes_to_file` | `emit(Warn, "emit-test", Some(0xDEAD_BEEF), ...)` written to file; line contains `[WARN]`, component, message, and PC |
| T-EMIT-04 | `emit_all_levels_no_panic` | All four `DiagLevel` variants emitted to `null:` without panic |

T-EMIT-03 verifies the PC format: the test checks for `"0x00000000deadbeef"` (the
`{:#018x}` output of `0xDEAD_BEEF` as `u64`).

---

## 8. Macro Call Site Tests — `tests/macros.rs`

Location: `framework/helm-diag/tests/macros.rs` (integration test)

Run: `cargo test -p helm-diag --test macros`

Helper `open_capture()` opens a uniquely-named temporary file backend (using
`subsec_nanos()` in the filename) and installs the monitor. `read_lines()` reads it
back after dropping the sink. `cleanup()` removes the file.

| Test ID | Function | What it checks |
|---------|----------|----------------|
| T-MACRO-01 | `sim_stub_with_pc` | `sim_stub!(component=..., pc=0x4000_0000_u64, "stub message {}", 42)` produces `[STUB]`, component, message, and `"0x0000000040000000"` in output |
| T-MACRO-02 | `sim_stub_without_pc` | `sim_stub!(component=..., "no pc stub")` produces `[STUB]` and `"pc=?"` |
| T-MACRO-03 | `sim_warn_with_pc` | `sim_warn!(component="pl011", pc=..., "write to read-only reg {:#x}", 0x18)` produces `[WARN]` and `"pl011"` |
| T-MACRO-04 | `sim_warn_without_pc` | `sim_warn!(component="helm-loader", "...")` produces `[WARN]` and `"pc=?"` |
| T-MACRO-05 | `sim_info_emits_info_level` | `sim_info!(component="helm-loader", "ELF loaded: ...")` produces `[INFO]`, component, message |
| T-MACRO-06 | `macros_accept_format_args` | Three macros with format args each write one line; file has exactly 3 lines |
| T-MACRO-07 | `macros_without_monitor_do_not_panic` | All three macros called with no monitor installed; no panic |

---

## 9. Multi-entry Tests — `tests/multi_entry.rs`

Location: `framework/helm-diag/tests/multi_entry.rs` (integration test)

Run: `cargo test -p helm-diag --test multi_entry`

| Test ID | Function | What it checks |
|---------|----------|----------------|
| T-ORDER-01 | `entries_arrive_in_send_order` | 100 entries sent via `monitor.try_send`; file has exactly 100 lines; each line `i` contains `"entry-{i:04}"` in order |
| T-ORDER-02 | `full_queue_try_send_does_not_block` | 8192 entries sent to `null:` (queue depth 4096); all calls return without blocking |

T-ORDER-01 uses `drop(monitor); drop(sink)` to flush and join before reading the file.
T-ORDER-02 sends twice the queue depth to verify that `try_send` silently discards
entries without blocking when the queue is full.

---

## 10. Test Matrix

| ID | Description | File | Pass criteria |
|----|-------------|------|---------------|
| T-LEVEL-01 | Level ordering: Info < Stub < Warn < Error | `src/entry.rs` | all asserts pass |
| T-LEVEL-02 | Info is minimum level | `src/entry.rs` | all >= Info |
| T-LEVEL-03 | as_tag() returns correct 4-char strings | `src/entry.rs` | INFO/STUB/WARN/ERR |
| T-LEVEL-04 | as_tag() is always exactly 4 chars | `src/entry.rs` | len == 4 for all |
| T-LEVEL-05 | DiagLevel is Copy and Clone | `src/entry.rs` | compiles; copy == clone |
| T-LEVEL-06 | DiagLevel has exactly four variants | `src/entry.rs` | count == 4 |
| T-ENTRY-01 | format() starts with correct level tag | `src/entry.rs` | starts_with `[LEVL]` |
| T-ENTRY-02 | sim_ns is 12-digit zero-padded | `src/entry.rs` | `sim_ns=000000001234` |
| T-ENTRY-03 | sim_insns is 12-digit zero-padded | `src/entry.rs` | `insns=000000005678` |
| T-ENTRY-04 | format() contains component string | `src/entry.rs` | contains component |
| T-ENTRY-05 | pc=Some renders 0x-prefixed 18-char hex | `src/entry.rs` | `pc=0x0000000040201234` |
| T-ENTRY-06 | pc=None renders "pc=?" | `src/entry.rs` | contains `"pc=?"` |
| T-ENTRY-07 | format() contains message after " \| " | `src/entry.rs` | `"\| write to..."` |
| T-ENTRY-08 | format() is a single line (no newline) | `src/entry.rs` | `!contains('\n')` |
| T-ENTRY-09 | DiagEntry derives Clone | `src/entry.rs` | clone.format() == entry.format() |
| T-ENTRY-10 | sim_ns=0 and sim_insns=0 render as 12 zeros | `src/entry.rs` | `sim_ns=000000000000` |
| T-URI-01 | empty string -> Stderr | `src/sink.rs` | `Ok` |
| T-URI-02 | `"stderr"` -> Stderr | `src/sink.rs` | `Ok` |
| T-URI-03 | `"stderr:"` -> Stderr | `src/sink.rs` | `Ok` |
| T-URI-04 | `"null"` -> Null | `src/sink.rs` | `Ok` |
| T-URI-05 | `"null:"` -> Null | `src/sink.rs` | `Ok` |
| T-URI-06 | `"file:/path"` -> File created | `src/sink.rs` | `Ok`, file exists |
| T-URI-07 | unknown scheme -> Err with message | `src/sink.rs` | `Err`; message contains `"helm-diag"` |
| T-URI-08 | `"tcp:127.0.0.1:1"` -> connection refused | `src/sink.rs` | `Err` |
| T-SINK-01 | null: backend; 10k sends without block | `src/sink.rs` | no hang |
| T-SINK-02 | stderr: backend; accepts entry | `src/sink.rs` | no panic |
| T-SINK-03 | file: backend; write + read back | `src/sink.rs` | line contains message |
| T-SINK-04 | drop DiagSink joins drain thread | `src/sink.rs` | drop returns |
| T-SINK-05 | open_or_stderr(None) returns working pair | `src/sink.rs` | no panic |
| T-SINK-06 | open_or_stderr falls back on bad URI | `src/sink.rs` | no panic |
| T-SINK-07 | try_send does not block on full queue | `src/sink.rs` | 4097 sends return |
| T-TL-01 | install_monitor sets DIAG_MONITOR to Some | `src/lib.rs` | `is_some() == true` |
| T-TL-02 | uninstall_monitor clears DIAG_MONITOR | `src/lib.rs` | `is_none() == true` |
| T-TL-03 | update_sim_ctx computes sim_ns correctly | `src/lib.rs` | `ns == 1_000_000_000` |
| T-TL-04 | update_sim_ctx with freq=0 stores insns | `src/lib.rs` | `sim_ns == 42` |
| T-TL-05 | install_monitor replaces previous | `src/lib.rs` | no panic |
| T-TL-06 | new thread starts with no monitor | `src/lib.rs` | `is_none() == true` |
| T-EMIT-01 | emit() without monitor does not panic | `src/lib.rs` | no panic |
| T-EMIT-02 | emit() with null: monitor is non-blocking | `src/lib.rs` | no hang |
| T-EMIT-03 | emit() with file: monitor writes to file | `src/lib.rs` | line has `[WARN]`, component, message, PC |
| T-EMIT-04 | emit() all four levels, no panic | `src/lib.rs` | no panic |
| T-MACRO-01 | sim_stub! with pc= emits STUB + PC | `tests/macros.rs` | `[STUB]`, component, PC |
| T-MACRO-02 | sim_stub! without pc= renders pc=? | `tests/macros.rs` | `"pc=?"` |
| T-MACRO-03 | sim_warn! with pc= emits WARN | `tests/macros.rs` | `[WARN]`, component |
| T-MACRO-04 | sim_warn! without pc= renders pc=? | `tests/macros.rs` | `"pc=?"` |
| T-MACRO-05 | sim_info! emits INFO level | `tests/macros.rs` | `[INFO]`, component, message |
| T-MACRO-06 | all macros accept format args | `tests/macros.rs` | exactly 3 lines in file |
| T-MACRO-07 | macros without monitor: no panic | `tests/macros.rs` | no panic |
| T-ORDER-01 | 100 entries arrive in send order | `tests/multi_entry.rs` | entry-0000...entry-0099 in order |
| T-ORDER-02 | full queue: try_send non-blocking | `tests/multi_entry.rs` | 8192 sends return fast |

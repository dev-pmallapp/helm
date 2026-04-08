# helm-diag — High-Level Design

> **Crate:** `helm-diag`
> **Location:** `framework/helm-diag/`
> **Phase:** Phase 0 (extracted from `helm-debug::sim_trace` before any other crate ships)
> **Dependencies:** none mandatory (`log` crate is optional via `log-fallback` feature only)

---

## 1. Purpose and Motivation

### 1.1 The Layer Violation

`helm-arch` and `helm-devices` both need to emit diagnostic messages. The natural
primitives for this are `sim_stub!`, `sim_warn!`, and `sim_info!` — structured,
async-delivery, zero-blocking macros that write to a configured backend. Those macros
originally lived in `helm-debug::sim_trace`.

The problem is the dependency direction:

```
helm-arch     --depends on-->  helm-debug   (runtime depends on debug tool -- cycle risk)
helm-devices  -------------->  helm-debug   (SDK depends on debug tool -- violates layering)
```

`helm-debug` depends on `helm-core` and `helm-memory`. If `helm-arch` depended on
`helm-debug`, we would have a chain where framework crates depend on runtime crates.
`helm-devices` would be even worse: the Device SDK would pull in the debug infrastructure
as a mandatory dependency for every device and every test.

### 1.2 The Solution: Extract to `helm-diag`

`helm-diag` is the extraction of `helm-debug::sim_trace` into a standalone framework
crate with **zero mandatory dependencies**. It contains only:

- The `DiagEntry` struct and `DiagLevel` enum (the data model)
- `DiagMonitor` — the cheap, clonable, non-blocking sender
- `DiagSink` — the background drain thread and URI-based backend
- Thread-locals `DIAG_MONITOR` and `SIM_CTX`, plus `install_monitor`, `uninstall_monitor`,
  `is_monitor_active`, and `update_sim_ctx`
- `emit()` — the non-blocking dispatch function
- `sim_stub!`, `sim_warn!`, `sim_info!` — the call-site macros

Because `helm-diag` has zero mandatory dependencies, every crate in the project can
depend on it without risk of creating a cycle.

### 1.3 What Changes at Call Sites

**Old** (before extraction):

```rust
use helm_debug::sim_stub;
sim_stub!(component = "aarch64-sysreg", pc = state.pc, "MRS {:?} -> 0", reg);
```

**New** (after extraction):

```rust
use helm_diag::sim_stub;
sim_stub!(component = "aarch64-sysreg", pc = state.pc, "MRS {:?} -> 0", reg);
```

The macro signature, behavior, and output format are identical. Only the defining crate
changes. `helm-debug` re-exports the macros at its root as a compatibility shim.

---

## 2. Scope

### 2.1 What `helm-diag` Contains

| Item | Description |
|------|-------------|
| `DiagLevel` | `Info`, `Stub`, `Warn`, `Error` — four variants with derived ordering |
| `DiagEntry` | Structured log record: level + component + pc + timestamps + message |
| `DiagContext` | `{ sim_ns: u64, sim_insns: u64 }` — updated by the engine per step |
| `DiagMonitor` | `Clone`-able, non-blocking `SyncSender<DiagEntry>` wrapper |
| `DiagSink` | Background drain thread; owns `Backend`; URI constructor; Drop joins thread |
| `Backend` | `Stderr | File | Tcp | Null` — internal, not public |
| `DIAG_MONITOR` | `thread_local! RefCell<Option<DiagMonitor>>` — current thread's sender |
| `SIM_CTX` | `thread_local! RefCell<DiagContext>` — current thread's time context |
| `install_monitor(m)` | Registers a `DiagMonitor` on the calling thread |
| `uninstall_monitor()` | Clears the calling thread's `DiagMonitor` |
| `is_monitor_active()` | Returns `true` if a monitor is installed on the calling thread |
| `update_sim_ctx(insns, freq_hz)` | Advances the thread-local `DiagContext` |
| `emit(level, component, pc, msg)` | Non-blocking dispatch; falls back to `eprintln!` |
| `sim_stub!(...)` | Macro for Stub-level messages (with or without `pc=`) |
| `sim_warn!(...)` | Macro for Warn-level messages (with or without `pc=`) |
| `sim_info!(...)` | Macro for Info-level messages (no `pc=` form) |

### 2.2 What `helm-diag` Does NOT Contain

| Item | Where it lives | Why not in helm-diag |
|------|----------------|----------------------|
| `sim_branch!` | Deleted | Branch events go through `probe!(probes.branch, BranchEvent{...})` at Layer 1 |
| `TraceLogger` / `TraceEvent` | `helm-debug` | Rich JSONL structured events — debug tool |
| `GdbServer` | `helm-debug` | Debug tool, not a primitive |
| `CheckpointManager` | `helm-debug` | Debug tool, not a primitive |
| `HelmEventBus` | `helm-devices` | Synchronous pub-sub — separate system |
| `Probe<T>`, `probe!()` | `helm-probe` | Typed, zero-cost probe points — separate system |
| `HelmPluginRegistry` | `helm-plugin` | Legacy callback registry — compatibility-only observability surface |

### 2.3 The Absence of `DiagLevel::Branch`

The old `helm-debug::sim_trace::Level` enum had five variants including `Branch`. The
`sim_branch!` macro emitted `Branch`-level records so that `branch_trace.py` could parse
BRNC lines and resolve branch targets.

In Instrumentation-v2, this role belongs to `Probe<BranchEvent>` (Layer 1) feeding
through `ProbePluginBridge` (Layer 2) to the `BranchTrace` plugin. The probe path is:

- Zero-cost in release (ZST probe, whole block eliminated)
- Typed — `BranchEvent { from_pc, to_pc, taken, kind }` is a Rust struct, not a parsed string
- Pluggable — the `BranchTrace` plugin can output to any `TraceSink`

`DiagLevel` therefore has **four variants** only. `DiagLevel::Branch` does not exist.

---

## 3. Dependency Graph

### 3.1 Position in the Crate DAG

```
(no deps)
    |
    v
helm-diag  <-----------------------------------------------------------+
    |                                                                   |
    +-- helm-arch      (sysreg stubs, unimplemented opcodes)           |
    +-- helm-engine    (update_sim_ctx, is_monitor_active in hot loop) |
    +-- helm-debug     (re-exports macros; compatibility shim)         |
    +-- helm-python    (DiagSink::open from Python set_sim_trace call) +
    +-- helm-cli       (DiagSink::open_or_stderr at startup)
```

The key property: `helm-diag` has zero mandatory dependencies. Any crate can depend on
it. `helm-debug` gains a dependency on `helm-diag` (not the reverse).

### 3.2 Actual Usage Observed in `runtime/`

- `helm-arch` — every execute sub-module (`fp.rs`, `simd.rs`, `branch.rs`, `dp.rs`,
  `ldst.rs`, `mul_div.rs`, `sysreg.rs`, `helpers.rs`) imports `sim_stub!` and `sim_warn!`
- `helm-engine/src/lib.rs` — calls `is_monitor_active()` to skip `update_sim_ctx` cost
  when no backend is active; calls `update_sim_ctx(insns_retired, 1_000_000_000)` per step
- `helm-debug/src/lib.rs` — re-exports `sim_stub!`, `sim_warn!`, `sim_info!` via
  `pub use helm_diag::{sim_stub, sim_warn, sim_info}`
- `helm-python/src/lib.rs` — calls `DiagSink::open(uri)` then `install_monitor(monitor)`
- `helm-cli/src/lib.rs` — calls `DiagSink::open_or_stderr(Some(uri))` then
  `install_monitor(monitor)`

---

## 4. Public API

### 4.1 `DiagLevel`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagLevel {
    Info,   // lowest severity
    Stub,
    Warn,
    Error,  // highest severity
}
```

Ordering is derived in declaration order: `Info < Stub < Warn < Error`. The sentinel
value for "pass all" is `DiagLevel::Info`. `as_tag()` returns `"INFO"`, `"STUB"`,
`"WARN"`, or `"ERR "` (space-padded to 4 chars).

### 4.2 `DiagEntry::format()` Output Format

```
[STUB] sim_ns=000001234 insns=000025750 gicv2-dist       pc=0x0000000040201234 | MRS ID_AA64MMFR4_EL1 -> 0
[WARN] sim_ns=000012300 insns=000025600 pl011-uart        pc=?                  | write to read-only reg 0x18
[INFO] sim_ns=000000000 insns=000000000 helm-loader       pc=?                  | ELF loaded: entry=0x4000_0000
[ERR ] sim_ns=000012500 insns=000025800 aarch64-execute   pc=0x000000004020ffff | unhandled exception ESR=0x96000004
```

Columns (stable; downstream parsers may rely on this format):

| Column | Width | Format |
|--------|-------|--------|
| `[LEVL]` | 6 chars | bracket + 4-char tag + bracket |
| `sim_ns=NNNNNNNNNNNN` | 18 chars | label + 12-digit zero-padded decimal |
| `insns=NNNNNNNNNNNN` | 18 chars | label + 12-digit zero-padded decimal |
| `<component>` | 16 chars min | left-justified, space-padded |
| `pc=0xHHHHHHHHHHHHHHHH` or `pc=?` | 20 chars | `{:#018x}` or placeholder |
| `| <message>` | variable | free-form text |

### 4.3 Thread-locals and Lifecycle

```
DiagSink::open(uri) --> (DiagSink, DiagMonitor)
                                       |
                         install_monitor(monitor)  [on sim thread]
                                       |
                         update_sim_ctx(insns, freq_hz)  [per step or quantum]
                                       |
                         sim_stub! / sim_warn! / sim_info!  [at call sites]
                                       |
                         uninstall_monitor()  [optional, at teardown]
                         drop(sink)          [joins drain thread, flushes]
```

`is_monitor_active()` is a fast path guard: the engine calls it to skip the
`update_sim_ctx` RefCell borrow when no backend is active, eliminating measurable
overhead at simulation speed.

### 4.4 Macros

All three macros delegate unconditionally to `emit()`. The format string is evaluated
at every call site regardless of whether a monitor is installed.

| Macro | Level | `pc=` form | no-pc form |
|-------|-------|------------|------------|
| `sim_stub!` | `Stub` | yes | yes |
| `sim_warn!` | `Warn` | yes | yes |
| `sim_info!` | `Info` | no | yes (only form) |

---

## 5. `DiagSink` URI System

`DiagSink::open(uri)` parses a URI string and opens the corresponding backend:

| URI | Backend | Notes |
|-----|---------|-------|
| `stderr:` or `stderr` or `""` | `Backend::Stderr` | Default; `eprintln!` each line |
| `null:` or `null` | `Backend::Null` | All entries discarded; benchmarking mode |
| `file:/path/to/file` | `Backend::File` | Appended; created if absent |
| `tcp:host:port` | `Backend::Tcp` | Connects; streams with `TCP_NODELAY` |
| *(unrecognized)* | `Err(InvalidInput)` | Error message mentions `helm-diag` |

`DiagSink::open_or_stderr(uri)` is the infallible constructor: logs a warning to
`eprintln!` and falls back to stderr if the URI fails. Used by `helm-cli`.

### 5.1 Enable / Disable

There is no global enable/disable switch. Use `null:` URI for benchmarking runs where
diagnostic overhead must be zero. The `is_monitor_active()` guard in the engine
eliminates the `update_sim_ctx` cost when the URI is `null:` or when no monitor is
installed.

---

## 6. `emit()` Fallback Behavior

When no `DiagMonitor` is installed on the calling thread, `emit()` falls back:

- Without `log-fallback` feature: `eprintln!` the formatted entry (all levels)
- With `log-fallback` feature: routes through the `log` crate at the appropriate level
  (`Error -> log::error!`, `Warn -> log::warn!`, `Stub -> log::debug!`, `Info -> log::info!`)

This ensures diagnostics are always visible even before engine startup installs a monitor.

---

## 7. Relationship to the Old `helm-debug::sim_trace`

`helm-diag` is a direct extraction of `helm-debug::sim_trace`. The public API is
intentionally almost identical; the differences are:

| Old (`helm-debug::sim_trace`) | New (`helm-diag`) | Change |
|-------------------------------|-------------------|--------|
| `Level` enum (5 variants incl. Branch) | `DiagLevel` enum (4 variants) | `Branch` removed |
| `MonitorEntry` | `DiagEntry` | Rename; same fields |
| `Monitor` | `DiagMonitor` | Rename; same behavior |
| `MonitorSink` | `DiagSink` | Rename; same behavior |
| `SIM_MONITOR` thread-local | `DIAG_MONITOR` thread-local | Rename |
| `sim_branch!` macro | Deleted | Replaced by `probe!(probes.branch, BranchEvent{...})` |
| Module path: `helm_debug::sim_trace::*` | Crate: `helm_diag::*` | Import path change |

`helm-debug` re-exports `sim_stub!`, `sim_warn!`, and `sim_info!` at its root so that
existing `use helm_debug::{sim_stub, ...}` import paths continue to compile. The
`sim_branch!` macro is not re-exported; missing callers get a compile error that
directs them to migrate.

---

## 8. Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Location | `framework/helm-diag/` | Framework crates have zero external deps; all other crates can depend on them |
| Dependencies | Zero mandatory deps | Prevents any cycle; `log` is an optional feature only |
| `DiagLevel::Branch` | Absent | Branch events belong in the probe/plugin layer, not the diag channel |
| Type rename (`Monitor` -> `DiagMonitor`) | Yes | Avoids ambiguity with future `Monitor` types; makes public API self-documenting |
| Queue depth | 4096 entries | Sufficient for any device-stub burst; at 1 GHz, a full queue represents ~4 us of simulation time |
| Drain timeout | 50 ms | Background thread flushes at most 20 times/second when idle; keeps file/TCP latency bounded |
| `emit()` fallback | `eprintln!` for all levels if no monitor | Diagnostics always visible even without a configured backend |
| `is_monitor_active()` | Separate predicate | Allows engine to skip `update_sim_ctx` RefCell borrow on STUB-free runs |
| `install_monitor` / `update_sim_ctx` | Thread-local, not `Arc<Mutex<>>` | Hot path must not block; one engine thread per hart is the standard model |
| `sim_branch!` | Deleted, not moved | Missing re-export produces a compile error that enforces migration |

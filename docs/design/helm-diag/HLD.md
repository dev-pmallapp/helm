# helm-diag — High-Level Design

> **Crate:** `helm-diag`
> **Location:** `framework/helm-diag/`
> **Phase:** Phase 0 (extracted from `helm-debug::sim_trace` before any other crate ships)
> **Dependencies:** none (zero external deps; `log` crate is optional feature only)

---

## 1. Purpose and Motivation

### 1.1 The Layer Violation

`helm-arch` and `helm-devices` both need to emit diagnostic messages. The natural
primitives for this are `sim_stub!`, `sim_warn!`, and `sim_info!` — structured,
async-delivery, zero-blocking macros that write to a configured backend. Those macros
currently live in `helm-debug::sim_trace`.

The problem is the dependency direction:

```
helm-arch  ──depends on──►  helm-debug   ✗  (runtime depends on debug tool — cycle risk)
helm-devices  ──────────►   helm-debug   ✗  (SDK depends on debug tool — violates layering)
```

`helm-debug` depends on `helm-core` and `helm-memory`. If `helm-arch` depended on
`helm-debug`, we would have a chain where framework crates depend on runtime crates.
`helm-devices` would be even worse: the Device SDK would pull in the debug infrastructure
as a mandatory dependency for every device and every test.

### 1.2 The Solution: Extract to `helm-diag`

`helm-diag` is the extraction of `helm-debug::sim_trace` into a standalone framework
crate with **zero dependencies**. It contains only:

- The `DiagEntry` struct and `DiagLevel` enum (the data model)
- `DiagMonitor` — the cheap, clonable, non-blocking sender
- `DiagSink` — the background drain thread and URI-based backend
- Thread-local `DIAG_MONITOR` and `SIM_CTX` and `install_monitor` / `update_sim_ctx`
- `emit()` — the non-blocking dispatch function
- `sim_stub!`, `sim_warn!`, `sim_info!` — the call-site macros

Because `helm-diag` has zero mandatory dependencies, every crate in the project can
depend on it without risk of creating a cycle.

### 1.3 What Changes at Call Sites

**Old** (before extraction):

```rust
// In helm-arch/src/aarch64/execute/sysreg.rs
use helm_debug::sim_stub;
sim_stub!(component = "aarch64-sysreg", pc = state.pc, "MRS {:?} → 0", reg);
```

**New** (after extraction):

```rust
// In helm-arch/src/aarch64/execute/sysreg.rs
use helm_diag::sim_stub;
sim_stub!(component = "aarch64-sysreg", pc = state.pc, "MRS {:?} → 0", reg);
```

The macro signature, behavior, and output format are identical. Only the crate that
defines them changes.

---

## 2. Scope

### 2.1 What `helm-diag` Contains

| Item | Description |
|------|-------------|
| `DiagLevel` | `Info`, `Warn`, `Stub`, `Error` — four levels with ordering |
| `DiagEntry` | Structured log record: level + component + pc + timestamps + message |
| `DiagMonitor` | `Clone`-able, non-blocking `SyncSender<DiagEntry>` wrapper |
| `DiagSink` | Background drain thread; owns `Backend`; URI constructor |
| `Backend` | `Stderr \| File \| Tcp \| Null` — internal, not public |
| `SimContext` | `{ sim_ns: u64, sim_insns: u64 }` — updated by the engine per step |
| `DIAG_MONITOR` | `thread_local! RefCell<Option<DiagMonitor>>` — current thread's sender |
| `SIM_CTX` | `thread_local! RefCell<SimContext>` — current thread's time context |
| `install_monitor(m)` | Registers a `DiagMonitor` on the calling thread |
| `update_sim_ctx(insns, freq_hz)` | Advances the thread-local `SimContext` |
| `emit(level, component, pc, msg)` | Non-blocking dispatch; falls back to `eprintln!` |
| `sim_stub!(...)` | Macro for STUB-level messages |
| `sim_warn!(...)` | Macro for WARN-level messages |
| `sim_info!(...)` | Macro for INFO-level messages |

### 2.2 What `helm-diag` Does NOT Contain

| Item | Where it lives | Why not in helm-diag |
|------|----------------|----------------------|
| `sim_branch!` | **Deleted** | Branch events now go through `probe!(probes.branch, BranchEvent{...})` at Layer 1; the `MonitorEntry`-based BRNC path is removed as part of Instrumentation-v2 |
| `TraceLogger` / `TraceEvent` | `helm-debug` | Rich JSONL structured events — belongs in the debug tool |
| `GdbServer` | `helm-debug` | Debug tool, not a primitive |
| `CheckpointManager` | `helm-debug` | Debug tool, not a primitive |
| `HelmEventBus` | `helm-devices` | Synchronous pub-sub — separate system |
| `Probe<T>`, `probe!()` | `helm-probe` | Typed, zero-cost probe points — separate system |
| `PluginRegistry` | `helm-plugin` | Typed callback registry — separate system |

### 2.3 The Deletion of `sim_branch!`

`sim_branch!` emitted `Branch`-level `MonitorEntry` records. Its purpose was to let
`branch_trace.py` parse BRNC lines from a log file and resolve branch targets.

In Instrumentation-v2, this role is taken over by `Probe<BranchEvent>` (Layer 1) feeding
through `ProbePluginBridge` (Layer 2) to the `BranchTrace` plugin. The probe path is:

- Zero-cost in release (ZST probe, whole block eliminated)
- Typed — `BranchEvent { from_pc, to_pc, taken, kind }` is a Rust struct, not a parsed string
- Pluggable — the `BranchTrace` plugin can output to any `TraceSink`, including a file

`DiagLevel::Branch` is therefore removed from `helm-diag`. The `Level` enum in
`helm-debug::sim_trace` had five variants; `DiagLevel` in `helm-diag` has four.

---

## 3. Dependency Graph

### 3.1 Position in the Crate DAG

```
(no deps)
    │
    ▼
helm-diag   ◄──────────────────────────────────────────────────────────┐
    │                                                                    │
    ├── helm-arch       (sysreg stubs, unimplemented opcodes)            │
    ├── helm-devices    (Device SDK — device MMIO stubs)                 │
    ├── helm-engine     (boot progress, loader messages)                 │
    ├── helm-debug      (opens DiagSink, installs DiagMonitor at startup)│
    └── helm-python     (forwards URI from Python config to DiagSink)   ─┘
```

The key property: `helm-diag` has zero dependencies. Any crate can depend on it.
`helm-debug` no longer needs to export `sim_trace` — it delegates to `helm-diag` and
adds its own higher-level tooling (TraceLogger, GDB, Checkpoint) on top.

### 3.2 Comparison with the Old Graph

Before extraction (`helm-debug::sim_trace` in `runtime/helm-debug`):

```
helm-core ─► helm-debug  ─► ...
                 ▲
        helm-arch would need to depend here  (runtime → debug violation)
        helm-devices would need to depend here  (SDK → debug violation)
```

After extraction (`helm-diag` in `framework/helm-diag`):

```
(zero deps) ─► helm-diag ─► helm-arch, helm-devices, helm-engine, helm-debug
```

No cycles. No layer violations. `helm-debug` gains a dependency on `helm-diag` (not the
reverse), which is the correct direction.

---

## 4. `DiagEntry` Output Format

`DiagEntry::format()` produces a single line. The format is identical to the old
`MonitorEntry::format()` except the `BRNC` prefix is absent (Branch level is removed):

```
[STUB] sim_ns=000001234 insns=000025750 gicv2-dist       pc=0x0000000040201234 | MRS ID_AA64MMFR4_EL1 → 0
[WARN] sim_ns=000012300 insns=000025600 pl011-uart       pc=?                  | write to read-only reg 0x18
[INFO] sim_ns=000000000 insns=000000000 helm-loader      pc=?                  | ELF loaded: entry=0x4000_0000
[ERR ] sim_ns=000012500 insns=000025800 aarch64-execute  pc=0x000000004020ffff | unhandled exception ESR=0x96000004
```

Columns:
- `[LEVL]` — 4-character level tag, space-padded for ERR
- `sim_ns=NNNNNNNNNNNN` — 12-digit simulated nanoseconds
- `insns=NNNNNNNNNNNN` — 12-digit instruction count
- `component` — left-padded to 16 characters
- `pc=0xADDR` or `pc=?` — 18-character hex or placeholder
- `| message` — free-form message

---

## 5. The `DiagSink` URI System

`DiagSink::open(uri)` parses a URI string and opens the corresponding backend. The URI
format is identical to the old `MonitorSink::open(uri)`:

| URI | Backend | Notes |
|-----|---------|-------|
| `stderr:` or `stderr` | `Backend::Stderr` | Default; `eprintln!` each line |
| `null:` or `null` | `Backend::Null` | All entries discarded; benchmarking mode |
| `file:/path/to/file` | `Backend::File` | Appends; creates if absent |
| `tcp:host:port` | `Backend::Tcp` | Connects; streams with TCP_NODELAY |
| *(empty string)* | `Backend::Stderr` | Treated as `stderr:` |

If the URI is unrecognized, `open` returns `Err(io::Error)` with a descriptive message.

`DiagSink::open_or_stderr(uri)` is the convenience constructor that logs a warning and
falls back to stderr if the URI fails.

### 5.1 Enable / Disable

| Mechanism | Effect |
|-----------|--------|
| No `DiagSink` installed | `emit()` falls back to `eprintln!` for all non-Branch levels |
| URI `null:` | All entries are discarded silently |
| URI `stderr:` | All entries are written to stderr (default) |
| URI `file:/path` | Entries appended to the file |
| URI `tcp:host:port` | Entries streamed to a TCP listener |

There is no global enable/disable switch. The `null:` URI is the intended mechanism
for benchmarking runs where diagnostic overhead must be zero.

---

## 6. Relationship to the Old `helm-debug::sim_trace` Module

`helm-diag` is a direct extraction of `helm-debug::sim_trace`. The public API is
intentionally almost identical; the only differences are:

| Old (`helm-debug::sim_trace`) | New (`helm-diag`) | Change |
|-------------------------------|-------------------|--------|
| `Level` enum (5 variants incl. Branch) | `DiagLevel` enum (4 variants) | `Branch` removed |
| `MonitorEntry` | `DiagEntry` | Rename; same fields |
| `Monitor` | `DiagMonitor` | Rename; same behavior |
| `MonitorSink` | `DiagSink` | Rename; same behavior |
| `SIM_MONITOR` thread-local | `DIAG_MONITOR` thread-local | Rename |
| `sim_branch!` macro | Deleted | Replaced by `probe!(probes.branch, BranchEvent{...})` |
| Module path: `helm_debug::sim_trace::*` | Crate: `helm_diag::*` | Import path change |

After the extraction, `helm-debug` re-exports `helm_diag` types for any callers that
used the old path, and eventually removes them.

---

## 7. Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Location | `framework/helm-diag/` | Framework crates have zero external deps; all other crates can depend on them |
| Dependencies | Zero mandatory deps | Prevents any cycle; `log` is an optional feature only |
| `DiagLevel::Branch` | Deleted | Branch events belong in the probe/plugin layer, not the diag channel |
| Type rename (`Monitor` → `DiagMonitor`) | Yes | Avoids ambiguity with future `Monitor` types; makes the crate's public API self-documenting |
| Queue depth | 4096 entries | Identical to the original; sufficient for any device-stub burst without significant memory cost |
| Drain timeout | 50 ms | Keeps the background thread responsive without spinning; periodic flush every 50 ms |
| `emit()` fallback | `eprintln!` for non-Branch levels if no monitor | Diagnostics are always visible, even without a configured backend |
| `install_monitor` / `update_sim_ctx` | Thread-local, not `Arc<Mutex<>>` | Hot path must not block; one engine thread per hart is the standard model |
| `sim_branch!` call sites | Migrated to `probe!(...)` in `helm-arch` | Instrumentation-v2 mandate; compile error (removed macro) enforces migration |

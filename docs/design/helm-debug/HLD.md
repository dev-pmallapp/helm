# helm-debug — High-Level Design

> **Crate:** `helm-debug`
> **Phase:** Phase 2 (GDB stub, CheckpointManager), Phase 2+ (Watchpoint/Breakpoint engine)
> **Dependencies:** `helm-core`, `helm-probe`, `helm-diag`
>
> **Instrumentation-v2 change:** `sim_trace.rs` and `TraceLogger` have been extracted to
> `helm-diag` and deleted from this crate respectively. See
> [../instrumentation-v2/CHANGES.md](../instrumentation-v2/CHANGES.md) for migration detail.

---

## Overview

`helm-debug` provides developer tooling that gives visibility into and control over a
running simulation. It is **not** an analysis or delivery system — those concerns live in
`helm-spy` and `helm-report`. It has no hot-path code.

| Subsystem | Purpose | Phase |
|-----------|---------|-------|
| `GdbServer` | GDB Remote Serial Protocol server over TCP/Unix socket | Phase 2 |
| `CheckpointManager` | Full-state save/restore via the `HelmAttr` system | Phase 2 |
| `WatchpointEngine` | Software watchpoints via `Probe<MemAccessEvent>` subscription | Phase 2+ |
| `BreakpointEngine` | PC breakpoints via `Probe<CpuStepEvent>` (pre_step) subscription | Phase 2+ |
| `InspectionAPI` | Dump arch state, memory range, device registers on demand | Phase 3 |

None of these subsystems are on the hot instruction-fetch path. GDB runs in a dedicated
thread. Watchpoints and breakpoints are checked via probe subscriptions, which are
zero-cost in release builds.

---

## What Was Removed (Instrumentation-v2)

| Removed | Where it went |
|---|---|
| `src/sim_trace.rs` (entire module) | Moved to `framework/helm-diag/` as a standalone micro-crate |
| `TraceLogger` struct | Deleted — it was a stub. Replaced by `helm-spy::EventStream<InsnInfo>` |
| `HelmEventBus` dependency (for TraceLogger) | Gone — probes replace event bus for instrumentation |

---

## Subsystem Overviews

### 1. GDB Server

Implements the GDB Remote Serial Protocol (RSP) so that a stock `gdb` or `lldb` binary
can debug a running simulation without modifying the simulated software.

- Binds a TCP port (default `1234`) or Unix domain socket.
- Runs in a dedicated `std::thread` separate from the simulation thread.
- Pauses and resumes the simulation via an `AtomicBool` halt flag read by the engine.
- Exposes the `GdbTarget` trait that `HelmEngine<T>` implements.
- Minimum RSP packet set: `?`, `g`, `G`, `m`, `M`, `c`, `s`, `z0`/`Z0`, `k`, `D`.
- LLDB compatibility via `qXfer:features:read` + `target.xml` for AArch64 and RISC-V.

### 2. CheckpointManager

Saves and restores the complete simulation state.

- Checkpoint format: CBOR binary with a JSON fallback for human inspection.
- Version header: `{ version, helm_version, isa, mode, created_at }`.
- `HelmAttr` is the sole serialization mechanism — no manual `checkpoint_save()` per
  component.
- After restore, component `init()` is re-run to re-establish probe subscriptions.
- Note: probe subscriptions (from `helm-spy::SpySession`) are NOT checkpointed.
  The session is rebuilt from Python config after restore.

### 3. WatchpointEngine

Software watchpoints without hardware support. Implemented as a probe subscriber.

```rust
pub struct WatchpointEngine {
    watchpoints: Vec<Watchpoint>,
}
pub struct Watchpoint {
    pub addr:   u64,
    pub size:   usize,
    pub kind:   WatchKind,   // Read | Write | ReadWrite
    pub action: Box<dyn Fn(&MemAccessEvent) + Send + Sync>,
}
impl WatchpointEngine {
    /// Subscribe to mem probe. Each access checks the watchpoint list.
    /// Zero cost in release (probe is ZST).
    pub fn subscribe(&mut self, probes: &mut CpuProbes) { … }
}
```

Cost in dev: one range check per registered watchpoint per memory access.
Cost in release: zero (probe is ZST).

### 4. BreakpointEngine

PC breakpoints implemented via the `pre_step` probe.

```rust
pub struct BreakpointEngine {
    breakpoints: Vec<Breakpoint>,
}
pub struct Breakpoint {
    pub pc:     u64,
    pub action: Box<dyn Fn(u64) + Send + Sync>,
    pub one_shot: bool,
}
impl BreakpointEngine {
    /// Subscribe to pre_step probe. Fires action when PC matches.
    pub fn subscribe(&mut self, probes: &mut CpuProbes) { … }
}
```

### 5. InspectionAPI (Phase 3)

On-demand dump of simulator internal state without stopping or modifying execution.

```rust
pub struct InspectionAPI<'a> {
    engine: &'a HelmEngine<impl TimingModel>,
}
impl<'a> InspectionAPI<'a> {
    pub fn arch_state(&self) -> ArchSnapshot { … }
    pub fn read_memory(&self, addr: u64, len: usize) -> Vec<u8> { … }
    pub fn disassemble(&self, addr: u64, count: usize) -> Vec<String> { … }
    pub fn symbol_at(&self, addr: u64) -> Option<&str> { … }
}
```

---

## Diagnostic Channel

`helm-debug` opens a `DiagSink` (from `helm-diag`) at startup and installs a
`DiagMonitor` on the simulation thread. This replaces the old `MonitorSink` that was
inside `sim_trace.rs`. The `helm-diag` crate owns the emit path; `helm-debug` owns the
sink lifecycle.

```rust
// In HelmEngine startup (called from Python build_simulator()):
let (sink, monitor) = helm_diag::DiagSink::open(uri.as_deref())?;
helm_diag::install_monitor(monitor);
engine.diag_sink = Some(sink);   // dropped at engine end → joins drain thread
```

---

## Module Structure

```
runtime/helm-debug/
└── src/
    ├── lib.rs               # Public re-exports; no sim_trace module
    ├── gdb/
    │   ├── mod.rs           # GdbServer, GdbTarget, StopReason, BreakpointKind
    │   ├── rsp.rs           # RSP packet framing, checksum, packet handlers
    │   ├── target.rs        # GdbReg enum, GdbTarget trait
    │   └── xml.rs           # target.xml generation for RISC-V / AArch64
    ├── checkpoint/
    │   ├── mod.rs           # CheckpointManager
    │   ├── format.rs        # CBOR header, version struct
    │   └── error.rs         # CheckpointError
    ├── watchpoint.rs        # WatchpointEngine, Watchpoint, WatchKind
    ├── breakpoint.rs        # BreakpointEngine, Breakpoint
    └── inspect.rs           # InspectionAPI (Phase 3)
```

---

## Dependencies

| Crate | Usage |
|---|---|
| `helm-core` | `ThreadContext`, `ArchState`, `AttrRegistry` |
| `helm-probe` | `CpuProbes` — Watchpoint + Breakpoint subscribe to probe events |
| `helm-diag` | Open `DiagSink`; install `DiagMonitor` at startup |
| `ciborium` (or `serde_cbor`) | Checkpoint CBOR encoding/decoding |
| `serde` | Version header serialization |
| `std::net` / `std::os::unix::net` | GDB server socket |

Not in dependencies: `helm-spy`, `helm-report`, `helm-plugin` (none of these).

---

## Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| TraceLogger removed | Deleted (was stub) | `helm-spy::EventStream<InsnInfo>` is the correct replacement |
| sim_trace moved to helm-diag | Extracted, not deleted | Device stubs need a diagnostic channel; layer DAG requires it outside helm-debug |
| Watchpoints via probes | `Probe<MemAccessEvent>` subscription | Zero cost in release; no per-access check overhead when no watchpoints configured |
| Breakpoints via pre_step probe | `Probe<CpuStepEvent>` subscription | Same zero-cost guarantee |
| GDB thread model | Dedicated `std::thread` + halt AtomicBool | Decoupled; no shared state crossing thread boundary during execution |
| Checkpoint format | CBOR primary, JSON fallback | Compact binary for production; human-readable for debugging |
| No HelmEventBus dep | Removed | Probes replace event bus for instrumentation; bus was only used by TraceLogger |

# helm-debug — High-Level Design

> **Crate:** `helm-debug`
> **Phase:** Phase 1 (GDB stub), Phase 2 (TraceLogger, CheckpointManager)
> **Dependencies:** `helm-core`, `helm-devices/src/bus/event_bus`, `helm-memory`

---

## Overview

`helm-debug` provides three subsystems that give the user visibility into and control over a running simulation without modifying the simulated software:

| Subsystem | Purpose | Phase |
|-----------|---------|-------|
| `GdbServer` | GDB Remote Serial Protocol server over TCP/Unix socket | Phase 1 |
| `TraceLogger` | Ring-buffer event recorder; subscriber to `HelmEventBus` | Phase 2 |
| `CheckpointManager` | Full-state save/restore via the `HelmAttr` system | Phase 2 |

All three subsystems observe the simulation through `HelmEventBus`. None of them are on the hot instruction-fetch path.

---

## Subsystem Overviews

### 1. GDB Server

Implements the GDB Remote Serial Protocol (RSP) so that a stock `gdb` or `lldb` binary can debug a running simulation with no modifications to the simulated software.

- Binds a TCP port (default `1234`) or Unix domain socket.
- Runs in a dedicated `std::thread` separate from the simulation thread.
- Pauses and resumes the simulation by posting a stop/resume event to `HelmEventBus`.
- Exposes the `GdbTarget` trait that `HelmEngine<T>` implements.
- Minimum RSP packet set for Phase 1: `?`, `g`, `G`, `m`, `M`, `c`, `s`, `z0`/`Z0`, `k`, `D`.
- LLDB compatibility via `qXfer:features:read` and `target.xml` for RISC-V and AArch64 register descriptions.
- Multi-hart support via `vCont;c:1;s:2` (per-thread control, Phase 1).

### 2. TraceLogger

A structured event recorder that produces a `.jsonl` (JSON Lines) file.

- Subscribes to `HelmEventBus` as an `EventHandle` holder during `elaborate()`.
- Uses a lock-free ring buffer (capacity configurable, default 65 536 events) to absorb bursts without blocking the simulation loop.
- Full policy on ring-buffer overwrite: oldest events are silently discarded (circular overwrite).
- Supports Python callbacks via PyO3 (GIL acquired per callback).
- Flush is on-demand or at simulation exit.

### 3. CheckpointManager

Saves and restores the complete simulation state using the `HelmAttr` attribute system.

- Checkpoint format: CBOR binary (compact, versioned header) with a JSON fallback for inspection.
- Full-state checkpoint for Phase 0/Phase 2 (no differential; differential deferred to future phase).
- Version header: `{ version, helm_version, isa, mode, created_at }`.
- `HelmAttr` is the sole serialization mechanism; no manual `checkpoint_save()` methods on individual components.
- After restore, each component's `init()` is re-run so that `HelmEventBus` subscriptions are re-established.

---

## Integration — How the Three Subsystems Fit Together

```
                 ┌─────────────────────────────────────────┐
                 │           HelmEngine<T>                  │
                 │  (hot instruction loop)                  │
                 └───────────────┬─────────────────────────┘
                                 │ fires HelmEvent variants
                                 ▼
                 ┌─────────────────────────────────────────┐
                 │           HelmEventBus                   │
                 │  (synchronous pub-sub)                   │
                 └────────┬────────────────────────────────┘
          ┌───────────────┼─────────────────────────────┐
          │               │                             │
          ▼               ▼                             ▼
  ┌──────────────┐ ┌──────────────┐         ┌─────────────────────┐
  │  GdbServer   │ │ TraceLogger  │         │ CheckpointManager   │
  │  (TCP/Unix)  │ │ (ring buffer)│         │ (CBOR snapshots)    │
  │              │ │              │         │                     │
  │ GDB RSP ◄───┘│ .jsonl flush  │         │ save / restore      │
  │ pause/resume │ Python cbs    │         │ World state         │
  └──────────────┘ └──────────────┘         └─────────────────────┘
```

The `GdbServer` thread never touches the simulation state directly; it sends pause/resume signals through `HelmEventBus` and reads/writes state only after the simulation loop has quiesced.

---

## Dependencies

| Crate | Usage |
|-------|-------|
| `helm-core` | `ThreadContext`, `ArchState`, `HelmObject`, `AttrStore` |
| `helm-devices/src/bus/event_bus` | `HelmEventBus`, `HelmEvent`, `HelmEventKind`, `SubscriberId` |
| `helm-memory` | `MemoryMap` (for GDB memory reads/writes) |
| `serde` + `serde_json` | `TraceEvent` serialization to JSON Lines |
| `ciborium` (or `serde_cbor`) | Checkpoint CBOR encoding/decoding |
| `pyo3` (optional feature) | Python trace callbacks |
| `std::net` / `std::os::unix::net` | GDB server socket |

---

## Module Structure

```
helm-debug/
└── src/
    ├── lib.rs               # Public re-exports
    ├── gdb/
    │   ├── mod.rs           # GdbServer, GdbTarget, StopReason, BreakpointKind
    │   ├── rsp.rs           # RSP packet framing, checksum, packet handlers
    │   ├── target.rs        # GdbReg enum, GdbTarget trait
    │   └── xml.rs           # LLDB target.xml generation for RISC-V / AArch64
    ├── trace/
    │   ├── mod.rs           # TraceLogger, TraceEvent
    │   └── ring.rs          # Lock-free ring buffer implementation
    └── checkpoint/
        ├── mod.rs           # CheckpointManager
        ├── format.rs        # CBOR header, version struct
        └── error.rs         # CheckpointError
```

---

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| GDB thread model | Dedicated `std::thread` | Keeps RSP I/O off the simulation hot loop |
| GDB/simulation communication | Channel + `HelmEventBus` pause/resume | Decoupled; no shared mutable state crossing the thread boundary |
| Trace output format | JSON Lines (`.jsonl`) | One event per line; stream-friendly; easy to process with `jq` |
| Ring buffer full policy | Overwrite oldest | Maintains recent history; avoids blocking the simulation |
| Ring buffer capacity default | 65 536 events | Covers ~1 ms of activity at 100 MHz; configurable |
| Python callback dispatch | PyO3 GIL acquire per callback | Safe; callbacks are rare (user-defined filters) |
| Checkpoint format | CBOR primary, JSON fallback | Compact binary for production; human-readable JSON for debugging |
| Checkpoint strategy | Full-state (Phase 2) | Simpler to implement correctly; differential deferred |
| Checkpoint mechanism | `HelmAttr` system only | Single source of truth; no duplicate serialization logic per component |
| Post-restore subscription renewal | Component `init()` re-run | `HelmEventBus` subscriptions are registered in `init()`; re-running it re-connects all subscribers |

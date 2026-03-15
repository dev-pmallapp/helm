# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project State

**Design-complete, implementation not yet started.** The repository contains comprehensive architecture documentation but no Rust/Python code. All `crates/` directories exist in design docs only. Start by reading `AGENT.md` (387 lines) — it is the authoritative agent onboarding guide.

## Key Documentation

- `AGENT.md` — Agent onboarding: crate map, 15 critical design rules, phased build plan
- `docs/ARCHITECTURE.md` — Full system architecture and type hierarchy
- `docs/design/HLD.md` — Canonical top-level design doc and crate DAG
- `docs/object-model.md` — SimObject lifecycle, wiring rules, checkpoint protocol
- `docs/traits.md` — All trait definitions (ExecContext, SimObject, Device, TimingModel, etc.)
- `docs/api.md` — Rust and Python API reference
- `docs/testing.md` — Testing strategy (ISA tests, differential vs. QEMU/Spike, property-based)
- `docs/design/DESIGN-QUESTIONS.md` — 110 resolved design Q&As with diagrams
- `docs/design/<crate>/` — Per-crate HLD + LLD-*.md + TEST.md for all 10 crates

## Build Commands (once Cargo workspace exists)

```bash
cargo build --workspace
cargo test --workspace
cargo test --package helm-arch          # ISA tests only
cargo test --lib --workspace            # Unit tests only
cargo clippy --all --all-targets -- -D warnings
cargo fmt --check
cargo doc --no-deps --open
```

## Architecture Summary

### 10-Crate Workspace (planned: `crates/`)

| Crate | Responsibility |
|---|---|
| `helm-core` | ArchState, ExecContext, ThreadContext, MemInterface — no deps |
| `helm-arch` | ISA decode + execute: riscv/, aarch64/, aarch32/ |
| `helm-memory` | MemoryRegion tree, FlatView, MMIO dispatch, TLB/cache |
| `helm-timing` | Virtual / Interval / Accurate timing models |
| `helm-event` | EventQueue (BinaryHeap, discrete-event scheduling) |
| `helm-engine` | HelmEngine<T>, HelmSim enum, ExecMode, World, syscall emulation |
| `helm-devices` | Device trait, InterruptPin/Wire/Sink, DeviceRegistry, .so loader |
| `helm-debug` | GDB RSP stub, TraceLogger, CheckpointManager |
| `helm-stats` | PerfCounter, PerfHistogram, StatsRegistry |
| `helm-python` | PyO3 bindings + helm_ng Python config package |

### Irreducible Core

Every path through the simulator reduces to:
1. **ArchState** — register file + PC
2. **Decoder** — bytes → Instruction
3. **Executor** — (ArchState, Insn, MemInterface) → ΔArchState
4. **MemInterface** — read/write(addr, size) ↔ bytes

### Two Distinct Event Systems

- **EventQueue** (`helm-event`): schedule callbacks at future tick T — asynchronous/deferred
- **HelmEventBus** (`helm-devices/bus`): observable named events — synchronous/inline, not checkpointed

### Critical Design Rules (inviolable)

1. **Monomorphize timing only** — `HelmEngine<T: TimingModel>` is the sole generic parameter; timing is inlined, not vtable-dispatched
2. **ISA/mode are enum-dispatched** — one `match` per Python call, zero per instruction
3. **No dark state** — every persistent field must be a registered `AttrDescriptor`
4. **Device knows no base address** — `MemoryMap` owns placement; device registers MMIO via platform wiring
5. **Device knows no IRQ number** — `InterruptPin` fires a signal; the platform routes it
6. **No dynamic lookup in the hot loop** — all cross-component `Arc` refs stored during `elaborate()`
7. **Python describes; Rust simulates** — config is frozen after `build_simulator()`; no mutation during simulation
8. **Determinism by default** — no wall-clock, no background threads in the hot loop
9. **`HelmEventBus` is synchronous** — not checkpointed; subscribers re-register on checkpoint restore
10. **`init()` is self-contained** — no cross-component access; that happens in `elaborate(system)`

### SimObject Lifecycle

```
CONSTRUCT → init() → elaborate(system) → startup() → RUN → reset() / checkpoint_save/restore
```

- `init()`: internal state only
- `elaborate(system)`: register MMIO, store `Arc` refs, wire interrupts
- `startup()`: schedule initial events, assert signals
- `reset()`: return to post-startup state, idempotent
- `checkpoint_save/restore`: architectural state only — no perf counters

### `HelmSim` — PyO3 Boundary

`HelmSim` is an enum (`Virtual` | `Interval` | `Accurate`) that wraps `HelmEngine<T>`. It is the sole object exposed to Python. All Python calls enter through `HelmSim`; ISA and mode are dispatched once per call, not per instruction.

## Phased Build Plan

| Phase | Deliverables |
|---|---|
| **0 — MVP** | RISC-V SE simulator, ~50 Linux syscalls, riscv-tests pass, no timing |
| **1 — Timing** | EventQueue, MemoryRegion tree, GDB stub, Interval timing, UART/PLIC devices |
| **2 — Python** | helm_ng config package, AArch64 ISA, TraceLogger, Checkpoint |
| **3 — Full System** | Linux boot, OoO pipeline, AArch32, JIT/binary translation |

## Testing Strategy

- **ISA correctness**: official riscv-tests vectors + AArch64 torture tests
- **Differential testing**: QEMU/Spike traces vs. helm-ng execution
- **Property-based**: `proptest` for memory layouts and instruction sequences
- **Benchmarks**: `criterion` for IPC accuracy regressions (Interval vs. Accurate)
- **Python config tests**: `pytest` for the helm_ng package (Phase 2+)

See `docs/testing.md` for the full strategy and planned test locations per crate.

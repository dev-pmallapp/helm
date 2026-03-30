# Overview

helm-ng is a research-grade hardware simulator written in Rust with a
Python configuration layer. It combines ideas from four established
simulators — QEMU (fast emulation), gem5 (configurable
microarchitecture), Simics (scriptable platform construction), and
Higan (multi-fidelity accuracy selection) — into a single, modular
codebase designed from first principles.

## Design Goals

1. **Multi-fidelity in one binary** — switch between functional
   emulation (IPC=1, QEMU-speed) and cycle-accurate simulation
   (gem5 O3CPU-style) without rebuilding.

2. **Composable platforms** — wire up CPUs, buses, caches, and devices
   in Python (gem5-style `fs.py`) or Rust; everything is a trait
   object.

3. **ISA extensibility** — new architectures (RISC-V, AArch64, AArch32)
   plug in via enum dispatch without touching the engine.

4. **Determinism by default** — no wall-clock, no background threads
   in the simulation hot loop. Every run is reproducible.

5. **Plugin-first instrumentation** — instruction tracing, cache
   simulation, and profiling are all pluggable via the `HelmPlugin`
   trait.

## Positioning

| Dimension | QEMU | gem5 | Simics | Higan | helm-ng |
|-----------|------|------|--------|-------|---------|
| Language | C | C++ / Python | C / DML | C++ | Rust / Python |
| Primary use | Fast emulation | uArch research | Platform modelling | Console accuracy | All four |
| Timing models | None (FE only) | Atomic / Minor / O3 | Transaction-level | Per-chip | Virtual / Interval / Accurate |
| Config layer | CLI + QOM | Python SimObjects | Python + DML | Built-in | Python + Rust traits |
| JIT | TCG → host | N/A (interp) | JIT (x86) | Per-chip JIT | Pluggable JitBackend |
| Device model | QOM + MMIO | Ports + MemObject | DML interfaces | Per-chip | `Device` trait + bus tree |
| Plugin API | TCG plugins (C) | Probes (C++) | Haps (C) | N/A | Rust `HelmPlugin` trait |
| Determinism | Best-effort | Yes | Yes | Cycle-exact | Yes (by design) |

## The 4-Item Irreducible Core

First-principles analysis of every existing simulator reveals that all
simulation complexity decomposes to exactly four irreducible
abstractions. Everything else — caches, timing, events, devices, OS
interfaces, config layers — composes on top of these four.

```text
1. ArchState     — All architecturally-visible state: integer/FP
                   registers, PC, CSRs (RISC-V) or system registers
                   (AArch64), PSTATE.
                   Crate: helm-core (trait ArchState)

2. Decoder       — bytes → Instruction enum
                   Consumes raw bytes, returns a decoded instruction.
                   Pure function, no side effects.
                   Crate: helm-arch (aarch64::decode, riscv::decode)

3. Executor      — (ArchState, Instruction, MemInterface) → ΔArchState
                   Applies instruction semantics. Reads/writes
                   ArchState. Calls MemInterface for load/store.
                   Crate: helm-arch (aarch64::execute, riscv::execute)

4. MemInterface  — read(addr, size) → bytes | write(addr, size, bytes)
                   Bridge between CPU and memory subsystem.
                   Crate: helm-core (trait MemInterface)
```

**Architectural consequence:** build and validate these four items
first. A passing ISA test suite against just these four (flat RAM, no
events, no timing, no devices) is Phase 0 complete. Every other crate
layers on top.

## Execution Modes

helm-ng decomposes simulation along two orthogonal axes:

### Execution mode — what hardware surface the workload sees

- **FE (Functional Emulation)** — execute instructions correctly, no
  timing, no OS interface. Used for ISA validation, fast-forward, and
  correctness testing.

- **SE (Syscall Emulation)** — FE + syscall interception. Linux
  syscalls are intercepted and emulated on the host via
  `LinuxAarch64SyscallHandler` or `LinuxRiscv64SyscallHandler`.

- **FS (Full System)** — complete hardware platform simulation. Boot a
  real kernel on an emulated SoC with GIC, UART, VirtIO, timers, and a
  generated device tree.

### Timing accuracy — how much microarchitectural detail is modelled

- **VirtualTiming** — ideal-IPC model (IPC=1). Global virtual clock,
  discrete event queue. Fastest mode. Like QEMU.

- **IntervalTiming** — Sniper-style interval simulation. Execute
  intervals functionally, apply timing at miss events. ~5% IPC error
  vs cycle-accurate. Like Simics.

- **AccurateTiming** — full cycle-accurate pipeline simulation. Maximum
  fidelity. Like gem5 O3CPU.

These axes are independent: you can boot Linux in FS+VirtualTiming for
rapid bring-up, then switch to FS+IntervalTiming for a region of
interest.

## High-Level Data Flow

```text
Guest Binary / Kernel
        │
        ▼
┌─────────────┐     ┌──────────────┐
│  ELF Loader  │ or │ Image Loader  │    (helm-engine::loader)
└──────┬──────┘     └──────┬───────┘
       │                   │
       ▼                   ▼
┌──────────────────────────────────┐
│     HelmEngine<T: TimingModel>   │    (helm-engine)
│  fetch → decode → execute/step   │
│  ArchState: X0-X30, SP, PC,     │
│    NZCV, V0-V31, sysregs        │
└────┬─────────────────┬──────────┘
     │ SE mode          │ FS mode
     ▼                  ▼
┌──────────┐   ┌────────────────┐
│ Syscall  │   │ HelmBoard      │   (helm-engine::se / session)
│ Handler  │   │ GIC, UART, etc │
└──────────┘   └────────────────┘
     │                  │
     ▼                  ▼
┌──────────────────────────────┐
│       FlatMem / Address      │   (helm-memory)
│       Space + MMIO dispatch  │
└──────────────────────────────┘
```

## Workspace Layout

The project is a Cargo workspace with 27 Rust crates plus a Python
package, organized into four domain directories:

- **`framework/`** — stable APIs and shared primitives (11 crates)
- **`runtime/`** — execution engine and frontends (6 crates)
- **`hw/`** — concrete hardware device implementations (8 crates)
- **`debug/`** — instrumentation and analysis (2 crates: helm-spy,
  helm-report)

See [Crate Map](crate-map.md) for the full inventory and dependency
graph.

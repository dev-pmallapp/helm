# Overview

HELM (**Hybrid Emulation Layer for Microarchitecture**) is a Rust-based
system simulator that can run AArch64 binaries and boot Linux kernels on
emulated ARM platforms. It combines ideas from QEMU (fast binary
translation), gem5 (configurable microarchitecture), and Simics
(scriptable platform construction) into a single, modular codebase.

## Design Goals

1. **Multi-fidelity in one binary** — switch between functional
   emulation (IPC = 1, QEMU-speed) and cycle-accurate simulation
   (gem5 O3CPU-style) without rebuilding.
2. **Composable platforms** — wire up CPUs, buses, caches, and devices
   in Python (gem5-style `fs.py`) or Rust; everything is a trait object.
3. **ISA extensibility** — new architectures (RISC-V, x86 stubs exist)
   plug in via the `IsaFrontend` trait without touching the engine.
4. **Plugin-first instrumentation** — instruction tracing, cache
   simulation, hot-block profiling, and fault detection are all plugins
   that can be hot-loaded between simulation phases.
5. **TDD from day one** — every crate carries a `src/tests/` tree; the
   project follows red-green-refactor discipline.

## Positioning vs QEMU, gem5, Simics

| Dimension | QEMU | gem5 | Simics | HELM |
|-----------|------|------|--------|------|
| Language | C | C++ / Python | C / DML | Rust / Python |
| Primary use | Fast emulation | uArch research | Platform modelling | All three |
| Timing models | None (FE only) | Atomic / Minor / O3 | Transaction-level | FE / ITE / CAE |
| Config layer | CLI + QOM | Python SimObjects | Python + DML | Python + Rust traits |
| Binary translation | TCG → host | N/A (interp only) | JIT (x86 host) | TCG IR → Cranelift JIT |
| Device model | QOM + MMIO | Ports + MemObject | DML interfaces | `Device` trait + bus tree |
| Plugin API | TCG plugins (C) | Probes (C++) | Haps (C) | Rust trait + callbacks |

## Execution Modes

HELM decomposes simulation along two orthogonal axes:

**Execution mode** — what hardware surface the workload sees:

- **SE (Syscall Emulation)** — run a user-space ELF binary; Linux
  syscalls are intercepted and emulated on the host.
- **FS (Full System)** — boot a real kernel on an emulated SoC with
  GIC, UART, VirtIO, timers, and a generated device tree.

**Timing accuracy** — how much microarchitectural detail is modelled:

- **FE (Functional Emulation)** — IPC = 1, no cache or pipeline model.
  100–1000 MIPS. Like QEMU.
- **ITE (Interval-Timing Emulation)** — per-instruction-class latencies,
  cache-level stalls, optional branch penalty. 1–100 MIPS. Like Simics.
- **CAE (Cycle-Accurate Emulation)** — full OoO pipeline (ROB, rename,
  IQ, LSQ), branch predictor, cache coherence. 0.1–1 MIPS. Like gem5 O3.

These axes are independent: you can boot Linux in FS+FE mode for rapid
bring-up, then switch to FS+ITE for a region of interest.

## High-Level Data Flow

```text
Guest Binary / Kernel
        │
        ▼
┌─────────────┐     ┌──────────────┐
│  ELF Loader  │ or │ Image Loader  │    (helm-engine)
└──────┬──────┘     └──────┬───────┘
       │                   │
       ▼                   ▼
┌──────────────────────────────────┐
│          Aarch64Cpu              │    (helm-isa)
│  fetch → decode → execute/step  │
│  regs: X0-X30, SP, PC, NZCV,   │
│         V0-V31, sysregs         │
└────┬─────────────────┬──────────┘
     │ SE mode          │ FS mode
     ▼                  ▼
┌──────────┐   ┌────────────────┐
│ Syscall  │   │ Platform + Bus │   (helm-device)
│ Handler  │   │ GIC, UART, etc │
└──────────┘   └────────────────┘
     │                  │
     ▼                  ▼
┌──────────────────────────────┐
│       AddressSpace           │   (helm-memory)
│   RAM regions + IoHandler    │
│   MMU / TLB / Cache          │
└──────────────────────────────┘
```

In SE mode the instruction stream also flows through the TCG path
(`helm-tcg`) for JIT-compiled execution via Cranelift. FS mode
uses the same TCG path with additional exception handling and
MMU integration.

## Workspace Layout

The project is a Cargo workspace with 19 Rust crates plus a Python
package. See [crate-map.md](crate-map.md) for the full dependency
graph. The Python layer lives in `python/helm/` and mirrors the
Rust platform/session APIs for scriptable configuration.

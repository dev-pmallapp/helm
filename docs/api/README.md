# API Guides

This directory contains task-oriented guides for the three main extension points of helm-ng. Each guide is written for an experienced Rust and systems engineer who wants to get productive quickly without reading the full design documentation first.

---

## Start Here

**"What am I trying to do?"**

| I want to... | Read this guide |
|--------------|----------------|
| Write a new simulated device (UART, timer, VirtIO, custom IP block) | [Device Author Guide](./device-author-guide.md) |
| Define a new machine or SoC platform (memory map, interrupt routing, boot config) | [Machine Author Guide](./machine-author-guide.md) |
| Add a new ISA or extend an existing one (MIPS, x86-64, new RISC-V extension) | [ISA Author Guide](./isa-author-guide.md) |
| Understand the overall architecture before reading any guide | [AGENT.md](../../AGENT.md) |

---

## Guide Summaries

### Device Author Guide

[`device-author-guide.md`](./device-author-guide.md)

For engineers implementing a new simulated device — a UART, timer, interrupt controller, VirtIO disk, or custom IP block. Covers the `Device` trait, the two-phase lifecycle (`alloc → init → finalize → run → deinit`), the `register_bank!` macro for MMIO register modeling with automatic serde checkpoint and Python introspection, interrupt output via `InterruptPin`, checkpoint correctness (what constitutes dark state and how to avoid it), device-to-device communication through `InterfaceRegistry`, attribute declaration for Python-configurable parameters, testing a device in isolation using headless `World` mode, and writing a `.so` plugin device with the C-ABI `helm_device_register` entry point and ABI versioning.

### Machine Author Guide

[`machine-author-guide.md`](./machine-author-guide.md)

For engineers defining a new machine or SoC platform — a minimal RISC-V virt board, a Raspberry Pi-like AArch64 board, or a custom embedded platform. Covers the Python configuration DSL, memory map design (address layout, device placement, reserved regions), interrupt routing from device pins through PLIC/CLINT to CPU, the boot sequence (ELF loading, reset vector, Device Tree), multi-core setup with per-hart PLIC and CLINT connections, attaching peripherals with their configuration parameters, subscribing to `HelmEventBus` for observability (exception tracing, CSR write monitoring, memory write tracing), and a complete reference for all `Simulation`, `Cpu`, `Memory`, `Uart16550`, `Plic`, and `Clint` constructor parameters.

### ISA Author Guide

[`isa-author-guide.md`](./isa-author-guide.md)

For engineers adding support for a new instruction set architecture or extending an existing one. Covers the complete `ExecContext` trait (the hot-path interface called billions of times per second, always statically dispatched as a generic parameter) and `ThreadContext` trait (the cold-path interface used by GDB, checkpoint, and syscall emulation, always `&mut dyn ThreadContext`), with the exact method signatures from the design. Also covers the fetch-decode-execute loop structure (including RISC-V C extension pre-expansion), CSR and system register modeling, exception entry via `raise_exception()` and the `StopReason` unwind mechanism, privilege level handling for both RISC-V (M/S/U) and AArch64 (EL0/EL1/EL2/EL3), `SyscallAbi` implementation for SE-mode syscall dispatch, registering the new ISA in the `Isa` enum and `HelmSim` factory, and testing strategy including `riscv-tests` integration and QEMU differential testing.

---

## Architecture Overview

For the full system architecture — crate dependency graph, design rules, execution modes, timing models, memory system, and object model — see [`AGENT.md`](../../AGENT.md).

For detailed per-crate design documentation, see:

- [`docs/design/helm-core/`](../design/helm-core/) — `ArchState`, `ExecContext`, `ThreadContext`, `AttrValue`
- [`docs/design/helm-arch/`](../design/helm-arch/) — RISC-V and AArch64 decode/execute
- [`docs/design/helm-devices/`](../design/helm-devices/) — `Device` trait, `register_bank!`, interrupt model, device registry
- [`docs/design/helm-engine/`](../design/helm-engine/) — `World`, `HelmEngine<T>`, `HelmSim`, scheduler, SE mode
- [`docs/design/helm-memory/`](../design/helm-memory/) — `MemoryRegion`, `MemoryMap`, `FlatView`, `CacheModel`
- [`docs/design/DESIGN-QUESTIONS.md`](../design/DESIGN-QUESTIONS.md) — 110 design questions with rationale and trade-offs

For the public API reference (traits and type signatures): [`docs/traits.md`](../traits.md).

For testing strategy: [`docs/testing.md`](../testing.md).

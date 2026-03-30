# Architecture

This section explains the design of helm-ng at the systems level.
Each page covers one major subsystem, describes the Rust types
involved, and explains how helm-ng differs from QEMU, gem5, and
Simics in that area.

## Pages

| Page | Topic |
|------|-------|
| [Overview](overview.md) | Design goals, positioning, and 4-item irreducible core |
| [Crate Map](crate-map.md) | All 25 crates with dependency graph |
| [Execution Pipeline](execution-pipeline.md) | Fetch-decode-execute data flow |
| [Memory Model](memory-model.md) | FlatMem, HelmAddressSpace, MMU, TLB |
| [Timing Model](timing-model.md) | VirtualTiming / IntervalTiming / AccurateTiming |
| [Exception Model](exception-model.md) | AArch64 exception levels and delivery |
| [Device Model](device-model.md) | Device trait, MMIO dispatch, interrupt wiring |
| [Event Systems](event-systems.md) | EventQueue vs HelmEventBus |
| [Platform & SoC](platform-and-soc.md) | Platform trait, ARM virt machine |
| [SimObject Lifecycle](simobject-lifecycle.md) | Construct → init → elaborate → run |
| [JIT Framework](jit-framework.md) | JitBackend trait, dynasm backend |
| [Plugin Architecture](plugin-architecture.md) | HelmPlugin trait, probe framework |
| [Python-Rust Boundary](python-rust-boundary.md) | HelmSim, PyO3, SimObject hierarchy |
| [Comparison: QEMU](comparison-qemu.md) | Structured comparison tables |
| [Comparison: gem5](comparison-gem5.md) | Structured comparison tables |
| [Comparison: Simics](comparison-simics.md) | Structured comparison tables |
| [Comparison: Higan](comparison-higan.md) | Multi-fidelity emulation comparison |

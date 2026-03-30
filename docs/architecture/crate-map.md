# Crate Map

helm-ng is a Cargo workspace with 27 Rust crates organized into four
domain directories. This page lists every crate, its responsibility,
and key public types.

## framework/ — Stable APIs and Shared Primitives (11 crates)

| Crate | Responsibility | Key Public Types |
|-------|---------------|-----------------|
| `helm-core` | Register state, execution context, memory interface | `ArchState`, `ExecContext`, `ThreadContext`, `MemInterface`, `MemFault`, `HartException` |
| `helm-decode` | QEMU-style `.decode` file parser + code generator | `DecodeTree`, `DecodePattern`, `BitField`, `FormatDef`, `generate_decoder()` |
| `helm-devices` | Device SDK: trait, registry, bus infrastructure | `Device`, `Transaction`, `InterruptPin`, `DeviceRegistry`, `HelmEventBus`, `MmioBus`, `AddressMap` |
| `helm-diag` | Structured diagnostic channel | `DiagEntry`, `DiagLevel`, `DiagContext`, `DiagMonitor`, `emit()` |
| `helm-event` | Discrete-event scheduler (min-heap) | `EventQueue`, `Tick`, `EventId` |
| `helm-jit` | Pluggable JIT backend framework | `JitBackend`, `CompiledBlock`, `JitCache` |
| `helm-memory` | Memory region tree, FlatMem, MMIO dispatch | `MemoryRegion`, `FlatView`, `MemoryMap`, `FlatMem`, `HelmAddressSpace` |
| `helm-plugin` | Simulation instrumentation API | `HelmPlugin`, `HelmPluginArgs`, `HelmPluginRegistry` |
| `helm-probe` | Zero-cost typed probe points | `Probe<T>`, `CpuProbes`, `GicProbes`, `BranchEvent`, `MemAccessEvent` |
| `helm-stats` | Lock-free performance counters | `PerfCounter`, `PerfHistogram`, `StatsRegistry` |
| `helm-timing` | Timing model trait + three implementations | `TimingModel`, `VirtualTiming`, `IntervalTiming`, `AccurateTiming`, `TimingInsnInfo` |

## runtime/ — Execution Engine and Frontends (6 crates)

| Crate | Responsibility | Key Public Types |
|-------|---------------|-----------------|
| `helm-arch` | ISA decode + execute (AArch64, RISC-V, AArch32) | `Aarch64ArchState`, `ArmCoreModel`, `Aarch64Insn`, `RiscvInsn`, `DecodeError` |
| `helm-cli` | CLI launchers with embedded Python interpreter | `run_python()`, `print_cpu_help()`, `handle_help_flags()` |
| `helm-debug` | GDB RSP stub + checkpoint serialization | `GdbServer`, `CheckpointManager`, `DebugError` |
| `helm-engine` | Simulation kernel, generic over timing | `HelmEngine<T>`, `HelmSim`, `Isa`, `ExecMode`, `StopReason`, `build_simulator()` |
| `helm-platform` | Platform trait + device topology | `Platform`, `AttachableSlot`, `SlotType`, `PlatformBuildPlan` |
| `helm-python` | PyO3 bindings (`_helm_ng` module) | `SimObject`, `HelmSystem`, `Cpu`, `Ram`, `HelmSpy` |

## hw/ — Concrete Hardware Implementations (8 crates)

| Crate | Responsibility | Key Public Types |
|-------|---------------|-----------------|
| `helm-hw-char` | Character devices (PL011 UART) | `Pl011` |
| `helm-hw-dma` | DMA controller models | `DmaEngine`, `DmaPort` |
| `helm-hw-intc` | Interrupt controllers (GICv2, GICv3) | `Gicv2Distributor`, `Gicv2CpuInterface`, `Gicv3Distributor`, `build_gicv2()` |
| `helm-hw-iommu` | IOMMU models (SMMUv3, AMD-Vi, RISC-V) | `SmmuState`, `IommuFault`, `IommuTlb`, `GuestMem` |
| `helm-hw-pci` | PCI ECAM host bridge | `PciBus`, `Bdf`, `PciEndpoint`, `PciConfigSpace` |
| `helm-hw-rtc` | Real-time clock (PL031 RTC) | `Pl031` |
| `helm-hw-timer` | Timer devices (SP804 dual timer) | `Sp804` |
| `helm-hw-virtio` | VirtIO MMIO transport + device backends | `VirtioBackend`, `proto`, `blk`, `console`, `net`, `rng` |

## debug/ — Instrumentation and Analysis (2 crates)

| Crate | Responsibility | Key Public Types |
|-------|---------------|-----------------|
| `helm-spy` | Collection layer: analysis primitives and models | `Counter`, `Histogram`, `HeatMap`, `InsnMix`, `CacheModel`, `BranchPredictor` |
| `helm-report` | Delivery layer: output sinks and formatters | `Report`, `FileSink`, `TcpSink`, `BinaryTraceSink`, CSV/JSON/Text formatters |

## Dependency Graph

```text
                    ┌──────────┐
                    │ helm-core│  (zero deps — traits only)
                    └────┬─────┘
                         │
          ┌──────────────┼──────────────────┐
          │              │                  │
    ┌─────▼─────┐  ┌─────▼──────┐    ┌─────▼──────┐
    │ helm-event│  │ helm-stats │    │ helm-probe │
    │ (no deps) │  │            │    │ (no deps)  │
    └─────┬─────┘  └─────┬──────┘    └────────────┘
          │              │
    ┌─────▼──────────────▼──┐
    │     helm-timing       │
    └─────────┬─────────────┘
              │
    ┌─────────▼─────────┐         ┌─────────────┐
    │   helm-devices    │◄────────│  helm-diag   │
    └─────────┬─────────┘         └──────────────┘
              │
    ┌─────────▼─────────┐
    │   helm-memory     │
    └─────────┬─────────┘
              │
    ┌─────────▼─────────┐    ┌────────────┐
    │    helm-arch      │    │ helm-decode│
    └─────────┬─────────┘    └────────────┘
              │
    ┌─────────▼─────────┐    ┌──────────────────┐
    │   helm-engine     │◄───│ hw/ device crates │
    └─────────┬─────────┘    └──────────────────┘
              │
    ┌─────────▼─────────┐
    │  helm-platform    │
    └─────────┬─────────┘
              │
    ┌─────────▼─────────┐    ┌────────────┐
    │   helm-python     │    │  helm-cli  │
    └───────────────────┘    └────────────┘
```

## Feature Flags

| Crate | Feature | Effect |
|-------|---------|--------|
| `helm-jit` | `backend-dynasm` (default) | Enable dynasm-rs x86-64 code generator |
| `helm-engine` | `jit-dynasm` | Wire JIT backend into simulation loop |
| `helm-diag` | `log-fallback` | Fall back to `log` crate when no monitor installed |
| `helm-plugin` | `builtins` (default) | Include built-in instrumentation plugins |
| `helm-probe` | `probe-full` | Enable richer probe event fields |
| `helm-hw-intc` | `probe` | Enable GIC probe points |

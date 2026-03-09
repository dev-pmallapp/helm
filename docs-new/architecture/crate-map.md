# Crate Map

HELM is a Cargo workspace with 19 Rust crates organised into five layers.
All crates depend on `helm-core` for shared types and error handling.

## Layer Diagram

```text
                          ┌─────────────┐
                          │  helm-cli   │  Binaries: helm, helm-arm,
                          └──────┬──────┘  helm-system-aarch64
                                 │
                    ┌────────────┼────────────┐
                    │            │            │
              ┌─────┴─────┐ ┌───┴───┐ ┌──────┴──────┐
              │helm-engine│ │helm-  │ │ helm-python  │
              │           │ │plugin │ │ (PyO3 cdylib)│
              └─────┬─────┘ └───┬───┘ └─────────────┘
                    │           │
    ┌───────┬───────┼───────┬───┴──────┬──────────┐
    │       │       │       │          │          │
┌───┴──┐┌───┴──┐┌───┴───┐┌─┴──────┐┌──┴───┐┌─────┴───┐
│helm- ││helm- ││helm-  ││helm-   ││helm- ││helm-    │
│tcg   ││isa   ││memory ││device  ││timing││syscall  │
└───┬──┘└───┬──┘└───┬───┘└───┬────┘└──┬───┘└────┬────┘
    │       │       │        │        │         │
    │  ┌────┴────┐  │   ┌────┴───┐    │         │
    │  │helm-    │  │   │helm-   │    │         │
    │  │decode   │  │   │object  │    │         │
    │  └────┬────┘  │   └────┬───┘    │         │
    │       │       │        │        │         │
    └───────┴───────┴────────┴────────┴─────────┘
                         │
                  ┌──────┴──────┐
                  │  helm-core  │   Foundation: types, IR, error, config
                  └─────────────┘

Additional crates (not shown above):
  helm-translate   Dynamic binary translation cache (SE mode)
  helm-pipeline    OoO pipeline model (CAE mode)
  helm-stats       Statistics collection (EventObserver)
  helm-kvm         KVM backend for near-native execution
  helm-systemc     SystemC/TLM-2.0 bridge
  helm-llvm        LLVM IR frontend for accelerator simulation
```

## Crate Descriptions

### Foundation

| Crate | Description |
|-------|-------------|
| `helm-core` | IR (`MicroOp`, `Opcode`), types (`Addr`, `RegId`, `Cycle`), `HelmError`, `PlatformConfig`, `SimEvent`, `IrqSignal` |
| `helm-object` | HELM Object Model (HOM): `HelmObject` trait, typed `Property`, `TypeRegistry`, `ObjectTree` |
| `helm-stats` | Atomic counters, `StatsCollector` (`EventObserver`), `SimResults` with IPC / branch MPKI |

### ISA & Translation

| Crate | Description |
|-------|-------------|
| `helm-isa` | `IsaFrontend` trait; full AArch64 executor (`Aarch64Cpu` + `step()`), RISC-V/x86 stubs |
| `helm-decode` | QEMU-compatible `.decode` file parser and Rust code generator (dual TCG + static backend) |
| `helm-tcg` | TCG IR (`TcgOp`), interpreter, Cranelift JIT compiler, threaded dispatch, A64 emitter |
| `helm-translate` | SE-mode translation cache: `Translator`, `TranslationCache`, `TranslatedBlock` |

### Memory & Timing

| Crate | Description |
|-------|-------------|
| `helm-memory` | `AddressSpace` (RAM + IoHandler), `Cache` (set-associative), `Tlb` (ASID-tagged), `Mmu` (ARMv8 walker), `CoherenceController` (MOESI stub) |
| `helm-timing` | `TimingModel` trait (FE/APE/CAE), `EventQueue`, `TemporalDecoupler`, `SamplingController` |
| `helm-pipeline` | OoO pipeline: `ReorderBuffer`, `RenameUnit`, `Scheduler`, `BranchPredictor` (Static / Bimodal / GShare / TAGE / Tournament) |

### Platform & Devices

| Crate | Description |
|-------|-------------|
| `helm-device` | `Device` trait, `DeviceBus` (hierarchical routing), `IrqRouter`, `DmaEngine`, `Platform` builder, FDT generation, VirtIO stack, ARM peripherals (GIC, PL011, SP804, BCM2837) |
| `helm-syscall` | Linux syscall emulation: `Aarch64SyscallHandler` (~50 syscalls), `FdTable`, FreeBSD stub |

### Orchestration

| Crate | Description |
|-------|-------------|
| `helm-engine` | `Simulation` driver, `SeSession` / `FsSession`, ELF64 + ARM64 Image loaders, SE runners, timing integration |
| `helm-plugin` | Unified plugin system: API traits, `PluginRegistry` (callbacks), built-in plugins (`insn-count`, `execlog`, `hotblocks`, `howvec`, `syscall-trace`, `fault-detect`, `cache-sim`) |

### Bindings & CLI

| Crate | Description |
|-------|-------------|
| `helm-python` | PyO3 cdylib (`_helm_core`): exposes `SeSession`, `FsSession` to Python |
| `helm-cli` | Three binaries: `helm` (generic), `helm-arm` (SE runner with plugins + embedded Python), `helm-system-aarch64` (FS runner) |

### Specialist

| Crate | Description |
|-------|-------------|
| `helm-kvm` | Linux KVM backend: `KvmVm`, `KvmVcpu`, `GuestMemory`, in-kernel GIC setup |
| `helm-systemc` | SystemC/TLM-2.0 bridge: `StubBridge`, `Clock`, `TlmPayload` |
| `helm-llvm` | LLVM IR parser for hardware accelerator co-simulation (inspired by gem5-SALAM) |

## Ownership Rules

- `helm-core` owns all shared types; other crates import but never modify them.
- `helm-isa` owns the CPU state (`Aarch64Regs`, `Aarch64Cpu`).
- `helm-device` owns all device models and the platform builder.
- `helm-engine` orchestrates everything and is the only crate that links all subsystems.
- `helm-python` depends on `helm-core`, `helm-engine`, and `helm-plugin` only.

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project State

Active implementation. AArch64 SE+FS pipeline working. RISC-V RV64I+Zicsr decode/execute implemented. **Current focus:** Complete RISC-V SE (`LinuxRiscv64SyscallHandler`) and ship `helm-riscv64` binary. See `docs/plans/riscv64-se-emulation.md`.

Read `AGENT.md` (400 lines) for the authoritative agent onboarding guide before working on this project.

## Key Documentation

- `AGENT.md` — Agent onboarding: crate map, design rules, execution modes, object model
- `docs/ARCHITECTURE.md` — Full system architecture and type hierarchy
- `docs/design/HLD.md` — Canonical top-level design doc and crate DAG
- `docs/object-model.md` — SimObject lifecycle, wiring rules, checkpoint protocol
- `docs/traits.md` — All trait definitions (ExecContext, SimObject, Device, TimingModel, etc.)
- `docs/api.md` — Rust and Python API reference
- `docs/testing.md` — Testing strategy
- `docs/TODO.md` — Open work items
- `docs/plans/riscv64-se-emulation.md` — Active RISC-V SE plan
- `docs/design/<crate>/` — Per-crate HLD + LLD-*.md + TEST.md

## Build Commands

```bash
cargo build --workspace
cargo test --workspace
cargo test --package helm-arch          # ISA tests only
cargo test --lib --workspace            # Unit tests only
cargo clippy --all --all-targets -- -D warnings
cargo fmt --check
cargo doc --no-deps --open
```

## Workspace Layout (domain-based)

**`framework/`** — stable APIs and shared primitives

| Crate | Key Types | Notes |
|---|---|---|
| `helm-core` | `ArchState`, `ExecContext`, `ThreadContext`, `MemInterface` | Zero helm-* deps |
| `helm-memory` | `MemoryRegion`, `MemoryMap`, `FlatView`, `HelmAddressSpace` | QEMU-inspired MemoryRegion tree |
| `helm-timing` | `VirtualTiming`, `IntervalTiming`, `AccurateTiming`, `TimingModel` | Three timing models |
| `helm-event` | `EventQueue`, `EventClass`, `PendingEvent` | BinaryHeap, discrete-event |
| `helm-devices` | `Device` trait, `InterruptPin`, `HelmEventBus`, `DeviceRegistry` | SDK only; bus controllers here |
| `helm-stats` | `PerfCounter`, `PerfHistogram`, `StatsRegistry` | Dot-path namespaced |
| `helm-plugin` | `HelmPluginRegistry`, `PluginDescriptor` | Engine extension system |
| `helm-decode` | `DecodeTree`, `Pattern`, `Field` | QEMU-style .decode parser + codegen |
| `helm-jit` | `JitBackend` trait, cache, dynasm/stencil backends | `jit-tiered` feature: stencil baseline + dynasm hot-tier |

**`runtime/`** — execution engine and frontends

| Crate | Key Types | Notes |
|---|---|---|
| `helm-arch` | `RiscvArchState`, `Aarch64ArchState`, `Instruction` | ISA decode+execute only |
| `helm-engine` | `HelmEngine<T>`, `HelmSim`, `ExecMode`, `FlatMem` | `se/` = syscall handlers; `fs.rs` = FS-mode step loop |
| `helm-platform` | `ArmVirtPlatform` | ARM virt machine builder, loads kernel/DTB/initrd |
| `helm-debug` | `GdbServer`, `TraceLogger`, `CheckpointManager` | GDB RSP stub |
| `helm-python` | PyO3 → `_helm_ng` module | `python/helm/` package; SimObject, System, CPU, RAM |
| `helm-cli` | `helm-aarch64` binary | CLI launcher with embedded Python |

**`hw/`** — concrete hardware implementations

| Crate | Devices |
|---|---|
| `helm-hw-char` | PL011 UART |
| `helm-hw-timer` | SP804 dual timer |
| `helm-hw-rtc` | PL031 RTC |
| `helm-hw-dma` | DMA engine |
| `helm-hw-intc` | GICv2 (distributor + CPU interface) |
| `helm-hw-pci` | PCI ECAM host bridge |
| `helm-hw-virtio` | VirtIO MMIO transport |

**`debug/`** — analysis and delivery

| Crate | Purpose |
|---|---|
| `helm-spy` | Analysis models, `HelmSpy` |
| `helm-report` | Output sinks (JSON, CSV) |

## Critical Design Rules (inviolable)

1. **Monomorphize timing only** — `HelmEngine<T: TimingModel>` is the sole generic parameter; ISA/mode dispatch via enum
2. **ISA/mode are enum-dispatched** — one `match` per Python call, zero per instruction
3. **No dark state** — every persistent field must be a registered `AttrDescriptor`; unregistered = lost on restore
4. **Device knows no base address** — `MemoryMap` owns placement; device sees only `offset`
5. **Device knows no IRQ number** — `InterruptPin::assert()` fires the signal; platform owns routing
6. **No dynamic lookup in the hot loop** — store all cross-component `Arc` refs during `elaborate()`
7. **Python describes; Rust simulates** — config frozen after `build_simulator()`; no mutation during sim
8. **Determinism by default** — no wall-clock, no background threads in the hot loop
9. **`HelmEventBus` is synchronous** — not checkpointed; subscribers re-register in `init()` on every load
10. **`init()` is self-contained** — no cross-component access; that happens in `elaborate(system)`

## SimObject Lifecycle

```
CONSTRUCT → init() → elaborate(system) → startup() → RUN → reset() / checkpoint_save/restore
```

- `init()`: internal state only, no cross-component
- `elaborate(system)`: register MMIO, store `Arc` refs, wire interrupts
- `startup()`: schedule initial events, assert initial signals
- `reset()`: return to post-startup state, idempotent
- `checkpoint_save/restore`: architectural state only — no perf counters

## Irreducible Core

Every execution path reduces to:
1. **ArchState** — register file + PC
2. **Decoder** — bytes → Instruction
3. **Executor** — (ArchState, Insn, MemInterface) → ΔArchState
4. **MemInterface** — read/write(addr, size) ↔ bytes

## Two Distinct Event Systems

- **EventQueue** (`helm-event`): schedule callbacks at future tick T — **asynchronous/deferred**
- **HelmEventBus** (`helm-devices/bus`): observable named events — **synchronous/inline, not checkpointed**

## `HelmSim` — PyO3 Boundary

```rust
pub enum HelmSim {
    VirtualTiming(HelmEngine<VirtualTiming>),    // >100 MIPS
    IntervalTiming(HelmEngine<IntervalTiming>),  // Sniper-style, <15% MAPE, >10 MIPS
    AccurateTiming(HelmEngine<AccurateTiming>),  // cycle-accurate, <10% IPC err, >200 KIPS
    Hardware(HardwareEngine),                    // KVM/HVMX — real hardware timing
}
```

`HelmSim` is the sole object exposed to Python. All Python calls enter here; ISA and mode dispatched once per call, not per instruction.

## Primary Device Modeling Primitive

`register_bank!` macro — replaces manual MMIO switch statements:

```rust
register_bank! {
    UartRegs for Uart16550 at offset 0x0 {
        reg RHR @ 0x00 is read_only  { field DATA [7:0] }
        reg THR @ 0x00 is write_only { field DATA [7:0] }
        reg LSR @ 0x14 is read_only  { field THRE [5]; field DR [0] }
    }
}
// Generates: MmioHandler impl, serde checkpoint, AttrDescriptors, Python introspection
```

## Quick Reference

| Need to... | Look at |
|---|---|
| Add a new ISA | `helm-arch/src/{new_isa}/` + implement `Hart` trait from `helm-core` |
| Add a new device | Implement `Device` trait from `helm-devices`, use `register_bank!` |
| Add a new timing model | Implement `TimingModel` from `helm-timing`, add variant to `HelmSim` |
| Add a RISC-V syscall | `helm-engine/src/se/linux_riscv64.rs` |
| Add an AArch64 syscall | `helm-engine/src/se/linux_aarch64.rs` |
| Add a GDB packet | `helm-debug/src/gdb_server.rs` |
| Change Python API | `helm-python/src/` + `python/helm/` |
| Debug a checkpoint | All persistent state must be in `AttrStore` with `AttrKind::Required` |

## Naming Reference (use these exact names)

| Correct Name | Location |
|---|---|
| `HelmEngine<T>` | helm-engine |
| `HelmSim` | helm-engine |
| `VirtualTiming` / `IntervalTiming` / `AccurateTiming` | helm-timing |
| `ExecMode::Functional` / `::Syscall` / `::System` / `::Hardware` | helm-engine |
| `HardwareEngine` | helm-engine/src/kvm/ |
| `HelmAddressSpace` | helm-memory/src/address_space.rs |
| `HelmSpy` | helm-spy |
| `DiagContext` | helm-diag |
| `DeviceContext` | helm-devices |
| `HelmPluginRegistry` | helm-plugin |
| `TimingInsnInfo` | helm-timing |
| `HelmSystem` (Rust) / `"System"` (Python) | helm-python |

## Phased Build Plan

| Phase | Deliverables | Status |
|---|---|---|
| **0 — MVP** | RISC-V SE simulator, riscv-tests pass | In progress — RV64I+Zicsr done |
| **1** | AArch64 SE+FS, GDB stub, ARM virt platform, timing | Largely done |
| **2** | RISC-V SE completion, `helm-riscv64` binary, riscv-tests gate | **Current** |
| **3** | Boot Linux RISC-V, OoO pipeline, AArch32, JIT | Future |

## Testing Strategy

- **ISA correctness**: official riscv-tests vectors + AArch64 torture tests
- **Differential testing**: QEMU/Spike traces vs. helm-ng execution
- **Property-based**: `proptest` for memory layouts and instruction sequences
- **Benchmarks**: `criterion` for IPC accuracy regressions

See `docs/testing.md` for the full strategy and planned test locations per crate.

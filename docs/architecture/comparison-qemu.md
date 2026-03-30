# Comparison with QEMU

Architectural parallels and differences between helm-ng and QEMU.

## Overview

QEMU is the industry-standard fast emulator. helm-ng borrows several
QEMU concepts (MemoryRegion tree, `.decode` file format, SysBus-style
platform construction) but diverges on language, timing support, and
device abstraction.

## Binary Translation / Execution

| Aspect | QEMU | helm-ng |
|--------|------|---------|
| Default path | Always JIT (TCG) | Interpreter (JIT optional) |
| IR | TCG ops (C structs) | Direct enum dispatch |
| Frontend | Hand-written C translators | Hand-written Rust + `.decode` code gen |
| Backend | Custom register allocator + emitter per host | dynasm-rs (x86-64) via `JitBackend` trait |
| Block cache | Hash table (`tb_jmp_cache`) | 4096-entry direct-mapped `JitCache` |
| Guest register mapping | TCG globals → host registers | Explicit load/store from `[u64; 48]` array |
| Interpreter | None | Match-based dispatch (always available) |

helm-ng reuses QEMU's `.decode` file format via `helm-decode`, so
upstream ARM decode specifications can be imported.

## Memory Model

| Aspect | QEMU | helm-ng |
|--------|------|---------|
| Guest memory | `MemoryRegion` tree + `FlatView` | `MemoryRegion` enum tree + `FlatView` + `FlatMem` |
| RAM backend | `RAMBlock` (mmap) | Page table `Vec<*mut u8>` (mmap) |
| softmmu TLB | Inline in generated code (`cputlb.c`) | `Tlb` struct, checked per instruction |
| Page walker | Inline in `cputlb.c` | Separate `mmu::walk()` pure function |
| MMIO dispatch | `MemoryRegionOps` callbacks | `AddressMap` → `Device::transact()` |
| Cache model | None | Future (`helm-memory` infrastructure exists) |

Both use a tree of regions flattened into a sorted, non-overlapping
view. helm-ng's `FlatMem` uses a flat page table for O(1) host-pointer
lookup instead of QEMU's `RAMBlock` + `ram_addr_t` indexing.

## Device Model

| Aspect | QEMU | helm-ng |
|--------|------|---------|
| Object system | QOM (`TypeInfo`, properties, class hierarchy) | `Device` trait + `DeviceRegistry` |
| MMIO registration | `memory_region_init_io()` + `sysbus_mmio_map()` | `AddressMap::register()` via platform |
| IRQ model | `qemu_irq` + GPIO lines | `InterruptPin` + `InterruptSink` |
| Address ownership | Device often knows its SysBus address | Device is address-oblivious |
| IRQ ownership | Device often knows its IRQ number | Device is IRQ-oblivious |
| Bus types | SysBus, PCI, I2C, SPI | `MmioBus`, PCI, I2C, SPI, AMBA |
| Dynamic loading | Built-in QOM (all devices compiled in) | DLD `.so` loading via `DeviceRegistry` |
| Chardev | `Chardev` backend hierarchy | `CharBackend` trait |

The key design difference: in QEMU, devices frequently call
`sysbus_mmio_map()` and `sysbus_init_irq()` to wire themselves. In
helm-ng, the platform handles all wiring — devices receive an
`InterruptPin` and register MMIO ranges without knowing the system-wide
address or IRQ number.

## Platform / Machine Type

| Aspect | QEMU | helm-ng |
|--------|------|---------|
| Machine class | `MachineClass` + `machine_init()` | `Platform` trait + `PlatformBuildPlan` |
| Config input | CLI `-M`, `-device`, `-drive` | Python scripts + CLI |
| DTB | Generated in C | Loaded from file |
| Device creation | Inline in `machine_init()` | Driven by `PlatformBuildPlan` |

## Timing

| Aspect | QEMU | helm-ng |
|--------|------|---------|
| Timing models | None — functional only | VirtualTiming, IntervalTiming, AccurateTiming |
| IPC | Always 1 | Configurable per model |
| Hot-loop overhead | Zero (no timing) | Zero (monomorphized) |

This is the fundamental difference: QEMU is a pure functional emulator
with no timing model. helm-ng was designed from the start to support
multiple timing fidelities in the same binary.

## Event System

| Aspect | QEMU | helm-ng |
|--------|------|---------|
| Scheduled events | `timer_mod()` / `timer_new()` | `EventQueue::post_at()` |
| Observable events | Ad-hoc callbacks | `HelmEventBus::emit()` |
| Separation | Partial (timer vs callback) | Clean (EventQueue vs HelmEventBus) |

## Plugin API

| Aspect | QEMU | helm-ng |
|--------|------|---------|
| API | TCG plugin C API (`qemu_plugin.h`) | Rust `HelmPlugin` trait |
| Hooks | `tb_trans`, `insn_exec`, `mem` | `on_insn`, `on_mem`, `on_syscall` |
| Loading | `.so` dynamic loading | Trait objects + optional DLD `.so` |
| Zero-cost probes | No | Yes (`helm-probe::Probe<T>`) |

## Checkpoint / Migration

| Aspect | QEMU | helm-ng |
|--------|------|---------|
| Framework | VMState / Migration | `CheckpointManager` + `AttrRegistry` |
| Scope | All QOM properties | All registered attributes |
| Dark state risk | Common (missed VMState fields) | Forbidden (all fields must be registered) |

# Comparison with QEMU

Architectural parallels and differences between HELM and QEMU.

## Binary Translation

| Aspect | QEMU | HELM |
|--------|------|------|
| IR | TCG ops (C structs) | `TcgOp` enum (Rust) |
| Frontend | Hand-written C translators | Auto-generated from `.decode` files |
| Backend | Custom register allocator + code emitter per host | Cranelift (production JIT) |
| Interpreter | None (always JIT) | Match-based + threaded dispatch |
| Block cache | Hash table | Direct-mapped array |

HELM reuses QEMU's `.decode` file format via `helm-decode`, so upstream
ARM decode specifications can be imported directly.

## Memory Model

| Aspect | QEMU | HELM |
|--------|------|------|
| Guest memory | `MemoryRegion` tree + `FlatView` | `AddressSpace` + `MemRegion` list |
| softmmu | Inline TLB in generated code | `Tlb` struct checked at block boundaries |
| Page walk | Inline in `cputlb.c` | Separate `mmu::walk()` function |
| Cache model | None | `helm-memory::Cache` (set-associative) |

## Device Model

| Aspect | QEMU | HELM |
|--------|------|------|
| Object system | QOM (`TypeInfo`, properties, class hierarchy) | `HelmObject` trait + `TypeRegistry` |
| MMIO dispatch | `MemoryRegionOps` callbacks | `Device::transact()` via `DeviceBus` |
| IRQ | `qemu_irq` + GPIO lines | `IrqLine` + `IrqRouter` |
| Bus | SysBus / PCI | `DeviceBus` (hierarchical, nestable) |
| DMA | `dma_memory_read/write` | `DmaEngine` with scatter-gather |
| Chardev | `Chardev` backend | `CharBackend` trait |

## Platform / Machine Type

| Aspect | QEMU | HELM |
|--------|------|------|
| Machine class | `MachineClass` + `machine_init` | `Platform` struct + builder functions |
| Config | CLI `-M`, `-device`, `-drive` | CLI + Python scripts + `PlatformConfig` |
| DTB | Generated in C | `FdtBuilder` with overlay support |

## Plugin API

| Aspect | QEMU | HELM |
|--------|------|------|
| API | TCG plugin C API (qemu_plugin.h) | Rust `HelmPlugin` trait |
| Hooks | `tb_trans`, `insn_exec`, `mem` | Same plus `syscall`, `fault`, `vcpu` |
| Loading | `.so` dynamic loading | Rust trait objects + optional `.so` |
| State | Per-plugin opaque pointer | `Scoreboard<T>` + plugin struct fields |

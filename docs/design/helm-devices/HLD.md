# helm-devices — High-Level Design

> Crate-level design document for `helm-devices`.
> Cross-references: [`ARCHITECTURE.md`](../../ARCHITECTURE.md) · [`object-model.md`](../../object-model.md) · [`traits.md`](../../traits.md) · [`LLD-device-trait.md`](./LLD-device-trait.md) · [`LLD-interrupt-model.md`](./LLD-interrupt-model.md) · [`LLD-register-bank-macro.md`](./LLD-register-bank-macro.md) · [`LLD-device-registry.md`](./LLD-device-registry.md)

---

## Table of Contents

1. [Crate Purpose](#1-crate-purpose)
2. [What This Crate Contains](#2-what-this-crate-contains)
3. [What This Crate Does Not Contain](#3-what-this-crate-does-not-contain)
4. [Module Structure](#4-module-structure)
5. [Dependency Graph](#5-dependency-graph)
6. [Relationship to World and Full System](#6-relationship-to-deviceworld-and-full-system)
7. [Key Design Decisions](#7-key-design-decisions)
8. [Answered Design Questions](#8-answered-design-questions)

---

## 1. Crate Purpose

`helm-devices` is the **device modeling infrastructure** crate. It provides the primitive types, traits, macros, and registry machinery that any device implementation needs — without containing any device implementations itself.

The distinction is deliberate: `helm-devices` is the standard library for device authors. Concrete device models (UART 16550, PLIC, CLINT, VirtIO) live in separate crates or in `examples/`. This separation allows:

- **No bloat in the default build.** A project using only `helm-devices` for its infrastructure does not compile any concrete device code.
- **Stable API contract.** The device trait interface is versioned independently of any specific device model.
- **Plugin loading without circular dependencies.** Plugins link against `helm-devices` for the trait definitions; the host links `helm-devices` for the registry. Neither side needs to know about concrete implementations.

The crate's purpose can be stated as: define everything a device author needs to write a device, and everything a platform author needs to wire devices together — no more.

---

## 2. What This Crate Contains

### Core Primitives

| Item | Module | Purpose |
|------|--------|---------|
| `Device` trait | `device` | The fundamental device interface: MMIO read/write at offsets, signals, region size |
| `DeviceConfig` / `DeviceError` | `device` | Infallible builder → fallible realize pattern |
| `InterruptPin` | `interrupt` | A device's interrupt output pin — no IRQ number, no routing knowledge |
| `InterruptWire` | `interrupt` | Internal type connecting a pin to a sink |
| `InterruptSink` trait | `interrupt` | Implemented by interrupt controllers (PLIC, GIC, PIC) |
| `WireId` | `interrupt` | Opaque wire identifier passed to sink callbacks |
| `SignalInterface` | `signal` | Canonical protocol for named signal assertion/deassertion |
| `Connect<T>` / `Port<T>` | `port` | Typed port wiring (SIMICS-style connect/port) resolved at elaborate time |
| `register_bank!` macro | `register_bank` (proc-macro crate) | Declarative register bank definition |
| `DeviceDescriptor` | `registry` | Runtime device type record: name, version, factory fn, Python class string |
| `DeviceRegistry` | `registry` | HashMap of descriptors; .so plugin loader |
| `ParamSchema` / `ParamField` / `ParamType` | `params` | Typed device configuration schema |
| `DeviceParams` | `params` | Runtime parameter map — typed accessors |
| `PluginError` | `registry` | Error type for plugin loading and device creation |

### Bus Infrastructure

| Module | Content |
|--------|---------|
| `bus/pci/` | PCI/PCIe config space layout, BAR types, MSI/MSI-X descriptor types |
| `bus/amba/` | AMBA/AHB/APB bus transaction types and address decode helpers |

Bus modules define infrastructure types — bus protocols, address decode helpers, config space layouts. They do not contain bus controller implementations.

---

## 3. What This Crate Does Not Contain

**No concrete device models.** UART, PLIC, CLINT, VirtIO disk, VirtIO network, GIC — none of these live here. They belong in:

- `examples/plugin-uart/` — UART 16550 as a standalone `.so` plugin example
- `examples/plugin-plic/` — PLIC as a standalone plugin example
- A future `helm-devices-riscv-virt/` crate for the canonical RISC-V virt platform devices

**No address knowledge.** `helm-devices` types never hold a base address. The `MemoryMap` in `helm-memory` owns address placement. A device sees byte offsets within its mapped region, not absolute addresses. This is enforced structurally: no field in any `helm-devices` type holds an address.

**No IRQ number knowledge.** Devices have no concept of IRQ numbers, interrupt controller inputs, or routing. A device holds an `InterruptPin` and calls `pin.assert()`. Where that signal goes is configured by the platform at elaborate time via `World::wire_interrupt()`. IRQ numbers are a platform integration concern, not a device concern.

**No `helm-memory`, `helm-engine`, `helm-arch`, or `helm-event` dependencies.** The crate depends only on `helm-core`. This is a hard constraint:

- A device model must be compilable without pulling in the full simulation engine.
- Plugin `.so` files link against `helm-devices` ABI only; they must not transitively link `helm-engine` or `helm-memory`.
- `World` (in `helm-engine/`) links `helm-devices` + `helm-memory` + `helm-event` from above; `helm-devices` itself does not.

---

## 4. Module Structure

```
helm-devices/
├── Cargo.toml
└── src/
    ├── lib.rs              # Re-exports all public items; DeviceRegistry top-level methods
    ├── device.rs           # Device trait, DeviceConfig, DeviceError
    ├── interrupt.rs        # InterruptPin, InterruptWire, InterruptSink, WireId
    ├── signal.rs           # SignalInterface, named signal constants
    ├── port.rs             # Connect<T>, Port<T>, typed port wiring
    ├── params.rs           # ParamSchema, ParamField, ParamType, ParamValue, DeviceParams
    ├── registry.rs         # DeviceRegistry, DeviceDescriptor, PluginError, .so loader
    └── bus/
        ├── mod.rs
        ├── pci/
        │   ├── mod.rs
        │   ├── config_space.rs   # PCI config header layout, BAR descriptors
        │   └── msi.rs            # MSI/MSI-X capability structures
        └── amba/
            ├── mod.rs
            └── decode.rs         # AMBA address decode helpers, AHB/APB transaction types
```

The `register_bank!` proc-macro lives in a companion crate `helm-devices-macros` (a `proc-macro = true` crate in the workspace). `helm-devices/Cargo.toml` re-exports it via a dependency, so users see `helm_devices::register_bank!` with no separate import.

```toml
# helm-devices/Cargo.toml
[dependencies]
helm-core       = { path = "../helm-core" }
helm-devices-macros = { path = "../helm-devices-macros" }
inventory       = "0.3"
libloading      = "0.8"
serde           = { version = "1", features = ["derive"] }
log             = "0.4"
```

---

## 5. Dependency Graph

```
helm-devices
    ├── helm-core               (ArchState, MemFault, common types — no ISA, no engine)
    ├── helm-devices-macros     (register_bank! proc-macro companion crate)
    ├── inventory               (self-registration for built-in device types)
    ├── libloading              (.so plugin loading)
    ├── serde                   (register_bank! generates serde impls)
    └── log                     (warn!() on unconnected InterruptPin::assert())

helm-engine               (uses helm-devices, adds helm-memory + helm-event)
    ├── helm-devices            ← this crate
    ├── helm-memory             (MemoryMap, MemoryRegion, MmioHandler)
    └── helm-event              (EventQueue for device timer callbacks)

helm-python                         (PyO3 bindings)
    └── helm-devices            ← this crate (for DeviceRegistry, Python class injection)
```

The crate dependency order enforces the constraint: `helm-devices` depends on nothing above `helm-core`. No circular dependencies are possible by construction.

---

## 6. Relationship to World and Full System

The same `Device` trait implementation runs unchanged in three contexts:

```
Context 1: World (headless testing / fuzzing)
    World owns: MemoryMap + EventQueue + HelmEventBus + VirtualClock
    A Device is driven by: World::mmio_read() / mmio_write() / advance()
    No CPU. No ISA. No ArchState.

Context 2: Full System (HelmEngine<T>)
    System owns: MemoryMap + EventQueue + HelmEventBus + TimingModel
    A Device is driven by: CPU MMIO accesses routed through MemoryMap
    Full simulation. CPU, ISA, timing model all present.

Context 3: Plugin Test (.so + test harness)
    World instantiates a device from the plugin registry.
    Same MMIO path. No host system or CPU model required.
```

**A device that passes all World tests is guaranteed to work identically in a full system.** The `Device` trait implementation is world-agnostic: it receives offsets, sizes, and values — never absolute addresses or context pointers.

The `SimObject` lifecycle (`init → elaborate → startup → reset → checkpoint_save / checkpoint_restore`) is an optional extension. Devices that need lifecycle management implement both `Device` and `SimObject`. Devices used only in headless testing scenarios may implement `Device` alone — `World` does not require `SimObject`. This orthogonality is design question Q60's resolution.

---

## 7. Key Design Decisions

### Devices Have No Address or IRQ Knowledge (Q60, Q61, Q62)

A `Device` receives byte offsets within its mapped region. The `MemoryMap` in `helm-memory` owns address placement. The `InterruptPin` owned by the device has no knowledge of which interrupt controller input it is connected to. Both address and IRQ routing are platform/SoC integration concerns expressed in Python configuration.

This mirrors real hardware: a UART IP block has an `irq` output pin. The SoC designer connects it to interrupt controller input N in the netlist. The UART RTL has no `#define IRQ_NUM N`.

### Device: SimObject Is Orthogonal (Q60)

`Device` and `SimObject` are separate traits with no inheritance relationship. A device may implement:

- `Device` only — for headless `World` usage
- `Device` + `SimObject` — for full system participation with lifecycle and checkpointing

`World` requires only `Device`. `System` (full simulation) requires both. Plugin devices choose based on their intended use.

### region_size() Is Fixed at Construction for Phase 0 (Q61)

`Device::region_size() -> u64` returns a value set at construction time and does not change. This simplifies `MemoryMap` — it never needs to re-flatten the `FlatView` because of a BAR resize. PCIe BAR dynamic resizing is a Phase 3+ concern and will require a `MemoryMap::resize_region()` notification path when implemented.

### InterruptPin Connections Set at finalize() via World::wire_interrupt() (Q62)

`InterruptPin` fields are `None` at construction and `Some(wire)` after `World::wire_interrupt()` is called during the `elaborate()` phase. After `startup()`, the wiring graph is frozen. A device never sets its own `InterruptPin` connection — the platform configuration does.

### register_bank! Is a Proc-Macro (Q63–Q66)

The `register_bank!` macro is a procedural macro that generates: an `MmioHandler` implementation with a dispatch table keyed by offset, `serde` checkpoint serialization, and `AttrDescriptor` Python introspection data. Side-effect methods (`on_write_<reg>`, `on_read_<reg>`) are hooks the device author provides; the macro calls them at the correct point in the dispatch.

### Python Class Name Conflicts Are Errors at Load Time (Q67)

When a plugin is loaded, its embedded `PYTHON_CLASS` string is `exec()`'d into the `helm_ng` module namespace. If a name already exists in that namespace from a previously loaded plugin, the loader raises `PluginError::PythonNameConflict` and the load fails. The plugin is not partially registered.

### InterruptPin Is Not Clone (Q70)

`InterruptPin` does not implement `Clone`. A device has one interrupt output pin and one wire. One-to-one wiring is enforced by the type system. Fan-out (one device IRQ to multiple sinks) requires an intermediate fan-out device or a platform-level interrupt combiner.

### InterruptPin::assert() When Not Connected Logs a Warning (Q71)

If `assert()` is called on an unconnected pin (`wire` is `None`), the call is a no-op and a `log::warn!()` message is emitted. The simulator does not panic. This allows devices to be tested in minimal harnesses without wiring every interrupt before testing unrelated functionality.

### Device::read() Takes &mut self (Q1.1)

`fn read(&mut self, offset: u64, size: usize) -> u64` — mutable receiver removes the need for interior mutability for clear-on-read registers and FIFO drain operations. Safe because simulation is single-threaded (design rule 8). See [`LLD-device-trait.md`](./LLD-device-trait.md) §2.

### TimerScheduler Trait in helm-core (Q1.4)

Devices that need to schedule timer callbacks depend on `TimerScheduler` from `helm-core` (not on `EventQueue` from `helm-event`). This preserves the `helm-devices → helm-core only` dependency constraint. The engine provides a concrete impl at `elaborate()` via `System`.

```rust
// helm-core/src/timer.rs
pub trait TimerScheduler: Send + Sync {
    fn schedule_after(&self, delay_ns: u64, callback: Box<dyn FnOnce() + Send>);
    fn cancel(&self, handle: TimerHandle) -> bool;
}
```

### Dependency Graph Update for Q1.4

```
helm-devices
    ├── helm-core              (ArchState, MemFault, TimerScheduler ← NEW)
    ├── helm-devices-macros
    ├── inventory
    ├── libloading
    ├── serde
    └── log
```

`EventQueue` (the concrete impl of `TimerScheduler`) stays in `helm-event`. The `TimerScheduler` trait interface lives in `helm-core` so `helm-devices` can depend on it without pulling in `helm-event`.

### SysRegMap for ARM System Register Dispatch (Q2.2, Q2.7)

ARM system registers (MSR/MRS) are dispatched via `SysRegMap` in `helm-core`. Injected into `ArchState` at `elaborate()`:

```rust
pub enum SysRegEntry {
    Inline { read_offset: usize, write_offset: Option<usize> },
    Handler(Box<dyn SysRegHandler>),
}
```

MPIDR_EL1 → `Inline`. ICC_IAR1_EL1, CNTPCT_EL0 → `Handler`.

### GIC Uses Arc<UnsafeCell<GicState>> (Q2.1)

GIC split into three `Device` impls (`GicDistributor`, `GicRedistributor`, `GicIts`) sharing state through `Arc<UnsafeCell<GicState>>`. Safe because the hot loop is single-threaded (design rule 8).

### PowerController Trait in helm-core (Q2.5)

PSCI device calls `PowerController`, a trait in `helm-core`:

```rust
pub trait PowerController: Send {
    fn cpu_on(&self, mpidr: u64, entry: u64, context_id: u64) -> PsciError;
    fn cpu_off(&self) -> !;
    fn system_reset(&self) -> !;
}
```

Engine implements `PowerController`; PSCI device receives `Arc<dyn PowerController>` at `elaborate()`.

### DmaPort Trait for DMA Devices (Q4.5)

DMA-capable devices receive `Arc<dyn DmaPort>` at `elaborate()`. Writes flow through `MemoryMap` for SMMU observability:

```rust
pub trait DmaPort: Send + Sync {
    fn dma_read(&self, addr: u64, buf: &mut [u8]) -> Result<(), DmaError>;
    fn dma_write(&self, addr: u64, buf: &[u8]) -> Result<(), DmaError>;
}
```

### RemapCommand Queue for BAR Reprogramming (Q4.6)

PCI BAR writes push `RemapCommand` onto `PciBus`'s internal queue. Caller drains after `Device::write()` returns. `FlatView` recomputed lazily on next miss. Avoids re-entrance into `MemoryMap` during active write dispatch.

### World::affinity_map() for GIC CPU Affinity (Q2.10)

`World::affinity_map() -> &AffinityMap` configured by Python before `build_simulator()`. Maps `mpidr → cpu_index` for GIC routing.

---

## 8. Answered Design Questions

| Q# | Question | Answer |
|----|----------|--------|
| Q60 | Device: SimObject orthogonal? | Yes — `Device` and `SimObject` are separate traits. `World` needs `Device` without `SimObject` lifecycle. |
| Q61 | region_size() fixed at construction? | Yes — fixed for Phase 0. PCIe BAR resize deferred to Phase 3+. |
| Q62 | InterruptPin connections set how? | At `elaborate()` time via `World::wire_interrupt()`. Frozen after `startup()`. |
| Q63 | register_bank! on_write/on_read hook API? | Method hooks named `on_write_<regname>` and `on_read_<regname>` on the device struct. |
| Q64 | register_bank! generates serde derive? | Yes — generated automatically. Device author does not write serde impls. |
| Q65 | Split-function registers (THR/RHR)? | `is write_only` / `is read_only` qualifiers in macro syntax. |
| Q66 | register_bank! generates Python introspection? | Yes — `AttrDescriptor` array for register names, offsets, field names. |
| Q67 | Python class name conflict at plugin load? | `PluginError::PythonNameConflict` — error, load fails. |
| Q68 | Plugin versioning against ABI mismatch? | `HELM_DEVICES_ABI_VERSION` symbol in every plugin; checked before calling `helm_device_register`. |
| Q69 | Multiple devices per .so? | Yes — multiple `r.register()` calls in one `helm_device_register` invocation. |
| Q70 | InterruptPin clone-able? | No — one-to-one, not `Clone`. |
| Q71 | InterruptPin::assert() when not connected? | `log::warn!()` + no-op. No panic. |
| Q1.1 | Device::read() signature? | `&mut self` — no interior mutability; single-threaded hot loop makes it safe. |
| Q1.4 | Device timer scheduling? | `TimerScheduler` trait in `helm-core`; engine injects impl at `elaborate()`. |
| Q1.5 | When to bypass register_bank!? | Multi-bank (GIC, SMMU), index-addressed, dynamic count, or deep interdependency. |
| Q1.6 | ParamSchema validation phases? | Assignment-time (type/presence) + realize-time (semantic cross-field) — two separate phases. |
| Q1.7 | Register width in register_bank!? | `width 32` (default) or `width 64` qualifier; hook signatures use matching type. |
| Q1.8 | Checkpoint serialization? | `bincode` + `CKPT_VERSION: u32` (Phase 0); schema-hash auto-detection (Phase 2+). |
| Q1.9 | Checkpoint granularity? | `MemoryMap`-level `checkpoint_save`/`restore` (Phase 0–2); per-device delta (Phase 3+). |
| Q1.10 | W1C hook contract? | Hook sees post-W1C `new`; raw write value available only via separate accessor. |
| Q2.1 | GIC internal state sharing? | `Arc<UnsafeCell<GicState>>` shared by 3 Device impls; safe in single-threaded hot loop. |
| Q2.2 | ARM system register dispatch? | `SysRegMap` with `Inline`/`Handler` entries in `helm-core`; injected at `elaborate()`. |
| Q2.5 | PSCI power control interface? | `PowerController` trait in `helm-core`; engine implements it; PSCI device receives `Arc<dyn>`. |
| Q2.8 | EventQueue drain points? | Instruction boundaries only — drains after each completed instruction, not mid-decode. |
| Q2.9 | Watchdog / bus error notification? | `HelmEventBus::DeviceAction::Signal` — synchronous, not return-value. |
| Q2.10 | GIC CPU affinity configuration? | `World::affinity_map()` API; Python configures before `build_simulator()`. |
| Q3.2 | Plugin C ABI? | Pure `extern "C"` via cbindgen; `HELM_DEVICES_ABI_VERSION: u32` confirmed correct. |
| Q3.5 | Python class authority? | `ParamSchema` authoritative; Python class auto-generated — no hand-written string. |
| Q3.6 | Plugin checkpoint migration? | `serde` + version tag + `helm_{name}_migrate_checkpoint` C export in plugin. |
| Q3.7 | Device type aliases? | `aliases: &'static [&'static str]` on `DeviceDescriptor`; all resolve to same descriptor. |
| Q3.8 | Device capability requirements? | `required_capabilities: &'static [HostCapability]` + `check_requirements()` at load time. |
| Q4.1 | PCI config space decode? | `PciBus` internal ECAM decode — existing design confirmed correct. |
| Q4.2 | MSI-X address-space path? | Full `MemoryMap` path (Phase 0–2); MSI shortcut (direct to GIC) in Phase 3+. |
| Q4.3 | I2C multi-master? | Single-master I2C for Phase 0–2; multi-master arbitration deferred to Phase 3+. |
| Q4.4 | AXI backpressure? | Zero-latency (Virtual timing), estimated (Interval), AXI-AT (Phase 3+). |
| Q4.5 | DMA port abstraction? | `DmaPort` trait; device receives `Arc<dyn DmaPort>` at `elaborate()`; writes via `MemoryMap`. |
| Q4.6 | PCI BAR reprogramming? | Post-write `RemapCommand` queue on `PciBus`; lazy `FlatView` recompute. |
| Q4.9 | PCIe hotplug? | No hotplug Phase 0–2; two-phase (eject + insert) hotplug in Phase 3+. |
| Q4.10 | xHCI/USB doorbell-driven? | Doorbell register write synchronously processes ring — no timer polling. |

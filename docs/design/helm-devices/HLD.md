# helm-devices — High-Level Design

> Crate-level design document for `helm-devices`.
> Cross-references: [`ARCHITECTURE.md`](../../ARCHITECTURE.md) · [`object-model.md`](../../object-model.md) · [`traits.md`](../../traits.md) · [`LLD-device-trait.md`](./LLD-device-trait.md) · [`LLD-interrupt-model.md`](./LLD-interrupt-model.md) · [`LLD-register-bank-macro.md`](./LLD-register-bank-macro.md) · [`LLD-device-registry.md`](./LLD-device-registry.md) · [`LLD-bus-framework.md`](./LLD-bus-framework.md) · [`LLD-device-sdk.md`](./LLD-device-sdk.md)

---

## Table of Contents

1. [Crate Purpose](#1-crate-purpose)
2. [What This Crate Contains](#2-what-this-crate-contains)
3. [What This Crate Does Not Contain](#3-what-this-crate-does-not-contain)
4. [Module Structure](#4-module-structure)
5. [Cargo Features](#5-cargo-features)
6. [Dependency Graph](#6-dependency-graph)
7. [Versioned Device SDK](#7-versioned-device-sdk)
8. [DLD Workflow (Dynamically Loaded Devices)](#8-dld-workflow-dynamically-loaded-devices)
9. [Relationship to World and Full System](#9-relationship-to-world-and-full-system)
10. [Key Design Decisions](#10-key-design-decisions)
11. [Answered Design Questions](#11-answered-design-questions)

---

## 1. Crate Purpose

`helm-devices` is a **unified device infrastructure crate** that provides:

- The **Device SDK** (traits, types, macros) that out-of-tree device authors compile against to produce `.so` DLDs (Dynamically Loaded Devices).
- **Built-in device implementations** — generic ARM IP, SoC-specific blocks, VirtIO devices — organized in feature-gated module groups.
- **Bus protocol implementations** — AMBA (AHB/APB), AXI4, PCI/PCIe, VirtIO MMIO transport, I2C, SPI.
- **Platform wiring** — pre-built machine definitions that compose devices and buses into concrete memory maps with IRQ routes.

### Why One Crate?

The previous generation split framework and devices across many crates. In practice this created friction: tight coupling between bus protocol types and device implementations made cross-crate boundaries artificial. A single crate with **module-level separation** and **Cargo feature gates** gives the same compile-time control without the crate management overhead:

- **Default build** (`default = []`) compiles only the SDK framework — traits, registry, interrupt model, `register_bank!` re-export. No device code, no bus protocols.
- **Feature-gated modules** (`amba`, `pci`, `virtio`, `gic`, `bcm`, etc.) pull in only what a platform needs.
- **Out-of-tree DLDs** depend on `helm-devices` with `default-features = false` — they get the SDK and nothing else.

The distinction is: modules under `framework/` are the standard library for device authors. Everything else is an implementation that happens to live in the same crate for convenience. The SDK boundary is versioned independently of any specific device implementation (§7).

---

## 2. What This Crate Contains

### Framework (always compiled — the Device SDK)

| Item | Module | Purpose |
|------|--------|---------|
| `Device` trait | `framework::device` | Core device interface: MMIO read/write at offsets, signals, region size |
| `DeviceConfig` / `DeviceError` | `framework::device` | Infallible builder → fallible realize pattern |
| `Transaction` / `TransactionAttrs` | `framework::transaction` | Bus-aware transaction context with initiator, security, cacheability |
| `InterruptPin` | `framework::interrupt` | Device interrupt output pin — no IRQ number, no routing knowledge |
| `InterruptWire` | `framework::interrupt` | Internal type connecting a pin to a sink |
| `InterruptSink` trait | `framework::interrupt` | Implemented by interrupt controllers (PLIC, GIC, PIC) |
| `IrqRouter` | `framework::interrupt` | Route table: source device → controller → IRQ number |
| `WireId` | `framework::interrupt` | Opaque wire identifier passed to sink callbacks |
| `SignalInterface` | `framework::signal` | Canonical protocol for named signal assertion/deassertion |
| `Connect<T>` / `Port<T>` | `framework::port` | Typed port wiring (SIMICS-style connect/port) resolved at elaborate time |
| `DeviceDescriptor` | `framework::registry` | Runtime device type record: name, version, factory fn, param schema |
| `DeviceRegistry` | `framework::registry` | HashMap of descriptors; .so DLD loader |
| `ParamSchema` / `ParamField` / `ParamType` | `framework::params` | Typed device configuration schema |
| `DeviceParams` | `framework::params` | Runtime parameter map — typed accessors |
| `DeviceCtx` | `framework::device_ctx` | Lifecycle context for realize/unrealize (MMIO registration, IRQ wiring) |
| `AddressMap` | `framework::address_map` | O(log n) flat-view dispatch with transactional mutations |
| `CharBackend` / `BlockBackend` | `framework::backend` | I/O backend traits for character and block devices |
| `DldError` | `framework::registry` | Error type for DLD loading and device creation |
| `register_bank!` macro | (re-exported from `helm-devices-macros`) | Declarative register bank definition |

### Bus Protocols (feature-gated)

| Module | Feature | Content |
|--------|---------|---------|
| `bus::event_bus` | *(always)* | HelmEventBus — synchronous pub-sub observability (NOT checkpointed) |
| `bus::amba` | `amba` | AHB + APB bridges, address decode helpers, bridge latency model |
| `bus::axi` | `axi` | AXI4 burst timing, data-width configuration, beat-based latency |
| `bus::pci/` | `pci` | PCI config space, BAR sizing, ECAM decode, PciEndpoint trait, MSI/MSI-X, capability chain (PM, AER, PCIe, ACS) |
| `bus::virtio/` | `virtio` | VirtioMmioTransport, split/packed virtqueues, feature bits, VirtioDeviceBackend trait |
| `bus::i2c` | `i2c` | I2cBus master controller, I2cDevice trait, 7-bit addressing |
| `bus::spi` | `spi` | SpiBus master controller, SpiDevice trait, chip-select |
| `bus::usb` | `usb` | USB host controller (xHCI doorbell model) |

### Generic Devices (feature-gated — reusable ARM IP)

| Module | Feature | Device |
|--------|---------|--------|
| `generic::pl011` | `arm-ip` | PL011 UART — full register map, FIFO, CharBackend |
| `generic::pl031` | `arm-ip` | PL031 RTC — real-time clock counter |
| `generic::pl061` | `arm-ip` | PL061 GPIO — 8-bit GPIO with interrupt masking |
| `generic::sp804` | `arm-ip` | SP804 dual 32-bit timer with load/count |
| `generic::sp805` | `arm-ip` | SP805 watchdog timer with lock register |
| `generic::dma` | `dma` | DMA engine — scatter-gather with bus-beat timing |
| `generic::virtio::blk` | `virtio` | VirtIO block device backend |
| `generic::virtio::net` | `virtio` | VirtIO network device backend |
| `generic::virtio::console` | `virtio` | VirtIO serial console backend |
| `generic::virtio::rng` | `virtio` | VirtIO RNG backend |
| `generic::virtio::gpu` | `virtio` | VirtIO GPU backend (2D) |
| `generic::virtio::input` | `virtio` | VirtIO input (keyboard/mouse) |
| `generic::virtio::*` | `virtio` | Additional VirtIO backends (vsock, balloon, fs, crypto, etc.) |

### SoC Devices (feature-gated — vendor-specific)

| Module | Feature | Device |
|--------|---------|--------|
| `soc::gic::v2` | `gic` | ARM GICv2 — Distributor + CPU interface (256 IRQs) |
| `soc::gic::v3` | `gic` | ARM GICv3 — Distributor + redistributors + ICC system registers |
| `soc::gic::v4` | `gic` | ARM GICv4 — vLPI/vSGI extension |
| `soc::gic::distributor` | `gic` | GICD shared state |
| `soc::gic::redistributor` | `gic` | GICR per-PE state |
| `soc::gic::its` | `gic` | ITS command processing |
| `soc::sysregs` | `arm-ip` | RealView platform system registers |
| `soc::bcm::mini_uart` | `bcm` | BCM2837 mini UART |
| `soc::bcm::gpio` | `bcm` | BCM2837 GPIO controller |
| `soc::bcm::sys_timer` | `bcm` | BCM2837 system timer |
| `soc::bcm::mailbox` | `bcm` | BCM2837 mailbox |

### Platform Wiring (feature-gated — machine definitions)

| Module | Feature | Machine |
|--------|---------|---------|
| `platform::arm_virt` | `platform-virt` | QEMU-style "virt" machine — GIC + PL011 + VirtIO slots |
| `platform::realview` | `platform-realview` | ARM RealView PB-A8 — PL011×4 + SP804 + PL061×3 + SP805 + PL031 |
| `platform::rpi3` | `platform-rpi3` | Raspberry Pi 3 (BCM2837) — mini UART + GPIO + system timer + mailbox |

---

## 3. What This Crate Does Not Contain

**No address knowledge.** `helm-devices` types never hold a base address. The `MemoryMap` in `helm-memory` owns address placement. A device sees byte offsets within its mapped region, not absolute addresses. This is enforced structurally: no field in any `helm-devices` type holds an address.

**No IRQ number knowledge.** Devices have no concept of IRQ numbers, interrupt controller inputs, or routing. A device holds an `InterruptPin` and calls `pin.assert()`. Where that signal goes is configured by the platform at elaborate time via `World::wire_interrupt()`. IRQ numbers are a platform integration concern, not a device concern.

**No `helm-memory`, `helm-engine`, `helm-arch`, or `helm-event` dependencies.** The crate depends only on `helm-core`. This is a hard constraint:

- A device model must be compilable without pulling in the full simulation engine.
- DLD `.so` files link against `helm-devices` ABI only; they must not transitively link `helm-engine` or `helm-memory`.
- `World` (in `helm-engine/`) links `helm-devices` + `helm-memory` + `helm-event` from above; `helm-devices` itself does not.

---

## 4. Module Structure

```
helm-devices/
├── Cargo.toml
└── src/
    ├── lib.rs                      # Re-exports, top-level doc, feature gate routing
    │
    ├── framework/                  # ═══ Device SDK (always compiled) ═══
    │   ├── mod.rs                  # pub use of all framework types
    │   ├── device.rs               # Device trait, DeviceConfig, DeviceError
    │   ├── transaction.rs          # Transaction, TransactionAttrs
    │   ├── interrupt.rs            # InterruptPin, InterruptWire, InterruptSink, WireId
    │   ├── irq_router.rs           # IrqRouter, IrqRoute, InterruptController trait
    │   ├── signal.rs               # SignalInterface, named signal constants
    │   ├── port.rs                 # Connect<T>, Port<T>, typed port wiring
    │   ├── params.rs               # ParamSchema, ParamField, ParamType, ParamValue, DeviceParams
    │   ├── registry.rs             # DeviceRegistry, DeviceDescriptor, DldError, .so loader
    │   ├── device_ctx.rs           # DeviceCtx (realize/unrealize lifecycle)
    │   ├── address_map.rs          # AddressMap, FlatViewEntry, transactional mutations
    │   ├── backend.rs              # CharBackend, BlockBackend, NullBackend, BufferBackend
    │   └── sdk.rs                  # SDK version constants, ABI version, prelude re-exports
    │
    ├── bus/                        # ═══ Bus protocols (feature-gated) ═══
    │   ├── mod.rs                  # Bus trait, BusAddress, BusDevice, BusError
    │   ├── event_bus.rs            # HelmEventBus (always compiled, synchronous pub-sub)
    │   ├── amba.rs                 # [feature = "amba"] AHB + APB bridges
    │   ├── axi.rs                  # [feature = "axi"] AXI4 burst timing
    │   ├── i2c/                    # [feature = "i2c"]
    │   │   ├── mod.rs              # I2cBus, I2cDevice trait
    │   │   └── types.rs            # I2cAddr, I2cDirection, I2cState
    │   ├── spi/                    # [feature = "spi"]
    │   │   ├── mod.rs              # SpiBus, SpiDevice trait
    │   │   └── types.rs            # SpiMode, SpiFrame
    │   ├── pci/                    # [feature = "pci"]
    │   │   ├── mod.rs              # PciBus, PciEndpoint trait
    │   │   ├── bdf.rs              # Bus:Device.Function addressing
    │   │   ├── config.rs           # Type-0 config space, BAR sizing protocol
    │   │   ├── host.rs             # PCI host bridge root complex
    │   │   └── capability/         # PCI capabilities
    │   │       ├── mod.rs
    │   │       ├── pm.rs           # Power management
    │   │       ├── msix.rs         # MSI-X vectors
    │   │       ├── aer.rs          # Advanced error reporting
    │   │       ├── pcie.rs         # PCIe features
    │   │       └── acs.rs          # Access control services
    │   ├── virtio/                 # [feature = "virtio"]
    │   │   ├── mod.rs              # VirtioMmioTransport, VirtioDeviceBackend trait
    │   │   ├── transport.rs        # MMIO register interface (spec 4.2)
    │   │   ├── queue.rs            # Split and packed virtqueues
    │   │   └── features.rs         # Feature bit definitions (spec 1.4)
    │   └── usb.rs                  # [feature = "usb"] USB host controller
    │
    ├── generic/                    # ═══ Reusable IP devices (feature-gated) ═══
    │   ├── mod.rs
    │   ├── pl011.rs                # [feature = "arm-ip"] PL011 UART
    │   ├── pl031.rs                # [feature = "arm-ip"] PL031 RTC
    │   ├── pl061.rs                # [feature = "arm-ip"] PL061 GPIO
    │   ├── sp804.rs                # [feature = "arm-ip"] SP804 Timer
    │   ├── sp805.rs                # [feature = "arm-ip"] SP805 Watchdog
    │   ├── dma.rs                  # [feature = "dma"] DMA engine + channels
    │   └── virtio/                 # [feature = "virtio"]
    │       ├── mod.rs
    │       ├── blk.rs              # VirtIO block backend
    │       ├── net.rs              # VirtIO network backend
    │       ├── console.rs          # VirtIO console backend
    │       ├── rng.rs              # VirtIO RNG backend
    │       ├── gpu.rs              # VirtIO GPU backend
    │       ├── input.rs            # VirtIO input backend
    │       ├── vsock.rs            # VirtIO vsock backend
    │       ├── balloon.rs          # VirtIO memory balloon
    │       ├── fs.rs               # VirtIO shared filesystem
    │       └── ...                 # Additional backends as needed
    │
    ├── soc/                        # ═══ Vendor-specific IP (feature-gated) ═══
    │   ├── mod.rs
    │   ├── gic/                    # [feature = "gic"]
    │   │   ├── mod.rs              # GIC version selection
    │   │   ├── v2.rs               # GICv2 — distributor + CPU interface
    │   │   ├── v3.rs               # GICv3 — distributor + redistributors + ICC
    │   │   ├── v4.rs               # GICv4 — vLPI/vSGI extension
    │   │   ├── distributor.rs      # GICD shared state
    │   │   ├── redistributor.rs    # GICR per-PE state
    │   │   ├── its.rs              # ITS command processing
    │   │   ├── lpi.rs              # LPI config tables
    │   │   └── common.rs           # Bitmap/priority helpers
    │   ├── sysregs.rs              # [feature = "arm-ip"] RealView system registers
    │   └── bcm/                    # [feature = "bcm"]
    │       ├── mod.rs
    │       ├── mini_uart.rs        # BCM2837 mini UART
    │       ├── gpio.rs             # BCM2837 GPIO controller
    │       ├── sys_timer.rs        # BCM2837 system timer
    │       └── mailbox.rs          # BCM2837 mailbox
    │
    └── platform/                   # ═══ Machine definitions (feature-gated) ═══
        ├── mod.rs                  # Platform builder, memory map helpers
        ├── arm_virt.rs             # [feature = "platform-virt"] QEMU-style "virt"
        ├── realview.rs             # [feature = "platform-realview"] RealView PB-A8
        └── rpi3.rs                 # [feature = "platform-rpi3"] Raspberry Pi 3
```

The `register_bank!` proc-macro lives in a companion crate `helm-devices-macros` (a `proc-macro = true` crate in the workspace). `helm-devices/Cargo.toml` re-exports it via a dependency, so users see `helm_devices::register_bank!` with no separate import.

### Module Category Summary

| Directory | Role | Feature-gated? | In SDK? |
|-----------|------|---------------|---------|
| `framework/` | Device SDK — traits, registry, interrupt model | No (always compiled) | Yes |
| `bus/event_bus` | HelmEventBus (synchronous observability) | No (always compiled) | Yes |
| `bus/*` (other) | Bus protocol definitions + controllers | Yes | No |
| `generic/` | Reusable IP blocks (PL011, VirtIO, DMA) | Yes | No |
| `soc/` | Vendor-specific IP (GIC, BCM) | Yes | No |
| `platform/` | Machine definitions (memory map + IRQ routes) | Yes | No |

---

## 5. Cargo Features

```toml
# helm-devices/Cargo.toml

[features]
default = []

# Bus protocols
amba     = []
axi      = []
pci      = []
virtio   = []
i2c      = []
spi      = []
usb      = []

# Device groups
arm-ip   = []                           # PL011, PL031, PL061, SP804, SP805, sysregs
gic      = []                           # GICv2/v3/v4
bcm      = []                           # BCM2837 (RPi3)
dma      = []                           # DMA engine

# Platforms (pull in their device/bus dependencies)
platform-virt    = ["amba", "gic", "arm-ip", "virtio"]
platform-realview = ["amba", "arm-ip"]
platform-rpi3    = ["bcm"]

# Convenience
all-buses    = ["amba", "axi", "pci", "virtio", "i2c", "spi", "usb"]
all-devices  = ["arm-ip", "gic", "bcm", "dma"]
all-platforms = ["platform-virt", "platform-realview", "platform-rpi3"]
full         = ["all-buses", "all-devices", "all-platforms"]
```

### Feature Design Principles

- **Default compiles only the SDK.** `cargo add helm-devices` gives you `Device` trait, `DeviceRegistry`, `InterruptPin`, `register_bank!`, `Transaction`, and nothing else.
- **Platform features are transitive.** Enabling `platform-virt` automatically enables `amba`, `gic`, `arm-ip`, and `virtio`.
- **Out-of-tree DLDs use `default-features = false`.** They depend on the SDK only.

---

## 6. Dependency Graph

```
helm-devices (the crate)
    ├── helm-core               (ArchState, MemFault, TimerScheduler — no ISA, no engine)
    ├── helm-devices-macros     (register_bank! proc-macro companion crate)
    ├── inventory               (self-registration for built-in device types)
    ├── libloading              (.so DLD loading)
    ├── serde                   (register_bank! generates serde impls; checkpoint)
    └── log                     (warn!() on unconnected InterruptPin::assert())

helm-engine               (uses helm-devices, adds helm-memory + helm-event)
    ├── helm-devices            ← this crate
    ├── helm-memory             (MemoryMap, MemoryRegion, MmioHandler)
    └── helm-event              (EventQueue for device timer callbacks)

helm-python                         (PyO3 bindings)
    └── helm-devices            ← this crate (for DeviceRegistry, Python class injection)

Out-of-tree DLD (.so)
    └── helm-devices            ← this crate (default-features = false; SDK only)
```

The crate dependency order enforces the constraint: `helm-devices` depends on nothing above `helm-core`. No circular dependencies are possible by construction.

---

## 7. Versioned Device SDK

The Device SDK is the versioned interface that **all** device implementations compile against — built-in and out-of-tree alike. The ABI version is the sole compatibility gate: any device `.so` compiled against ABI version N can be loaded alongside any other `.so` at ABI version N, regardless of who built it, when it was built, or which SDK minor/patch version was used.

### The Ecosystem Model

```
                    ┌─────────────────────────────────────────┐
                    │          Device SDK (ABI v1)             │
                    │   Device trait, Transaction, Registry    │
                    └──────────┬──────────────────────────────┘
                               │
         ┌─────────────────────┼─────────────────────┐
         │                     │                     │
   ┌─────▼──────┐      ┌──────▼──────┐      ┌───────▼──────────┐
   │  Built-in   │      │ Team A .so  │      │ Third-party .so  │
   │  devices    │      │ libgic.so   │      │ libaccel.so      │
   │  (in-tree)  │      │ SDK 1.0     │      │ SDK 1.2          │
   └─────┬──────┘      └──────┬──────┘      └───────┬──────────┘
         │                     │                     │
         └─────────────────────┼─────────────────────┘
                               │
                    ┌──────────▼──────────┐
                    │   DeviceRegistry    │
                    │   ABI v1 check      │
                    │   all are peers     │
                    └──────────┬──────────┘
                               │
                    ┌──────────▼──────────┐
                    │   Python config     │
                    │   wire them together│
                    └─────────────────────┘
```

All devices — whether compiled into the main binary via `inventory::submit!` or loaded at runtime via `load_dld()` — register through the same `DeviceRegistry`, implement the same `Device` trait, and can be wired together by the platform configuration. A GIC from one `.so` receives `InterruptPin::assert()` calls from a UART in a different `.so` compiled months apart. The ABI version guarantees vtable layout, struct sizes, and calling conventions are identical.

### SDK Versioning Scheme

```rust
// framework/sdk.rs

/// Semantic version of the Device SDK.
///
/// This is NOT the ABI version (which is a single u32 — see below).
/// SDK version follows semver:
///   - Major: breaking changes to Device trait, Transaction, or registry protocol
///   - Minor: additive changes (new optional trait methods, new ParamType variants)
///   - Patch: bug fixes that don't affect the API surface
pub const SDK_VERSION: &str = "1.0.0";
pub const SDK_VERSION_MAJOR: u32 = 1;
pub const SDK_VERSION_MINOR: u32 = 0;
pub const SDK_VERSION_PATCH: u32 = 0;

/// ABI version — single u32 checked at DLD load time.
///
/// Incremented on any change that makes compiled DLDs incompatible:
///   - Device trait method signature change
///   - DeviceDescriptor struct layout change
///   - DeviceParams / ParamValue enum variant change
///   - Transaction struct layout change
///
/// NOT incremented for:
///   - New optional trait methods with default impls
///   - New DldError variants
///   - New feature-gated modules (they don't affect the SDK)
///   - Changes to built-in device implementations
pub const HELM_DEVICES_ABI_VERSION: u32 = 1;
```

### SDK Surface — What Is Guaranteed Stable

The following types and traits comprise the SDK surface. Changes to these require an ABI version bump:

| Category | Items |
|----------|-------|
| **Core trait** | `Device` (all methods), `SimObject` (all methods) |
| **Transaction** | `Transaction`, `TransactionAttrs` |
| **Interrupt** | `InterruptPin`, `InterruptSink`, `InterruptWire`, `WireId` |
| **Registry** | `DeviceDescriptor`, `DeviceRegistry` (the `register()` ABI), `DldError` |
| **Params** | `ParamSchema`, `ParamField`, `ParamType`, `ParamValue`, `DeviceParams` |
| **Backend** | `CharBackend`, `BlockBackend` |
| **Macro** | `register_bank!` (generated code ABI) |
| **Constants** | `HELM_DEVICES_ABI_VERSION`, `SDK_VERSION` |

### SDK Surface — What Is NOT Guaranteed

Bus protocols, device implementations, and platform wiring are explicitly **not part of the SDK contract**. They may change freely without an ABI version bump. DLDs that need bus protocol types (e.g., PCI endpoint trait) take a source-level dependency on the feature-gated modules, but these are NOT ABI-stable — recompilation is required when they change.

### Compatibility Matrix

| DLD A | DLD B | Host | Result |
|-------|-------|------|--------|
| ABI 1 (SDK 1.0) | ABI 1 (SDK 1.3) | ABI 1 (SDK 1.5) | All compatible — load succeeds, devices interoperate |
| ABI 1 | ABI 2 | ABI 1 | DLD B rejected — `AbiVersionMismatch` |
| ABI 1 | ABI 1 | ABI 2 | Both DLDs rejected — host requires ABI 2 |
| ABI 2 | ABI 2 | ABI 2 | All compatible |

There is no minor/patch split at the ABI level. Any breaking change bumps the integer. Non-breaking additions (new optional `Device` methods with default impls) do not require a bump.

### SDK Prelude

DLD authors import the SDK via a convenience prelude:

```rust
// In the DLD crate:
use helm_devices::prelude::*;
// Brings in: Device, SimObject, InterruptPin, InterruptSink, WireId,
//            DeviceDescriptor, DeviceParams, ParamSchema, DldError,
//            Transaction, TransactionAttrs, CharBackend, register_bank!
```

```rust
// framework/sdk.rs (continued)

/// Prelude module — convenience re-exports for device authors.
pub mod prelude {
    pub use super::super::framework::device::{Device, DeviceConfig, DeviceError};
    pub use super::super::framework::transaction::{Transaction, TransactionAttrs};
    pub use super::super::framework::interrupt::{InterruptPin, InterruptSink, WireId};
    pub use super::super::framework::params::{ParamSchema, ParamField, ParamType, ParamValue, DeviceParams};
    pub use super::super::framework::registry::{DeviceDescriptor, DeviceRegistry, DldError};
    pub use super::super::framework::backend::{CharBackend, BlockBackend};
    pub use super::super::framework::signal::SignalInterface;
    pub use super::super::{HELM_DEVICES_ABI_VERSION, SDK_VERSION};
}
```

---

## 8. DLD Workflow (Dynamically Loaded Devices)

Devices can be delivered as standalone `.so` libraries (DLDs) and loaded into any compatible host. This applies equally to first-party devices shipped separately from the simulator and to third-party or user-written devices.

### DLD Crate Setup

A DLD author creates a standalone Rust crate with `crate-type = ["cdylib"]`:

```toml
# my-custom-device/Cargo.toml
[package]
name = "helm-dld-my-device"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
# SDK only — no bus protocols, no built-in devices
helm-devices = { version = "1", default-features = false }
log = "0.4"
```

### DLD Implementation

```rust
// my-custom-device/src/lib.rs
use helm_devices::prelude::*;

pub struct MyDevice { /* ... */ }

impl Device for MyDevice {
    fn read(&mut self, offset: u64, size: usize) -> u64 { /* ... */ 0 }
    fn write(&mut self, offset: u64, size: usize, val: u64) { /* ... */ }
    fn region_size(&self) -> u64 { 256 }
}

// ABI version — must match host
#[no_mangle]
pub static HELM_DEVICES_ABI_VERSION: u32 = helm_devices::HELM_DEVICES_ABI_VERSION;

// SDK version — informational, logged at load time
#[no_mangle]
pub static HELM_DEVICES_SDK_VERSION: [u32; 3] = [
    helm_devices::SDK_VERSION_MAJOR,
    helm_devices::SDK_VERSION_MINOR,
    helm_devices::SDK_VERSION_PATCH,
];

#[no_mangle]
pub extern "C" fn helm_device_register(registry: *mut DeviceRegistry) {
    let r = unsafe { &mut *registry };
    let _ = r.register(DeviceDescriptor {
        name: "my_device",
        version: "0.1.0",
        description: "My custom device",
        factory: |params| Ok(Box::new(MyDevice { /* ... */ })),
        param_schema: || ParamSchema::new(),
        python_class_extra: None,
        aliases: &[],
        required_capabilities: &[],
    });
}
```

### Build and Load

```bash
# Build the DLD
cd my-custom-device
cargo build --release
# Output: target/release/libhelm_dld_my_device.so
```

```python
# Load in Python configuration
import helm_ng

helm_ng.load_dld("path/to/libhelm_dld_my_device.so")
dev = helm_ng.MyDevice()
system.map_device(dev, base=0x4000_0000)
```

### Attaching a DLD to an Existing Platform

The Python configuration layer is where devices get wired into platforms. A DLD is attached the same way as any built-in device — the platform script creates it, maps it into the address space, and wires its interrupts.

**Example: Adding a custom PCI device to the ARM virt platform.**

```python
#!/usr/bin/env python3
"""ARM virt platform with a custom PCI accelerator DLD."""
import helm_ng

# Load the DLD — registers MyAccel class in helm_ng namespace
helm_ng.load_dld("/opt/devices/libhelm_dld_accel.so")

def build_platform(args):
    platform = helm_ng.Platform("arm-virt")

    # ── Standard virt devices (built-in) ──────────────────────────
    gic  = helm_ng.Gic(version=2, max_irqs=256)
    uart = helm_ng.Pl011(name="uart0", serial="stdio")
    platform.add_device("gic",   0x0800_0000, gic)
    platform.add_device("uart0", 0x0900_0000, uart)

    # ── PCI bus (built-in) ────────────────────────────────────────
    pci = helm_ng.PciBus(name="pci0")
    platform.add_device("pci0", 0x3000_0000, pci)   # 256 MiB ECAM

    # ── DLD on PCI ────────────────────────────────────────────────
    accel = helm_ng.MyAccel(lanes=8, freq_mhz=1000)
    pci.attach_endpoint(slot=0, function=0, device=accel)

    # ── Interrupt wiring ──────────────────────────────────────────
    # UART IRQ → GIC SPI 33
    platform.wire_interrupt(uart.irq_out, gic.spi(33))
    # Accelerator IRQ → GIC SPI 40
    platform.wire_interrupt(accel.irq_out, gic.spi(40))

    return platform
```

**Example: Adding a DLD I2C sensor to the RPi3 platform.**

```python
#!/usr/bin/env python3
"""RPi3 platform with a custom I2C temperature sensor."""
import helm_ng

helm_ng.load_dld("/opt/devices/libhelm_dld_tmp102.so")

def build_platform(args):
    platform = helm_ng.Platform("rpi3")

    # Standard BCM devices
    uart0 = helm_ng.Pl011(name="uart0", serial="stdio")
    gpio  = helm_ng.BcmGpio(name="gpio")
    platform.add_device("uart0", 0x3F20_1000, uart0)
    platform.add_device("gpio",  0x3F20_0000, gpio)

    # I2C bus controller (built-in) at MMIO address
    i2c = helm_ng.I2cBus(name="i2c1")
    platform.add_device("i2c1", 0x3F80_4000, i2c)

    # DLD sensor on I2C bus at address 0x48
    sensor = helm_ng.Tmp102(alert_temp_c=85)
    i2c.attach_i2c(device=sensor, address=0x48)

    # Sensor alert IRQ → GPIO pin 4
    platform.wire_interrupt(sensor.alert_out, gpio.pin_input(4))

    return platform
```

**The key principle:** a DLD is indistinguishable from a built-in device at the Python configuration level. `load_dld()` injects the Python class; after that, instantiation, mapping, bus attachment, and interrupt wiring all use the same API. The device has no knowledge of its address or IRQ — the platform script decides.

### Programmatic Rust API

The same wiring is available from Rust without Python. `DeviceRegistry` and `World` are Rust-first APIs — the Python layer is a thin wrapper.

```rust
use helm_devices::prelude::*;
use helm_engine::World;

fn build_system() -> World {
    let mut registry = DeviceRegistry::new();  // collects built-ins

    // Load DLDs programmatically
    registry.load_dld("/opt/devices/libhelm_dld_accel.so".as_ref()).unwrap();
    registry.load_dld("/opt/devices/libhelm_dld_nvme.so".as_ref()).unwrap();

    let mut world = World::new();

    // Create devices by name from the registry
    let gic  = registry.create("gic", DeviceParams::from([("max_irqs", 256.into())])).unwrap();
    let uart = registry.create("pl011", DeviceParams::from([("serial", "stdio".into())])).unwrap();
    let accel = registry.create("my_accel", DeviceParams::from([
        ("lanes", 8.into()),
        ("freq_mhz", 1000.into()),
    ])).unwrap();

    // Map into address space
    let gic_id  = world.add_device("gic", gic);
    let uart_id = world.add_device("uart0", uart);
    let accel_id = world.add_device("accel", accel);
    world.map_device(gic_id,   0x0800_0000);
    world.map_device(uart_id,  0x0900_0000);
    world.map_device(accel_id, 0x0B00_0000);

    // Wire interrupts
    world.wire_interrupt(uart_id,  "irq_out", gic_id, WireId::from(33u32));
    world.wire_interrupt(accel_id, "irq_out", gic_id, WireId::from(40u32));

    world.elaborate();
    world
}
```

This is not a secondary API — it is the same code path the Python layer calls. The Python `platform.add_device()` method ultimately calls `World::add_device()` + `World::map_device()`.

### Programmatic Python API (gem5-style)

Python scripts are the primary user-facing configuration interface, modeled after gem5:

```python
import helm_ng

# Load any number of DLDs
helm_ng.load_dld("/opt/devices/libhelm_dld_accel.so")

# DeviceRegistry is accessible from Python
registry = helm_ng.registry()
print(registry.list())         # all registered device types
print(registry.schema("my_accel"))  # ParamSchema for my_accel

# Create devices — Python classes auto-generated from ParamSchema
accel = helm_ng.MyAccel(lanes=8, freq_mhz=1000)
uart  = helm_ng.Pl011(serial="stdio")

# Build platform programmatically
platform = helm_ng.Platform("custom")
platform.add_device("uart0", 0x0900_0000, uart)
platform.add_device("accel", 0x0B00_0000, accel)
platform.wire_interrupt(uart.irq_out, gic.spi(33))
platform.wire_interrupt(accel.irq_out, gic.spi(40))

# Or create by registry name (useful for dynamic/data-driven configs)
dev = helm_ng.create_device("my_accel", lanes=8, freq_mhz=1000)
platform.add_device("accel2", 0x0C00_0000, dev)
```

Both `helm_ng.MyAccel(...)` (class-based) and `helm_ng.create_device("my_accel", ...)` (name-based) call the same registry factory underneath.

### Adding a device to a platform via CLI (--dld / --device flags)

As a convenience shortcut, the CLI supports `--device` for ad-hoc device addition without modifying the platform script:

```bash
# Load DLD and add device at a specific address
helm-system-aarch64 examples/fs/virt.py \
    --dld /opt/devices/libhelm_dld_accel.so \
    --device my_accel,base=0x0B000000,lanes=8,freq_mhz=1000
```

The `--dld` flag calls `load_dld()` before platform construction. The `--device` flag calls `create_device()` + `add_device()` with the specified parameters. This is a thin CLI wrapper around the programmatic Python API above.

### Load Sequence (detailed)

```
1. Python calls helm_ng.load_dld(path)
2. PyO3 → DeviceRegistry::load_dld(path)
3. dlopen(path)                        → libloading::Library
4. Read HELM_DEVICES_ABI_VERSION       → u32 from DLD
5. Compare to host ABI version         → DldError::AbiVersionMismatch on mismatch
6. Read HELM_DEVICES_SDK_VERSION       → [u32; 3] (optional, logged if present)
7. Load helm_device_register symbol
8. Call helm_device_register(&mut registry)
9.   DLD calls r.register() for each device type
10.  Each register() validates name uniqueness
11.  Each register() auto-generates Python class from ParamSchema
12.  Each register() injects Python class into helm_ng namespace
13. Keep Library handle alive in registry._libs
14. Return Ok(()) to Python
```

### Version Diagnostics

When a DLD is loaded, the host logs SDK version information:

```
[INFO] Loading DLD: /opt/helm/lib/libhelm_dld_my_device.so
[INFO]   ABI version: 1 (matches host)
[INFO]   SDK version: 1.0.0 (host: 1.2.0)
[INFO]   Registered: my_device v0.1.0
```

If the DLD was compiled against SDK 1.0.0 and the host is at 1.2.0, this is fine (same ABI version). The log helps diagnose issues.

---

## 9. Relationship to World and Full System

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

Context 3: DLD Test (.so + test harness)
    World instantiates a device from the DLD registry.
    Same MMIO path. No host system or CPU model required.
```

**A device that passes all World tests is guaranteed to work identically in a full system.** The `Device` trait implementation is world-agnostic: it receives offsets, sizes, and values — never absolute addresses or context pointers.

The `SimObject` lifecycle (`init → elaborate → startup → reset → checkpoint_save / checkpoint_restore`) is an optional extension. Devices that need lifecycle management implement both `Device` and `SimObject`. Devices used only in headless testing scenarios may implement `Device` alone — `World` does not require `SimObject`. This orthogonality is design question Q60's resolution.

---

## 10. Key Design Decisions

### Devices Have No Address or IRQ Knowledge (Q60, Q61, Q62)

A `Device` receives byte offsets within its mapped region. The `MemoryMap` in `helm-memory` owns address placement. The `InterruptPin` owned by the device has no knowledge of which interrupt controller input it is connected to. Both address and IRQ routing are platform/SoC integration concerns expressed in Python configuration.

This mirrors real hardware: a UART IP block has an `irq` output pin. The SoC designer connects it to interrupt controller input N in the netlist. The UART RTL has no `#define IRQ_NUM N`.

### Device: SimObject Is Orthogonal (Q60)

`Device` and `SimObject` are separate traits with no inheritance relationship. A device may implement:

- `Device` only — for headless `World` usage
- `Device` + `SimObject` — for full system participation with lifecycle and checkpointing

`World` requires only `Device`. `System` (full simulation) requires both. DLD authors choose based on their intended use.

### Single Crate, Module-Level Separation

The entire device ecosystem lives in one `helm-devices` crate. The framework (SDK) is always compiled; everything else is feature-gated. This avoids the crate explosion of the previous generation while maintaining clean separation:

- **`framework/`** = the API contract, versioned, stable
- **`bus/`** = protocol definitions and controllers, may change freely
- **`generic/`** = reusable IP, may change freely
- **`soc/`** = vendor-specific, may change freely
- **`platform/`** = machine definitions, may change freely

### Three Device Categories

1. **Generic devices** (`generic/`) — ARM IP blocks that follow a standard bus protocol and are reused across many SoCs (PL011, SP804). They talk through `Transaction` and never know their address.

2. **SoC devices** (`soc/`) — Vendor-specific blocks tied to a particular silicon (GIC is ARM-specific, BCM mailbox is Broadcom-specific). Still use `Device` trait, but have SoC-specific register maps and behaviors.

3. **Platform wiring** (`platform/`) — Composes generic + SoC devices into a concrete machine. Owns the memory map, IRQ routes, and bus topology. This is where `0x0900_0000 → PL011` gets decided.

### Transaction-Based Bus Hierarchy

The `Transaction` type carries full context as it flows through the bus hierarchy:

```rust
pub struct Transaction {
    pub addr: u64,                    // Absolute address on originating bus
    pub offset: u64,                  // Offset relative to device base
    pub size: usize,                  // 1, 2, 4, 8, or 16 bytes
    pub data: [u8; 16],              // Up to 128 bits for SIMD/LDP/STP
    pub is_write: bool,
    pub attrs: TransactionAttrs,
    pub stall_cycles: u64,           // Accumulated latency through bus hierarchy
}
```

Each bridge and device can inspect attributes (secure, privileged, cacheable) and accumulate `stall_cycles`. FE-mode devices use the simplified `Device::read()/write()` path; FS-mode with timing uses the full `Transaction` path.

### region_size() Is Fixed at Construction for Phase 0 (Q61)

`Device::region_size() -> u64` returns a value set at construction time and does not change. This simplifies `MemoryMap` — it never needs to re-flatten the `FlatView` because of a BAR resize. PCIe BAR dynamic resizing is a Phase 3+ concern and will require a `MemoryMap::resize_region()` notification path when implemented.

### InterruptPin Connections Set at finalize() via World::wire_interrupt() (Q62)

`InterruptPin` fields are `None` at construction and `Some(wire)` after `World::wire_interrupt()` is called during the `elaborate()` phase. After `startup()`, the wiring graph is frozen. A device never sets its own `InterruptPin` connection — the platform configuration does.

### register_bank! Is a Proc-Macro (Q63–Q66)

The `register_bank!` macro is a procedural macro that generates: an `MmioHandler` implementation with a dispatch table keyed by offset, `serde` checkpoint serialization, and `AttrDescriptor` Python introspection data. Side-effect methods (`on_write_<reg>`, `on_read_<reg>`) are hooks the device author provides; the macro calls them at the correct point in the dispatch.

### Python Class Name Conflicts Are Errors at Load Time (Q67)

When a DLD is loaded, its embedded `PYTHON_CLASS` string is `exec()`'d into the `helm_ng` module namespace. If a name already exists in that namespace from a previously loaded DLD, the loader raises `DldError::PythonNameConflict` and the load fails. The DLD is not partially registered.

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

## 11. Answered Design Questions

| Q# | Question | Answer |
|----|----------|--------|
| Q60 | Device: SimObject orthogonal? | Yes — `Device` and `SimObject` are separate traits. `World` needs `Device` without `SimObject` lifecycle. |
| Q61 | region_size() fixed at construction? | Yes — fixed for Phase 0. PCIe BAR resize deferred to Phase 3+. |
| Q62 | InterruptPin connections set how? | At `elaborate()` time via `World::wire_interrupt()`. Frozen after `startup()`. |
| Q63 | register_bank! on_write/on_read hook API? | Method hooks named `on_write_<regname>` and `on_read_<regname>` on the device struct. |
| Q64 | register_bank! generates serde derive? | Yes — generated automatically. Device author does not write serde impls. |
| Q65 | Split-function registers (THR/RHR)? | `is write_only` / `is read_only` qualifiers in macro syntax. |
| Q66 | register_bank! generates Python introspection? | Yes — `AttrDescriptor` array for register names, offsets, field names. |
| Q67 | Python class name conflict at DLD load? | `DldError::PythonNameConflict` — error, load fails. |
| Q68 | DLD versioning against ABI mismatch? | `HELM_DEVICES_ABI_VERSION` symbol in every DLD; checked before calling `helm_device_register`. |
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
| Q3.2 | DLD C ABI? | Pure `extern "C"` via cbindgen; `HELM_DEVICES_ABI_VERSION: u32` confirmed correct. |
| Q3.5 | Python class authority? | `ParamSchema` authoritative; Python class auto-generated — no hand-written string. |
| Q3.6 | DLD checkpoint migration? | `serde` + version tag + `helm_{name}_migrate_checkpoint` C export in DLD. |
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
| **NEW** | Single crate vs. multi-crate? | Single crate with feature-gated modules. SDK is `framework/`; devices are `generic/` + `soc/`; machines are `platform/`. |
| **NEW** | SDK versioning for out-of-tree DLDs? | Semver SDK version + single u32 ABI version. ABI checked at dlopen; SDK logged for diagnostics. |
| **NEW** | DLD Cargo.toml dependency? | `helm-devices = { version = "1", default-features = false }` — SDK only, no device code. |
| **NEW** | SDK prelude for DLD authors? | `use helm_devices::prelude::*` — all SDK types in one import. |
| **NEW** | Generic vs. SoC vs. Platform? | Generic = reusable IP (PL011). SoC = vendor-specific (GIC, BCM). Platform = machine definition (memory map + IRQ routes). |
| **NEW** | Programmatic device wiring? | Rust `World` API and Python `Platform` API are equivalent — both support `load_dld()`, `create_device()`, `add_device()`, `wire_interrupt()`. CLI `--device` is a convenience wrapper. |
| **NEW** | Can DLDs from different SDK versions coexist? | Yes — any `.so` at ABI version N loads alongside any other at ABI version N, regardless of SDK minor/patch version or build date. |

# helm-devices — High-Level Design

> Crate-level design document for `helm-devices`.
> Cross-references: [`ARCHITECTURE.md`](../../ARCHITECTURE.md) · [`object-model.md`](../../object-model.md) · [`traits.md`](../../traits.md) · [`LLD-device-trait.md`](./LLD-device-trait.md) · [`LLD-interrupt-model.md`](./LLD-interrupt-model.md) · [`LLD-register-bank-macro.md`](./LLD-register-bank-macro.md) · [`LLD-device-registry.md`](./LLD-device-registry.md) · [`LLD-bus-framework.md`](./LLD-bus-framework.md) · [`LLD-device-sdk.md`](./LLD-device-sdk.md)

---

## Table of Contents

1. [Crate Purpose](#1-crate-purpose)
2. [What This Crate Contains](#2-what-this-crate-contains)
3. [What This Crate Does Not Contain](#3-what-this-crate-does-not-contain)
4. [Module Structure](#4-module-structure)
5. [Dependency Graph](#5-dependency-graph)
6. [Versioned Device SDK](#6-versioned-device-sdk)
7. [DLD Workflow (Dynamically Loaded Devices)](#7-dld-workflow-dynamically-loaded-devices)
8. [Relationship to World and Full System](#8-relationship-to-world-and-full-system)
9. [Key Design Decisions](#9-key-design-decisions)
10. [Answered Design Questions](#10-answered-design-questions)

---

## 1. Crate Purpose

`helm-devices` is the **Device SDK crate** — the stable, versioned interface that all device implementations compile against. It provides:

- The **Device SDK** (traits, types, macros) that device authors compile against to produce `.so` DLDs (Dynamically Loaded Devices) and in-tree device crates.
- **Bus abstraction traits** — `Bus`, `BusDevice`, `BusAddress`, `BusError` — that define the contract for bus protocol implementations without providing any concrete protocol.
- **HelmEventBus** — the synchronous pub-sub observability system.

### Why SDK-Only?

The previous design placed both framework and concrete device implementations in a single feature-gated crate. In practice this created problems: feature flags accumulated, dependency edges between device modules violated the "SDK depends on nothing above `helm-core`" invariant, and out-of-tree DLD authors needed to understand which features to disable. The split is clean:

- **`helm-devices`** is the SDK. It lives at `framework/helm-devices/`. No feature gates, no conditional compilation. Every type in the crate is always compiled. Out-of-tree DLD authors depend on it directly with no `default-features = false` gymnastics.
- **`hw/` crates** (`helm-hw-amba`, `helm-hw-pci`, `helm-hw-virtio`) contain concrete bus controllers, device IP blocks, and protocol implementations. They depend on `helm-devices` for the SDK types and implement `Device`, `Bus`, and `BusDevice` traits defined here.

The distinction is: everything in `helm-devices` is the standard library for device authors — the API contract. Everything in `hw/` is an implementation built on top of that contract.

---

## 2. What This Crate Contains

### Framework (the Device SDK)

| Item | Module | Purpose |
|------|--------|---------|
| `Device` trait | `framework::device` | Core device interface: MMIO read/write at offsets, signals, region size |
| `DeviceConfig` / `DeviceError` | `framework::device` | Infallible builder → fallible realize pattern |
| `Transaction` / `TransactionAttrs` | `framework::transaction` | Bus-aware transaction context with initiator, security, cacheability |
| `InterruptPin` | `framework::interrupt` | Device interrupt output pin — no IRQ number, no routing knowledge |
| `InterruptWire` | `framework::interrupt` | Internal type connecting a pin to a sink |
| `InterruptSink` trait | `framework::interrupt` | Implemented by interrupt controllers (PLIC, GIC, PIC) |
| `IrqRouter` | `framework::irq_router` | Route table: source device → controller → IRQ number |
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

### Bus Abstraction (traits only)

| Item | Module | Purpose |
|------|--------|---------|
| `Bus` trait | `bus` | Addressable device interconnect: attach, enumerate |
| `BusDevice` trait | `bus` | Register-level access for bus-attached devices |
| `BusAddress` enum | `bus` | Protocol-specific address (I2C 7-bit, SPI CS, PCI BDF, custom) |
| `BusDeviceDescriptor` | `bus` | Returned by `Bus::enumerate()` |
| `BusAttachError` / `BusError` | `bus` | Error types for bus operations |
| `HelmEventBus` | `bus::event_bus` | Synchronous pub-sub observability (NOT checkpointed) |

The bus module defines **traits and types only**. It does not contain any concrete bus protocol implementation — no AMBA, no PCI, no VirtIO, no I2C, no SPI. Those live in `hw/` crates.

---

## 3. What This Crate Does Not Contain

**No concrete device implementations.** PL011, SP804, PL031, DMA, GIC, BCM peripherals, and all other device models live in `hw/` crates or out-of-tree DLDs. `helm-devices` provides the traits they implement but none of the implementations.

**No concrete bus protocol implementations.** AMBA (AHB/APB) bus controllers, PCI ECAM host bridges, VirtIO MMIO transports, I2C bus masters, and SPI bus masters all live in `hw/` crates:

| Implementation | Crate |
|----------------|-------|
| `AhbBus`, `ApbBus` | `hw/helm-hw-amba` |
| `I2cBus`, `SpiBus` | `hw/helm-hw-amba` |
| `PL011`, `SP804`, `PL031`, `DMA` | `hw/helm-hw-amba` |
| `PciBus`, `PciEndpoint`, `PciConfigSpace`, `Bdf` | `hw/helm-hw-pci` |
| `VirtioMmioTransport`, `VirtioBackend`, feature constants | `hw/helm-hw-virtio` |

**No platform wiring or machine definitions.** Memory maps, IRQ route tables, and machine builders are a platform integration concern, not an SDK concern.

**No SoC-specific IP.** GIC, BCM peripherals, system registers — all vendor-specific IP lives outside this crate.

**No address knowledge.** `helm-devices` types never hold a base address. The `MemoryMap` in `helm-memory` owns address placement. A device sees byte offsets within its mapped region, not absolute addresses. This is enforced structurally: no field in any `helm-devices` type holds an address.

**No IRQ number knowledge.** Devices have no concept of IRQ numbers, interrupt controller inputs, or routing. A device holds an `InterruptPin` and calls `pin.assert()`. Where that signal goes is configured by the platform at elaborate time via `World::wire_interrupt()`. IRQ numbers are a platform integration concern, not a device concern.

**No `helm-memory`, `helm-engine`, `helm-arch`, or `helm-event` dependencies.** The crate depends only on `helm-core`. This is a hard constraint:

- A device model must be compilable without pulling in the full simulation engine.
- DLD `.so` files link against `helm-devices` ABI only; they must not transitively link `helm-engine` or `helm-memory`.
- `World` (in `helm-engine/`) links `helm-devices` + `helm-memory` + `helm-event` from above; `helm-devices` itself does not.

---

## 4. Module Structure

```
framework/helm-devices/
├── Cargo.toml
└── src/
    ├── lib.rs                      # Re-exports, top-level doc
    │
    ├── framework/                  # ═══ Device SDK ═══
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
    └── bus/                        # ═══ Bus abstraction (traits only) ═══
        ├── mod.rs                  # Bus trait, BusAddress, BusDevice, BusError
        └── event_bus.rs            # HelmEventBus (synchronous pub-sub)
```

The `register_bank!` proc-macro lives in a companion crate `helm-devices-macros` (a `proc-macro = true` crate in the workspace). `helm-devices/Cargo.toml` re-exports it via a dependency, so users see `helm_devices::register_bank!` with no separate import.

### Concrete Implementations (in hw/ crates)

```
hw/
├── helm-hw-amba/                   # AMBA bus + ARM IP peripherals
│   └── src/
│       ├── lib.rs
│       ├── amba.rs                  # AhbBus, ApbBus
│       ├── i2c.rs                   # I2cBus (I2C master controller)
│       ├── spi.rs                   # SpiBus (SPI master controller)
│       ├── pl011.rs                 # PL011 UART
│       ├── sp804.rs                 # SP804 dual timer
│       ├── pl031.rs                 # PL031 RTC
│       └── dma.rs                   # DMA engine
│
├── helm-hw-pci/                    # PCI/PCIe host bridge
│   └── src/
│       ├── lib.rs
│       └── config.rs               # PCI config space, ECAM decode
│
└── helm-hw-virtio/                 # VirtIO MMIO transport + backends
    └── src/
        ├── lib.rs
        ├── transport.rs             # VirtioMmioTransport
        └── features.rs             # VirtIO feature bit constants
```

---

## 5. Dependency Graph

```
helm-devices (the crate — SDK only)
    ├── helm-core               (ArchState, MemFault, TimerScheduler — no ISA, no engine)
    ├── thiserror               (error derive)
    └── log                     (warn!() on unconnected InterruptPin::assert())

hw/helm-hw-amba
    └── helm-devices            ← this crate (SDK types: Device, Bus, InterruptPin, etc.)

hw/helm-hw-pci
    └── helm-devices            ← this crate

hw/helm-hw-virtio
    └── helm-devices            ← this crate

helm-engine               (uses helm-devices, adds helm-memory + helm-event)
    ├── helm-devices            ← this crate
    ├── helm-memory             (MemoryMap, MemoryRegion, MmioHandler)
    └── helm-event              (EventQueue for device timer callbacks)

helm-python                         (PyO3 bindings)
    └── helm-devices            ← this crate (for DeviceRegistry, Python class injection)

Out-of-tree DLD (.so)
    └── helm-devices            ← this crate (SDK only — it is always SDK only)
```

The crate dependency order enforces the constraint: `helm-devices` depends on nothing above `helm-core`. No circular dependencies are possible by construction. The `hw/` crates are leaf crates — they depend on `helm-devices` for the SDK but are not depended on by it.

---

## 6. Versioned Device SDK

The Device SDK is the versioned interface that **all** device implementations compile against — in-tree `hw/` crates and out-of-tree DLDs alike. The ABI version is the sole compatibility gate: any device `.so` compiled against ABI version N can be loaded alongside any other `.so` at ABI version N, regardless of who built it, when it was built, or which SDK minor/patch version was used.

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
   │  hw/ crates │      │ Team A .so  │      │ Third-party .so  │
   │ helm-hw-*   │      │ libgic.so   │      │ libaccel.so      │
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

All devices — whether compiled into the main binary from `hw/` crates, or loaded at runtime via `load_dld()` — register through the same `DeviceRegistry`, implement the same `Device` trait, and can be wired together by the platform configuration. A GIC from one `.so` receives `InterruptPin::assert()` calls from a UART in a different `.so` compiled months apart. The ABI version guarantees vtable layout, struct sizes, and calling conventions are identical.

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
///   - Changes to hw/ crate implementations (they don't affect the SDK)
///   - Changes to bus trait method signatures (bus traits are not ABI-stable)
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

Bus abstraction traits (`Bus`, `BusDevice`, `BusAddress`, `BusError`) are **not part of the ABI contract**. They are source-level APIs that `hw/` crates and DLDs use, but changes to them do not require an ABI version bump — only recompilation of crates that use them. Similarly, all types in `hw/` crates are explicitly not ABI-stable.

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

## 7. DLD Workflow (Dynamically Loaded Devices)

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
# SDK only — helm-devices is always SDK-only, no features needed
helm-devices = { version = "1" }
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

The Python configuration layer is where devices get wired into platforms. A DLD is attached the same way as any in-tree device from an `hw/` crate — the platform script creates it, maps it into the address space, and wires its interrupts.

**Example: Adding a custom PCI device to the ARM virt platform.**

```python
#!/usr/bin/env python3
"""ARM virt platform with a custom PCI accelerator DLD."""
import helm_ng

# Load the DLD — registers MyAccel class in helm_ng namespace
helm_ng.load_dld("/opt/devices/libhelm_dld_accel.so")

def build_platform(args):
    platform = helm_ng.Platform("arm-virt")

    # ── Standard virt devices (from hw/ crates) ──────────────────
    gic  = helm_ng.Gic(version=2, max_irqs=256)
    uart = helm_ng.Pl011(name="uart0", serial="stdio")
    platform.add_device("gic",   0x0800_0000, gic)
    platform.add_device("uart0", 0x0900_0000, uart)

    # ── PCI bus (from helm-hw-pci) ────────────────────────────────
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

    # I2C bus controller (from helm-hw-amba) at MMIO address
    i2c = helm_ng.I2cBus(name="i2c1")
    platform.add_device("i2c1", 0x3F80_4000, i2c)

    # DLD sensor on I2C bus at address 0x48
    sensor = helm_ng.Tmp102(alert_temp_c=85)
    i2c.attach_i2c(device=sensor, address=0x48)

    # Sensor alert IRQ → GPIO pin 4
    platform.wire_interrupt(sensor.alert_out, gpio.pin_input(4))

    return platform
```

**The key principle:** a DLD is indistinguishable from an in-tree `hw/` crate device at the Python configuration level. `load_dld()` injects the Python class; after that, instantiation, mapping, bus attachment, and interrupt wiring all use the same API. The device has no knowledge of its address or IRQ — the platform script decides.

### Programmatic Rust API

The same wiring is available from Rust without Python. `DeviceRegistry` and `World` are Rust-first APIs — the Python layer is a thin wrapper.

```rust
use helm_devices::prelude::*;
use helm_engine::World;

fn build_system() -> World {
    let mut registry = DeviceRegistry::new();  // collects in-tree hw/ devices

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

## 8. Relationship to World and Full System

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

**A device that passes all World tests is guaranteed to work identically in a full system.** The `Device` trait implementation is world-agnostic: it receives offsets, sizes, and values — never absolute addresses or context pointers. This is true regardless of whether the device lives in an `hw/` crate or a DLD `.so`.

The `SimObject` lifecycle (`init → elaborate → startup → reset → checkpoint_save / checkpoint_restore`) is an optional extension. Devices that need lifecycle management implement both `Device` and `SimObject`. Devices used only in headless testing scenarios may implement `Device` alone — `World` does not require `SimObject`. This orthogonality is design question Q60's resolution.

---

## 9. Key Design Decisions

### SDK Crate, Separate Implementation Crates

`helm-devices` contains exclusively the Device SDK — traits, types, macros, registry, interrupt model, bus abstraction traits, and the event bus. All concrete implementations live in `hw/` crates:

- **`framework/helm-devices/`** = the API contract, versioned, ABI-stable
- **`hw/helm-hw-amba/`** = AMBA bus controllers + ARM IP peripherals (PL011, SP804, PL031, DMA, I2C, SPI)
- **`hw/helm-hw-pci/`** = PCI ECAM host bridge, config space
- **`hw/helm-hw-virtio/`** = VirtIO MMIO transport, backend trait, feature constants

This replaced the previous single-crate-with-feature-gates design. The split eliminates all Cargo feature flags from `helm-devices` and ensures the SDK crate has a minimal dependency footprint (`helm-core` + `thiserror` + `log`).

### Devices Have No Address or IRQ Knowledge (Q60, Q61, Q62)

A `Device` receives byte offsets within its mapped region. The `MemoryMap` in `helm-memory` owns address placement. The `InterruptPin` owned by the device has no knowledge of which interrupt controller input it is connected to. Both address and IRQ routing are platform/SoC integration concerns expressed in Python configuration.

This mirrors real hardware: a UART IP block has an `irq` output pin. The SoC designer connects it to interrupt controller input N in the netlist. The UART RTL has no `#define IRQ_NUM N`.

### Device: SimObject Is Orthogonal (Q60)

`Device` and `SimObject` are separate traits with no inheritance relationship. A device may implement:

- `Device` only — for headless `World` usage
- `Device` + `SimObject` — for full system participation with lifecycle and checkpointing

`World` requires only `Device`. `System` (full simulation) requires both. DLD authors choose based on their intended use.

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

## 10. Answered Design Questions

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
| Q4.1 | PCI config space decode? | `PciBus` internal ECAM decode — concrete impl in `helm-hw-pci`. |
| Q4.2 | MSI-X address-space path? | Full `MemoryMap` path (Phase 0–2); MSI shortcut (direct to GIC) in Phase 3+. |
| Q4.3 | I2C multi-master? | Single-master I2C for Phase 0–2; multi-master arbitration deferred to Phase 3+. |
| Q4.4 | AXI backpressure? | Zero-latency (Virtual timing), estimated (Interval), AXI-AT (Phase 3+). |
| Q4.5 | DMA port abstraction? | `DmaPort` trait; device receives `Arc<dyn DmaPort>` at `elaborate()`; writes via `MemoryMap`. |
| Q4.6 | PCI BAR reprogramming? | Post-write `RemapCommand` queue on `PciBus`; lazy `FlatView` recompute. |
| Q4.9 | PCIe hotplug? | No hotplug Phase 0–2; two-phase (eject + insert) hotplug in Phase 3+. |
| Q4.10 | xHCI/USB doorbell-driven? | Doorbell register write synchronously processes ring — no timer polling. |
| **NEW** | SDK crate vs. monolithic crate? | SDK-only crate (`framework/helm-devices/`) with concrete implementations in `hw/` crates. No feature gates. |
| **NEW** | SDK versioning for out-of-tree DLDs? | Semver SDK version + single u32 ABI version. ABI checked at dlopen; SDK logged for diagnostics. |
| **NEW** | DLD Cargo.toml dependency? | `helm-devices = { version = "1" }` — the crate is always SDK-only, no feature flags needed. |
| **NEW** | SDK prelude for DLD authors? | `use helm_devices::prelude::*` — all SDK types in one import. |
| **NEW** | Where do concrete devices live? | `hw/helm-hw-amba` (AMBA buses + ARM IP), `hw/helm-hw-pci` (PCI), `hw/helm-hw-virtio` (VirtIO). All depend on `helm-devices` for SDK types. |
| **NEW** | Programmatic device wiring? | Rust `World` API and Python `Platform` API are equivalent — both support `load_dld()`, `create_device()`, `add_device()`, `wire_interrupt()`. CLI `--device` is a convenience wrapper. |
| **NEW** | Can DLDs from different SDK versions coexist? | Yes — any `.so` at ABI version N loads alongside any other at ABI version N, regardless of SDK minor/patch version or build date. |

# helm-devices — LLD: Versioned Device SDK

> Low-level design for the Device SDK versioning, out-of-tree DLD ABI contract, `Transaction` type, and SDK prelude.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-device-trait.md`](./LLD-device-trait.md) · [`LLD-device-registry.md`](./LLD-device-registry.md)

---

## Table of Contents

1. [Purpose](#1-purpose)
2. [SDK Version Constants](#2-sdk-version-constants)
3. [ABI Contract](#3-abi-contract)
4. [Transaction Type](#4-transaction-type)
5. [SDK Prelude](#5-sdk-prelude)
6. [DLD Crate Template](#6-dld-crate-template)
7. [DLD Load Sequence](#7-dld-load-sequence)
8. [Platform Integration — Wiring DLD Devices](#8-platform-integration--wiring-dld-devices)
9. [SDK Evolution Policy](#9-sdk-evolution-policy)
10. [Fast-Path vs Transaction-Path Device API](#10-fast-path-vs-transaction-path-device-api)
11. [SDK Header Generation (cbindgen)](#11-sdk-header-generation-cbindgen)

---

## 1. Purpose

The Device SDK is the versioned interface that **all** device implementations — built-in or out-of-tree — compile against. It exists to enable an ecosystem of independently compiled, mix-and-match device libraries:

- **Any device compiled against a compatible ABI version can be loaded alongside any other.** A GIC built by team A, a UART built by team B, and a custom accelerator built by a third party all coexist in the same simulation — as long as they share the same ABI version.
- **Devices compiled at different times are compatible.** A DLD built against SDK 1.0 loads on a host running SDK 1.3 (same ABI version). A DLD built against SDK 1.3 loads on a host running SDK 1.0. The ABI version is the sole compatibility gate.
- **The simulator host and device libraries evolve independently.** The host can upgrade its engine, memory subsystem, and timing model without recompiling any device `.so`. Device authors can ship updates without waiting for a simulator release.
- **When a breaking change is unavoidable**, the ABI version is bumped and all DLDs must recompile against the new SDK.

The SDK surface lives entirely in `framework/` and is always compiled (no feature gates). Everything outside `framework/` is implementation detail and is **not part of the SDK contract**.

### Device Ecosystem Model

```
                    ┌─────────────────────────────────────────┐
                    │          Device SDK (ABI v1)             │
                    │   Device trait, Transaction, Registry    │
                    └──────────┬──────────────────────────────┘
                               │
              ┌────────────────┼────────────────────┐
              │                │                    │
    ┌─────────▼──────┐  ┌─────▼────────┐  ┌───────▼──────────┐
    │ Built-in devices│  │ Team A DLD   │  │ Third-party DLD   │
    │ (in-tree, same │  │ libgic.so    │  │ libaccel.so       │
    │  cargo build)  │  │ SDK 1.0      │  │ SDK 1.2           │
    └─────────┬──────┘  └─────┬────────┘  └───────┬──────────┘
              │               │                    │
              └───────────────┼────────────────────┘
                              │
                    ┌─────────▼─────────┐
                    │   DeviceRegistry  │
                    │   (host runtime)  │
                    │   ABI v1 check    │
                    └─────────┬─────────┘
                              │
                    ┌─────────▼─────────┐
                    │  Python config    │
                    │  load_dld()       │
                    │  helm_ng.Gic()    │
                    │  helm_ng.Accel()  │
                    └───────────────────┘
```

All devices — whether compiled into the main binary via `inventory::submit!` or loaded at runtime via `load_dld()` — register through the same `DeviceRegistry` and implement the same `Device` trait. The distinction between "built-in" and "DLD" is purely a packaging concern, not an API concern.

---

## 2. SDK Version Constants

```rust
// framework/sdk.rs

/// Semantic version of the Device SDK.
///
/// Follows semver strictly:
///   Major: breaking changes to any SDK surface type
///   Minor: additive, backward-compatible changes
///   Patch: bug fixes with no API impact
pub const SDK_VERSION: &str = "1.0.0";
pub const SDK_VERSION_MAJOR: u32 = 1;
pub const SDK_VERSION_MINOR: u32 = 0;
pub const SDK_VERSION_PATCH: u32 = 0;

/// ABI version — single u32 checked at DLD load time.
///
/// This is the ONLY version that gates DLD compatibility.
/// It is separate from SDK_VERSION because minor/patch SDK changes
/// do not affect binary compatibility.
///
/// Rules:
///   Bump when:
///     - Device trait method signature changes
///     - DeviceDescriptor struct layout changes
///     - ParamValue enum variants change (add/remove/reorder)
///     - Transaction struct layout changes
///     - DeviceRegistry::register() ABI changes
///   Do NOT bump when:
///     - New optional Device methods with default impls added
///     - New DldError variants added
///     - Built-in device implementations change
///     - Feature-gated modules change
///     - New SDK_VERSION_MINOR/PATCH
pub const HELM_DEVICES_ABI_VERSION: u32 = 1;
```

### Version Embedding in DLDs

Every DLD `.so` exports two symbols:

```rust
// Required: ABI compatibility check (load fails without this)
#[no_mangle]
pub static HELM_DEVICES_ABI_VERSION: u32 = helm_devices::HELM_DEVICES_ABI_VERSION;

// Optional: SDK version diagnostics (logged at load time)
#[no_mangle]
pub static HELM_DEVICES_SDK_VERSION: [u32; 3] = [
    helm_devices::SDK_VERSION_MAJOR,
    helm_devices::SDK_VERSION_MINOR,
    helm_devices::SDK_VERSION_PATCH,
];
```

The `HELM_DEVICES_ABI_VERSION` symbol is required. If absent, `DldError::MissingAbiSymbol` is returned. The `HELM_DEVICES_SDK_VERSION` symbol is optional — if present, it is logged for diagnostic purposes.

---

## 3. ABI Contract

### What the ABI Guarantees

The ABI contract is a binary-level guarantee: **any `.so` compiled against ABI version N will load and interoperate with any host and any other `.so` at ABI version N**, regardless of SDK minor/patch version differences or when each was compiled.

This means:
- A DLD built with SDK 1.0 and a DLD built with SDK 1.5 coexist in the same simulation (both ABI v1).
- A GIC DLD can `InterruptSink::on_assert()` from a UART DLD's `InterruptPin` — both compiled separately, possibly years apart.
- Built-in devices (compiled into the host binary) and dynamically loaded DLDs share the same trait vtable layout and can be wired together by the platform configuration.

**Guaranteed stable across ABI version N:**

| Type | Guarantee |
|------|-----------|
| `Device` trait vtable layout | Method order, signatures, return types |
| `DeviceDescriptor` struct | Field order, field types, struct size |
| `DeviceParams` | All accessor methods, `ParamValue` variant layout |
| `ParamSchema` | Builder methods, validation behavior |
| `Transaction` | Field order, field types, struct size |
| `TransactionAttrs` | Field order, field types |
| `InterruptPin` | `assert()`, `deassert()`, `is_asserted()`, `new()` |
| `InterruptSink` trait | `on_assert(WireId)`, `on_deassert(WireId)` |
| `WireId` | `new(u64)`, `as_u64()`, `From<u32/u64/usize>` |
| `DldError` | Existing variants (new variants may be added without bump) |
| `CharBackend` / `BlockBackend` | Trait method signatures |
| `register_bank!` generated code | `MmioHandler` calling convention, hook signatures |
| `helm_device_register` | `extern "C" fn(*mut DeviceRegistry)` |

### What Is NOT Guaranteed

Anything outside `framework/` — bus protocols, device implementations, platform wiring — may change freely without an ABI bump. DLDs must not `use` types from `bus::`, `generic::`, `soc::`, or `platform::`.

**Runtime behavior** of SDK types (e.g., validation logic in `ParamSchema::validate()`) may change in minor versions as long as the function signature is preserved.

### ABI Version History

| ABI | SDK | Changes |
|-----|-----|---------|
| 1 | 1.0.0 | Initial SDK: Device, Transaction, InterruptPin, DeviceRegistry |

---

## 4. Transaction Type

`Transaction` is the bus-aware counterpart to `Device::read()/write()`. Devices that participate in timed bus hierarchies receive transactions instead of raw offset/size/value calls.

```rust
// framework/transaction.rs

/// A bus transaction carrying full context through the bus hierarchy.
///
/// Created by the CPU or DMA engine, flows through bus bridges,
/// accumulates latency, and arrives at the target device.
///
/// Used in full-system (FS) mode with timing. In functional-emulation
/// (FE/SE) mode, the simplified Device::read()/write() path is used instead.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct Transaction {
    /// Absolute address on the originating bus segment.
    pub addr: u64,

    /// Offset relative to the target device's mapped base.
    /// Set by the address map dispatch before calling the device.
    pub offset: u64,

    /// Access size in bytes: 1, 2, 4, 8, or 16 (for SIMD/LDP/STP).
    pub size: usize,

    /// Data buffer — up to 128 bits for SIMD paired load/store.
    /// For reads, the device fills this buffer.
    /// For writes, the initiator fills this buffer.
    pub data: [u8; 16],

    /// true = write, false = read.
    pub is_write: bool,

    /// Initiator and access attributes.
    pub attrs: TransactionAttrs,

    /// Accumulated stall cycles through the bus hierarchy.
    /// Each bus bridge and device adds its latency contribution.
    /// The timing model reads this after the transaction completes.
    pub stall_cycles: u64,
}

/// Attributes carried by every transaction.
///
/// These describe the initiator and access properties. Bus bridges
/// and devices inspect these to make routing and access-control decisions.
#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct TransactionAttrs {
    /// Initiator ID — CPU core index or DMA engine ID.
    pub initiator_id: u32,

    /// TrustZone secure bit — true for Secure world accesses.
    pub secure: bool,

    /// Cacheability — true for cacheable accesses.
    pub cacheable: bool,

    /// Privilege level — true for privileged (EL1+) accesses.
    pub privileged: bool,
}

impl Transaction {
    /// Create a read transaction.
    pub fn read(addr: u64, size: usize) -> Self {
        Self {
            addr,
            offset: 0,
            size,
            data: [0u8; 16],
            is_write: false,
            attrs: TransactionAttrs::default(),
            stall_cycles: 0,
        }
    }

    /// Create a write transaction with data.
    pub fn write(addr: u64, size: usize, data: &[u8]) -> Self {
        let mut txn = Self {
            addr,
            offset: 0,
            size,
            data: [0u8; 16],
            is_write: true,
            attrs: TransactionAttrs::default(),
            stall_cycles: 0,
        };
        txn.data[..size.min(16)].copy_from_slice(&data[..size.min(16)]);
        txn
    }

    /// Read the data as a u64 (for sub-16-byte accesses).
    pub fn data_u64(&self) -> u64 {
        u64::from_le_bytes(self.data[..8].try_into().unwrap_or([0; 8]))
    }

    /// Set the data from a u64.
    pub fn set_data_u64(&mut self, val: u64) {
        self.data[..8].copy_from_slice(&val.to_le_bytes());
    }
}
```

### Transaction vs Device::read()/write()

| Aspect | `Device::read()/write()` | `Transaction` |
|--------|-------------------------|---------------|
| Used by | FE/SE mode (functional) | FS mode (with timing) |
| Context | offset + size + value only | Full bus context + attributes + stall_cycles |
| Performance | Minimal overhead | One allocation per MMIO access |
| Bus hierarchy | Not modeled | Full bridge traversal with latency accumulation |

Both paths ultimately call the same device logic. The `Device` trait provides the simplified path; devices that also support the transaction path implement a `transact(&mut self, txn: &mut Transaction)` method (optional, with a default that delegates to `read()/write()`).

---

## 5. SDK Prelude

```rust
// framework/sdk.rs

/// Convenience re-exports for out-of-tree DLD authors.
///
/// Usage:
///   use helm_devices::prelude::*;
///
/// This brings in every SDK type needed to write a DLD.
/// It does NOT bring in bus protocols, built-in devices, or platform types.
pub mod prelude {
    // Core trait
    pub use crate::framework::device::{Device, DeviceConfig, DeviceError};

    // Transaction
    pub use crate::framework::transaction::{Transaction, TransactionAttrs};

    // Interrupt model
    pub use crate::framework::interrupt::{InterruptPin, InterruptSink, WireId};

    // Parameters
    pub use crate::framework::params::{
        DeviceParams, ParamField, ParamSchema, ParamType, ParamValue,
    };

    // Registry
    pub use crate::framework::registry::{
        DeviceDescriptor, DeviceRegistry, HostCapability, DldError,
    };

    // Backends
    pub use crate::framework::backend::{CharBackend, BlockBackend};

    // Signal
    pub use crate::framework::signal::SignalInterface;

    // Version constants
    pub use crate::framework::sdk::{
        HELM_DEVICES_ABI_VERSION, SDK_VERSION,
        SDK_VERSION_MAJOR, SDK_VERSION_MINOR, SDK_VERSION_PATCH,
    };

    // Macro
    pub use helm_devices_macros::register_bank;
}
```

---

## 6. DLD Crate Template

### Cargo.toml

```toml
[package]
name = "helm-dld-example"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
helm-devices = { version = "1", default-features = false }
log = "0.4"
```

Key points:
- `crate-type = ["cdylib"]` — produces a `.so` (Linux), `.dylib` (macOS), or `.dll` (Windows).
- `default-features = false` — compiles only the SDK framework, no bus protocols or built-in devices.
- `version = "1"` — depends on SDK major version 1.x.

### src/lib.rs (minimal)

```rust
use helm_devices::prelude::*;

// ── Device implementation ────────────────────────────────────────────────────

pub struct ExampleDevice {
    pub irq_out: InterruptPin,
    counter: u32,
}

impl ExampleDevice {
    pub fn new() -> Self {
        Self {
            irq_out: InterruptPin::new(),
            counter: 0,
        }
    }
}

impl Device for ExampleDevice {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        match offset {
            0x00 => self.counter as u64,
            _ => 0,
        }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        match offset {
            0x00 => {
                self.counter = val as u32;
                if self.counter > 100 {
                    self.irq_out.assert();
                }
            }
            _ => {}
        }
    }

    fn region_size(&self) -> u64 { 4 }
}

// ── DLD ABI exports ──────────────────────────────────────────────────────────

#[no_mangle]
pub static HELM_DEVICES_ABI_VERSION: u32 = helm_devices::HELM_DEVICES_ABI_VERSION;

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
        name: "example_device",
        version: "0.1.0",
        description: "Example threshold counter with interrupt",
        factory: |_params| Ok(Box::new(ExampleDevice::new())),
        param_schema: || ParamSchema::new(),
        python_class_extra: None,
        aliases: &[],
        required_capabilities: &[],
    });
}
```

### Build and Test

```bash
# Build
cargo build --release
ls -la target/release/libhelm_dld_example.so

# Verify ABI symbol
nm -D target/release/libhelm_dld_example.so | grep HELM_DEVICES_ABI_VERSION
# Expected: D HELM_DEVICES_ABI_VERSION

# Load in Python
python3 -c "
import helm_ng
helm_ng.load_dld('target/release/libhelm_dld_example.so')
dev = helm_ng.ExampleDevice()
print(f'Loaded: {dev}')
"
```

---

## 7. DLD Load Sequence

```
DeviceRegistry::load_dld(path: &Path) -> Result<(), DldError>

1.  dlopen(path)
    → Error: DldError::DlopenFailed(msg)
    → On success: libloading::Library handle

2.  Load symbol: HELM_DEVICES_ABI_VERSION (u32)
    → Error: DldError::MissingAbiSymbol
    → On success: read DLD ABI version

3.  Compare: dld_abi == host_abi
    → Mismatch: DldError::AbiVersionMismatch { expected, found }
    → Match: continue

4.  Load symbol: HELM_DEVICES_SDK_VERSION ([u32; 3]) — OPTIONAL
    → Missing: log::debug!("DLD has no SDK version symbol")
    → Present: log::info!("DLD SDK: {}.{}.{}", major, minor, patch)

5.  Load symbol: helm_device_register (extern "C" fn(*mut DeviceRegistry))
    → Error: DldError::MissingRegisterSymbol
    → On success: function pointer ready

6.  Call: helm_device_register(&mut self)
    → DLD calls self.register(descriptor) one or more times
    → Each register() call:
       a. Check name uniqueness → DldError::NameConflict
       b. Check alias uniqueness → DldError::NameConflict
       c. Check host capabilities → DldError::CapabilityMissing
       d. Auto-generate Python class from ParamSchema
       e. Check Python namespace → DldError::PythonNameConflict
       f. Inject Python class into helm_ng module
       g. Store descriptor in registry HashMap

7.  Load symbol: helm_{name}_migrate_checkpoint — OPTIONAL per device
    → Missing: no checkpoint migration support
    → Present: stored for CheckpointManager to call during restore

8.  Store Library handle in self._libs (prevent dlclose)

9.  Return Ok(())
```

### Error Recovery

DLD loading is atomic per-DLD: if any step fails, the entire DLD is rejected. However, if `helm_device_register` successfully registers some devices before failing on a later one, those devices remain registered. The partial registration is acceptable because:
- Each `register()` call is independent.
- The DLD's Library handle is kept alive regardless (to avoid dangling vtable pointers).
- The host logs which devices registered and which failed.

---

## 8. Platform Integration — Wiring DLD Devices

Once a DLD is loaded, its devices are first-class citizens in the Python configuration layer. This section shows the full integration path from `load_dld()` through to a running simulation.

### Integration Model

A platform script (Python) is the sole authority on how devices are composed:

```
load_dld(path)             ← registers device types in DeviceRegistry
  ↓
helm_ng.MyDevice(params)   ← Python class auto-generated from ParamSchema
  ↓                          calls factory → Box<dyn Device>
platform.add_device(...)   ← maps device into address space (base address)
  or
bus.attach_endpoint(...)   ← attaches to a bus controller (PCI slot, I2C addr, SPI CS)
  ↓
platform.wire_interrupt()  ← connects InterruptPin to InterruptSink (IRQ routing)
  ↓
session.run()              ← simulation starts; all devices are peers
```

The device itself never knows its base address, IRQ number, or bus position. All of that is decided by the platform script.

### MMIO Device on a Platform

The simplest case — a device mapped directly into the system address space:

```python
import helm_ng

# Load DLD
helm_ng.load_dld("libhelm_dld_accel.so")

# Create + map
accel = helm_ng.MyAccel(lanes=8)
platform.add_device("accel", 0x0B00_0000, accel)

# Wire interrupt: accel.irq_out → GIC SPI 40
platform.wire_interrupt(accel.irq_out, gic.spi(40))
```

On the Rust side, `platform.add_device()` calls `MemoryMap::map_region()` with the device's `region_size()`. CPU accesses to `0x0B00_0000 + offset` are dispatched to `accel.read(offset, size)` / `accel.write(offset, size, val)`.

### PCI Device on a Bus

PCI devices don't appear directly in the `MemoryMap`. They attach to a `PciBus` which owns the ECAM config window:

```python
# PCI bus already mapped at 0x3000_0000 (256 MiB ECAM)
pci = helm_ng.PciBus(name="pci0")
platform.add_device("pci0", 0x3000_0000, pci)

# DLD PCI endpoint
helm_ng.load_dld("libhelm_dld_nvme.so")
nvme = helm_ng.NvmeController(queues=4, ns_size="1GiB")

# Attach to PCI bus — slot 2, function 0
pci.attach_endpoint(slot=2, function=0, device=nvme)

# NVMe MSI-X → GIC SPI 48 (via PCI bus interrupt routing)
platform.wire_interrupt(nvme.irq_out, gic.spi(48))
```

The PCI bus controller handles config space reads/writes (vendor ID, device ID, BAR sizing). The NVMe DLD implements `PciEndpoint` (which extends `BusDevice`) and provides `config_read()` / `config_write()` / BAR declarations. The DLD only needs to know its register offsets relative to BAR base — the PCI bus and `MemoryMap` handle address translation.

**DLD PCI endpoint implementation:**

```rust
// In the DLD crate:
use helm_devices::prelude::*;
use helm_devices::bus::pci::{PciEndpoint, PciConfigSpace, BarDecl};

pub struct NvmeController {
    config: PciConfigSpace,
    pub irq_out: InterruptPin,
    // ...device state...
}

impl PciEndpoint for NvmeController {
    fn config_read(&self, offset: u8, size: usize) -> u64 {
        self.config.read(offset, size)
    }
    fn config_write(&mut self, offset: u8, size: usize, val: u64) {
        self.config.write(offset, size, val);
    }
    fn vendor_id(&self) -> u16 { 0x8086 }
    fn device_id(&self) -> u16 { 0x5845 }
    fn class_code(&self) -> u16 { 0x0108 } // NVMe
    fn bars(&self) -> &[BarDecl] {
        &[BarDecl::Memory64 { size: 0x4000 }] // BAR0: 16 KiB
    }
}

impl Device for NvmeController {
    // BAR0 MMIO — offsets within the BAR, not absolute addresses
    fn read(&mut self, offset: u64, size: usize) -> u64 { /* ... */ 0 }
    fn write(&mut self, offset: u64, size: usize, val: u64) { /* ... */ }
    fn region_size(&self) -> u64 { 0x4000 }
}
```

Note: `PciEndpoint` is part of `bus::pci/` which is feature-gated (`pci`), NOT part of the stable SDK. A PCI DLD takes a source-level dependency on this module and must recompile if it changes. This is acceptable because PCI config space semantics rarely change.

### I2C / SPI Device on a Bus

Peripheral-bus devices attach to a bus controller, not to the `MemoryMap`:

```python
# I2C controller at MMIO address (8 bytes of control registers)
i2c = helm_ng.I2cBus(name="i2c0")
platform.add_device("i2c0", 0x1000_1000, i2c)

# DLD temperature sensor at I2C address 0x48
helm_ng.load_dld("libhelm_dld_tmp102.so")
sensor = helm_ng.Tmp102(alert_temp_c=85)
i2c.attach_i2c(device=sensor, address=0x48)

# Sensor alert → GPIO pin 7
platform.wire_interrupt(sensor.alert_out, gpio.pin_input(7))
```

The `Tmp102` DLD implements `I2cDevice` (from `bus::i2c/`). The I2C bus controller drives the protocol — START, address, data bytes, STOP — by calling `sensor.on_start()`, `sensor.on_write_byte()`, `sensor.on_read_byte()`, `sensor.on_stop()`. The sensor never sees I2C protocol details or bus-level registers.

### VirtIO Device

VirtIO devices are backends wrapped in a `VirtioMmioTransport`:

```python
# VirtIO MMIO slot at 0x0A00_0000
helm_ng.load_dld("libhelm_dld_virtio_gpu.so")
gpu_backend = helm_ng.VirtioGpuBackend(width=1920, height=1080)
gpu_transport = helm_ng.VirtioMmioTransport(backend=gpu_backend)
platform.add_device("virtio-gpu", 0x0A00_0000, gpu_transport)
platform.wire_interrupt(gpu_transport.irq_out, gic.spi(44))
```

The DLD implements `VirtioDeviceBackend` (from `bus::virtio/`). The transport handles virtqueue setup, feature negotiation, and MMIO register access.

### Cross-DLD Interaction

Devices from different DLDs interact through the standard wiring mechanisms — they never call each other directly:

```python
# DLD A: custom DMA engine
helm_ng.load_dld("libhelm_dld_dma.so")
dma = helm_ng.CustomDma(channels=4)
platform.add_device("dma", 0x0C00_0000, dma)

# DLD B: custom codec
helm_ng.load_dld("libhelm_dld_codec.so")
codec = helm_ng.AudioCodec(sample_rate=48000)
platform.add_device("codec", 0x0D00_0000, codec)

# Wire DMA completion → codec (via named signal, not direct call)
# The DMA engine asserts its irq_out; the platform routes it
platform.wire_interrupt(dma.irq_out, gic.spi(50))
platform.wire_interrupt(codec.irq_out, gic.spi(51))

# DMA reads/writes flow through MemoryMap — the codec's MMIO region
# is accessible to DMA just like RAM. No special cross-DLD API needed.
```

**There is no direct function-call path between DLDs.** Interaction happens through:
1. **Shared memory** — DMA writes to a codec's MMIO region via `MemoryMap`
2. **Interrupts** — one device asserts its `InterruptPin`, which reaches another device's `InterruptSink` through the platform wiring
3. **HelmEventBus** — synchronous named events for observability (not for data transfer)

This isolation is by design: it ensures DLDs can be developed, tested, and deployed independently.

---

## 9. SDK Evolution Policy

### Non-Breaking Changes (no ABI bump)

These changes are safe to make in minor/patch SDK releases:

| Change | Why safe |
|--------|---------|
| Add new optional `Device` method with default impl | Existing vtables still valid; new method slot appended |
| Add new `DldError` variant | DLDs produce errors, don't match on them exhaustively |
| Add new `ParamType` variant | DLDs that don't use it are unaffected |
| Add new fields to `TransactionAttrs` at the end | `#[repr(C)]` — existing field offsets unchanged |
| Change validation logic in `ParamSchema::validate()` | Behavioral, not ABI |
| Bug fixes in SDK types | No signature change |

### Breaking Changes (ABI bump required)

| Change | Why breaking |
|--------|-------------|
| Change `Device::read()` signature | Vtable layout changes |
| Remove a `Device` method | Vtable layout changes |
| Add a required (no-default) `Device` method | Existing DLDs don't implement it |
| Change `DeviceDescriptor` field order or types | Struct layout changes |
| Rename or remove `ParamValue` variants | Pattern match in DLD code breaks |
| Change `Transaction` field types or order | `#[repr(C)]` layout changes |
| Change `helm_device_register` calling convention | DLD entry point breaks |

### Migration Path for Breaking Changes

When a breaking ABI change is necessary:

1. Bump `HELM_DEVICES_ABI_VERSION` from N to N+1.
2. Document the change in the ABI Version History table (§3).
3. Update the SDK prelude if any types are renamed.
4. Device authors recompile against the new SDK and produce new `.so` files.
5. The host logs a clear error message: "ABI version mismatch: host=N+1, DLD=N — recompile DLD against helm-devices SDK N+1".

---

## 10. Fast-Path vs Transaction-Path Device API

Devices support two access paths:

### Fast Path (FE/SE mode)

```rust
trait Device {
    fn read(&mut self, offset: u64, size: usize) -> u64;
    fn write(&mut self, offset: u64, size: usize, val: u64);
}
```

Used when timing accuracy is not needed. Zero allocation, no bus context. This is the hot path for SE-mode simulation.

### Transaction Path (FS mode with timing)

```rust
trait Device {
    /// Handle a bus transaction. Default delegates to read()/write().
    fn transact(&mut self, txn: &mut Transaction) -> Result<(), DeviceError> {
        if txn.is_write {
            self.write(txn.offset, txn.size, txn.data_u64());
        } else {
            let val = self.read(txn.offset, txn.size);
            txn.set_data_u64(val);
        }
        Ok(())
    }
}
```

Used when the bus hierarchy models latency. The device can inspect `txn.attrs` for security/privilege decisions and add to `txn.stall_cycles` for device-internal latency.

**DLD authors** implement `read()/write()` at minimum. Overriding `transact()` is optional and only needed for devices that care about bus attributes or contribute to stall cycle accounting.

---

## 11. SDK Header Generation (cbindgen)

For non-Rust DLD authors (C/C++), a C header is generated from the SDK types:

```bash
# Generate C header from SDK types
cbindgen --config cbindgen.toml --crate helm-devices --output helm_devices.h
```

```toml
# cbindgen.toml
[export]
include = [
    "Transaction", "TransactionAttrs",
    "DeviceDescriptor", "ParamType", "ParamValue", "ParamField",
    "ParamSchema", "DeviceParams",
    "WireId", "HostCapability",
    "HELM_DEVICES_ABI_VERSION", "SDK_VERSION_MAJOR", "SDK_VERSION_MINOR", "SDK_VERSION_PATCH",
]

[export.rename]
"HELM_DEVICES_ABI_VERSION" = "HELM_DEVICES_ABI_VERSION"

language = "C"
```

The generated header allows C/C++ device implementations that export the same `helm_device_register` ABI. The Device trait vtable is not directly expressible in C — C DLDs use a function-pointer struct:

```c
// helm_devices.h (generated excerpt)

typedef struct {
    uint64_t (*read)(void* self, uint64_t offset, size_t size);
    void (*write)(void* self, uint64_t offset, size_t size, uint64_t val);
    uint64_t (*region_size)(void* self);
    void (*signal)(void* self, const char* name, uint64_t val);
    void (*destroy)(void* self);
} HelmDeviceVtable;

typedef struct {
    void* opaque;
    HelmDeviceVtable vtable;
} HelmDeviceFfi;

extern void helm_device_register(void* registry);
extern const uint32_t HELM_DEVICES_ABI_VERSION;
```

The host wraps `HelmDeviceFfi` in a Rust `FfiDevice` newtype that implements `Device` by forwarding to the vtable. This is a Phase 3+ concern — Rust-only DLDs are sufficient for Phase 0–2.

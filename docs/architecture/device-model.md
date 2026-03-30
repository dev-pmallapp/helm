# Device Model

How helm-ng models hardware devices — the Device trait, MMIO dispatch,
interrupt wiring, and dynamically loaded devices (DLDs).

## Design Principles

Two rules govern device modeling in helm-ng:

1. **Device knows no base address** — `AddressMap` owns placement;
   the device registers MMIO ranges via the platform wiring, not
   internally.

2. **Device knows no IRQ number** — `InterruptPin` fires a signal;
   the platform routes it to the interrupt controller at a specific
   SPI number.

These rules ensure devices are reusable across platforms without
modification, matching Simics's interface-based approach and improving
on QEMU's SysBus model where devices often hardcode addresses.

## The Device Trait

Defined in `helm-devices::framework::device`:

```rust
pub trait Device: Send {
    fn name(&self) -> &str;
    fn transact(&mut self, txn: &mut Transaction) -> Result<(), DeviceError>;
    fn reset(&mut self) {}
}
```

`Transaction` carries the access details:

| Field | Type | Description |
|-------|------|-------------|
| `offset` | `u64` | Offset within the device's MMIO region |
| `data` | `&mut [u8]` | Read/write buffer |
| `is_write` | `bool` | Direction |
| `attrs` | `TransactionAttrs` | Access attributes (size, privilege) |

## MMIO Dispatch

In FS mode, `HelmAddressSpace` routes memory accesses:

```text
Guest PA
  │
  ▼
AddressMap::lookup(addr)
  │
  ├── None → FlatMem (RAM access)
  └── Some(device_id, offset) → devices[device_id].transact(offset, data)
```

`AddressMap` maintains a sorted `Vec<(base, size, DeviceId)>` with
binary search lookup. Device registration happens during platform
construction, before simulation starts.

`MmioBus` in `helm-devices::bus::mmio` provides an alternative
dispatch path with named region tracking.

## Interrupt Wiring

### InterruptPin

`InterruptPin` in `helm-devices::framework::interrupt` is a
lightweight signal that connects a device to an interrupt controller:

```text
Device ──► InterruptPin ──► InterruptSink (e.g., GIC)
```

The device calls `pin.assert()` or `pin.deassert()` without knowing
which interrupt controller receives the signal or at which SPI number.
The platform wires the connection during construction.

### WireId

`WireId` identifies a specific interrupt wire in the system. The
interrupt controller maps `WireId` to its internal SPI/PPI numbering.

## Device Registry and DLDs

### Static Devices

Devices compiled into the binary (all `hw/` crates) are available
directly:

| Crate | Devices |
|-------|---------|
| `helm-hw-char` | `Pl011` (PL011 UART) |
| `helm-hw-intc` | `Gicv2Distributor`, `Gicv2CpuInterface`, `Gicv3Distributor`, `Gicv3Redistributor` |
| `helm-hw-timer` | `Sp804` (SP804 dual timer) |
| `helm-hw-rtc` | `Pl031` (PL031 RTC) |
| `helm-hw-dma` | `DmaEngine` |
| `helm-hw-pci` | `PciBus` (ECAM host bridge) |
| `helm-hw-virtio` | `VirtioBackend` implementations: block, console, net, rng |
| `helm-hw-iommu` | `SmmuState` (SMMUv3), AMD-Vi stub, RISC-V IOMMU stub |

### Dynamically Loaded Devices (DLDs)

`DeviceRegistry` in `helm-devices::framework::registry` supports
runtime loading of device `.so` files:

```text
load_dld("path/to/device.so")
  │
  ▼
dlopen → find DeviceDescriptor symbol
  │
  ▼
ABI version check (HELM_DEVICES_ABI_VERSION)
  │
  ▼
Register DeviceDescriptor in DeviceRegistry
```

`DeviceDescriptor` provides:
- Device name and version
- `ParamSchema` describing configuration parameters
- Factory function to create device instances

The ABI version (`HELM_DEVICES_ABI_VERSION`) is the sole compatibility
gate. SDK semantic versioning (`SDK_VERSION`) is informational only.

## Character and Block Backends

`helm-devices::framework::backend` provides I/O backend traits:

| Trait | Purpose | Implementations |
|-------|---------|----------------|
| `CharBackend` | Character I/O (serial, console) | `NullCharBackend`, `BufferCharBackend`, `StdioCharBackend` |
| `BlockBackend` | Block I/O (disk, flash) | Future |

`Pl011` connects to a `CharBackend` for UART→stdout bridging.

## Bus Infrastructure

`helm-devices::bus` provides bus protocol controllers:

| Type | Protocol | Description |
|------|----------|-------------|
| `MmioBus` | MMIO | Named region dispatch |
| `AhbBus` | AMBA AHB | Advanced High-performance Bus |
| `ApbBus` | AMBA APB | Advanced Peripheral Bus |
| `I2cBus` | I2C | Two-wire serial bus |
| `SpiBus` | SPI | Serial Peripheral Interface |
| `HelmEventBus` | Events | Synchronous named event bus |

Bus controllers live in `helm-devices` (not `hw/`) because they are
infrastructure, not concrete devices.

## Comparison

| Aspect | QEMU | gem5 | Simics | helm-ng |
|--------|------|------|--------|---------|
| Device trait | QOM `TypeInfo` + `MemoryRegionOps` | `SimObject` + `Port` | DML interface | `Device` trait + `transact()` |
| IRQ model | `qemu_irq` + GPIO lines | `Port` signaling | Wire interfaces | `InterruptPin` + `InterruptSink` |
| Address ownership | Device knows via SysBus | Port binding | Config-driven | Platform-driven (device is oblivious) |
| Dynamic loading | Built-in QOM | Compile-time only | DML modules (.so) | DLD loading via `DeviceRegistry` |
| Bus hierarchy | SysBus / PCI | Port connections | Bus interfaces | `MmioBus` + AMBA + I2C + SPI |

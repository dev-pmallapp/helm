# helm-engine — LLD: Bus Framework

> Low-level design for the `Bus` trait and concrete bus implementations (PCI, I2C, SPI) in the context of World.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-world.md`](./LLD-world.md) · [`TEST.md`](./TEST.md)

---

## Table of Contents

1. [Design Overview](#1-design-overview)
2. [Bus Trait](#2-bus-trait)
3. [BusDevice Descriptor](#3-busdevice-descriptor)
4. [attach_to_bus() API](#4-attach_to_bus-api)
5. [PCI Bus](#5-pci-bus)
6. [I2C Bus](#6-i2c-bus)
7. [SPI Bus](#7-spi-bus)
8. [Bus-to-MemoryMap Mapping](#8-bus-to-memorymap-mapping)

---

## 1. Design Overview

A bus in helm-ng is a device that multiplexes a set of child devices behind a single MMIO region. The bus sits in the parent `MemoryMap`; the child devices are attached to the bus (not to the `MemoryMap` directly). Transactions to the bus's MMIO region are decoded by the bus controller and forwarded to the appropriate child.

This mirrors the physical reality: PCI devices are not mapped into a CPU's address space individually. Instead, the PCI controller is mapped, and PCI config space (or BAR-based MMIO) is accessed via the controller's registers. The controller decodes the target device from the address or control registers and forwards the transaction.

### Bus Framework vs. Full System

In a full system (`HelmEngine<T>`), buses exist as part of the platform definition and are wired through `World`. In `World`, buses are added via `add_device()` like any other device and mapped via `map_device()`. Child devices are attached to buses via `attach_to_bus()` instead of `map_device()`. The `Bus` trait is the only addition required.

### Bus Crate Location

Bus infrastructure lives in `helm-devices/src/bus/`:

```
helm-devices/src/bus/
├── mod.rs       # pub use Bus, BusDevice, BusAttachError; pub mod pci; pub mod i2c; pub mod spi;
├── pci/
│   ├── mod.rs   # PciBus, PciDevice, PciConfigSpace, PciBar
│   └── types.rs # PCI class codes, vendor IDs, capability types
├── i2c/
│   ├── mod.rs   # I2cBus, I2cDevice trait, I2cTransaction
│   └── types.rs # I2cAddr, I2cDirection, I2cState
└── spi/
    ├── mod.rs   # SpiBus, SpiDevice trait, SpiTransaction
    └── types.rs # SpiMode, SpiFrame
```

`World` uses the bus framework via `attach_to_bus()` in `helm-engine/src/bus_support.rs`.

---

## 2. Bus Trait

```rust
// helm-devices/src/bus/mod.rs

use crate::device::Device;
use crate::world::HelmObjectId;

/// A bus that multiplexes child devices.
///
/// A bus is itself a Device — it sits in the parent MemoryMap and forwards
/// MMIO transactions to child devices via its own decode logic.
///
/// Implementing `Bus` is not required for all devices — only those that are
/// themselves bus controllers (PCI host bridge, I2C master, SPI master).
pub trait Bus: Device {
    /// Attach a child device to this bus.
    ///
    /// The `address` is bus-specific: for I2C it is the 7-bit device address;
    /// for SPI it is the chip-select index; for PCI it is (bus, device, fn) 3-tuple.
    ///
    /// Returns Err if the address is already occupied.
    fn attach(
        &mut self,
        name:    &str,
        device:  Box<dyn BusDevice>,
        address: BusAddress,
    ) -> Result<HelmObjectId, BusAttachError>;

    /// Return descriptors for all attached child devices.
    fn enumerate(&self) -> Vec<BusDeviceDescriptor>;

    /// Perform a bus-protocol read to the child at `address`, offset `offset`.
    ///
    /// This is the bus's internal dispatch — not the same as the top-level
    /// Device::read() which is called by MemoryMap MMIO dispatch.
    fn bus_read(
        &self,
        address: BusAddress,
        offset:  u16,
        size:    usize,
    ) -> Result<u64, BusError>;

    /// Perform a bus-protocol write to the child at `address`, offset `offset`.
    fn bus_write(
        &mut self,
        address: BusAddress,
        offset:  u16,
        size:    usize,
        val:     u64,
    ) -> Result<(), BusError>;
}

/// A device that attaches to a bus (rather than directly to the MemoryMap).
///
/// Smaller interface than Device — no region_size, no direct MemoryMap presence.
pub trait BusDevice: Send {
    fn name(&self) -> &str;
    fn init(&mut self) {}
    fn read_register(&self, reg: u8, size: usize) -> u64;
    fn write_register(&mut self, reg: u8, size: usize, val: u64);
}

/// Bus-specific address type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BusAddress {
    I2c(u8),              // 7-bit I2C address (0x00–0x7F)
    Spi(u8),              // Chip-select index (0, 1, 2, ...)
    Pci { bus: u8, device: u8, function: u8 },
    Custom(u64),          // For extension buses
}

#[derive(Debug)]
pub enum BusAttachError {
    AddressInUse(BusAddress),
    AddressOutOfRange(BusAddress),
    TooManyDevices,
}

#[derive(Debug)]
pub enum BusError {
    NoDevice(BusAddress),
    Timeout,
    Nack,   // I2C NACK
    Arbitration,
}
```

---

## 3. BusDevice Descriptor

```rust
/// Descriptor returned by Bus::enumerate() for each attached device.
pub struct BusDeviceDescriptor {
    pub address:  BusAddress,
    pub name:     String,
    /// Bus-specific metadata
    pub metadata: BusDeviceMeta,
}

pub enum BusDeviceMeta {
    Pci {
        vendor_id:     u16,
        device_id:     u16,
        class_code:    u32,
        subsystem_id:  u16,
        revision_id:   u8,
    },
    I2c {
        address_bits: u8,   // 7 or 10
    },
    Spi {
        max_clock_hz: u32,
        mode:         SpiMode,
    },
    Generic,
}
```

---

## 4. attach_to_bus() API

`World` exposes `attach_to_bus()` as a higher-level method that registers a child device with the bus controller and sets up any necessary event routing.

```rust
// helm-engine/src/bus_support.rs

impl World {
    /// Attach a child device to a bus device.
    ///
    /// The bus device must have been registered via add_device() and
    /// must implement the Bus trait. The child device is NOT mapped
    /// directly into the MemoryMap — it is managed by the bus.
    ///
    /// Returns HelmObjectId for the child device (for later querying).
    ///
    /// Panics:
    ///   - if bus_id is not a registered Bus-implementing device.
    ///   - if elaborate() has already been called.
    pub fn attach_to_bus(
        &mut self,
        bus_id:  HelmObjectId,
        name:    &str,
        device:  Box<dyn BusDevice>,
        address: BusAddress,
    ) -> Result<HelmObjectId, BusAttachError> {
        assert!(!self.elaborated, "attach_to_bus() called after elaborate()");

        let child_id = HelmObjectId(self.next_id);
        self.next_id += 1;

        // Retrieve the bus device and attach the child
        let bus_reg = self.objects.get_mut(&bus_id)
            .unwrap_or_else(|| panic!("attach_to_bus: unknown bus_id {:?}", bus_id));

        let bus = bus_reg.device.as_bus_mut()
            .unwrap_or_else(|| panic!(
                "attach_to_bus: device '{}' does not implement Bus",
                bus_reg.name
            ));

        bus.attach(name, device, address.clone())?;

        Ok(child_id)
    }

    /// Return a reference to a registered Bus device for direct query.
    ///
    /// Returns None if `id` is not registered or does not implement Bus.
    pub fn get_bus(&self, id: HelmObjectId) -> Option<&dyn Bus> {
        self.objects.get(&id)?.device.as_bus()
    }
}
```

---

## 5. PCI Bus

### PciBus Struct

```rust
// helm-devices/src/bus/pci/mod.rs

use std::collections::HashMap;
use crate::device::{Device, SimObject};
use super::{Bus, BusAddress, BusDevice, BusDeviceDescriptor, BusAttachError, BusError};

/// PCI bus controller — models the host bridge and PCI config space mechanism.
///
/// Sits at a mapped MMIO address (the PCI config space window).
/// Attached child devices represent PCI endpoint devices (function 0 of each slot).
///
/// Config space is accessed via the PCI CAM (Configuration Access Mechanism):
///   - ECAM (PCIe): base + ((bus << 20) | (device << 15) | (fn << 12) | offset)
///   - Legacy: two-register approach (CONFIG_ADDRESS / CONFIG_DATA at 0xCF8/0xCFC)
///
/// This implementation uses the ECAM model.
pub struct PciBus {
    name:    String,
    /// Attached PCI endpoint devices, keyed by (bus, device, function)
    devices: HashMap<(u8, u8, u8), Box<dyn PciEndpoint>>,
    /// ECAM window base (set by MemoryMap at map_device time)
    base:    u64,
    /// ECAM window size: 256 MiB for a full hierarchy (256 buses * 32 devices * 8 fn)
    size:    u64,
}

/// A PCI endpoint device — has config space and optional BAR-mapped regions.
pub trait PciEndpoint: BusDevice {
    /// Return PCI config space value at `offset` for `size` bytes.
    fn config_read(&self, offset: u8, size: usize) -> u64;

    /// Write PCI config space at `offset` for `size` bytes.
    fn config_write(&mut self, offset: u8, size: usize, val: u64);

    /// Return vendor ID (config space offset 0x00, bytes 0–1).
    fn vendor_id(&self) -> u16;

    /// Return device ID (config space offset 0x00, bytes 2–3).
    fn device_id(&self) -> u16;

    /// Return class code (config space offset 0x08, bytes 2–3).
    fn class_code(&self) -> u16;
}

impl PciBus {
    /// Create a new PCI bus controller.
    pub fn new(name: impl Into<String>) -> Self {
        PciBus {
            name:    name.into(),
            devices: HashMap::new(),
            base:    0,
            size:    256 * 1024 * 1024,  // 256 MiB ECAM window
        }
    }

    /// Attach a PCI endpoint to a slot.
    ///
    /// `slot` is 0–31 (PCI device number on bus 0).
    /// `function` is 0–7. Most devices use function 0 only.
    pub fn attach_endpoint(
        &mut self,
        slot:     u8,
        function: u8,
        device:   Box<dyn PciEndpoint>,
    ) -> Result<(), BusAttachError> {
        let key = (0u8, slot, function);
        if self.devices.contains_key(&key) {
            return Err(BusAttachError::AddressInUse(
                BusAddress::Pci { bus: 0, device: slot, function }
            ));
        }
        self.devices.insert(key, device);
        Ok(())
    }

    /// Decode ECAM address to (bus, device, function, offset).
    fn decode_ecam(&self, addr: u64) -> (u8, u8, u8, u8) {
        let offset_in_window = addr - self.base;
        let bus      = ((offset_in_window >> 20) & 0xFF) as u8;
        let device   = ((offset_in_window >> 15) & 0x1F) as u8;
        let function = ((offset_in_window >> 12) & 0x07) as u8;
        let reg      = (offset_in_window & 0xFFF) as u8;
        (bus, device, function, reg)
    }
}

impl Device for PciBus {
    /// ECAM config space read.
    fn read(&self, offset: u64, size: usize) -> u64 {
        let addr = self.base + offset;
        let (bus, dev, fun, reg) = self.decode_ecam(addr);
        match self.devices.get(&(bus, dev, fun)) {
            Some(endpoint) => endpoint.config_read(reg, size),
            None           => 0xFFFF_FFFF_FFFF_FFFF,  // PCI: all-Fs for missing devices
        }
    }

    /// ECAM config space write.
    fn write(&mut self, offset: u64, size: usize, val: u64) {
        let addr = self.base + offset;
        let (bus, dev, fun, reg) = self.decode_ecam(addr);
        if let Some(endpoint) = self.devices.get_mut(&(bus, dev, fun)) {
            endpoint.config_write(reg, size, val);
        }
        // Writes to missing devices are silently ignored (PCI spec)
    }

    fn region_size(&self) -> u64 { self.size }
    fn signal(&mut self, _: &str, _: u64) {}
}

impl Bus for PciBus {
    fn attach(
        &mut self,
        _name:   &str,
        device:  Box<dyn BusDevice>,
        address: BusAddress,
    ) -> Result<HelmObjectId, BusAttachError> {
        let BusAddress::Pci { bus, device: slot, function } = address else {
            return Err(BusAttachError::AddressOutOfRange(address));
        };
        // Downcast to PciEndpoint (requires the device to implement it)
        // In practice, attach_endpoint() is used directly; this satisfies the trait
        Err(BusAttachError::TooManyDevices) // placeholder
    }

    fn enumerate(&self) -> Vec<BusDeviceDescriptor> {
        self.devices.iter().map(|((bus, dev, fun), endpoint)| {
            BusDeviceDescriptor {
                address: BusAddress::Pci { bus: *bus, device: *dev, function: *fun },
                name:    endpoint.name().to_string(),
                metadata: BusDeviceMeta::Pci {
                    vendor_id:    endpoint.vendor_id(),
                    device_id:    endpoint.device_id(),
                    class_code:   endpoint.class_code() as u32,
                    subsystem_id: 0,
                    revision_id:  0,
                },
            }
        }).collect()
    }

    fn bus_read(&self, address: BusAddress, offset: u16, size: usize) -> Result<u64, BusError> {
        let BusAddress::Pci { bus, device, function } = address else {
            return Err(BusError::NoDevice(address));
        };
        self.devices.get(&(bus, device, function))
            .map(|ep| ep.config_read(offset as u8, size))
            .ok_or(BusError::NoDevice(BusAddress::Pci { bus, device, function }))
    }

    fn bus_write(&mut self, address: BusAddress, offset: u16, size: usize, val: u64) -> Result<(), BusError> {
        let BusAddress::Pci { bus, device, function } = address else {
            return Err(BusError::NoDevice(address));
        };
        self.devices.get_mut(&(bus, device, function))
            .map(|ep| { ep.config_write(offset as u8, size, val); Ok(()) })
            .unwrap_or(Err(BusError::NoDevice(BusAddress::Pci { bus, device, function })))
    }
}
```

### PCI Enumeration in World (Without CPU)

```rust
#[test]
fn test_pci_enumerate_without_cpu() {
    let mut world = World::new();

    let mut pci_bus = PciBus::new("pci0");
    pci_bus.attach_endpoint(
        0, 0,
        Box::new(VirtioBlkEndpoint::new(VENDOR_VIRTIO, DEVICE_VIRTIO_BLK)),
    ).unwrap();
    pci_bus.attach_endpoint(
        1, 0,
        Box::new(VirtioNetEndpoint::new(VENDOR_VIRTIO, DEVICE_VIRTIO_NET)),
    ).unwrap();

    let bus_id = world.add_device("pci0", Box::new(pci_bus));
    world.map_device(bus_id, 0x3000_0000);
    world.elaborate();

    // Enumerate via Bus trait — no CPU involved
    let bus = world.get_bus(bus_id).expect("pci0 registered");
    let devs = bus.enumerate();

    assert_eq!(devs.len(), 2);

    let vendors: Vec<u16> = devs.iter().filter_map(|d| {
        if let BusDeviceMeta::Pci { vendor_id, .. } = d.metadata { Some(vendor_id) } else { None }
    }).collect();
    assert!(vendors.iter().all(|&v| v == VENDOR_VIRTIO));

    // Enumerate via MMIO config space reads — same as firmware would do
    // Bus 0, Device 0, Function 0, Offset 0 = vendor_id | device_id
    let ecam_base: u64 = 0x3000_0000;
    let bdf_0_0_0 = ecam_base | (0u64 << 20) | (0u64 << 15) | (0u64 << 12);
    let vid_did = world.mmio_read(bdf_0_0_0, 4);
    assert_eq!(vid_did & 0xFFFF, VENDOR_VIRTIO as u64);
}
```

---

## 6. I2C Bus

### I2cBus Struct

```rust
// helm-devices/src/bus/i2c/mod.rs

/// I2C bus master controller.
///
/// Sits in MMIO space. Software writes to its registers to initiate
/// START, address, data, and STOP sequences. Attached I2C devices
/// respond to their 7-bit or 10-bit addresses.
///
/// Register map (example, device-specific):
///   0x00: CONTROL — bit 0 = START, bit 1 = STOP, bit 2 = READ, bit 3 = WRITE
///   0x01: ADDRESS — bits [7:1] = 7-bit address, bit 0 = R/W direction
///   0x02: DATA_TX — byte to transmit
///   0x03: DATA_RX — last received byte (read-only)
///   0x04: STATUS  — bit 0 = BUSY, bit 1 = ACK, bit 2 = NACK, bit 3 = IRQ_PENDING
pub struct I2cBus {
    name:    String,
    devices: HashMap<u8, Box<dyn I2cDevice>>,  // 7-bit address → device
    state:   I2cState,
    // Control/status registers
    control:  u8,
    address:  u8,
    data_tx:  u8,
    data_rx:  u8,
    status:   u8,
    pub irq_out: InterruptPin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum I2cState {
    Idle,
    Start { target_addr: u8, direction: I2cDirection },
    Data  { target_addr: u8, direction: I2cDirection },
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum I2cDirection { Read, Write }

/// An I2C peripheral device — responds to START + address + data sequences.
pub trait I2cDevice: BusDevice {
    /// Called when an I2C transaction starts with this device's address.
    fn on_start(&mut self, direction: I2cDirection);

    /// Called for each data byte written to this device.
    ///
    /// Returns true for ACK, false for NACK.
    fn on_write_byte(&mut self, byte: u8) -> bool;

    /// Called when the master reads a byte from this device.
    ///
    /// Returns the next byte to transmit.
    fn on_read_byte(&mut self) -> u8;

    /// Called on STOP condition.
    fn on_stop(&mut self);
}

impl Device for I2cBus {
    fn read(&self, offset: u64, size: usize) -> u64 {
        let _ = size;
        match offset {
            0 => self.control as u64,
            1 => self.address as u64,
            2 => self.data_tx as u64,
            3 => self.data_rx as u64,
            4 => self.status as u64,
            _ => 0,
        }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        match offset {
            0 => { // CONTROL register
                self.control = val as u8;
                self.process_control();
            }
            1 => { self.address = val as u8; }
            2 => { self.data_tx = val as u8; }
            _ => {}  // STATUS and DATA_RX are read-only
        }
    }

    fn region_size(&self) -> u64 { 8 }
    fn signal(&mut self, _: &str, _: u64) {}
    fn irq_pin_mut(&mut self, name: &str) -> Option<&mut InterruptPin> {
        match name { "irq_out" => Some(&mut self.irq_out), _ => None }
    }
}

impl I2cBus {
    fn process_control(&mut self) {
        if self.control & 0x01 != 0 {
            // START: initiate transaction to target address
            let addr      = (self.address >> 1) & 0x7F;
            let direction = if self.address & 0x01 == 0 { I2cDirection::Write } else { I2cDirection::Read };

            if let Some(dev) = self.devices.get_mut(&addr) {
                dev.on_start(direction);
                self.state  = I2cState::Start { target_addr: addr, direction };
                self.status = 0x01 | 0x02;  // BUSY | ACK
            } else {
                self.status = 0x01 | 0x04;  // BUSY | NACK
            }
            self.irq_out.assert();
        }
        if self.control & 0x08 != 0 {
            // WRITE: send data_tx byte to current target
            if let I2cState::Start { target_addr, .. } | I2cState::Data { target_addr, .. } = self.state {
                if let Some(dev) = self.devices.get_mut(&target_addr) {
                    let ack = dev.on_write_byte(self.data_tx);
                    self.status = if ack { 0x02 } else { 0x04 };  // ACK or NACK
                }
                self.state = I2cState::Data { target_addr, direction: I2cDirection::Write };
            }
        }
        if self.control & 0x04 != 0 {
            // READ: read byte from current target
            if let I2cState::Start { target_addr, .. } | I2cState::Data { target_addr, .. } = self.state {
                if let Some(dev) = self.devices.get_mut(&target_addr) {
                    self.data_rx = dev.on_read_byte();
                    self.status = 0x02;  // ACK
                }
                self.state = I2cState::Data { target_addr, direction: I2cDirection::Read };
            }
        }
        if self.control & 0x02 != 0 {
            // STOP
            if let I2cState::Start { target_addr, .. } | I2cState::Data { target_addr, .. } = self.state {
                if let Some(dev) = self.devices.get_mut(&target_addr) {
                    dev.on_stop();
                }
            }
            self.state  = I2cState::Stop;
            self.status = 0x00;  // idle
            self.irq_out.deassert();
        }
    }
}
```

### I2C Test Example

```rust
#[test]
fn test_i2c_sensor_read() {
    let mut world = World::new();

    let mut i2c = I2cBus::new("i2c0");
    i2c.attach_i2c(Box::new(Tmp102::new()), 0x48).unwrap();  // TMP102 at 0x48

    let i2c_id = world.add_device("i2c0", Box::new(i2c));
    world.map_device(i2c_id, 0x1000_1000);
    world.wire_interrupt(i2c_id, "irq_out", world.irq_sink());
    world.elaborate();

    const I2C_BASE: u64 = 0x1000_1000;
    const CONTROL: u64 = 0;
    const ADDRESS: u64 = 1;
    const DATA_TX: u64 = 2;
    const DATA_RX: u64 = 3;
    const STATUS:  u64 = 4;

    // START + address 0x48 + READ direction (bit 0 = 1)
    world.mmio_write(I2C_BASE + ADDRESS, 1, (0x48 << 1) | 1);
    world.mmio_write(I2C_BASE + CONTROL, 1, 0x01);  // START

    // Allow transaction to complete (I2C is fast in simulation — 1 tick)
    world.advance(10);

    // Verify ACK
    let status = world.mmio_read(I2C_BASE + STATUS, 1);
    assert_eq!(status & 0x04, 0, "NACK from sensor — sensor not responding");

    // READ high byte
    world.mmio_write(I2C_BASE + CONTROL, 1, 0x04);  // READ
    world.advance(10);
    let high = world.mmio_read(I2C_BASE + DATA_RX, 1);

    // READ low byte
    world.mmio_write(I2C_BASE + CONTROL, 1, 0x04);  // READ
    world.advance(10);
    let low = world.mmio_read(I2C_BASE + DATA_RX, 1);

    // STOP
    world.mmio_write(I2C_BASE + CONTROL, 1, 0x02);
    world.advance(10);

    // TMP102 default temperature: 25°C = 0x0C80 >> 4
    let raw_temp = ((high as u16) << 4) | ((low as u16) >> 4);
    assert_eq!(raw_temp, 0x0C8, "TMP102 temperature mismatch: expected 25°C (0x0C8)");
}
```

---

## 7. SPI Bus

### SpiBus Struct

```rust
// helm-devices/src/bus/spi/mod.rs

/// SPI master controller.
///
/// Register map:
///   0x00: TX_DATA  — byte to shift out on MOSI
///   0x01: RX_DATA  — byte shifted in on MISO (read-only)
///   0x02: CONTROL  — bit 0 = start, bit 1 = full-duplex, bit 2 = CS
///   0x03: CS_REG   — chip select mask (bit N = select device N, active low)
///   0x04: STATUS   — bit 0 = busy, bit 1 = rx_valid
pub struct SpiBus {
    name:    String,
    devices: Vec<Option<Box<dyn SpiDevice>>>,  // CS index → device
    tx_data: u8,
    rx_data: u8,
    control: u8,
    cs_reg:  u8,
    status:  u8,
    pub irq_out: InterruptPin,
}

/// An SPI peripheral device.
pub trait SpiDevice: BusDevice {
    /// Called when CS is asserted (active low — CS goes to 0).
    fn on_cs_assert(&mut self);

    /// Called when CS is deasserted.
    fn on_cs_deassert(&mut self);

    /// Full-duplex byte exchange: receive `mosi_byte`, return MISO byte.
    fn transfer_byte(&mut self, mosi_byte: u8) -> u8;
}

impl Device for SpiBus {
    fn read(&self, offset: u64, _size: usize) -> u64 {
        match offset {
            0 => self.tx_data as u64,
            1 => self.rx_data as u64,
            2 => self.control as u64,
            3 => self.cs_reg as u64,
            4 => self.status as u64,
            _ => 0,
        }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        match offset {
            0 => { self.tx_data = val as u8; }
            2 => {
                self.control = val as u8;
                if val & 0x01 != 0 {
                    // START: transfer tx_data to selected devices
                    let cs = self.cs_reg;
                    for (idx, dev) in self.devices.iter_mut().enumerate() {
                        if let Some(dev) = dev {
                            if cs & (1 << idx) == 0 {  // active low
                                self.rx_data = dev.transfer_byte(self.tx_data);
                                self.status = 0x02;  // rx_valid
                                self.irq_out.assert();
                            }
                        }
                    }
                    self.status &= !0x01;  // not busy
                }
            }
            3 => {
                // CS register write — deassert old, assert new
                let old_cs = self.cs_reg;
                let new_cs = val as u8;
                for (idx, dev) in self.devices.iter_mut().enumerate() {
                    if let Some(dev) = dev {
                        let was_selected = old_cs & (1 << idx) == 0;
                        let is_selected  = new_cs & (1 << idx) == 0;
                        if was_selected && !is_selected {
                            dev.on_cs_deassert();
                        } else if !was_selected && is_selected {
                            dev.on_cs_assert();
                        }
                    }
                }
                self.cs_reg = new_cs;
            }
            _ => {}
        }
    }

    fn region_size(&self) -> u64 { 8 }
    fn signal(&mut self, _: &str, _: u64) {}
    fn irq_pin_mut(&mut self, name: &str) -> Option<&mut InterruptPin> {
        match name { "irq_out" => Some(&mut self.irq_out), _ => None }
    }
}
```

### SPI Test Example

```rust
#[test]
fn test_spi_flash_read_jedec_id() {
    let mut world = World::new();

    let mut spi = SpiBus::new("spi0");
    spi.attach_spi(Box::new(SpiNorFlash::new(8 * 1024 * 1024)), 0).unwrap();  // CS 0

    let spi_id = world.add_device("spi0", Box::new(spi));
    world.map_device(spi_id, 0x1000_2000);
    world.elaborate();

    const SPI_BASE: u64 = 0x1000_2000;

    // Assert CS 0 (active low: bit 0 = 0)
    world.mmio_write(SPI_BASE + 3, 1, 0xFE);   // CS_REG: select CS 0

    // Send JEDEC READ ID command (0x9F)
    world.mmio_write(SPI_BASE + 0, 1, 0x9F);   // TX_DATA = 0x9F
    world.mmio_write(SPI_BASE + 2, 1, 0x01);   // CONTROL: start
    world.advance(10);

    // Dummy byte — SPI flash shifts out first byte of ID
    world.mmio_write(SPI_BASE + 0, 1, 0x00);   // TX_DATA = dummy
    world.mmio_write(SPI_BASE + 2, 1, 0x01);   // start
    world.advance(10);
    let mfg_id = world.mmio_read(SPI_BASE + 1, 1);  // RX_DATA

    // Deassert CS
    world.mmio_write(SPI_BASE + 3, 1, 0xFF);   // CS_REG: deselect all

    // Winbond W25Q series manufacturer ID = 0xEF
    assert_eq!(mfg_id, 0xEF, "SPI flash manufacturer ID mismatch");
}
```

---

## 8. Bus-to-MemoryMap Mapping

Buses are mapped into `MemoryMap` like any other device. The bus's MMIO region covers the entire address window that the bus controller exposes to software. Child devices are not individually mapped — they are accessible only via the bus controller's MMIO registers.

### PCI ECAM Mapping

```
MemoryMap:
  0x3000_0000 .. 0x3FFF_FFFF  → PciBus.read() / PciBus.write()
                                 (256 MiB ECAM window)

PCI Config Space Address calculation:
  offset = (bus << 20) | (device << 15) | (function << 12) | register
  physical_addr = 0x3000_0000 + offset
```

### I2C / SPI Controller Register Mapping

```
MemoryMap:
  0x1000_1000 .. 0x1000_1007  → I2cBus (8 bytes, 5 registers)
  0x1000_2000 .. 0x1000_2007  → SpiBus (8 bytes, 5 registers)
```

Child I2C and SPI devices are not in `MemoryMap` at all — they are addressed via the I2C address field or SPI chip-select index in the controller's register protocol.

### Summary

| Bus type | MemoryMap entry | Child devices in MemoryMap? |
|---|---|---|
| PCI (ECAM) | 256 MiB ECAM window | No — config space via ECAM decode |
| I2C controller | 8 bytes (control regs) | No — addressed by 7-bit I2C address |
| SPI controller | 8 bytes (control regs) | No — selected by CS index |

---

*For the `World` struct API, see [`LLD-world.md`](./LLD-world.md). For tests, see [`TEST.md`](./TEST.md).*

//! Device bus — routes MMIO accesses to the correct device by address.
//!
//! A `DeviceBus` is itself a `MemoryMappedDevice`, so buses can nest to
//! model hierarchical topologies (system bus → PCI root → endpoints).
//! Each bus level adds its `bridge_latency` to the total stall cycles.

use super::mmio::{DeviceAccess, MemoryMappedDevice};
use helm_core::types::Addr;
use helm_core::HelmResult;

/// A device mapped at a specific base address on the bus.
pub struct DeviceSlot {
    pub name: String,
    pub base: Addr,
    pub size: u64,
    pub device: Box<dyn MemoryMappedDevice>,
}

/// Hierarchical device bus that dispatches MMIO reads/writes to devices.
///
/// Because `DeviceBus` implements [`MemoryMappedDevice`], buses can be
/// attached to other buses to form a tree:
///
/// ```text
/// system_bus (0 latency)
///   ├── uart @ 0x4000_0000
///   └── pci_bus @ 0xC000_0000 (1 cycle crossing)
///       ├── gpu @ 0x0000
///       └── nic @ 0x1000
/// ```
///
/// A CPU access to `0xC000_1000` traverses system → PCI (1 cycle) → NIC.
pub struct DeviceBus {
    name: String,
    slots: Vec<DeviceSlot>,
    /// Address window this bus covers.
    window_size: u64,
    /// Stall cycles added per bus crossing (bridge/protocol overhead).
    bridge_latency: u64,
}

impl DeviceBus {
    /// Create a bus with custom name, window size, and bridge latency.
    pub fn new(name: impl Into<String>, window_size: u64, bridge_latency: u64) -> Self {
        Self {
            name: name.into(),
            slots: Vec::new(),
            window_size,
            bridge_latency,
        }
    }

    /// System bus: 0 crossing latency, full 64-bit address space.
    pub fn system() -> Self {
        Self::new("system", u64::MAX, 0)
    }

    /// PCI root complex with configurable window size and 1-cycle crossing.
    pub fn pci(name: impl Into<String>, window_size: u64) -> Self {
        Self::new(name, window_size, 1)
    }

    /// USB host controller: 10-cycle protocol overhead, 16 MB window.
    pub fn usb(name: impl Into<String>) -> Self {
        Self::new(name, 0x100_0000, 10)
    }

    /// Map a device at the given base address.
    pub fn attach(
        &mut self,
        name: impl Into<String>,
        base: Addr,
        device: Box<dyn MemoryMappedDevice>,
    ) {
        let size = device.region_size();
        self.slots.push(DeviceSlot {
            name: name.into(),
            base,
            size,
            device,
        });
    }

    /// Read from the bus. Routes to the device whose region contains `addr`.
    /// Adds `bridge_latency` to the returned stall cycles.
    pub fn bus_read(&mut self, addr: Addr, size: usize) -> HelmResult<DeviceAccess> {
        for slot in &mut self.slots {
            if addr >= slot.base && addr < slot.base + slot.size {
                let offset = addr - slot.base;
                let mut access = slot.device.read(offset, size)?;
                access.stall_cycles += self.bridge_latency;
                return Ok(access);
            }
        }
        Err(helm_core::HelmError::Memory {
            addr,
            reason: "no device mapped at this address".into(),
        })
    }

    /// Write to the bus. Adds `bridge_latency` to the returned stall.
    pub fn bus_write(&mut self, addr: Addr, size: usize, value: u64) -> HelmResult<u64> {
        for slot in &mut self.slots {
            if addr >= slot.base && addr < slot.base + slot.size {
                let offset = addr - slot.base;
                let stall = slot.device.write(offset, size, value)?;
                return Ok(stall + self.bridge_latency);
            }
        }
        Err(helm_core::HelmError::Memory {
            addr,
            reason: "no device mapped at this address".into(),
        })
    }

    /// List all attached devices.
    pub fn devices(&self) -> Vec<(&str, Addr, u64)> {
        self.slots
            .iter()
            .map(|s| (s.name.as_str(), s.base, s.size))
            .collect()
    }

    /// Reset all devices.
    pub fn reset_all(&mut self) -> HelmResult<()> {
        for slot in &mut self.slots {
            slot.device.reset()?;
        }
        Ok(())
    }

    /// Bridge latency for this bus level.
    pub fn bridge_latency(&self) -> u64 {
        self.bridge_latency
    }

    /// Check if an address maps to any device on this bus.
    pub fn contains(&self, addr: Addr) -> bool {
        self.slots
            .iter()
            .any(|s| addr >= s.base && addr < s.base + s.size)
    }
}

impl Default for DeviceBus {
    fn default() -> Self {
        Self::system()
    }
}

// DeviceBus is itself a MemoryMappedDevice, enabling hierarchical nesting.
impl MemoryMappedDevice for DeviceBus {
    fn read(&mut self, offset: Addr, size: usize) -> HelmResult<DeviceAccess> {
        self.bus_read(offset, size)
    }

    fn write(&mut self, offset: Addr, size: usize, value: u64) -> HelmResult<u64> {
        self.bus_write(offset, size, value)
    }

    fn region_size(&self) -> u64 {
        self.window_size
    }

    fn device_name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.reset_all()
    }
}

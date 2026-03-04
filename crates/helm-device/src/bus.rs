//! Device bus — routes MMIO accesses to the correct device by address.

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

/// System bus that dispatches MMIO reads/writes to devices.
pub struct DeviceBus {
    slots: Vec<DeviceSlot>,
}

impl DeviceBus {
    pub fn new() -> Self {
        Self { slots: Vec::new() }
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
    pub fn read(&mut self, addr: Addr, size: usize) -> HelmResult<DeviceAccess> {
        for slot in &mut self.slots {
            if addr >= slot.base && addr < slot.base + slot.size {
                let offset = addr - slot.base;
                return slot.device.read(offset, size);
            }
        }
        Err(helm_core::HelmError::Memory {
            addr,
            reason: "no device mapped at this address".into(),
        })
    }

    /// Write to the bus.
    pub fn write(&mut self, addr: Addr, size: usize, value: u64) -> HelmResult<u64> {
        for slot in &mut self.slots {
            if addr >= slot.base && addr < slot.base + slot.size {
                let offset = addr - slot.base;
                return slot.device.write(offset, size, value);
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
}

impl Default for DeviceBus {
    fn default() -> Self {
        Self::new()
    }
}

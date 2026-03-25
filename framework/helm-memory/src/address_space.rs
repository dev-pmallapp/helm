//! RAM + MMIO device dispatch composed over [`FlatMem`].

use helm_core::{AccessType, MemFault, MemInterface};
use helm_devices::{AddressMap, Device};

use crate::FlatMem;

/// System memory: RAM with MMIO device dispatch.
pub struct HelmAddressSpace {
    /// Backing RAM (sparse page table).
    pub ram: FlatMem,
    /// Address-to-device routing table.
    pub address_map: AddressMap,
    /// Indexed by `DeviceId.0` — matches the ID used in `AddressMap`.
    pub devices: Vec<Box<dyn Device>>,
    /// RAM base address — used for the fast-path range check.
    ram_base: u64,
    /// RAM size in bytes — used for the fast-path range check.
    ram_size: u64,
}

impl HelmAddressSpace {
    /// Create a new system memory with backing RAM.
    pub fn new(ram: FlatMem) -> Self {
        let ram_base = ram.base;
        let ram_size = ram.size_bytes;
        Self {
            ram,
            address_map: AddressMap::new(),
            devices: Vec::new(),
            ram_base,
            ram_size,
        }
    }

    /// Add a device and map it at `base`. Returns the device index.
    pub fn add_device(&mut self, base: u64, device: Box<dyn Device>) -> usize {
        let idx = self.devices.len();
        let size = device.region_size();
        self.devices.push(device);

        use helm_devices::framework::address_map::MappedRegion;
        use helm_devices::DeviceId;
        self.address_map.map_region(MappedRegion {
            device_id: DeviceId(idx as u64),
            base,
            size,
            priority: 0,
        });
        self.address_map.commit();
        idx
    }

    /// Get mutable reference to a device by index.
    pub fn device_mut(&mut self, idx: usize) -> &mut dyn Device {
        self.devices[idx].as_mut()
    }
}

impl MemInterface for HelmAddressSpace {
    fn read(&mut self, addr: u64, size: usize, ty: AccessType) -> Result<u64, MemFault> {
        if addr.wrapping_sub(self.ram_base) < self.ram_size {
            return self.ram.read(addr, size, ty);
        }
        if let Some(entry) = self.address_map.lookup(addr) {
            let offset = addr - entry.base + entry.offset_in_device;
            let dev = &mut self.devices[entry.device_id.0 as usize];
            Ok(dev.read(offset, size))
        } else {
            self.ram.read(addr, size, ty)
        }
    }

    fn write(&mut self, addr: u64, size: usize, val: u64, ty: AccessType) -> Result<(), MemFault> {
        if addr.wrapping_sub(self.ram_base) < self.ram_size {
            return self.ram.write(addr, size, val, ty);
        }
        if let Some(entry) = self.address_map.lookup(addr) {
            let offset = addr - entry.base + entry.offset_in_device;
            let dev = &mut self.devices[entry.device_id.0 as usize];
            dev.write(offset, size, val);
            Ok(())
        } else {
            self.ram.write(addr, size, val, ty)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockDevice {
        last_read_offset: u64,
        last_write_offset: u64,
        last_write_val: u64,
    }

    impl MockDevice {
        fn new() -> Self {
            Self {
                last_read_offset: u64::MAX,
                last_write_offset: u64::MAX,
                last_write_val: 0,
            }
        }
    }

    impl Device for MockDevice {
        fn read(&mut self, offset: u64, _size: usize) -> u64 {
            self.last_read_offset = offset;
            0xDEAD_BEEF
        }
        fn write(&mut self, offset: u64, _size: usize, val: u64) {
            self.last_write_offset = offset;
            self.last_write_val = val;
        }
        fn region_size(&self) -> u64 {
            0x1000
        }
    }

    #[test]
    fn device_dispatch_read() {
        let ram = FlatMem::new(0, 0);
        let mut sys = HelmAddressSpace::new(ram);
        sys.add_device(0x0900_0000, Box::new(MockDevice::new()));

        let val = sys.read(0x0900_0010, 4, AccessType::Load).unwrap();
        assert_eq!(val, 0xDEAD_BEEF);
    }

    #[test]
    fn device_dispatch_write() {
        let ram = FlatMem::new(0, 0);
        let mut sys = HelmAddressSpace::new(ram);
        let idx = sys.add_device(0x0900_0000, Box::new(MockDevice::new()));

        sys.write(0x0900_0020, 4, 0x42, AccessType::Store).unwrap();

        let dev = sys.device_mut(idx);
        let mock = unsafe { &*(dev as *const dyn Device as *const MockDevice) };
        assert_eq!(mock.last_write_offset, 0x20);
        assert_eq!(mock.last_write_val, 0x42);
    }

    #[test]
    fn unmapped_falls_to_ram() {
        let ram = FlatMem::new(0, 0);
        let mut sys = HelmAddressSpace::new(ram);
        sys.add_device(0x0900_0000, Box::new(MockDevice::new()));

        sys.write(0x4000_0000, 4, 0x1234, AccessType::Store)
            .unwrap();
        let val = sys.read(0x4000_0000, 4, AccessType::Load).unwrap();
        assert_eq!(val, 0x1234);
    }
}

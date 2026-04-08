//! RAM + MMIO device dispatch composed over [`FlatMem`].
//!
//! This is the active physical-memory surface used by the runtime today:
//! RAM accesses go through [`FlatMem`] and MMIO dispatch goes through
//! [`helm_devices::AddressMap`]. Phase 3 memory-surface work should target this
//! type (or shared adapters around it) until the experimental `MemoryMap`
//! grows complete alias/container/remap semantics.

use std::collections::HashMap;

use helm_core::{AccessType, MemFault, MemInterface};
use helm_devices::{AddressMap, Device};

use crate::FlatMem;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PciBarRegionKey {
    bus: u8,
    device: u8,
    function: u8,
    bar_idx: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PciBarRegion {
    device_idx: usize,
    base: u64,
    size: u64,
    priority: i32,
}

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
    /// PCI BAR-backed MMIO regions tracked on the live address-space surface.
    pci_bar_regions: HashMap<PciBarRegionKey, PciBarRegion>,
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
            pci_bar_regions: HashMap::new(),
        }
    }

    /// Add a device and map it at `base`. Returns the device index.
    pub fn add_device(&mut self, base: u64, device: Box<dyn Device>) -> usize {
        let idx = self.devices.len();
        let size = device.region_size();
        self.devices.push(device);

        let _ = self.queue_device_region_map(idx, base, size, 0);
        self.commit_device_regions();
        idx
    }

    /// Get mutable reference to a device by index.
    pub fn device_mut(&mut self, idx: usize) -> &mut dyn Device {
        self.devices[idx].as_mut()
    }

    /// Try to view a mapped device as a concrete type.
    pub fn device_as_mut<T: Device + 'static>(&mut self, idx: usize) -> Option<&mut T> {
        let dev: &mut dyn Device = self.devices.get_mut(idx)?.as_mut();
        let any: &mut dyn std::any::Any = dev;
        any.downcast_mut::<T>()
    }

    /// Run a closure against a concrete mapped device if the type matches.
    pub fn with_device_mut<T: Device + 'static, R>(
        &mut self,
        idx: usize,
        f: impl FnOnce(&mut T) -> R,
    ) -> Option<R> {
        let dev = self.device_as_mut::<T>(idx)?;
        Some(f(dev))
    }

    /// Queue a device-backed MMIO region mapping.
    ///
    /// Mutations are not visible until [`Self::commit_device_regions`] is
    /// called. This lets the active physical-memory owner express the same
    /// batched remap semantics as [`helm_devices::AddressMap`].
    pub fn queue_device_region_map(
        &mut self,
        idx: usize,
        base: u64,
        size: u64,
        priority: i32,
    ) -> bool {
        if idx >= self.devices.len() {
            return false;
        }

        use helm_devices::framework::address_map::MappedRegion;
        use helm_devices::DeviceId;
        self.address_map.map_region(MappedRegion {
            device_id: DeviceId(idx as u64),
            base,
            size,
            priority,
        });
        true
    }

    /// Queue removal of a previously mapped device region.
    ///
    /// Mutations are not visible until [`Self::commit_device_regions`] is
    /// called.
    pub fn queue_device_region_unmap(&mut self, idx: usize, base: u64) -> bool {
        if idx >= self.devices.len() {
            return false;
        }

        use helm_devices::DeviceId;
        self.address_map.unmap_region(DeviceId(idx as u64), base);
        true
    }

    /// Apply all queued MMIO mapping mutations.
    pub fn commit_device_regions(&mut self) {
        self.address_map.commit();
    }

    /// Atomically remap a device region from `old_base` to `new_base`.
    ///
    /// This is the current authoritative remap path for the live RAM+MMIO
    /// surface. It queues the unmap and map together, then commits once so no
    /// intermediate partially-updated view becomes visible.
    pub fn remap_device_region(
        &mut self,
        idx: usize,
        old_base: u64,
        new_base: u64,
        size: u64,
        priority: i32,
    ) -> bool {
        if !self.queue_device_region_unmap(idx, old_base) {
            return false;
        }
        if !self.queue_device_region_map(idx, new_base, size, priority) {
            return false;
        }
        self.commit_device_regions();
        true
    }

    /// Register a PCI BAR-backed region on the live address-space surface.
    ///
    /// This records the authoritative mapping metadata needed to project
    /// future BAR remap commands onto `HelmAddressSpace` without requiring the
    /// framework layer to depend on a concrete PCI bus type.
    pub fn register_pci_bar_region(
        &mut self,
        bus: u8,
        device: u8,
        function: u8,
        bar_idx: u8,
        device_idx: usize,
        base: u64,
        size: u64,
        priority: i32,
    ) -> bool {
        if device_idx >= self.devices.len() {
            return false;
        }

        let key = PciBarRegionKey {
            bus,
            device,
            function,
            bar_idx,
        };
        self.pci_bar_regions.insert(
            key,
            PciBarRegion {
                device_idx,
                base,
                size,
                priority,
            },
        );
        true
    }

    /// Apply a PCI BAR remap against the authoritative live MMIO surface.
    ///
    /// Returns `false` if no BAR region is registered for the given BDF/BAR or
    /// if the caller's `old_base` no longer matches the current authoritative
    /// base (stale remap command).
    pub fn apply_pci_bar_remap(
        &mut self,
        bus: u8,
        device: u8,
        function: u8,
        bar_idx: u8,
        old_base: u64,
        new_base: u64,
    ) -> bool {
        let key = PciBarRegionKey {
            bus,
            device,
            function,
            bar_idx,
        };
        let Some(mut region) = self.pci_bar_regions.get(&key).copied() else {
            return false;
        };
        if region.base != old_base {
            return false;
        }
        if !self.remap_device_region(
            region.device_idx,
            region.base,
            new_base,
            region.size,
            region.priority,
        ) {
            return false;
        }
        region.base = new_base;
        self.pci_bar_regions.insert(key, region);
        true
    }

    /// Read an arbitrary byte range from guest physical memory.
    ///
    /// This intentionally uses byte accesses so MMIO-visible side effects match
    /// the currently defined device contract. Wider DMA/bulk transaction shapes
    /// can be introduced later once the framework has one authoritative
    /// transaction-width story.
    pub fn read_bytes(&mut self, addr: u64, buf: &mut [u8]) -> Result<(), MemFault> {
        for (offset, byte) in buf.iter_mut().enumerate() {
            *byte = self.read(addr + offset as u64, 1, AccessType::Load)? as u8;
        }
        Ok(())
    }

    /// Write an arbitrary byte range to guest physical memory.
    ///
    /// This mirrors [`Self::read_bytes`] and keeps device-visible behavior
    /// explicit until a wider DMA transaction contract exists.
    pub fn write_bytes(&mut self, addr: u64, data: &[u8]) -> Result<(), MemFault> {
        for (offset, byte) in data.iter().enumerate() {
            self.write(addr + offset as u64, 1, u64::from(*byte), AccessType::Store)?;
        }
        Ok(())
    }
}

impl MemInterface for HelmAddressSpace {
    #[inline]
    fn read(&mut self, addr: u64, size: usize, ty: AccessType) -> Result<u64, MemFault> {
        if addr.wrapping_sub(self.ram_base) < self.ram_size {
            return self.ram.read(addr, size, ty);
        }
        if let Some(entry) = self.address_map.lookup(addr) {
            let offset = addr - entry.base + entry.offset_in_device;
            let dev_idx = entry.device_id.0 as usize;
            debug_assert!(
                dev_idx < self.devices.len(),
                "HelmAddressSpace::read: device_id {} out of bounds (have {})",
                dev_idx,
                self.devices.len()
            );
            let dev = &mut self.devices[dev_idx];
            Ok(dev.read(offset, size))
        } else {
            self.ram.read(addr, size, ty)
        }
    }

    #[inline]
    fn write(&mut self, addr: u64, size: usize, val: u64, ty: AccessType) -> Result<(), MemFault> {
        if addr.wrapping_sub(self.ram_base) < self.ram_size {
            return self.ram.write(addr, size, val, ty);
        }
        if let Some(entry) = self.address_map.lookup(addr) {
            let offset = addr - entry.base + entry.offset_in_device;
            let dev_idx = entry.device_id.0 as usize;
            debug_assert!(
                dev_idx < self.devices.len(),
                "HelmAddressSpace::write: device_id {} out of bounds (have {})",
                dev_idx,
                self.devices.len()
            );
            let dev = &mut self.devices[dev_idx];
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

    struct OtherDevice;

    impl Device for OtherDevice {
        fn read(&mut self, _offset: u64, _size: usize) -> u64 {
            0
        }

        fn write(&mut self, _offset: u64, _size: usize, _val: u64) {}

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

        let mock = sys.device_as_mut::<MockDevice>(idx).unwrap();
        assert_eq!(mock.last_write_offset, 0x20);
        assert_eq!(mock.last_write_val, 0x42);
    }

    #[test]
    fn typed_device_access_rejects_wrong_type() {
        let ram = FlatMem::new(0, 0);
        let mut sys = HelmAddressSpace::new(ram);
        let idx = sys.add_device(0x0900_0000, Box::new(OtherDevice));

        assert!(sys.device_as_mut::<MockDevice>(idx).is_none());
    }

    #[test]
    fn closure_typed_device_access_updates_device() {
        let ram = FlatMem::new(0, 0);
        let mut sys = HelmAddressSpace::new(ram);
        let idx = sys.add_device(0x0900_0000, Box::new(MockDevice::new()));

        let last = sys.with_device_mut::<MockDevice, _>(idx, |dev| {
            dev.last_write_val = 0x55;
            dev.last_write_val
        });

        assert_eq!(last, Some(0x55));
        assert_eq!(
            sys.device_as_mut::<MockDevice>(idx).unwrap().last_write_val,
            0x55
        );
    }

    #[test]
    fn bulk_byte_helpers_round_trip_ram() {
        let ram = FlatMem::new(0x4000_0000, 0x1000);
        let mut sys = HelmAddressSpace::new(ram);

        sys.write_bytes(0x4000_0010, &[1, 2, 3, 4, 5]).unwrap();

        let mut buf = [0u8; 5];
        sys.read_bytes(0x4000_0010, &mut buf).unwrap();
        assert_eq!(buf, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn queued_mapping_is_invisible_until_commit() {
        let ram = FlatMem::new(0, 0);
        let mut sys = HelmAddressSpace::new(ram);
        let idx = sys.add_device(0x0900_0000, Box::new(MockDevice::new()));

        assert!(sys.queue_device_region_map(idx, 0x0A00_0000, 0x1000, 0));
        assert!(sys.address_map.lookup(0x0A00_0000).is_none());

        sys.commit_device_regions();
        let entry = sys.address_map.lookup(0x0A00_0000).unwrap();
        assert_eq!(entry.device_id.0, idx as u64);
        assert_eq!(entry.base, 0x0A00_0000);
    }

    #[test]
    fn remap_device_region_moves_live_mmio_window() {
        let ram = FlatMem::new(0, 0);
        let mut sys = HelmAddressSpace::new(ram);
        let idx = sys.add_device(0x0900_0000, Box::new(MockDevice::new()));

        sys.write(0x0900_0020, 4, 0x55, AccessType::Store).unwrap();
        assert_eq!(
            sys.device_as_mut::<MockDevice>(idx)
                .unwrap()
                .last_write_offset,
            0x20
        );

        assert!(sys.remap_device_region(idx, 0x0900_0000, 0x0A00_0000, 0x1000, 0));
        assert!(sys.address_map.lookup(0x0900_0020).is_none());
        assert!(sys.address_map.lookup(0x0A00_0020).is_some());

        sys.write(0x0A00_0020, 4, 0x77, AccessType::Store).unwrap();
        let mock = sys.device_as_mut::<MockDevice>(idx).unwrap();
        assert_eq!(mock.last_write_offset, 0x20);
        assert_eq!(mock.last_write_val, 0x77);
    }

    #[test]
    fn pci_bar_remap_updates_registered_live_region() {
        let ram = FlatMem::new(0, 0);
        let mut sys = HelmAddressSpace::new(ram);
        let idx = sys.add_device(0x0A00_0000, Box::new(MockDevice::new()));

        assert!(sys.register_pci_bar_region(0, 1, 0, 0, idx, 0x0A00_0000, 0x1000, 0));
        assert!(sys.apply_pci_bar_remap(0, 1, 0, 0, 0x0A00_0000, 0x0B00_0000));

        assert!(sys.address_map.lookup(0x0A00_0000).is_none());
        assert!(sys.address_map.lookup(0x0B00_0000).is_some());

        sys.write(0x0B00_0024, 4, 0xAB, AccessType::Store).unwrap();
        let mock = sys.device_as_mut::<MockDevice>(idx).unwrap();
        assert_eq!(mock.last_write_offset, 0x24);
        assert_eq!(mock.last_write_val, 0xAB);
    }

    #[test]
    fn pci_bar_remap_rejects_stale_old_base() {
        let ram = FlatMem::new(0, 0);
        let mut sys = HelmAddressSpace::new(ram);
        let idx = sys.add_device(0x0A00_0000, Box::new(MockDevice::new()));

        assert!(sys.register_pci_bar_region(0, 2, 0, 1, idx, 0x0A00_0000, 0x1000, 0));
        assert!(!sys.apply_pci_bar_remap(0, 2, 0, 1, 0x0A10_0000, 0x0B00_0000));
        assert!(sys.address_map.lookup(0x0A00_0000).is_some());
        assert!(sys.address_map.lookup(0x0B00_0000).is_none());
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

//! PCI ECAM host bridge -- bus/device/function decode and config space dispatch.
//!
//! Models a PCI Express Enhanced Configuration Access Mechanism (ECAM) host
//! bridge. The bridge occupies a contiguous MMIO region (default 256 MiB for
//! a full PCI hierarchy) and decodes the ECAM address to extract the target
//! bus/device/function and register offset.
//!
//! Endpoint devices implement [`PciEndpoint`] and provide their own
//! [`PciConfigSpace`](config::PciConfigSpace).

pub mod config;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use helm_devices::Device;

// ── Bdf ─────────────────────────────────────────────────────────────────────

/// PCI Bus/Device/Function address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bdf {
    /// Bus number (0..255).
    pub bus: u8,
    /// Device (slot) number (0..31).
    pub device: u8,
    /// Function number (0..7).
    pub function: u8,
}

impl Bdf {
    /// Create a new BDF address.
    pub fn new(bus: u8, device: u8, function: u8) -> Self {
        Self {
            bus,
            device,
            function,
        }
    }

    /// Decode an ECAM offset (relative to the ECAM window base) into a BDF
    /// and register offset within the 4 KiB config space.
    pub fn from_ecam_offset(offset: u64) -> (Bdf, u16) {
        let bus = ((offset >> 20) & 0xFF) as u8;
        let device = ((offset >> 15) & 0x1F) as u8;
        let function = ((offset >> 12) & 0x07) as u8;
        let reg_offset = (offset & 0xFFF) as u16;
        (
            Bdf {
                bus,
                device,
                function,
            },
            reg_offset,
        )
    }
}

// ── PciEndpoint ─────────────────────────────────────────────────────────────

/// A PCI endpoint device that responds to config space reads and writes.
///
/// Devices implement this trait to provide their PCI identity and handle
/// configuration space access. The [`PciBus`] dispatches ECAM transactions
/// to the endpoint matching the decoded BDF.
pub trait PciEndpoint: Send {
    /// Read `size` bytes from config space at `offset`.
    fn config_read(&self, offset: u16, size: usize) -> u32;

    /// Write `size` bytes to config space at `offset`.
    fn config_write(&mut self, offset: u16, size: usize, val: u32);

    /// Return the PCI vendor ID.
    fn vendor_id(&self) -> u16;

    /// Return the PCI device ID.
    fn device_id(&self) -> u16;

    /// Return the PCI class code (24 bits: class/subclass/prog-if).
    fn class_code(&self) -> u32;

    /// Return the currently programmed base address for `bar_index`, if any.
    fn bar_base(&self, _bar_index: u8) -> Option<u64> {
        None
    }

    /// Return the declared size of `bar_index`, if any.
    fn bar_size(&self, _bar_index: u8) -> Option<u64> {
        None
    }
}

/// PCI endpoint implementing a single BAR0-backed RAM window.
///
/// This is the minimal concrete endpoint/device pair used by higher-level
/// configuration layers to build a PCI-visible function without requiring a
/// transport-specific endpoint model.
pub struct PciRamBarEndpoint {
    config: Mutex<config::PciConfigSpace>,
    vendor: u16,
    device: u16,
    class: u32,
}

/// Device side of [`PciRamBarEndpoint`]'s BAR0 memory window.
pub struct PciRamBarDevice {
    bytes: Arc<Mutex<Box<[u8]>>>,
    size: u64,
}

/// Build a single-function PCI endpoint paired with a BAR0-backed MMIO device.
///
/// The returned endpoint is attached to a [`PciBus`], while the returned
/// device is mapped into the platform's MMIO attachment window and registered
/// as the authoritative BAR0 region owner.
pub fn build_pci_bar0_endpoint(
    vendor_id: u16,
    device_id: u16,
    class_code: u32,
    base: u64,
    size: u64,
) -> Result<PciRamBarEndpoint, String> {
    let size_u32 = u32::try_from(size)
        .map_err(|_| format!("PCI BAR size {size:#x} exceeds 32-bit BAR support"))?;
    if size_u32 < 16 || !size_u32.is_power_of_two() {
        return Err(format!(
            "PCI BAR size must be a power of two and >= 16 bytes, got {size:#x}"
        ));
    }
    let base_u32 = u32::try_from(base)
        .map_err(|_| format!("PCI BAR base {base:#x} exceeds 32-bit BAR support"))?;

    let mut cfg = config::PciConfigSpace::new(vendor_id, device_id, class_code, 0x00);
    cfg.set_bar_size(0, size_u32);
    cfg.write(0x10, 4, base_u32);

    Ok(PciRamBarEndpoint {
        config: Mutex::new(cfg),
        vendor: vendor_id,
        device: device_id,
        class: class_code,
    })
}

/// Build a single-function PCI endpoint paired with a BAR0-backed MMIO device.
///
/// The returned endpoint is attached to a [`PciBus`], while the returned
/// device is mapped into the platform's MMIO attachment window and registered
/// as the authoritative BAR0 region owner.
pub fn build_pci_ram_bar_pair(
    vendor_id: u16,
    device_id: u16,
    class_code: u32,
    base: u64,
    size: u64,
) -> Result<(PciRamBarEndpoint, PciRamBarDevice), String> {
    let bytes = Arc::new(Mutex::new(vec![0u8; size as usize].into_boxed_slice()));
    Ok((
        build_pci_bar0_endpoint(vendor_id, device_id, class_code, base, size)?,
        PciRamBarDevice { bytes, size },
    ))
}

// ── PciBus ──────────────────────────────────────────────────────────────────

// ── RemapCommand ──────────────────────────────────────────────────────────

/// A pending BAR re-programming command.
///
/// Queued when a guest writes to a BAR register. The active physical-memory
/// owner drains this queue after `Device::write()` returns and applies the
/// remap using its authoritative address-map surface.
#[derive(Debug, Clone)]
pub struct RemapCommand {
    /// BDF of the device being remapped.
    pub bdf: Bdf,
    /// BAR index (0-5).
    pub bar_idx: u8,
    /// Old base address (before the write).
    pub old_base: u64,
    /// New base address (after the write).
    pub new_base: u64,
    /// Size of the BAR region in bytes.
    pub size: u64,
}

/// PCI ECAM host bridge.
///
/// Models a PCI Express host bridge that decodes ECAM addresses and
/// dispatches to attached [`PciEndpoint`] devices. Reads to unoccupied
/// BDF slots return `0xFFFF_FFFF` (per PCI spec, indicating no device).
pub struct PciBus {
    name: String,
    /// Endpoint devices keyed by BDF.
    endpoints: HashMap<(u8, u8, u8), Box<dyn PciEndpoint>>,
    /// ECAM window size in bytes. Default 256 MiB.
    ecam_size: u64,
    /// Pending BAR re-programming commands, drained by the physical-memory
    /// owner after config-space writes complete.
    remap_queue: Vec<RemapCommand>,
}

impl PciBus {
    /// Create a new PCI host bridge with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            endpoints: HashMap::new(),
            // 256 MiB = 256 buses * 32 devices * 8 functions * 4 KiB
            ecam_size: 256 * 1024 * 1024,
            remap_queue: Vec::new(),
        }
    }

    /// Attach an endpoint device at the given BDF.
    ///
    /// Returns `Err` if the BDF is already occupied.
    pub fn attach_endpoint(
        &mut self,
        bdf: Bdf,
        endpoint: Box<dyn PciEndpoint>,
    ) -> Result<(), &'static str> {
        let key = (bdf.bus, bdf.device, bdf.function);
        if self.endpoints.contains_key(&key) {
            return Err("BDF already occupied");
        }
        self.endpoints.insert(key, endpoint);
        Ok(())
    }

    /// Return the bus name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Queue a BAR re-programming command. Called by config_write when
    /// a BAR register is modified.
    pub fn queue_remap(&mut self, cmd: RemapCommand) {
        self.remap_queue.push(cmd);
    }

    /// Drain pending remap commands. Called by the physical-memory owner after
    /// `Device::write()`.
    pub fn drain_remaps(&mut self) -> Vec<RemapCommand> {
        std::mem::take(&mut self.remap_queue)
    }

    /// Check if there are pending remaps.
    pub fn has_pending_remaps(&self) -> bool {
        !self.remap_queue.is_empty()
    }
}

impl Device for PciBus {
    fn read(&mut self, offset: u64, size: usize) -> u64 {
        let (bdf, reg_offset) = Bdf::from_ecam_offset(offset);
        let key = (bdf.bus, bdf.device, bdf.function);
        match self.endpoints.get(&key) {
            Some(ep) => ep.config_read(reg_offset, size) as u64,
            None => 0xFFFF_FFFF, // PCI: all-Fs for missing device
        }
    }

    fn write(&mut self, offset: u64, size: usize, val: u64) {
        let (bdf, reg_offset) = Bdf::from_ecam_offset(offset);
        let key = (bdf.bus, bdf.device, bdf.function);
        if let Some(ep) = self.endpoints.get_mut(&key) {
            let bar_index = if size == 4 && (0x10..0x28).contains(&(reg_offset as usize)) {
                Some(((reg_offset as usize - 0x10) / 4) as u8)
            } else {
                None
            };
            let old_base = bar_index.and_then(|idx| ep.bar_base(idx));
            let bar_size = bar_index.and_then(|idx| ep.bar_size(idx));
            ep.config_write(reg_offset, size, val as u32);
            if let Some(idx) = bar_index {
                let new_base = ep.bar_base(idx);
                if let (Some(size), Some(old_base), Some(new_base)) = (bar_size, old_base, new_base)
                {
                    if old_base != new_base {
                        self.queue_remap(RemapCommand {
                            bdf,
                            bar_idx: idx,
                            old_base,
                            new_base,
                            size,
                        });
                    }
                }
            }
        }
        // Writes to missing devices silently ignored (PCI spec)
    }

    fn region_size(&self) -> u64 {
        self.ecam_size
    }
}

impl PciEndpoint for PciRamBarEndpoint {
    fn config_read(&self, offset: u16, size: usize) -> u32 {
        self.config
            .lock()
            .expect("pci ram bar config mutex poisoned")
            .read(offset, size)
    }

    fn config_write(&mut self, offset: u16, size: usize, val: u32) {
        self.config
            .lock()
            .expect("pci ram bar config mutex poisoned")
            .write(offset, size, val);
    }

    fn vendor_id(&self) -> u16 {
        self.vendor
    }

    fn device_id(&self) -> u16 {
        self.device
    }

    fn class_code(&self) -> u32 {
        self.class
    }

    fn bar_base(&self, bar_index: u8) -> Option<u64> {
        self.config
            .lock()
            .expect("pci ram bar config mutex poisoned")
            .bar_address(bar_index as usize)
    }

    fn bar_size(&self, bar_index: u8) -> Option<u64> {
        self.config
            .lock()
            .expect("pci ram bar config mutex poisoned")
            .bar_size(bar_index as usize)
    }
}

impl Device for PciRamBarDevice {
    fn read(&mut self, offset: u64, size: usize) -> u64 {
        let Ok(start) = usize::try_from(offset) else {
            return 0;
        };
        let Ok(bytes) = self.bytes.lock() else {
            return 0;
        };
        if start >= bytes.len() {
            return 0;
        }

        let width = size.min(8);
        let end = start.saturating_add(width).min(bytes.len());
        let mut buf = [0u8; 8];
        let len = end.saturating_sub(start);
        buf[..len].copy_from_slice(&bytes[start..end]);
        u64::from_le_bytes(buf)
    }

    fn write(&mut self, offset: u64, size: usize, val: u64) {
        let Ok(start) = usize::try_from(offset) else {
            return;
        };
        let Ok(mut bytes) = self.bytes.lock() else {
            return;
        };
        if start >= bytes.len() {
            return;
        }

        let width = size.min(8);
        let end = start.saturating_add(width).min(bytes.len());
        let src = val.to_le_bytes();
        let len = end.saturating_sub(start);
        bytes[start..end].copy_from_slice(&src[..len]);
    }

    fn region_size(&self) -> u64 {
        self.size
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use config::PciConfigSpace;
    use helm_core::{AccessType, MemInterface};
    use helm_memory::{FlatMem, HelmAddressSpace};

    /// Minimal PCI endpoint for testing -- wraps a PciConfigSpace.
    struct TestEndpoint {
        config: PciConfigSpace,
        vendor: u16,
        device: u16,
        class: u32,
    }

    impl TestEndpoint {
        fn new(vendor_id: u16, device_id: u16, class_code: u32) -> Self {
            Self {
                config: PciConfigSpace::new(vendor_id, device_id, class_code, 0x00),
                vendor: vendor_id,
                device: device_id,
                class: class_code,
            }
        }

        fn with_bar0(mut self, base: u32, size: u32) -> Self {
            self.config.set_bar_size(0, size);
            self.config.write(0x10, 4, base);
            self
        }
    }

    impl PciEndpoint for TestEndpoint {
        fn config_read(&self, offset: u16, size: usize) -> u32 {
            // Need interior mutability for BAR sizing; for tests, use a
            // simple direct read of the raw data.
            let off = offset as usize;
            match size {
                1 => {
                    if off < 256 {
                        self.config.data_ref()[off] as u32
                    } else {
                        0
                    }
                }
                2 => {
                    if off + 1 < 256 {
                        u16::from_le_bytes([
                            self.config.data_ref()[off],
                            self.config.data_ref()[off + 1],
                        ]) as u32
                    } else {
                        0
                    }
                }
                4 => {
                    if off + 3 < 256 {
                        u32::from_le_bytes([
                            self.config.data_ref()[off],
                            self.config.data_ref()[off + 1],
                            self.config.data_ref()[off + 2],
                            self.config.data_ref()[off + 3],
                        ])
                    } else {
                        0
                    }
                }
                _ => 0,
            }
        }

        fn config_write(&mut self, offset: u16, size: usize, val: u32) {
            self.config.write(offset, size, val);
        }

        fn vendor_id(&self) -> u16 {
            self.vendor
        }
        fn device_id(&self) -> u16 {
            self.device
        }
        fn class_code(&self) -> u32 {
            self.class
        }

        fn bar_base(&self, bar_index: u8) -> Option<u64> {
            self.config.bar_address(bar_index as usize)
        }

        fn bar_size(&self, bar_index: u8) -> Option<u64> {
            self.config.bar_size(bar_index as usize)
        }
    }

    struct MockBarDevice {
        last_write_offset: u64,
        last_write_val: u64,
    }

    impl MockBarDevice {
        fn new() -> Self {
            Self {
                last_write_offset: u64::MAX,
                last_write_val: 0,
            }
        }
    }

    impl Device for MockBarDevice {
        fn read(&mut self, _offset: u64, _size: usize) -> u64 {
            0
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
    fn ecam_decode_bdf() {
        // Bus 0, Device 1, Function 0, Offset 0
        let offset = (0u64 << 20) | (1u64 << 15) | (0u64 << 12) | 0;
        let (bdf, reg) = Bdf::from_ecam_offset(offset);
        assert_eq!(bdf.bus, 0);
        assert_eq!(bdf.device, 1);
        assert_eq!(bdf.function, 0);
        assert_eq!(reg, 0);
    }

    #[test]
    fn ecam_decode_with_offset() {
        // Bus 1, Device 3, Function 2, Offset 0x10 (BAR0)
        let offset = (1u64 << 20) | (3u64 << 15) | (2u64 << 12) | 0x10;
        let (bdf, reg) = Bdf::from_ecam_offset(offset);
        assert_eq!(bdf.bus, 1);
        assert_eq!(bdf.device, 3);
        assert_eq!(bdf.function, 2);
        assert_eq!(reg, 0x10);
    }

    #[test]
    fn missing_device_returns_all_fs() {
        let mut bus = PciBus::new("pci0");
        // Read from an empty bus
        let val = bus.read(0, 4);
        assert_eq!(val, 0xFFFF_FFFF);
    }

    #[test]
    fn attach_and_read_vendor_id() {
        let mut bus = PciBus::new("pci0");
        let ep = TestEndpoint::new(0x1AF4, 0x1001, 0x010000);
        bus.attach_endpoint(Bdf::new(0, 0, 0), Box::new(ep))
            .unwrap();

        // ECAM offset for BDF 0:0.0, register 0x00
        let offset = (0u64 << 20) | (0u64 << 15) | (0u64 << 12) | 0x00;
        let val = bus.read(offset, 2);
        assert_eq!(val, 0x1AF4);
    }

    #[test]
    fn attach_duplicate_bdf_fails() {
        let mut bus = PciBus::new("pci0");
        let ep1 = TestEndpoint::new(0x1AF4, 0x1001, 0x010000);
        let ep2 = TestEndpoint::new(0x8086, 0x100E, 0x020000);
        bus.attach_endpoint(Bdf::new(0, 0, 0), Box::new(ep1))
            .unwrap();
        let result = bus.attach_endpoint(Bdf::new(0, 0, 0), Box::new(ep2));
        assert!(result.is_err());
    }

    #[test]
    fn write_to_missing_device_is_silent() {
        let mut bus = PciBus::new("pci0");
        // Should not panic
        bus.write(0, 4, 0x1234);
    }

    #[test]
    fn region_size_default() {
        let bus = PciBus::new("pci0");
        assert_eq!(bus.region_size(), 256 * 1024 * 1024);
    }

    #[test]
    fn bar_write_queues_remap_command() {
        let mut bus = PciBus::new("pci0");
        let ep = TestEndpoint::new(0x1AF4, 0x1001, 0x010000).with_bar0(0x0A00_0000, 0x1000);
        bus.attach_endpoint(Bdf::new(0, 1, 0), Box::new(ep))
            .unwrap();

        let bar0_off = (1u64 << 15) | 0x10;
        bus.write(bar0_off, 4, 0x0B00_0000);

        let cmds = bus.drain_remaps();
        assert_eq!(cmds.len(), 1);
        let cmd = &cmds[0];
        assert_eq!(cmd.bdf, Bdf::new(0, 1, 0));
        assert_eq!(cmd.bar_idx, 0);
        assert_eq!(cmd.old_base, 0x0A00_0000);
        assert_eq!(cmd.new_base, 0x0B00_0000);
        assert_eq!(cmd.size, 0x1000);
    }

    #[test]
    fn drained_remap_command_projects_onto_helm_address_space() {
        let mut bus = PciBus::new("pci0");
        let ep = TestEndpoint::new(0x1AF4, 0x1001, 0x010000).with_bar0(0x0A00_0000, 0x1000);
        bus.attach_endpoint(Bdf::new(0, 1, 0), Box::new(ep))
            .unwrap();

        let mut sys = HelmAddressSpace::new(FlatMem::new(0, 0));
        let dev_idx = sys.add_device(0x0A00_0000, Box::new(MockBarDevice::new()));
        assert!(sys.register_pci_bar_region(0, 1, 0, 0, dev_idx, 0x0A00_0000, 0x1000, 0));

        let bar0_off = (1u64 << 15) | 0x10;
        bus.write(bar0_off, 4, 0x0B00_0000);
        let cmd = bus.drain_remaps().pop().unwrap();

        assert!(sys.apply_pci_bar_remap(
            cmd.bdf.bus,
            cmd.bdf.device,
            cmd.bdf.function,
            cmd.bar_idx,
            cmd.old_base,
            cmd.new_base,
        ));
        assert!(sys.address_map.lookup(0x0A00_0000).is_none());
        assert!(sys.address_map.lookup(0x0B00_0000).is_some());

        sys.write(0x0B00_0018, 4, 0xCD, AccessType::Store).unwrap();
        let mock = sys.device_as_mut::<MockBarDevice>(dev_idx).unwrap();
        assert_eq!(mock.last_write_offset, 0x18);
        assert_eq!(mock.last_write_val, 0xCD);
    }

    #[test]
    fn pci_ram_bar_pair_exposes_identity_and_mmio_storage() {
        let (endpoint, mut device) =
            build_pci_ram_bar_pair(0xCAFE, 0x0001, 0xFF0000, 0x0A00_0000, 0x1000).unwrap();

        assert_eq!(endpoint.vendor_id(), 0xCAFE);
        assert_eq!(endpoint.device_id(), 0x0001);
        assert_eq!(endpoint.class_code(), 0xFF0000);
        assert_eq!(endpoint.bar_base(0), Some(0x0A00_0000));
        assert_eq!(endpoint.bar_size(0), Some(0x1000));

        device.write(0x20, 4, 0x1122_3344);
        assert_eq!(device.read(0x20, 4), 0x1122_3344);
    }
}

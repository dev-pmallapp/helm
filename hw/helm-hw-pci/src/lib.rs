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
}

// ── PciBus ──────────────────────────────────────────────────────────────────

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
}

impl PciBus {
    /// Create a new PCI host bridge with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            endpoints: HashMap::new(),
            // 256 MiB = 256 buses * 32 devices * 8 functions * 4 KiB
            ecam_size: 256 * 1024 * 1024,
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
            ep.config_write(reg_offset, size, val as u32);
        }
        // Writes to missing devices silently ignored (PCI spec)
    }

    fn region_size(&self) -> u64 {
        self.ecam_size
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use config::PciConfigSpace;

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
}

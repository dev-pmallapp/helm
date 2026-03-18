//! Generic MMIO dispatch bus.
//!
//! [`MmioBus`] routes MMIO reads and writes to child devices by matching
//! the transaction offset against each child's `[offset, offset + size)`
//! range. Reads to unmapped regions return 0; writes are silently ignored.
//!
//! Protocol-specific buses ([`AhbBus`](super::amba::AhbBus),
//! [`ApbBus`](super::amba::ApbBus)) wrap `MmioBus` and add timing
//! annotations.

use crate::Device;

// ── ChildDevice ─────────────────────────────────────────────────────────────

/// A device attached to an MMIO bus at a specific offset and size.
struct ChildDevice {
    /// Byte offset within the bus's MMIO region where this child starts.
    offset: u64,
    /// Size of this child's MMIO region in bytes.
    size: u64,
    /// The child device.
    device: Box<dyn Device>,
}

// ── MmioBus ─────────────────────────────────────────────────────────────────

/// Generic MMIO dispatch bus.
///
/// Routes transactions to child [`Device`]s by offset within the bus's
/// address window. This is the shared core for [`AhbBus`](super::amba::AhbBus)
/// and [`ApbBus`](super::amba::ApbBus), and can be used directly as a
/// simple flat MMIO bus.
pub struct MmioBus {
    name: String,
    children: Vec<ChildDevice>,
    /// Total size of the bus's MMIO window.
    region: u64,
}

impl MmioBus {
    /// Create a new MMIO bus with the given name and a 1 MiB default region.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            children: Vec::new(),
            region: 1024 * 1024, // 1 MiB default
        }
    }

    /// Create a new MMIO bus with an explicit region size.
    pub fn with_region_size(name: impl Into<String>, region: u64) -> Self {
        Self {
            name: name.into(),
            children: Vec::new(),
            region,
        }
    }

    /// Attach a child device at `offset` occupying `size` bytes.
    ///
    /// Returns `Ok(())` or `Err` if the region overlaps an existing child.
    pub fn attach_child(
        &mut self,
        offset: u64,
        size: u64,
        device: Box<dyn Device>,
    ) -> Result<(), &'static str> {
        let new_end = offset + size;
        for child in &self.children {
            let child_end = child.offset + child.size;
            if offset < child_end && new_end > child.offset {
                return Err("child region overlaps existing child");
            }
        }
        self.children.push(ChildDevice {
            offset,
            size,
            device,
        });
        Ok(())
    }

    /// Find the child device index that covers `offset`.
    fn find_child(&self, offset: u64) -> Option<usize> {
        for (i, child) in self.children.iter().enumerate() {
            if offset >= child.offset && offset < child.offset + child.size {
                return Some(i);
            }
        }
        None
    }

    /// Return the bus name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Device for MmioBus {
    fn read(&mut self, offset: u64, size: usize) -> u64 {
        if let Some(idx) = self.find_child(offset) {
            let child = &mut self.children[idx];
            let child_offset = offset - child.offset;
            child.device.read(child_offset, size)
        } else {
            0
        }
    }

    fn write(&mut self, offset: u64, size: usize, val: u64) {
        if let Some(idx) = self.find_child(offset) {
            let child = &mut self.children[idx];
            let child_offset = offset - child.offset;
            child.device.write(child_offset, size, val);
        }
    }

    fn region_size(&self) -> u64 {
        self.region
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal test device that stores a single u32 register.
    struct TestRegDevice {
        val: u32,
        size: u64,
    }

    impl TestRegDevice {
        fn new(size: u64) -> Self {
            Self { val: 0, size }
        }
    }

    impl Device for TestRegDevice {
        fn read(&mut self, offset: u64, _size: usize) -> u64 {
            if offset == 0 {
                self.val as u64
            } else {
                0
            }
        }

        fn write(&mut self, offset: u64, _size: usize, val: u64) {
            if offset == 0 {
                self.val = val as u32;
            }
        }

        fn region_size(&self) -> u64 {
            self.size
        }
    }

    #[test]
    fn attach_and_read_write() {
        let mut bus = MmioBus::with_region_size("bus0", 0x2000);
        bus.attach_child(0x0000, 0x1000, Box::new(TestRegDevice::new(0x1000)))
            .unwrap();

        bus.write(0x0000, 4, 0xDEAD_BEEF);
        assert_eq!(bus.read(0x0000, 4), 0xDEAD_BEEF);
    }

    #[test]
    fn unmapped_returns_zero() {
        let bus = &mut MmioBus::with_region_size("bus0", 0x2000);
        assert_eq!(bus.read(0x500, 4), 0);
    }

    #[test]
    fn multiple_children() {
        let mut bus = MmioBus::with_region_size("bus0", 0x3000);
        bus.attach_child(0x0000, 0x1000, Box::new(TestRegDevice::new(0x1000)))
            .unwrap();
        bus.attach_child(0x1000, 0x1000, Box::new(TestRegDevice::new(0x1000)))
            .unwrap();

        bus.write(0x0000, 4, 0xAAAA);
        bus.write(0x1000, 4, 0xBBBB);

        assert_eq!(bus.read(0x0000, 4), 0xAAAA);
        assert_eq!(bus.read(0x1000, 4), 0xBBBB);
    }

    #[test]
    fn overlap_rejected() {
        let mut bus = MmioBus::with_region_size("bus0", 0x2000);
        bus.attach_child(0x0000, 0x1000, Box::new(TestRegDevice::new(0x1000)))
            .unwrap();
        let result =
            bus.attach_child(0x0800, 0x1000, Box::new(TestRegDevice::new(0x1000)));
        assert!(result.is_err());
    }

    #[test]
    fn region_size() {
        let bus = MmioBus::with_region_size("bus0", 0x10000);
        assert_eq!(bus.region_size(), 0x10000);
    }

    #[test]
    fn write_to_unmapped_is_silent() {
        let mut bus = MmioBus::with_region_size("bus0", 0x2000);
        // Should not panic
        bus.write(0x500, 4, 0x1234);
    }
}

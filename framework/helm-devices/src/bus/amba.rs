//! AMBA bus protocols -- AHB and APB buses.
//!
//! Models the ARM Advanced Microcontroller Bus Architecture (AMBA):
//! - **AHB** (Advanced High-performance Bus): high-bandwidth interconnect
//!   with configurable wait states.
//! - **APB** (Advanced Peripheral Bus): lower-bandwidth bridge with a
//!   fixed bridge latency (default 1 cycle).
//!
//! Both buses delegate MMIO dispatch to [`MmioBus`](super::mmio::MmioBus)
//! and add protocol-specific timing annotations.

use super::mmio::MmioBus;
use crate::Device;

// ── AhbBus ──────────────────────────────────────────────────────────────────

/// AHB (Advanced High-performance Bus) controller.
///
/// Routes MMIO transactions to child devices via an internal [`MmioBus`].
/// Adds configurable `wait_states` for timing annotation.
pub struct AhbBus {
    inner: MmioBus,
    /// Configurable wait states added per transaction (timing annotation).
    pub wait_states: u64,
}

impl AhbBus {
    /// Create a new AHB bus with the given name and a 1 MiB default region.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            inner: MmioBus::new(name),
            wait_states: 0,
        }
    }

    /// Create a new AHB bus with an explicit region size.
    pub fn with_region_size(name: impl Into<String>, region: u64) -> Self {
        Self {
            inner: MmioBus::with_region_size(name, region),
            wait_states: 0,
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
        self.inner.attach_child(offset, size, device)
    }

    /// Return the bus name.
    pub fn name(&self) -> &str {
        self.inner.name()
    }
}

impl Device for AhbBus {
    fn read(&mut self, offset: u64, size: usize) -> u64 {
        self.inner.read(offset, size)
    }

    fn write(&mut self, offset: u64, size: usize, val: u64) {
        self.inner.write(offset, size, val);
    }

    fn region_size(&self) -> u64 {
        self.inner.region_size()
    }
}

// ── ApbBus ──────────────────────────────────────────────────────────────────

/// APB (Advanced Peripheral Bus) bridge.
///
/// Routes MMIO transactions to child devices via an internal [`MmioBus`].
/// Models the AHB-to-APB bridge with a configurable `bridge_latency`
/// (default 1 cycle). In a timed simulation the bridge latency is added
/// to each transaction's stall count.
pub struct ApbBus {
    inner: MmioBus,
    /// Bridge latency in cycles (default 1).
    pub bridge_latency: u64,
}

impl ApbBus {
    /// Create a new APB bus with the given name and a 1 MiB default region.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            inner: MmioBus::new(name),
            bridge_latency: 1,
        }
    }

    /// Create a new APB bus with an explicit region size.
    pub fn with_region_size(name: impl Into<String>, region: u64) -> Self {
        Self {
            inner: MmioBus::with_region_size(name, region),
            bridge_latency: 1,
        }
    }

    /// Attach a child device at `offset` occupying `size` bytes.
    pub fn attach_child(
        &mut self,
        offset: u64,
        size: u64,
        device: Box<dyn Device>,
    ) -> Result<(), &'static str> {
        self.inner.attach_child(offset, size, device)
    }

    /// Return the bus name.
    pub fn name(&self) -> &str {
        self.inner.name()
    }
}

impl Device for ApbBus {
    fn read(&mut self, offset: u64, size: usize) -> u64 {
        self.inner.read(offset, size)
    }

    fn write(&mut self, offset: u64, size: usize, val: u64) {
        self.inner.write(offset, size, val);
    }

    fn region_size(&self) -> u64 {
        self.inner.region_size()
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

    // ── AHB tests ───────────────────────────────────────────────────────

    #[test]
    fn ahb_attach_and_read_write() {
        let mut bus = AhbBus::with_region_size("ahb0", 0x2000);
        bus.attach_child(0x0000, 0x1000, Box::new(TestRegDevice::new(0x1000)))
            .unwrap();

        // Write to child
        bus.write(0x0000, 4, 0xDEAD_BEEF);
        assert_eq!(bus.read(0x0000, 4), 0xDEAD_BEEF);
    }

    #[test]
    fn ahb_unmapped_returns_zero() {
        let bus = &mut AhbBus::with_region_size("ahb0", 0x2000);
        assert_eq!(bus.read(0x500, 4), 0);
    }

    #[test]
    fn ahb_multiple_children() {
        let mut bus = AhbBus::with_region_size("ahb0", 0x3000);
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
    fn ahb_overlap_rejected() {
        let mut bus = AhbBus::with_region_size("ahb0", 0x2000);
        bus.attach_child(0x0000, 0x1000, Box::new(TestRegDevice::new(0x1000)))
            .unwrap();
        let result = bus.attach_child(0x0800, 0x1000, Box::new(TestRegDevice::new(0x1000)));
        assert!(result.is_err());
    }

    #[test]
    fn ahb_region_size() {
        let bus = AhbBus::with_region_size("ahb0", 0x10000);
        assert_eq!(bus.region_size(), 0x10000);
    }

    #[test]
    fn ahb_write_to_unmapped_is_silent() {
        let mut bus = AhbBus::with_region_size("ahb0", 0x2000);
        // Should not panic
        bus.write(0x500, 4, 0x1234);
    }

    // ── APB tests ───────────────────────────────────────────────────────

    #[test]
    fn apb_attach_and_read_write() {
        let mut bus = ApbBus::with_region_size("apb0", 0x2000);
        bus.attach_child(0x0000, 0x1000, Box::new(TestRegDevice::new(0x1000)))
            .unwrap();

        bus.write(0x0000, 4, 0xCAFE);
        assert_eq!(bus.read(0x0000, 4), 0xCAFE);
    }

    #[test]
    fn apb_default_bridge_latency() {
        let bus = ApbBus::new("apb0");
        assert_eq!(bus.bridge_latency, 1);
    }

    #[test]
    fn apb_unmapped_returns_zero() {
        let bus = &mut ApbBus::with_region_size("apb0", 0x2000);
        assert_eq!(bus.read(0x100, 4), 0);
    }

    #[test]
    fn apb_multiple_children() {
        let mut bus = ApbBus::with_region_size("apb0", 0x3000);
        bus.attach_child(0x0000, 0x1000, Box::new(TestRegDevice::new(0x1000)))
            .unwrap();
        bus.attach_child(0x1000, 0x1000, Box::new(TestRegDevice::new(0x1000)))
            .unwrap();

        bus.write(0x0000, 4, 0x1111);
        bus.write(0x1000, 4, 0x2222);

        assert_eq!(bus.read(0x0000, 4), 0x1111);
        assert_eq!(bus.read(0x1000, 4), 0x2222);
    }
}

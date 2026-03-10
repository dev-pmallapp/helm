use crate::device::Device;
use crate::platform_v2::PlatformV2;
use crate::region::{MemRegion, RegionKind};
use crate::transaction::Transaction;
use helm_core::HelmResult;

/// Test device with a single register.
struct TestDev {
    name: String,
    value: u64,
    region: MemRegion,
    clock: u64,
}

impl TestDev {
    fn new(name: &str, size: u64, value: u64) -> Self {
        Self {
            name: name.to_string(),
            value,
            region: MemRegion {
                name: name.to_string(),
                base: 0,
                size,
                kind: RegionKind::Io,
                priority: 0,
            },
            clock: 0,
        }
    }

    #[allow(dead_code)]
    fn with_clock(mut self, hz: u64) -> Self {
        self.clock = hz;
        self
    }
}

impl Device for TestDev {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if txn.is_write {
            self.value = txn.data_u64();
        } else {
            txn.set_data_u64(self.value);
        }
        Ok(())
    }
    fn regions(&self) -> &[MemRegion] {
        std::slice::from_ref(&self.region)
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn clock_hz(&self) -> u64 {
        self.clock
    }
}

#[test]
fn add_remove_lifecycle() {
    let mut platform = PlatformV2::new("test");

    let _id0 = platform
        .add_device("uart0", 0x1000, Box::new(TestDev::new("uart0", 0x100, 10)))
        .unwrap();
    let id1 = platform
        .add_device("uart1", 0x2000, Box::new(TestDev::new("uart1", 0x100, 20)))
        .unwrap();
    let _id2 = platform
        .add_device("timer", 0x3000, Box::new(TestDev::new("timer", 0x100, 30)))
        .unwrap();

    assert_eq!(platform.num_devices(), 3);

    // Remove one
    let removed = platform.remove_device(id1);
    assert!(removed.is_some());
    assert_eq!(platform.num_devices(), 2);

    // Remaining two still work
    assert_eq!(platform.read_fast(0x1000, 4).unwrap(), 10);
    assert_eq!(platform.read_fast(0x3000, 4).unwrap(), 30);

    // Removed one fails
    assert!(platform.read_fast(0x2000, 4).is_err());
}

#[test]
fn dispatch_to_device() {
    let mut platform = PlatformV2::new("test");
    platform
        .add_device(
            "uart",
            0x0900_0000,
            Box::new(TestDev::new("uart", 0x1000, 0x42)),
        )
        .unwrap();

    // Read via dispatch
    let mut txn = Transaction::read(0x0900_0000, 4);
    platform.dispatch(&mut txn).unwrap();
    assert_eq!(txn.data_u64(), 0x42);

    // Write + read via fast path
    platform.write_fast(0x0900_0000, 4, 0x99).unwrap();
    assert_eq!(platform.read_fast(0x0900_0000, 4).unwrap(), 0x99);
}

#[test]
fn hot_plug_mid_simulation() {
    let mut platform = PlatformV2::new("test");

    // Start with one device
    platform
        .add_device(
            "uart0",
            0x1000,
            Box::new(TestDev::new("uart0", 0x100, 0xAA)),
        )
        .unwrap();

    // Run some ticks
    let events = platform.tick(100).unwrap();
    assert!(events.is_empty());
    assert_eq!(platform.total_ticks(), 100);

    // Hot-plug a second device
    platform
        .add_device(
            "uart1",
            0x2000,
            Box::new(TestDev::new("uart1", 0x100, 0xBB)),
        )
        .unwrap();

    // Run more ticks
    platform.tick(100).unwrap();
    assert_eq!(platform.total_ticks(), 200);

    // Both devices work
    assert_eq!(platform.read_fast(0x1000, 4).unwrap(), 0xAA);
    assert_eq!(platform.read_fast(0x2000, 4).unwrap(), 0xBB);
}

#[test]
fn reset_all_devices() {
    let mut platform = PlatformV2::new("test");
    platform
        .add_device(
            "uart0",
            0x1000,
            Box::new(TestDev::new("uart0", 0x100, 42)),
        )
        .unwrap();
    assert!(platform.reset().is_ok());
}

#[test]
fn empty_platform() {
    let mut platform = PlatformV2::new("empty");
    assert_eq!(platform.num_devices(), 0);
    assert_eq!(platform.total_ticks(), 0);
    assert!(platform.tick(10).unwrap().is_empty());
}

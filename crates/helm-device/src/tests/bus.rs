use crate::bus::*;
use crate::mmio::*;
use helm_core::types::Addr;
use helm_core::HelmResult;

/// A trivial device for testing: a single 64-bit register.
struct TestRegister {
    value: u64,
}

impl TestRegister {
    fn new() -> Self {
        Self { value: 0 }
    }
}

impl MemoryMappedDevice for TestRegister {
    fn read(&mut self, _offset: Addr, _size: usize) -> HelmResult<DeviceAccess> {
        Ok(DeviceAccess {
            data: self.value,
            stall_cycles: 1,
        })
    }
    fn write(&mut self, _offset: Addr, _size: usize, value: u64) -> HelmResult<u64> {
        self.value = value;
        Ok(1)
    }
    fn region_size(&self) -> u64 {
        8
    }
    fn device_name(&self) -> &str {
        "test-register"
    }
}

#[test]
fn attach_and_read() {
    let mut bus = DeviceBus::system();
    bus.attach("reg0", 0x1000, Box::new(TestRegister::new()));
    let access = bus.read(0x1000, 8).unwrap();
    assert_eq!(access.data, 0);
}

#[test]
fn write_then_read() {
    let mut bus = DeviceBus::system();
    bus.attach("reg0", 0x1000, Box::new(TestRegister::new()));
    bus.write(0x1000, 8, 0xCAFE).unwrap();
    let access = bus.read(0x1000, 8).unwrap();
    assert_eq!(access.data, 0xCAFE);
}

#[test]
fn unmapped_address_fails() {
    let mut bus = DeviceBus::system();
    assert!(bus.read(0x9999, 4).is_err());
    assert!(bus.write(0x9999, 4, 0).is_err());
}

#[test]
fn multiple_devices() {
    let mut bus = DeviceBus::system();
    bus.attach("a", 0x1000, Box::new(TestRegister::new()));
    bus.attach("b", 0x2000, Box::new(TestRegister::new()));
    bus.write(0x1000, 8, 0xAA).unwrap();
    bus.write(0x2000, 8, 0xBB).unwrap();
    assert_eq!(bus.read(0x1000, 8).unwrap().data, 0xAA);
    assert_eq!(bus.read(0x2000, 8).unwrap().data, 0xBB);
}

#[test]
fn devices_list() {
    let mut bus = DeviceBus::system();
    bus.attach("uart", 0x4000_0000, Box::new(TestRegister::new()));
    let devs = bus.devices();
    assert_eq!(devs.len(), 1);
    assert_eq!(devs[0].0, "uart");
}

#[test]
fn devices_list_shows_correct_base_address() {
    let mut bus = DeviceBus::system();
    bus.attach("timer", 0x1234_0000, Box::new(TestRegister::new()));
    let devs = bus.devices();
    assert_eq!(devs[0].1, 0x1234_0000);
}

#[test]
fn bus_default_constructs_empty() {
    let mut bus = DeviceBus::default();
    assert!(bus.devices().is_empty());
    assert!(bus.read(0, 1).is_err());
}

#[test]
fn reset_all_does_not_error_with_devices() {
    let mut bus = DeviceBus::system();
    bus.attach("reg", 0x1000, Box::new(TestRegister::new()));
    assert!(bus.reset_all().is_ok());
}

#[test]
fn reset_all_empty_bus_is_ok() {
    let mut bus = DeviceBus::system();
    assert!(bus.reset_all().is_ok());
}

#[test]
fn devices_list_shows_correct_size() {
    let mut bus = DeviceBus::system();
    bus.attach("reg8", 0x1000, Box::new(TestRegister::new())); // region_size = 8
    let devs = bus.devices();
    assert_eq!(devs[0].2, 8); // size field
}

// --- Hierarchical bus tests ---

#[test]
fn system_bus_has_zero_bridge_latency() {
    let mut bus = DeviceBus::system();
    bus.attach("reg", 0x1000, Box::new(TestRegister::new()));
    let access = bus.read(0x1000, 8).unwrap();
    // Device returns 1 stall, system bus adds 0
    assert_eq!(access.stall_cycles, 1);
}

#[test]
fn pci_bus_adds_bridge_latency() {
    let mut pci = DeviceBus::pci("pci0", 0x1000_0000);
    pci.attach("dev", 0x0, Box::new(TestRegister::new()));
    let access = pci.read(0x0, 8).unwrap();
    // Device returns 1 stall, PCI adds 1
    assert_eq!(access.stall_cycles, 2);
}

#[test]
fn usb_bus_adds_10_cycle_latency() {
    let mut usb = DeviceBus::usb("usb0");
    usb.attach("dev", 0x0, Box::new(TestRegister::new()));
    let access = usb.read(0x0, 8).unwrap();
    // Device returns 1 stall, USB adds 10
    assert_eq!(access.stall_cycles, 11);
}

#[test]
fn nested_bus_accumulates_latency() {
    // system (0) → PCI (1) → device (1 stall) = total 2
    let mut pci = DeviceBus::pci("pci0", 0x1000);
    pci.attach("nic", 0x0, Box::new(TestRegister::new()));

    let mut system = DeviceBus::system();
    system.attach("pci0", 0xC000_0000, Box::new(pci));

    let access = system.read(0xC000_0000, 8).unwrap();
    // PCI bridge (1) + device (1) + system (0) = 2
    assert_eq!(access.stall_cycles, 2);
}

#[test]
fn triple_nested_bus_latency() {
    // system (0) → PCI (1) → USB (10) → device (1) = 12
    let mut usb = DeviceBus::usb("usb0");
    usb.attach("kbd", 0x0, Box::new(TestRegister::new()));

    let mut pci = DeviceBus::pci("pci0", 0x100_0000);
    pci.attach("usb0", 0x0, Box::new(usb));

    let mut system = DeviceBus::system();
    system.attach("pci0", 0xC000_0000, Box::new(pci));

    let access = system.read(0xC000_0000, 8).unwrap();
    assert_eq!(access.stall_cycles, 12); // 0 + 1 + 10 + 1
}

#[test]
fn nested_bus_write_accumulates_latency() {
    let mut pci = DeviceBus::pci("pci0", 0x1000);
    pci.attach("dev", 0x0, Box::new(TestRegister::new()));

    let mut system = DeviceBus::system();
    system.attach("pci0", 0xA000, Box::new(pci));

    let stall = system.write(0xA000, 8, 42).unwrap();
    // device write stall (1) + PCI (1) + system (0)
    assert_eq!(stall, 2);
}

#[test]
fn nested_bus_data_passes_through() {
    let mut pci = DeviceBus::pci("pci0", 0x1000);
    pci.attach("dev", 0x0, Box::new(TestRegister::new()));

    let mut system = DeviceBus::system();
    system.attach("pci0", 0xA000, Box::new(pci));

    system.write(0xA000, 8, 0xDEAD).unwrap();
    let access = system.read(0xA000, 8).unwrap();
    assert_eq!(access.data, 0xDEAD);
}

#[test]
fn bus_contains_checks_address() {
    let mut bus = DeviceBus::system();
    bus.attach("dev", 0x1000, Box::new(TestRegister::new()));
    assert!(bus.contains(0x1000));
    assert!(bus.contains(0x1007)); // within 8-byte region
    assert!(!bus.contains(0x1008)); // past end
    assert!(!bus.contains(0x0));
}

#[test]
fn custom_bridge_latency() {
    let mut bus = DeviceBus::new("spi", 0x1000, 5);
    bus.attach("flash", 0x0, Box::new(TestRegister::new()));
    let access = bus.read(0x0, 8).unwrap();
    assert_eq!(access.stall_cycles, 6); // device (1) + bus (5)
}

#[test]
fn bus_implements_device_trait() {
    // Verify DeviceBus can be used as Box<dyn MemoryMappedDevice>
    let mut inner = DeviceBus::pci("pci0", 0x100);
    inner.attach("dev", 0x0, Box::new(TestRegister::new()));

    let device: Box<dyn MemoryMappedDevice> = Box::new(inner);
    assert_eq!(device.region_size(), 0x100);
    assert_eq!(device.device_name(), "pci0");
}

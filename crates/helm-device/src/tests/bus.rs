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
    let mut bus = DeviceBus::new();
    bus.attach("reg0", 0x1000, Box::new(TestRegister::new()));
    let access = bus.read(0x1000, 8).unwrap();
    assert_eq!(access.data, 0);
}

#[test]
fn write_then_read() {
    let mut bus = DeviceBus::new();
    bus.attach("reg0", 0x1000, Box::new(TestRegister::new()));
    bus.write(0x1000, 8, 0xCAFE).unwrap();
    let access = bus.read(0x1000, 8).unwrap();
    assert_eq!(access.data, 0xCAFE);
}

#[test]
fn unmapped_address_fails() {
    let mut bus = DeviceBus::new();
    assert!(bus.read(0x9999, 4).is_err());
    assert!(bus.write(0x9999, 4, 0).is_err());
}

#[test]
fn multiple_devices() {
    let mut bus = DeviceBus::new();
    bus.attach("a", 0x1000, Box::new(TestRegister::new()));
    bus.attach("b", 0x2000, Box::new(TestRegister::new()));
    bus.write(0x1000, 8, 0xAA).unwrap();
    bus.write(0x2000, 8, 0xBB).unwrap();
    assert_eq!(bus.read(0x1000, 8).unwrap().data, 0xAA);
    assert_eq!(bus.read(0x2000, 8).unwrap().data, 0xBB);
}

#[test]
fn devices_list() {
    let mut bus = DeviceBus::new();
    bus.attach("uart", 0x4000_0000, Box::new(TestRegister::new()));
    let devs = bus.devices();
    assert_eq!(devs.len(), 1);
    assert_eq!(devs[0].0, "uart");
}

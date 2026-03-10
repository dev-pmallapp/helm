use crate::mmio::*;
use helm_core::types::Addr;
use helm_core::HelmResult;

struct ConstantDevice {
    region: u64,
}

impl MemoryMappedDevice for ConstantDevice {
    fn read(&mut self, _offset: Addr, _size: usize) -> HelmResult<DeviceAccess> {
        Ok(DeviceAccess {
            data: 0xDEAD_BEEF,
            stall_cycles: 2,
        })
    }
    fn write(&mut self, _offset: Addr, _size: usize, _value: u64) -> HelmResult<u64> {
        Ok(0)
    }
    fn region_size(&self) -> u64 {
        self.region
    }
}

#[test]
fn device_access_data_field() {
    let acc = DeviceAccess {
        data: 42,
        stall_cycles: 1,
    };
    assert_eq!(acc.data, 42);
}

#[test]
fn device_access_stall_cycles_field() {
    let acc = DeviceAccess {
        data: 0,
        stall_cycles: 10,
    };
    assert_eq!(acc.stall_cycles, 10);
}

#[test]
fn device_access_clone() {
    let acc = DeviceAccess {
        data: 0xFF,
        stall_cycles: 3,
    };
    let cloned = acc.clone();
    assert_eq!(cloned.data, acc.data);
    assert_eq!(cloned.stall_cycles, acc.stall_cycles);
}

#[test]
fn memory_mapped_device_default_name() {
    let mut d = ConstantDevice { region: 0x100 };
    // Default device_name is "unnamed-device"
    // Our ConstantDevice doesn't override, so it returns the default.
    // Note: if ConstantDevice doesn't override device_name, it uses the trait default.
    let _ = d.device_name(); // just confirm it doesn't panic
}

#[test]
fn memory_mapped_device_default_init_ok() {
    let mut d = ConstantDevice { region: 0x100 };
    assert!(d.init().is_ok());
}

#[test]
fn memory_mapped_device_default_reset_ok() {
    let mut d = ConstantDevice { region: 0x100 };
    assert!(d.reset().is_ok());
}

#[test]
fn constant_device_read_returns_value() {
    let mut d = ConstantDevice { region: 0x100 };
    let acc = d.read(0, 4).unwrap();
    assert_eq!(acc.data, 0xDEAD_BEEF);
    assert_eq!(acc.stall_cycles, 2);
}

#[test]
fn constant_device_region_size() {
    let d = ConstantDevice { region: 0x400 };
    assert_eq!(d.region_size(), 0x400);
}

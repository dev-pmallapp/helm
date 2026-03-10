use crate::device_bridge::AcceleratorDevice;
use crate::Accelerator;
use helm_device::mmio::MemoryMappedDevice;

/// Build an accelerator from empty IR (the only thing the parser handles).
fn make_accel() -> AcceleratorDevice {
    let accel = Accelerator::from_string("").build().unwrap();
    AcceleratorDevice::new(accel)
}

#[test]
fn accelerator_device_name() {
    let dev = make_accel();
    assert_eq!(dev.device_name(), "llvm-accelerator");
}

#[test]
fn accelerator_device_region_size() {
    let dev = make_accel();
    assert_eq!(dev.region_size(), 0x100);
}

#[test]
fn accelerator_device_status_starts_idle() {
    let mut dev = make_accel();
    let access = dev.read(0x00, 4).unwrap();
    assert_eq!(access.data, 0); // idle
}

#[test]
fn accelerator_device_reset() {
    let mut dev = make_accel();
    assert!(dev.reset().is_ok());
}

#[test]
fn accelerator_device_unknown_register_read() {
    let mut dev = make_accel();
    let access = dev.read(0x80, 4).unwrap();
    assert_eq!(access.data, 0);
}

#[test]
fn accelerator_device_read_stall_is_one() {
    let mut dev = make_accel();
    let access = dev.read(0x00, 4).unwrap();
    assert_eq!(access.stall_cycles, 1);
}

#[test]
fn accelerator_device_write_unknown_stall_is_one() {
    let mut dev = make_accel();
    let stall = dev.write(0x80, 4, 0).unwrap();
    assert_eq!(stall, 1);
}

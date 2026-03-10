use crate::device::*;
use crate::proto::amba::*;
use crate::proto::axi::*;
use crate::proto::i2c::*;
use crate::proto::spi::*;
use crate::proto::usb::*;
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::HelmResult;

struct EchoDevice {
    value: u64,
    region: MemRegion,
}

impl EchoDevice {
    fn new(name: &str) -> Self {
        Self {
            value: 0,
            region: MemRegion {
                name: name.to_string(),
                base: 0,
                size: 0x100,
                kind: crate::region::RegionKind::Io,
                priority: 0,
            },
        }
    }
}

impl Device for EchoDevice {
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
        &self.region.name
    }
}

// ── AHB ────────────────────────────────────────────────────────────────────

#[test]
fn ahb_bus_attach_and_list() {
    let mut bus = AhbBus::new("ahb0", 0x1_0000);
    bus.attach(0x0, 0x100, Box::new(EchoDevice::new("d0")));
    let devs = bus.devices();
    assert_eq!(devs.len(), 1);
}

#[test]
fn ahb_bus_transact_write_read() {
    let mut bus = AhbBus::new("ahb0", 0x1_0000);
    bus.attach(0x0, 0x100, Box::new(EchoDevice::new("d0")));
    let mut txn = Transaction::write(0x0, 4, 0xABCD);
    bus.transact(&mut txn).unwrap();
    let mut txn = Transaction::read(0x0, 4);
    bus.transact(&mut txn).unwrap();
    assert_eq!(txn.data_u64(), 0xABCD);
}

// ── APB ────────────────────────────────────────────────────────────────────

#[test]
fn apb_bus_attach_and_list() {
    let mut bus = ApbBus::new("apb0", 0x1_0000);
    bus.attach(0x0, 0x100, Box::new(EchoDevice::new("d0")));
    let periphs = bus.peripherals();
    assert_eq!(periphs.len(), 1);
}

// ── AXI ────────────────────────────────────────────────────────────────────

#[test]
fn axi_bus_attach() {
    let mut bus = AxiBus::new("axi0", 0x10_0000);
    bus.attach(0x0, 0x100, Box::new(EchoDevice::new("d0")));
}

#[test]
fn axi_bus_transact() {
    let mut bus = AxiBus::new("axi0", 0x10_0000);
    bus.attach(0x0, 0x100, Box::new(EchoDevice::new("d0")));
    let mut txn = Transaction::write(0x0, 4, 0x1234);
    bus.transact(&mut txn).unwrap();
    let mut txn = Transaction::read(0x0, 4);
    bus.transact(&mut txn).unwrap();
    assert_eq!(txn.data_u64(), 0x1234);
}

// ── I2C ────────────────────────────────────────────────────────────────────

#[test]
fn i2c_bus_attach() {
    let mut bus = I2cBus::new("i2c0");
    bus.attach(0x50, Box::new(EchoDevice::new("eeprom")));
}

#[test]
fn i2c_bus_transact_missing_device_fails() {
    let mut bus = I2cBus::new("i2c0");
    let mut txn = Transaction::read(0x50, 4);
    assert!(bus.transact(&mut txn).is_err());
}

// ── SPI ────────────────────────────────────────────────────────────────────

#[test]
fn spi_bus_attach() {
    let mut bus = SpiBus::new("spi0");
    bus.attach(0, Box::new(EchoDevice::new("flash")));
}

#[test]
fn spi_bus_transact_missing_cs_fails() {
    let mut bus = SpiBus::new("spi0");
    let mut txn = Transaction::read(0x00, 4);
    assert!(bus.transact(&mut txn).is_err());
}

// ── USB ────────────────────────────────────────────────────────────────────

#[test]
fn usb_bus_attach() {
    let mut bus = UsbBus::new("usb0");
    bus.attach(1, Box::new(EchoDevice::new("hid")));
}

#[test]
fn usb_bus_transact_missing_endpoint_fails() {
    let mut bus = UsbBus::new("usb0");
    let mut txn = Transaction::read(0, 4);
    assert!(bus.transact(&mut txn).is_err());
}

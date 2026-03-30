//! I2C master controller -- 5-register MMIO interface.
//!
//! Models a single-master I2C bus controller. Software writes to the
//! controller's registers to initiate START, STOP, read, and write
//! sequences. Attached [`I2cDevice`] peripherals respond via callbacks.
//!
//! # Register map (byte offsets from device base)
//!
//! | Offset | Name    | R/W | Description                                      |
//! |--------|---------|-----|--------------------------------------------------|
//! | 0x00   | CONTROL | RW  | bit0=START, bit1=STOP, bit2=READ, bit3=WRITE     |
//! | 0x04   | ADDRESS | RW  | bits[7:1]=7-bit addr, bit0=R/W direction          |
//! | 0x08   | DATA_TX | RW  | Byte to transmit                                 |
//! | 0x0C   | DATA_RX | R   | Last received byte (read-only)                   |
//! | 0x10   | STATUS  | R   | bit0=BUSY, bit1=ACK, bit2=NACK, bit3=IRQ_PENDING |

use std::collections::HashMap;

use crate::{Device, InterruptPin};

// ── Register offsets ────────────────────────────────────────────────────────

const REG_CONTROL: u64 = 0x00;
const REG_ADDRESS: u64 = 0x04;
const REG_DATA_TX: u64 = 0x08;
const REG_DATA_RX: u64 = 0x0C;
const REG_STATUS: u64 = 0x10;

// ── Control bits ────────────────────────────────────────────────────────────

const CTRL_START: u8 = 1 << 0;
const CTRL_STOP: u8 = 1 << 1;
const CTRL_READ: u8 = 1 << 2;
const CTRL_WRITE: u8 = 1 << 3;

// ── Status bits ─────────────────────────────────────────────────────────────

const STATUS_BUSY: u8 = 1 << 0;
const STATUS_ACK: u8 = 1 << 1;
const STATUS_NACK: u8 = 1 << 2;
#[allow(dead_code)]
const STATUS_IRQ: u8 = 1 << 3;

// ── I2cDevice trait ─────────────────────────────────────────────────────────

/// An I2C peripheral device that responds to bus transactions.
///
/// The I2C master calls these methods in sequence during a transaction:
/// `on_start` -> `on_write_byte`/`on_read_byte` (repeated) -> `on_stop`.
pub trait I2cDevice: Send {
    /// Called when a START condition addresses this device.
    fn on_start(&mut self);

    /// Called for each byte written to this device.
    /// Returns `true` for ACK, `false` for NACK.
    fn on_write_byte(&mut self, byte: u8) -> bool;

    /// Called when the master reads a byte from this device.
    /// Returns the next byte to transmit on SDA.
    fn on_read_byte(&mut self) -> u8;

    /// Called on a STOP condition.
    fn on_stop(&mut self);
}

// ── I2cBus ──────────────────────────────────────────────────────────────────

/// I2C bus state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum I2cState {
    Idle,
    Addressed { target: u8 },
}

/// I2C master controller.
///
/// Implements the [`Device`] trait to expose a 5-register MMIO interface.
/// Attached I2C peripherals are keyed by 7-bit address (0x00..0x7F).
pub struct I2cBus {
    devices: HashMap<u8, Box<dyn I2cDevice>>,
    state: I2cState,
    control: u8,
    address: u8,
    data_tx: u8,
    data_rx: u8,
    status: u8,
    /// Interrupt output pin. Asserted when a transaction completes or
    /// an error occurs. The platform wires this to the interrupt controller.
    pub irq_out: InterruptPin,
}

impl I2cBus {
    /// Create a new I2C master controller.
    pub fn new() -> Self {
        Self {
            devices: HashMap::new(),
            state: I2cState::Idle,
            control: 0,
            address: 0,
            data_tx: 0,
            data_rx: 0,
            status: 0,
            irq_out: InterruptPin::new(),
        }
    }

    /// Attach an I2C peripheral at the given 7-bit address.
    pub fn attach_device(
        &mut self,
        addr: u8,
        device: Box<dyn I2cDevice>,
    ) -> Result<(), &'static str> {
        if addr > 0x7F {
            return Err("I2C address must be 7-bit (0x00..0x7F)");
        }
        if self.devices.contains_key(&addr) {
            return Err("I2C address already in use");
        }
        self.devices.insert(addr, device);
        Ok(())
    }

    /// Process the control register -- execute the requested operation.
    fn process_control(&mut self) {
        let ctrl = self.control;

        // START: initiate transaction
        if ctrl & CTRL_START != 0 {
            let addr = (self.address >> 1) & 0x7F;
            if let Some(dev) = self.devices.get_mut(&addr) {
                dev.on_start();
                self.state = I2cState::Addressed { target: addr };
                self.status = STATUS_BUSY | STATUS_ACK;
            } else {
                // No device at this address -- NACK
                self.status = STATUS_BUSY | STATUS_NACK;
                self.state = I2cState::Idle;
            }
            self.irq_out.assert();
        }

        // WRITE: send data_tx byte to current target
        if ctrl & CTRL_WRITE != 0 {
            if let I2cState::Addressed { target } = self.state {
                if let Some(dev) = self.devices.get_mut(&target) {
                    let ack = dev.on_write_byte(self.data_tx);
                    self.status = if ack { STATUS_ACK } else { STATUS_NACK };
                }
            }
        }

        // READ: read byte from current target
        if ctrl & CTRL_READ != 0 {
            if let I2cState::Addressed { target } = self.state {
                if let Some(dev) = self.devices.get_mut(&target) {
                    self.data_rx = dev.on_read_byte();
                    self.status = STATUS_ACK;
                }
            }
        }

        // STOP: end transaction
        if ctrl & CTRL_STOP != 0 {
            if let I2cState::Addressed { target } = self.state {
                if let Some(dev) = self.devices.get_mut(&target) {
                    dev.on_stop();
                }
            }
            self.state = I2cState::Idle;
            self.status = 0;
            self.irq_out.deassert();
        }
    }
}

impl Default for I2cBus {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for I2cBus {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        match offset {
            REG_CONTROL => self.control as u64,
            REG_ADDRESS => self.address as u64,
            REG_DATA_TX => self.data_tx as u64,
            REG_DATA_RX => self.data_rx as u64,
            REG_STATUS => self.status as u64,
            _ => 0,
        }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        match offset {
            REG_CONTROL => {
                self.control = val as u8;
                self.process_control();
            }
            REG_ADDRESS => {
                self.address = val as u8;
            }
            REG_DATA_TX => {
                self.data_tx = val as u8;
            }
            // DATA_RX and STATUS are read-only
            _ => {}
        }
    }

    fn region_size(&self) -> u64 {
        0x14 // 5 registers at 4-byte spacing = 20 bytes
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Test I2C device that stores written bytes and returns a counter on read.
    struct TestI2cSlave {
        started: bool,
        stopped: bool,
        written: Vec<u8>,
        read_counter: u8,
    }

    impl TestI2cSlave {
        fn new() -> Self {
            Self {
                started: false,
                stopped: false,
                written: Vec::new(),
                read_counter: 0x42,
            }
        }
    }

    impl I2cDevice for TestI2cSlave {
        fn on_start(&mut self) {
            self.started = true;
        }

        fn on_write_byte(&mut self, byte: u8) -> bool {
            self.written.push(byte);
            true // ACK
        }

        fn on_read_byte(&mut self) -> u8 {
            let val = self.read_counter;
            self.read_counter = self.read_counter.wrapping_add(1);
            val
        }

        fn on_stop(&mut self) {
            self.stopped = true;
        }
    }

    /// Test I2C device that NACKs all writes.
    struct NackSlave;

    impl I2cDevice for NackSlave {
        fn on_start(&mut self) {}
        fn on_write_byte(&mut self, _byte: u8) -> bool {
            false // NACK
        }
        fn on_read_byte(&mut self) -> u8 {
            0
        }
        fn on_stop(&mut self) {}
    }

    #[test]
    fn i2c_write_sequence() {
        let mut bus = I2cBus::new();
        bus.attach_device(0x48, Box::new(TestI2cSlave::new()))
            .unwrap();

        // Set address: 0x48 << 1 | 0 (write direction)
        bus.write(REG_ADDRESS, 1, (0x48 << 1) as u64);
        // START
        bus.write(REG_CONTROL, 1, CTRL_START as u64);

        // Check status: should be BUSY | ACK
        let status = bus.read(REG_STATUS, 1);
        assert_eq!(status & STATUS_ACK as u64, STATUS_ACK as u64);

        // Write a byte
        bus.write(REG_DATA_TX, 1, 0xAB);
        bus.write(REG_CONTROL, 1, CTRL_WRITE as u64);

        // Check ACK
        let status = bus.read(REG_STATUS, 1);
        assert_eq!(status & STATUS_ACK as u64, STATUS_ACK as u64);

        // STOP
        bus.write(REG_CONTROL, 1, CTRL_STOP as u64);
        let status = bus.read(REG_STATUS, 1);
        assert_eq!(status, 0); // Idle
    }

    #[test]
    fn i2c_read_sequence() {
        let mut bus = I2cBus::new();
        bus.attach_device(0x48, Box::new(TestI2cSlave::new()))
            .unwrap();

        // Set address: 0x48 << 1 | 1 (read direction)
        bus.write(REG_ADDRESS, 1, ((0x48 << 1) | 1) as u64);
        // START
        bus.write(REG_CONTROL, 1, CTRL_START as u64);

        // READ a byte
        bus.write(REG_CONTROL, 1, CTRL_READ as u64);
        let rx = bus.read(REG_DATA_RX, 1);
        assert_eq!(rx, 0x42); // First read counter value

        // READ another byte
        bus.write(REG_CONTROL, 1, CTRL_READ as u64);
        let rx = bus.read(REG_DATA_RX, 1);
        assert_eq!(rx, 0x43); // Incremented

        // STOP
        bus.write(REG_CONTROL, 1, CTRL_STOP as u64);
    }

    #[test]
    fn i2c_nack_on_missing_device() {
        let mut bus = I2cBus::new();
        // No device at 0x48

        // Set address and START
        bus.write(REG_ADDRESS, 1, (0x48 << 1) as u64);
        bus.write(REG_CONTROL, 1, CTRL_START as u64);

        // Check NACK
        let status = bus.read(REG_STATUS, 1);
        assert_ne!(status & STATUS_NACK as u64, 0);
    }

    #[test]
    fn i2c_nack_from_device() {
        let mut bus = I2cBus::new();
        bus.attach_device(0x10, Box::new(NackSlave)).unwrap();

        bus.write(REG_ADDRESS, 1, (0x10 << 1) as u64);
        bus.write(REG_CONTROL, 1, CTRL_START as u64);

        // Write should get NACK
        bus.write(REG_DATA_TX, 1, 0xFF);
        bus.write(REG_CONTROL, 1, CTRL_WRITE as u64);

        let status = bus.read(REG_STATUS, 1);
        assert_ne!(status & STATUS_NACK as u64, 0);

        bus.write(REG_CONTROL, 1, CTRL_STOP as u64);
    }

    #[test]
    fn i2c_region_size() {
        let bus = I2cBus::new();
        assert_eq!(bus.region_size(), 0x14);
    }

    #[test]
    fn i2c_duplicate_address_rejected() {
        let mut bus = I2cBus::new();
        bus.attach_device(0x48, Box::new(TestI2cSlave::new()))
            .unwrap();
        let result = bus.attach_device(0x48, Box::new(TestI2cSlave::new()));
        assert!(result.is_err());
    }
}

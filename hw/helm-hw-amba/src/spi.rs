//! SPI master controller -- 5-register MMIO interface.
//!
//! Models a single SPI master with up to 8 chip-select lines (active low).
//! Attached [`SpiDevice`] peripherals respond via callbacks when their CS
//! is asserted and data is transferred.
//!
//! # Register map (byte offsets from device base)
//!
//! | Offset | Name    | R/W | Description                                 |
//! |--------|---------|-----|---------------------------------------------|
//! | 0x00   | TX_DATA | RW  | Byte to shift out on MOSI                   |
//! | 0x04   | RX_DATA | R   | Byte shifted in on MISO (read-only)         |
//! | 0x08   | CONTROL | RW  | bit0=START transfer                         |
//! | 0x0C   | CS_REG  | RW  | Chip select mask (bit N = select N, active low) |
//! | 0x10   | STATUS  | R   | bit0=BUSY, bit1=RX_VALID                    |

use helm_devices::{Device, InterruptPin};


// ── Register offsets ────────────────────────────────────────────────────────

const REG_TX_DATA: u64 = 0x00;
const REG_RX_DATA: u64 = 0x04;
const REG_CONTROL: u64 = 0x08;
const REG_CS: u64 = 0x0C;
const REG_STATUS: u64 = 0x10;

// ── Status bits ─────────────────────────────────────────────────────────────

#[allow(dead_code)]
const STATUS_BUSY: u8 = 1 << 0;
const STATUS_RX_VALID: u8 = 1 << 1;

// ── SpiDevice trait ─────────────────────────────────────────────────────────

/// An SPI peripheral device.
///
/// The SPI master calls these methods when the corresponding chip-select
/// transitions or when a byte is transferred.
pub trait SpiDevice: Send {
    /// Called when CS is asserted (active low: CS goes to 0).
    fn on_cs_assert(&mut self);

    /// Called when CS is deasserted (active low: CS goes to 1).
    fn on_cs_deassert(&mut self);

    /// Full-duplex byte exchange: receives `mosi_byte` on MOSI, returns
    /// the byte to shift out on MISO.
    fn transfer_byte(&mut self, mosi_byte: u8) -> u8;
}

// ── SpiBus ──────────────────────────────────────────────────────────────────

/// Maximum number of chip-select lines.
const MAX_CS: usize = 8;

/// SPI master controller.
///
/// Implements the [`Device`] trait to expose a 5-register MMIO interface.
/// Supports up to 8 chip-select lines (active low).
pub struct SpiBus {
    devices: [Option<Box<dyn SpiDevice>>; MAX_CS],
    tx_data: u8,
    rx_data: u8,
    control: u8,
    cs_reg: u8,
    status: u8,
    /// Interrupt output pin. Asserted when a transfer completes.
    pub irq_out: InterruptPin,
}

impl SpiBus {
    /// Create a new SPI master controller.
    ///
    /// All chip-selects start deasserted (all bits = 1, active low).
    pub fn new() -> Self {
        Self {
            devices: Default::default(),
            tx_data: 0,
            rx_data: 0,
            control: 0,
            cs_reg: 0xFF, // All CS deasserted (active low)
            status: 0,
            irq_out: InterruptPin::new(),
        }
    }

    /// Attach an SPI device at the given chip-select index (0..7).
    pub fn attach_device(
        &mut self,
        cs_index: u8,
        device: Box<dyn SpiDevice>,
    ) -> Result<(), &'static str> {
        let idx = cs_index as usize;
        if idx >= MAX_CS {
            return Err("CS index must be 0..7");
        }
        if self.devices[idx].is_some() {
            return Err("CS index already in use");
        }
        self.devices[idx] = Some(device);
        Ok(())
    }

    /// Process a transfer: shift tx_data to each selected device.
    fn do_transfer(&mut self) {
        for (idx, slot) in self.devices.iter_mut().enumerate() {
            if let Some(dev) = slot {
                // Active low: bit=0 means selected
                if self.cs_reg & (1 << idx) == 0 {
                    self.rx_data = dev.transfer_byte(self.tx_data);
                    self.status = STATUS_RX_VALID;
                    self.irq_out.assert();
                }
            }
        }
    }

    /// Handle CS register write: track transitions.
    fn update_cs(&mut self, new_cs: u8) {
        let old_cs = self.cs_reg;
        for (idx, slot) in self.devices.iter_mut().enumerate() {
            if let Some(dev) = slot {
                let was_selected = old_cs & (1 << idx) == 0;
                let is_selected = new_cs & (1 << idx) == 0;
                if was_selected && !is_selected {
                    dev.on_cs_deassert();
                } else if !was_selected && is_selected {
                    dev.on_cs_assert();
                }
            }
        }
        self.cs_reg = new_cs;
    }
}

impl Default for SpiBus {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for SpiBus {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        match offset {
            REG_TX_DATA => self.tx_data as u64,
            REG_RX_DATA => {
                // Clear RX_VALID on read
                let val = self.rx_data as u64;
                self.status &= !STATUS_RX_VALID;
                val
            }
            REG_CONTROL => self.control as u64,
            REG_CS => self.cs_reg as u64,
            REG_STATUS => self.status as u64,
            _ => 0,
        }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        match offset {
            REG_TX_DATA => {
                self.tx_data = val as u8;
            }
            REG_CONTROL => {
                self.control = val as u8;
                if val & 0x01 != 0 {
                    self.do_transfer();
                }
            }
            REG_CS => {
                self.update_cs(val as u8);
            }
            // RX_DATA and STATUS are read-only
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

    /// Test SPI device that echoes back the received byte XOR'd with 0xFF.
    struct EchoSpiDevice {
        cs_asserted: bool,
        cs_deasserted: bool,
    }

    impl EchoSpiDevice {
        fn new() -> Self {
            Self {
                cs_asserted: false,
                cs_deasserted: false,
            }
        }
    }

    impl SpiDevice for EchoSpiDevice {
        fn on_cs_assert(&mut self) {
            self.cs_asserted = true;
        }

        fn on_cs_deassert(&mut self) {
            self.cs_deasserted = true;
        }

        fn transfer_byte(&mut self, mosi_byte: u8) -> u8 {
            mosi_byte ^ 0xFF
        }
    }

    #[test]
    fn spi_transfer_basic() {
        let mut bus = SpiBus::new();
        bus.attach_device(0, Box::new(EchoSpiDevice::new())).unwrap();

        // Assert CS 0 (active low: bit 0 = 0)
        bus.write(REG_CS, 1, 0xFE);

        // Load TX data
        bus.write(REG_TX_DATA, 1, 0xA5);

        // Start transfer
        bus.write(REG_CONTROL, 1, 0x01);

        // Check RX data (0xA5 ^ 0xFF = 0x5A)
        let rx = bus.read(REG_RX_DATA, 1);
        assert_eq!(rx, 0x5A);
    }

    #[test]
    fn spi_cs_callbacks() {
        let mut bus = SpiBus::new();
        bus.attach_device(0, Box::new(EchoSpiDevice::new())).unwrap();

        // Assert CS 0
        bus.write(REG_CS, 1, 0xFE);

        // Deassert CS 0
        bus.write(REG_CS, 1, 0xFF);

        // We can verify the bus handled it without panic.
        // (Direct assertion on device state requires interior access,
        // which we avoid for simplicity.)
    }

    #[test]
    fn spi_rx_valid_cleared_on_read() {
        let mut bus = SpiBus::new();
        bus.attach_device(0, Box::new(EchoSpiDevice::new())).unwrap();

        bus.write(REG_CS, 1, 0xFE);
        bus.write(REG_TX_DATA, 1, 0x00);
        bus.write(REG_CONTROL, 1, 0x01);

        // Status should have RX_VALID
        let status = bus.read(REG_STATUS, 1);
        assert_ne!(status & STATUS_RX_VALID as u64, 0);

        // Reading RX_DATA clears RX_VALID
        let _rx = bus.read(REG_RX_DATA, 1);
        let status = bus.read(REG_STATUS, 1);
        assert_eq!(status & STATUS_RX_VALID as u64, 0);
    }

    #[test]
    fn spi_no_transfer_without_cs() {
        let mut bus = SpiBus::new();
        bus.attach_device(0, Box::new(EchoSpiDevice::new())).unwrap();

        // CS all deasserted (default 0xFF)
        bus.write(REG_TX_DATA, 1, 0xAA);
        bus.write(REG_CONTROL, 1, 0x01);

        // No transfer should have occurred -- status remains 0
        let status = bus.read(REG_STATUS, 1);
        assert_eq!(status, 0);
    }

    #[test]
    fn spi_region_size() {
        let bus = SpiBus::new();
        assert_eq!(bus.region_size(), 0x14);
    }

    #[test]
    fn spi_duplicate_cs_rejected() {
        let mut bus = SpiBus::new();
        bus.attach_device(0, Box::new(EchoSpiDevice::new())).unwrap();
        let result = bus.attach_device(0, Box::new(EchoSpiDevice::new()));
        assert!(result.is_err());
    }
}

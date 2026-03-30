//! ARM PL011 UART -- PrimeCell UART (PL011) Technical Reference Manual.
//!
//! Implements the PL011 register map as used on ARM Versatile, Realview,
//! and QEMU virt platforms. Connects to a [`CharBackend`] for actual I/O.
//!
//! # Register map (offsets from device base, 4-byte spacing)
//!
//! | Offset | Name      | R/W | Description                           |
//! |--------|-----------|-----|---------------------------------------|
//! | 0x000  | UARTDR    | RW  | Data register (TX write / RX read)    |
//! | 0x004  | UARTRSR   | RW  | Receive status / error clear          |
//! | 0x018  | UARTFR    | R   | Flag register                         |
//! | 0x024  | UARTILPR  | RW  | IrDA low-power counter                |
//! | 0x028  | UARTIBRD  | RW  | Integer baud rate divisor             |
//! | 0x02C  | UARTFBRD  | RW  | Fractional baud rate divisor          |
//! | 0x030  | UARTLCR_H | RW  | Line control register                 |
//! | 0x034  | UARTCR    | RW  | Control register                      |
//! | 0x038  | UARTIFLS  | RW  | Interrupt FIFO level select           |
//! | 0x03C  | UARTIMSC  | RW  | Interrupt mask set/clear              |
//! | 0x040  | UARTRIS   | R   | Raw interrupt status                  |
//! | 0x044  | UARTMIS   | R   | Masked interrupt status               |
//! | 0x048  | UARTICR   | W   | Interrupt clear register              |
//! | 0x04C  | UARTDMACR | RW  | DMA control register                  |
//! | 0xFE0  | PeriphID0 | R   | Peripheral ID byte 0                  |
//! | 0xFE4  | PeriphID1 | R   | Peripheral ID byte 1                  |
//! | 0xFE8  | PeriphID2 | R   | Peripheral ID byte 2                  |
//! | 0xFEC  | PeriphID3 | R   | Peripheral ID byte 3                  |
//! | 0xFF0  | CellID0   | R   | PrimeCell ID byte 0                   |
//! | 0xFF4  | CellID1   | R   | PrimeCell ID byte 1                   |
//! | 0xFF8  | CellID2   | R   | PrimeCell ID byte 2                   |
//! | 0xFFC  | CellID3   | R   | PrimeCell ID byte 3                   |

use std::collections::VecDeque;

use helm_devices::{CharBackend, Device, InterruptPin};

// ── Register offsets ────────────────────────────────────────────────────────

const UARTDR: u64 = 0x000;
const UARTRSR: u64 = 0x004;
const UARTFR: u64 = 0x018;
const UARTILPR: u64 = 0x024;
const UARTIBRD: u64 = 0x028;
const UARTFBRD: u64 = 0x02C;
const UARTLCR_H: u64 = 0x030;
const UARTCR: u64 = 0x034;
const UARTIFLS: u64 = 0x038;
const UARTIMSC: u64 = 0x03C;
const UARTRIS: u64 = 0x040;
const UARTMIS: u64 = 0x044;
const UARTICR: u64 = 0x048;
const UARTDMACR: u64 = 0x04C;

// Identification registers (PL011 r1p5)
const PERIPH_ID0: u64 = 0xFE0;
const PERIPH_ID1: u64 = 0xFE4;
const PERIPH_ID2: u64 = 0xFE8;
const PERIPH_ID3: u64 = 0xFEC;
const CELL_ID0: u64 = 0xFF0;
const CELL_ID1: u64 = 0xFF4;
const CELL_ID2: u64 = 0xFF8;
const CELL_ID3: u64 = 0xFFC;

// ── Flag register bits ──────────────────────────────────────────────────────

/// Receive FIFO empty.
const FR_RXFE: u32 = 1 << 4;
/// Transmit FIFO full.
#[allow(dead_code)]
const FR_TXFF: u32 = 1 << 5;
/// Receive FIFO full.
const FR_RXFF: u32 = 1 << 6;
/// Transmit FIFO empty.
const FR_TXFE: u32 = 1 << 7;

// ── Control register bits ───────────────────────────────────────────────────

/// UART enable.
const CR_UARTEN: u32 = 1 << 0;
/// Transmit enable.
const CR_TXE: u32 = 1 << 8;
/// Receive enable.
const CR_RXE: u32 = 1 << 9;

// ── Interrupt bits ──────────────────────────────────────────────────────────

/// Transmit interrupt.
const INT_TX: u32 = 1 << 5;
/// Receive interrupt.
const INT_RX: u32 = 1 << 4;

// ── Line control bits ───────────────────────────────────────────────────────

/// FIFO enable bit in LCR_H.
const LCR_H_FEN: u32 = 1 << 4;

// ── FIFO depth ──────────────────────────────────────────────────────────────

const FIFO_DEPTH: usize = 16;

// ── Pl011 ───────────────────────────────────────────────────────────────────

/// ARM PL011 UART device.
///
/// Implements the [`Device`] trait for MMIO access. Uses a [`CharBackend`]
/// for host-side serial I/O.
///
/// The device knows no base address or IRQ number -- those are platform
/// configuration concerns.
pub struct Pl011 {
    backend: Box<dyn CharBackend>,
    rx_fifo: VecDeque<u8>,
    rsr: u32,
    ilpr: u32,
    ibrd: u32,
    fbrd: u32,
    lcr_h: u32,
    cr: u32,
    ifls: u32,
    imsc: u32,
    ris: u32,
    dmacr: u32,
    /// Interrupt output pin.
    pub irq_out: InterruptPin,
}

impl Pl011 {
    /// Create a new PL011 UART with the given character backend.
    pub fn new(backend: Box<dyn CharBackend>) -> Self {
        Self {
            backend,
            rx_fifo: VecDeque::with_capacity(FIFO_DEPTH),
            rsr: 0,
            ilpr: 0,
            ibrd: 0,
            fbrd: 0,
            lcr_h: 0,
            cr: CR_UARTEN | CR_TXE | CR_RXE,
            ifls: 0x12, // 1/2 full trigger level
            imsc: 0,
            ris: INT_TX, // TX FIFO starts empty
            dmacr: 0,
            irq_out: InterruptPin::new(),
        }
    }

    /// Whether FIFOs are enabled.
    fn fifo_enabled(&self) -> bool {
        self.lcr_h & LCR_H_FEN != 0
    }

    /// Effective FIFO depth.
    fn fifo_depth(&self) -> usize {
        if self.fifo_enabled() {
            FIFO_DEPTH
        } else {
            1
        }
    }

    /// Compute the flag register dynamically.
    fn compute_fr(&self) -> u32 {
        let mut fr = 0u32;
        if self.rx_fifo.is_empty() {
            fr |= FR_RXFE;
        }
        if self.rx_fifo.len() >= self.fifo_depth() {
            fr |= FR_RXFF;
        }
        // TX is always ready in simulation
        fr |= FR_TXFE;
        fr
    }

    /// Pull available data from the backend into the RX FIFO.
    fn fill_rx_fifo(&mut self) {
        while self.rx_fifo.len() < self.fifo_depth() {
            if let Some(byte) = self.backend.read() {
                self.rx_fifo.push_back(byte);
                self.ris |= INT_RX;
            } else {
                break;
            }
        }
    }

    /// Update the IRQ pin based on masked interrupt status.
    fn update_irq(&mut self) {
        let pending = (self.ris & self.imsc) != 0;
        if pending {
            self.irq_out.assert();
        } else {
            self.irq_out.deassert();
        }
    }
}

impl Device for Pl011 {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        let val = match offset {
            UARTDR => {
                self.fill_rx_fifo();
                if let Some(byte) = self.rx_fifo.pop_front() {
                    if self.rx_fifo.is_empty() {
                        self.ris &= !INT_RX;
                    }
                    self.update_irq();
                    byte as u32
                } else {
                    0
                }
            }
            UARTRSR => self.rsr,
            UARTFR => {
                self.fill_rx_fifo();
                self.compute_fr()
            }
            UARTILPR => self.ilpr,
            UARTIBRD => self.ibrd,
            UARTFBRD => self.fbrd,
            UARTLCR_H => self.lcr_h,
            UARTCR => self.cr,
            UARTIFLS => self.ifls,
            UARTIMSC => self.imsc,
            UARTRIS => {
                self.fill_rx_fifo();
                self.ris
            }
            UARTMIS => {
                self.fill_rx_fifo();
                self.ris & self.imsc
            }
            UARTDMACR => self.dmacr,
            // PL011 identification (r1p5, part 0x011)
            PERIPH_ID0 => 0x11,
            PERIPH_ID1 => 0x10,
            PERIPH_ID2 => 0x34,
            PERIPH_ID3 => 0x00,
            CELL_ID0 => 0x0D,
            CELL_ID1 => 0xF0,
            CELL_ID2 => 0x05,
            CELL_ID3 => 0xB1,
            _ => 0,
        };
        val as u64
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        let val32 = val as u32;
        match offset {
            UARTDR => {
                // TX: write byte to backend
                if self.cr & CR_UARTEN != 0 && self.cr & CR_TXE != 0 {
                    let byte = val32 as u8;
                    self.backend.write(&[byte]);
                }
                self.ris |= INT_TX; // TX FIFO empty (instant transmission)
                self.update_irq();
            }
            UARTRSR => {
                // Write clears error flags
                self.rsr = 0;
            }
            UARTILPR => self.ilpr = val32,
            UARTIBRD => self.ibrd = val32,
            UARTFBRD => self.fbrd = val32 & 0x3F,
            UARTLCR_H => {
                self.lcr_h = val32;
                // When FIFOs are disabled, flush to 1 entry
                if !self.fifo_enabled() {
                    self.rx_fifo.truncate(1);
                }
            }
            UARTCR => self.cr = val32,
            UARTIFLS => self.ifls = val32 & 0x3F,
            UARTIMSC => {
                self.imsc = val32;
                self.update_irq();
            }
            UARTICR => {
                // Write-1-to-clear raw interrupt bits
                self.ris &= !val32;
                self.update_irq();
            }
            UARTDMACR => self.dmacr = val32,
            _ => {} // Undefined / read-only registers are silently ignored
        }
    }

    fn region_size(&self) -> u64 {
        0x1000 // 4 KiB
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use helm_devices::BufferCharBackend;
    use std::sync::{Arc, Mutex};

    fn make_uart() -> Pl011 {
        Pl011::new(Box::new(BufferCharBackend::new()))
    }

    #[test]
    fn identification_registers() {
        let mut uart = make_uart();
        assert_eq!(uart.read(PERIPH_ID0, 4), 0x11);
        assert_eq!(uart.read(PERIPH_ID1, 4), 0x10);
        assert_eq!(uart.read(PERIPH_ID2, 4), 0x34);
        assert_eq!(uart.read(CELL_ID0, 4), 0x0D);
    }

    /// Shared char backend using Arc<Mutex> for safe test-side access.
    struct SharedCharBackend {
        inner: Arc<Mutex<BufferCharBackend>>,
    }

    impl SharedCharBackend {
        fn new(inner: Arc<Mutex<BufferCharBackend>>) -> Self {
            Self { inner }
        }
    }

    impl CharBackend for SharedCharBackend {
        fn write(&mut self, data: &[u8]) -> usize {
            self.inner.lock().unwrap().write(data)
        }
        fn read(&mut self) -> Option<u8> {
            self.inner.lock().unwrap().read()
        }
        fn can_write(&self) -> bool {
            self.inner.lock().unwrap().can_write()
        }
        fn can_read(&self) -> bool {
            self.inner.lock().unwrap().can_read()
        }
    }

    #[test]
    fn tx_write_goes_to_backend() {
        let shared = Arc::new(Mutex::new(BufferCharBackend::new()));
        let backend = SharedCharBackend::new(Arc::clone(&shared));
        let mut uart = Pl011::new(Box::new(backend));

        uart.write(UARTDR, 4, b'H' as u64);
        uart.write(UARTDR, 4, b'i' as u64);

        let output = shared.lock().unwrap().drain_tx();
        assert_eq!(output, b"Hi");
    }

    #[test]
    fn rx_from_backend() {
        let mut backend = BufferCharBackend::new();
        backend.inject_rx(b"AB");
        let mut uart = Pl011::new(Box::new(backend));

        // Read first byte
        let a = uart.read(UARTDR, 4);
        assert_eq!(a, b'A' as u64);

        // Read second byte
        let b = uart.read(UARTDR, 4);
        assert_eq!(b, b'B' as u64);

        // FIFO empty -- should return 0
        let empty = uart.read(UARTDR, 4);
        assert_eq!(empty, 0);
    }

    #[test]
    fn flag_register_rx_empty() {
        let mut uart = make_uart();
        let fr = uart.read(UARTFR, 4) as u32;
        assert_ne!(fr & FR_RXFE, 0, "RX FIFO should be empty");
        assert_ne!(fr & FR_TXFE, 0, "TX FIFO should be empty");
    }

    #[test]
    fn flag_register_rx_not_empty() {
        let mut backend = BufferCharBackend::new();
        backend.inject_rx(b"X");
        let mut uart = Pl011::new(Box::new(backend));

        let fr = uart.read(UARTFR, 4) as u32;
        assert_eq!(fr & FR_RXFE, 0, "RX FIFO should not be empty");
    }

    #[test]
    fn interrupt_mask_and_clear() {
        let mut uart = make_uart();

        // TX interrupt should be raw-set on init
        let ris = uart.read(UARTRIS, 4) as u32;
        assert_ne!(ris & INT_TX, 0);

        // But masked interrupt should be 0 (mask is 0)
        let mis = uart.read(UARTMIS, 4) as u32;
        assert_eq!(mis, 0);

        // Enable TX interrupt mask
        uart.write(UARTIMSC, 4, INT_TX as u64);
        let mis = uart.read(UARTMIS, 4) as u32;
        assert_ne!(mis & INT_TX, 0);

        // Clear TX interrupt
        uart.write(UARTICR, 4, INT_TX as u64);
        let ris = uart.read(UARTRIS, 4) as u32;
        assert_eq!(ris & INT_TX, 0);
    }

    #[test]
    fn region_size() {
        let uart = make_uart();
        assert_eq!(uart.region_size(), 0x1000);
    }

    #[test]
    fn control_register_writable() {
        let mut uart = make_uart();
        uart.write(UARTCR, 4, 0);
        assert_eq!(uart.read(UARTCR, 4), 0);

        uart.write(UARTCR, 4, (CR_UARTEN | CR_TXE) as u64);
        assert_eq!(uart.read(UARTCR, 4), (CR_UARTEN | CR_TXE) as u64);
    }

    #[test]
    fn error_status_clear_on_write() {
        let mut uart = make_uart();
        // Writing to RSR should clear error flags
        uart.write(UARTRSR, 4, 0xFF);
        assert_eq!(uart.read(UARTRSR, 4), 0);
    }
}

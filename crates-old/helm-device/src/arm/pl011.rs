//! ARM PL011 UART — PrimeCell UART (PL011) Technical Reference Manual.
//!
//! Implements the full PL011 register map as used on ARM Versatile,
//! Realview, and virt QEMU platforms. Connects to a [`CharBackend`]
//! for actual I/O.
//!
//! # Register Map (offsets from device base)
//!
//! | Offset | Name   | R/W | Description |
//! |--------|--------|-----|-------------|
//! | 0x000  | UARTDR | RW  | Data register |
//! | 0x004  | UARTRSR| RW  | Receive status / error clear |
//! | 0x018  | UARTFR | R   | Flag register |
//! | 0x024  | UARTILPR| RW | IrDA low-power counter |
//! | 0x028  | UARTIBRD| RW | Integer baud rate divisor |
//! | 0x02C  | UARTFBRD| RW | Fractional baud rate divisor |
//! | 0x030  | UARTLCR_H| RW| Line control register |
//! | 0x034  | UARTCR | RW  | Control register |
//! | 0x038  | UARTIFLS| RW | Interrupt FIFO level select |
//! | 0x03C  | UARTIMSC| RW | Interrupt mask set/clear |
//! | 0x040  | UARTRIS| R   | Raw interrupt status |
//! | 0x044  | UARTMIS| R   | Masked interrupt status |
//! | 0x048  | UARTICR| W   | Interrupt clear register |
//! | 0x04C  | UARTDMACR| RW| DMA control register |
//! | 0xFE0–0xFFC | PeriphID/CellID | R | Identification registers |

use crate::backend::CharBackend;
use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;
use std::collections::VecDeque;

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

// Identification registers (PL011 rev r1p5)
const PERIPH_ID0: u64 = 0xFE0;
const PERIPH_ID1: u64 = 0xFE4;
const PERIPH_ID2: u64 = 0xFE8;
const PERIPH_ID3: u64 = 0xFEC;
const CELL_ID0: u64 = 0xFF0;
const CELL_ID1: u64 = 0xFF4;
const CELL_ID2: u64 = 0xFF8;
const CELL_ID3: u64 = 0xFFC;

// ── Flag register bits ──────────────────────────────────────────────────────

/// Clear to send.
pub const FR_CTS: u32 = 1 << 0;
/// Data set ready.
pub const FR_DSR: u32 = 1 << 1;
/// Data carrier detect.
pub const FR_DCD: u32 = 1 << 2;
/// UART busy (transmitting data).
pub const FR_BUSY: u32 = 1 << 3;
/// Receive FIFO empty.
pub const FR_RXFE: u32 = 1 << 4;
/// Transmit FIFO full.
pub const FR_TXFF: u32 = 1 << 5;
/// Receive FIFO full.
pub const FR_RXFF: u32 = 1 << 6;
/// Transmit FIFO empty.
pub const FR_TXFE: u32 = 1 << 7;

// ── Control register bits ───────────────────────────────────────────────────

/// UART enable.
pub const CR_UARTEN: u32 = 1 << 0;
/// SIR enable (IrDA).
pub const CR_SIREN: u32 = 1 << 1;
/// SIR low-power mode.
pub const CR_SIRLP: u32 = 1 << 2;
/// Loopback enable.
pub const CR_LBE: u32 = 1 << 7;
/// Transmit enable.
pub const CR_TXE: u32 = 1 << 8;
/// Receive enable.
pub const CR_RXE: u32 = 1 << 9;

// ── Interrupt bits ──────────────────────────────────────────────────────────

pub const INT_OE: u32 = 1 << 10; // Overrun error
pub const INT_BE: u32 = 1 << 9; // Break error
pub const INT_PE: u32 = 1 << 8; // Parity error
pub const INT_FE: u32 = 1 << 7; // Framing error
pub const INT_RT: u32 = 1 << 6; // Receive timeout
pub const INT_TX: u32 = 1 << 5; // Transmit
pub const INT_RX: u32 = 1 << 4; // Receive
pub const INT_DSRM: u32 = 1 << 3; // DSR modem
pub const INT_DCDM: u32 = 1 << 2; // DCD modem
pub const INT_CTSM: u32 = 1 << 1; // CTS modem
pub const INT_RIM: u32 = 1 << 0; // RI modem

// ── Line control bits ───────────────────────────────────────────────────────

pub const LCR_H_FEN: u32 = 1 << 4; // FIFO enable
pub const LCR_H_WLEN_MASK: u32 = 3 << 5; // Word length

// ── FIFO depth ──────────────────────────────────────────────────────────────

const FIFO_DEPTH: usize = 16;

/// ARM PL011 UART device.
///
/// Implements the Device trait so it can be attached to a DeviceBus
/// (typically an AMBA APB bus). Uses a [`CharBackend`] for actual I/O.
pub struct Pl011 {
    /// Character backend (stdio, buffer, null, socket, etc.).
    backend: Box<dyn CharBackend>,
    /// Device name.
    dev_name: String,
    /// Region for Device trait.
    region: MemRegion,

    // ── Registers ───────────────────────────────────────────────────────
    /// Receive FIFO.
    rx_fifo: VecDeque<u8>,
    /// Receive status/error.
    rsr: u32,
    /// Flag register (computed dynamically).
    // fr: computed
    /// IrDA low-power counter.
    ilpr: u32,
    /// Integer baud rate divisor.
    ibrd: u32,
    /// Fractional baud rate divisor.
    fbrd: u32,
    /// Line control register.
    lcr_h: u32,
    /// Control register.
    cr: u32,
    /// Interrupt FIFO level select.
    ifls: u32,
    /// Interrupt mask set/clear.
    imsc: u32,
    /// Raw interrupt status.
    ris: u32,
    /// DMA control register.
    dmacr: u32,

    /// IRQ output line (updated after every register access).
    pub irq_level: bool,
}

impl Pl011 {
    /// Create a new PL011 UART with the given backend.
    pub fn new(name: impl Into<String>, backend: Box<dyn CharBackend>) -> Self {
        let n = name.into();
        Self {
            backend,
            dev_name: n.clone(),
            region: MemRegion {
                name: n,
                base: 0,
                size: 0x1000,
                kind: crate::region::RegionKind::Io,
                priority: 0,
            },
            rx_fifo: VecDeque::with_capacity(FIFO_DEPTH),
            rsr: 0,
            ilpr: 0,
            ibrd: 0,
            fbrd: 0,
            lcr_h: 0,
            cr: CR_UARTEN | CR_TXE | CR_RXE, // UART enabled (matches QEMU reset)
            ifls: 0x12,                      // 1/2 full trigger level
            imsc: 0,
            ris: INT_TX, // TX FIFO starts empty → TX interrupt
            dmacr: 0,
            irq_level: false,
        }
    }

    /// Access the backend.
    pub fn backend(&self) -> &dyn CharBackend {
        self.backend.as_ref()
    }

    /// Mutably access the backend.
    pub fn backend_mut(&mut self) -> &mut dyn CharBackend {
        self.backend.as_mut()
    }

    /// Whether FIFOs are enabled.
    fn fifo_enabled(&self) -> bool {
        self.lcr_h & LCR_H_FEN != 0
    }

    /// FIFO depth (1 when FIFOs disabled, 16 when enabled).
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
        // TX is always ready in simulation (instantaneous)
        fr |= FR_TXFE;
        // CTS always asserted
        fr |= FR_CTS;
        fr
    }

    /// Try to pull data from the backend into the RX FIFO.
    fn fill_rx_fifo(&mut self) {
        while self.rx_fifo.len() < self.fifo_depth() && self.backend.can_read() {
            let mut buf = [0u8; 1];
            if let Ok(1) = self.backend.read(&mut buf) {
                self.rx_fifo.push_back(buf[0]);
                self.ris |= INT_RX;
            } else {
                break;
            }
        }
    }

    /// Update the IRQ output based on masked interrupt status.
    fn update_irq(&mut self) {
        self.irq_level = (self.ris & self.imsc) != 0;
    }

    fn handle_read(&mut self, offset: u64) -> u32 {
        match offset {
            UARTDR => {
                // Reading DR pops from RX FIFO
                self.fill_rx_fifo();
                if let Some(byte) = self.rx_fifo.pop_front() {
                    // Clear RX interrupt if FIFO below threshold
                    if self.rx_fifo.is_empty() {
                        self.ris &= !INT_RX;
                        self.ris &= !INT_RT;
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

            // Identification registers (PL011 r1p5, part number 0x011)
            PERIPH_ID0 => 0x11,
            PERIPH_ID1 => 0x10,
            PERIPH_ID2 => 0x34, // revision 3, designer 0x4 (ARM)
            PERIPH_ID3 => 0x00,
            CELL_ID0 => 0x0D,
            CELL_ID1 => 0xF0,
            CELL_ID2 => 0x05,
            CELL_ID3 => 0xB1,

            _ => 0,
        }
    }

    fn handle_write(&mut self, offset: u64, value: u32) {
        match offset {
            UARTDR => {
                // Write byte to TX
                if self.cr & CR_UARTEN != 0 && self.cr & CR_TXE != 0 {
                    let byte = (value & 0xFF) as u8;
                    if self.cr & CR_LBE != 0 {
                        // Loopback: push to RX FIFO
                        if self.rx_fifo.len() < self.fifo_depth() {
                            self.rx_fifo.push_back(byte);
                            self.ris |= INT_RX;
                        }
                    } else {
                        let _ = self.backend.write(&[byte]);
                    }
                    // Don't re-assert INT_TX here.  With instantaneous TX
                    // the FIFO never transitions through the trigger level.
                    // The initial INT_TX (set at reset) lets the driver
                    // know TX is available; after the driver clears it via
                    // UARTICR the flag stays clear until the next reset.
                }
            }
            UARTRSR => {
                // Write clears error flags
                self.rsr = 0;
            }
            UARTILPR => self.ilpr = value,
            UARTIBRD => self.ibrd = value & 0xFFFF,
            UARTFBRD => self.fbrd = value & 0x3F,
            UARTLCR_H => {
                self.lcr_h = value;
                // Writing LCR_H flushes the FIFOs per spec
                self.rx_fifo.clear();
            }
            UARTCR => self.cr = value,
            UARTIFLS => self.ifls = value & 0x3F,
            UARTIMSC => self.imsc = value & 0x7FF,
            UARTICR => {
                // Write 1 to clear interrupt bits
                self.ris &= !value;
            }
            UARTDMACR => self.dmacr = value & 0x7,
            _ => {}
        }
        self.update_irq();
    }
}

impl Device for Pl011 {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if txn.is_write {
            self.handle_write(txn.offset, txn.data_u32());
        } else {
            let val = self.handle_read(txn.offset);
            txn.set_data_u32(val);
        }
        txn.stall_cycles += 1;
        Ok(())
    }

    fn regions(&self) -> &[MemRegion] {
        std::slice::from_ref(&self.region)
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.rx_fifo.clear();
        self.rsr = 0;
        self.ilpr = 0;
        self.ibrd = 0;
        self.fbrd = 0;
        self.lcr_h = 0;
        self.cr = CR_UARTEN | CR_TXE | CR_RXE;
        self.ifls = 0x12;
        self.imsc = 0;
        self.ris = INT_TX;
        self.dmacr = 0;
        self.irq_level = false;
        Ok(())
    }

    fn tick(&mut self, _cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        self.fill_rx_fifo();
        self.update_irq();
        let mut events = Vec::new();
        if self.irq_level {
            events.push(DeviceEvent::Irq {
                line: 0,
                assert: true,
            });
        }
        Ok(events)
    }

    fn checkpoint(&self) -> HelmResult<serde_json::Value> {
        Ok(serde_json::json!({
            "rsr": self.rsr,
            "ilpr": self.ilpr,
            "ibrd": self.ibrd,
            "fbrd": self.fbrd,
            "lcr_h": self.lcr_h,
            "cr": self.cr,
            "ifls": self.ifls,
            "imsc": self.imsc,
            "ris": self.ris,
            "dmacr": self.dmacr,
            "rx_fifo": self.rx_fifo.iter().copied().collect::<Vec<u8>>(),
        }))
    }

    fn restore(&mut self, state: &serde_json::Value) -> HelmResult<()> {
        if let Some(obj) = state.as_object() {
            if let Some(v) = obj.get("rsr").and_then(|v| v.as_u64()) {
                self.rsr = v as u32;
            }
            if let Some(v) = obj.get("ilpr").and_then(|v| v.as_u64()) {
                self.ilpr = v as u32;
            }
            if let Some(v) = obj.get("ibrd").and_then(|v| v.as_u64()) {
                self.ibrd = v as u32;
            }
            if let Some(v) = obj.get("fbrd").and_then(|v| v.as_u64()) {
                self.fbrd = v as u32;
            }
            if let Some(v) = obj.get("lcr_h").and_then(|v| v.as_u64()) {
                self.lcr_h = v as u32;
            }
            if let Some(v) = obj.get("cr").and_then(|v| v.as_u64()) {
                self.cr = v as u32;
            }
            if let Some(v) = obj.get("ifls").and_then(|v| v.as_u64()) {
                self.ifls = v as u32;
            }
            if let Some(v) = obj.get("imsc").and_then(|v| v.as_u64()) {
                self.imsc = v as u32;
            }
            if let Some(v) = obj.get("ris").and_then(|v| v.as_u64()) {
                self.ris = v as u32;
            }
            if let Some(v) = obj.get("dmacr").and_then(|v| v.as_u64()) {
                self.dmacr = v as u32;
            }
            if let Some(arr) = obj.get("rx_fifo").and_then(|v| v.as_array()) {
                self.rx_fifo.clear();
                for v in arr {
                    if let Some(b) = v.as_u64() {
                        self.rx_fifo.push_back(b as u8);
                    }
                }
            }
        }
        self.update_irq();
        Ok(())
    }

    fn name(&self) -> &str {
        &self.dev_name
    }

    // ── Fast path (FE mode) — skip Transaction + stall accumulation ─────

    fn read_fast(&mut self, offset: Addr, _size: usize) -> HelmResult<u64> {
        Ok(self.handle_read(offset) as u64)
    }

    fn write_fast(&mut self, offset: Addr, _size: usize, value: u64) -> HelmResult<()> {
        self.handle_write(offset, value as u32);
        Ok(())
    }
}

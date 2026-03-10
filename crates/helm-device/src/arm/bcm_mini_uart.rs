//! BCM2837 Mini UART — BCM2835 ARM Peripherals §2.2.
//!
//! 16550-like mini UART (UART1). Simpler than PL011 — no FIFO depth
//! control, fixed 8-bit data, limited baud rate options.

use crate::backend::CharBackend;
use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;
use std::collections::VecDeque;

// Register offsets (relative to AUX base + 0x40)
const AUX_MU_IO: u64 = 0x00; // I/O data
const AUX_MU_IER: u64 = 0x04; // Interrupt enable
const AUX_MU_IIR: u64 = 0x08; // Interrupt identify
const AUX_MU_LCR: u64 = 0x0C; // Line control
const AUX_MU_MCR: u64 = 0x10; // Modem control
const AUX_MU_LSR: u64 = 0x14; // Line status
const AUX_MU_MSR: u64 = 0x18; // Modem status
const AUX_MU_SCRATCH: u64 = 0x1C; // Scratch
const AUX_MU_CNTL: u64 = 0x20; // Extra control
const AUX_MU_STAT: u64 = 0x24; // Extra status
const AUX_MU_BAUD: u64 = 0x28; // Baudrate

// LSR bits
const LSR_DATA_READY: u32 = 1 << 0;
const LSR_TX_EMPTY: u32 = 1 << 5;
const LSR_TX_IDLE: u32 = 1 << 6;

pub struct BcmMiniUart {
    dev_name: String,
    region: MemRegion,
    backend: Box<dyn CharBackend>,
    rx_fifo: VecDeque<u8>,
    ier: u32,
    lcr: u32,
    mcr: u32,
    scratch: u32,
    cntl: u32,
    baud: u32,
    pub irq_level: bool,
}

impl BcmMiniUart {
    pub fn new(name: impl Into<String>, backend: Box<dyn CharBackend>) -> Self {
        let n = name.into();
        Self {
            region: MemRegion {
                name: n.clone(),
                base: 0,
                size: 0x1000,
                kind: crate::region::RegionKind::Io,
                priority: 0,
            },
            dev_name: n,
            backend,
            rx_fifo: VecDeque::with_capacity(8),
            ier: 0,
            lcr: 0,
            mcr: 0,
            scratch: 0,
            cntl: 3,   // TX + RX enabled
            baud: 270, // ~115200 at 250 MHz
            irq_level: false,
        }
    }

    fn fill_rx(&mut self) {
        while self.rx_fifo.len() < 8 && self.backend.can_read() {
            let mut buf = [0u8; 1];
            if let Ok(1) = self.backend.read(&mut buf) {
                self.rx_fifo.push_back(buf[0]);
            } else {
                break;
            }
        }
    }

    fn handle_read(&mut self, offset: u64) -> u32 {
        match offset {
            AUX_MU_IO => {
                self.fill_rx();
                self.rx_fifo.pop_front().unwrap_or(0) as u32
            }
            AUX_MU_IER => self.ier,
            AUX_MU_IIR => {
                // Bit 0: 0 = interrupt pending, 1 = no interrupt
                let mut iir = 1u32; // no interrupt
                if !self.rx_fifo.is_empty() && self.ier & 1 != 0 {
                    iir = 0x04; // RX data available
                }
                iir
            }
            AUX_MU_LCR => self.lcr,
            AUX_MU_MCR => self.mcr,
            AUX_MU_LSR => {
                self.fill_rx();
                let mut lsr = LSR_TX_EMPTY | LSR_TX_IDLE;
                if !self.rx_fifo.is_empty() {
                    lsr |= LSR_DATA_READY;
                }
                lsr
            }
            AUX_MU_MSR => 0x20, // CTS asserted
            AUX_MU_SCRATCH => self.scratch,
            AUX_MU_CNTL => self.cntl,
            AUX_MU_STAT => {
                self.fill_rx();
                let mut stat = 0u32;
                if !self.rx_fifo.is_empty() {
                    stat |= 1;
                } // symbol available
                stat |= 1 << 1; // space available for TX
                stat |= (self.rx_fifo.len() as u32 & 0xF) << 16;
                stat
            }
            AUX_MU_BAUD => self.baud,
            _ => 0,
        }
    }

    fn handle_write(&mut self, offset: u64, value: u32) {
        match offset {
            AUX_MU_IO => {
                if self.cntl & 2 != 0 {
                    // TX enabled
                    let _ = self.backend.write(&[(value & 0xFF) as u8]);
                }
            }
            AUX_MU_IER => self.ier = value & 3,
            AUX_MU_IIR => {
                // Writing bit 1 clears RX FIFO, bit 2 clears TX FIFO
                if value & 2 != 0 {
                    self.rx_fifo.clear();
                }
            }
            AUX_MU_LCR => self.lcr = value,
            AUX_MU_MCR => self.mcr = value & 3,
            AUX_MU_SCRATCH => self.scratch = value & 0xFF,
            AUX_MU_CNTL => self.cntl = value & 0xFF,
            AUX_MU_BAUD => self.baud = value & 0xFFFF,
            _ => {}
        }
    }
}

impl Device for BcmMiniUart {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if txn.is_write {
            self.handle_write(txn.offset, txn.data_u32());
        } else {
            txn.set_data_u32(self.handle_read(txn.offset));
        }
        txn.stall_cycles += 1;
        Ok(())
    }

    fn regions(&self) -> &[MemRegion] {
        std::slice::from_ref(&self.region)
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.rx_fifo.clear();
        self.ier = 0;
        self.lcr = 0;
        self.mcr = 0;
        self.scratch = 0;
        self.cntl = 3;
        self.baud = 270;
        self.irq_level = false;
        Ok(())
    }

    fn read_fast(&mut self, offset: Addr, _s: usize) -> HelmResult<u64> {
        Ok(self.handle_read(offset) as u64)
    }
    fn write_fast(&mut self, offset: Addr, _s: usize, v: u64) -> HelmResult<()> {
        self.handle_write(offset, v as u32);
        Ok(())
    }

    fn name(&self) -> &str {
        &self.dev_name
    }

    fn tick(&mut self, _cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        let rx_irq = !self.rx_fifo.is_empty() && (self.ier & 1) != 0;
        let tx_irq = (self.ier & 2) != 0;
        if rx_irq || tx_irq {
            Ok(vec![DeviceEvent::Irq {
                line: 29,
                assert: true,
            }])
        } else {
            Ok(vec![])
        }
    }
}

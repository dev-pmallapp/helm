//! BCM2837 GPIO — BCM2835 ARM Peripherals §6.
//!
//! 54 GPIO pins with function select (input, output, alt0-5),
//! pull-up/down control, and event detect.

use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;

// Register offsets
const GPFSEL0: u64 = 0x00;  // Function select 0-5 (6 registers, 10 pins each)
const GPSET0: u64 = 0x1C;   // Output set 0-1
const GPCLR0: u64 = 0x28;   // Output clear 0-1
const GPLEV0: u64 = 0x34;   // Pin level 0-1
const GPEDS0: u64 = 0x40;   // Event detect status 0-1
const GPREN0: u64 = 0x4C;   // Rising edge detect 0-1
const GPFEN0: u64 = 0x58;   // Falling edge detect 0-1
const GPHEN0: u64 = 0x64;   // High detect 0-1
const GPLEN0: u64 = 0x70;   // Low detect 0-1
const GPPUD: u64 = 0x94;    // Pull-up/down enable
const GPPUDCLK0: u64 = 0x98; // Pull-up/down clock 0-1

pub struct BcmGpio {
    dev_name: String,
    region: MemRegion,
    /// Function select (6 regs × 32 bits = 54 pins × 3 bits each).
    fsel: [u32; 6],
    /// Output level (2 × 32 bits for 54 pins).
    level: [u32; 2],
    /// Event detect status.
    eds: [u32; 2],
    /// Rising edge detect enable.
    ren: [u32; 2],
    /// Falling edge detect enable.
    fen: [u32; 2],
    /// High detect enable.
    hen: [u32; 2],
    /// Low detect enable.
    len: [u32; 2],
    /// Pull-up/down control.
    pud: u32,
    pud_clk: [u32; 2],
    /// Previous level for edge detection.
    old_level: [u32; 2],
}

impl BcmGpio {
    pub fn new(name: impl Into<String>) -> Self {
        let n = name.into();
        Self {
            region: MemRegion {
                name: n.clone(), base: 0, size: 0x1000,
                kind: crate::region::RegionKind::Io, priority: 0,
            },
            dev_name: n,
            fsel: [0; 6], level: [0; 2], eds: [0; 2],
            ren: [0; 2], fen: [0; 2], hen: [0; 2], len: [0; 2],
            pud: 0, pud_clk: [0; 2], old_level: [0; 2],
        }
    }

    /// Set a pin as input with a specific level (for simulation).
    pub fn set_pin(&mut self, pin: u8, high: bool) {
        if pin >= 54 { return; }
        let reg = (pin / 32) as usize;
        let bit = pin % 32;
        if high { self.level[reg] |= 1 << bit; }
        else { self.level[reg] &= !(1 << bit); }
    }

    fn handle_read(&self, offset: u64) -> u32 {
        match offset {
            o if o >= GPFSEL0 && o < GPFSEL0 + 24 => {
                self.fsel[((o - GPFSEL0) / 4) as usize]
            }
            o if o >= GPLEV0 && o < GPLEV0 + 8 => {
                self.level[((o - GPLEV0) / 4) as usize]
            }
            o if o >= GPEDS0 && o < GPEDS0 + 8 => {
                self.eds[((o - GPEDS0) / 4) as usize]
            }
            o if o >= GPREN0 && o < GPREN0 + 8 => {
                self.ren[((o - GPREN0) / 4) as usize]
            }
            o if o >= GPFEN0 && o < GPFEN0 + 8 => {
                self.fen[((o - GPFEN0) / 4) as usize]
            }
            o if o >= GPHEN0 && o < GPHEN0 + 8 => {
                self.hen[((o - GPHEN0) / 4) as usize]
            }
            o if o >= GPLEN0 && o < GPLEN0 + 8 => {
                self.len[((o - GPLEN0) / 4) as usize]
            }
            GPPUD => self.pud,
            o if o >= GPPUDCLK0 && o < GPPUDCLK0 + 8 => {
                self.pud_clk[((o - GPPUDCLK0) / 4) as usize]
            }
            _ => 0,
        }
    }

    fn handle_write(&mut self, offset: u64, value: u32) {
        match offset {
            o if o >= GPFSEL0 && o < GPFSEL0 + 24 => {
                self.fsel[((o - GPFSEL0) / 4) as usize] = value;
            }
            o if o >= GPSET0 && o < GPSET0 + 8 => {
                let idx = ((o - GPSET0) / 4) as usize;
                self.level[idx] |= value;
            }
            o if o >= GPCLR0 && o < GPCLR0 + 8 => {
                let idx = ((o - GPCLR0) / 4) as usize;
                self.level[idx] &= !value;
            }
            o if o >= GPEDS0 && o < GPEDS0 + 8 => {
                let idx = ((o - GPEDS0) / 4) as usize;
                self.eds[idx] &= !value; // W1C
            }
            o if o >= GPREN0 && o < GPREN0 + 8 => {
                self.ren[((o - GPREN0) / 4) as usize] = value;
            }
            o if o >= GPFEN0 && o < GPFEN0 + 8 => {
                self.fen[((o - GPFEN0) / 4) as usize] = value;
            }
            o if o >= GPHEN0 && o < GPHEN0 + 8 => {
                self.hen[((o - GPHEN0) / 4) as usize] = value;
            }
            o if o >= GPLEN0 && o < GPLEN0 + 8 => {
                self.len[((o - GPLEN0) / 4) as usize] = value;
            }
            GPPUD => self.pud = value & 3,
            o if o >= GPPUDCLK0 && o < GPPUDCLK0 + 8 => {
                self.pud_clk[((o - GPPUDCLK0) / 4) as usize] = value;
            }
            _ => {}
        }
    }
}

impl Device for BcmGpio {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if txn.is_write { self.handle_write(txn.offset, txn.data_u32()); }
        else { txn.set_data_u32(self.handle_read(txn.offset)); }
        txn.stall_cycles += 1;
        Ok(())
    }

    fn regions(&self) -> &[MemRegion] { std::slice::from_ref(&self.region) }

    fn reset(&mut self) -> HelmResult<()> {
        self.fsel = [0; 6]; self.level = [0; 2]; self.eds = [0; 2];
        self.ren = [0; 2]; self.fen = [0; 2]; self.hen = [0; 2]; self.len = [0; 2];
        self.pud = 0; self.pud_clk = [0; 2]; self.old_level = [0; 2];
        Ok(())
    }

    fn read_fast(&mut self, offset: Addr, _s: usize) -> HelmResult<u64> {
        Ok(self.handle_read(offset) as u64)
    }
    fn write_fast(&mut self, offset: Addr, _s: usize, v: u64) -> HelmResult<()> {
        self.handle_write(offset, v as u32); Ok(())
    }

    fn name(&self) -> &str { &self.dev_name }
}

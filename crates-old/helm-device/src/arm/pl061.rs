//! PL061 GPIO — ARM DDI0190.
//!
//! 8-bit GPIO controller with programmable direction, interrupt
//! generation (edge/level), and alternate function support.

use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;

const GPIODATA: u64 = 0x000; // 0x000–0x3FC (address-masked data)
const GPIODIR: u64 = 0x400;
const GPIOIS: u64 = 0x404; // Interrupt sense (0=edge, 1=level)
const GPIOIBE: u64 = 0x408; // Interrupt both edges
const GPIOIEV: u64 = 0x40C; // Interrupt event (0=falling/low, 1=rising/high)
const GPIOIE: u64 = 0x410; // Interrupt mask enable
const GPIORIS: u64 = 0x414; // Raw interrupt status
const GPIOMIS: u64 = 0x418; // Masked interrupt status
const GPIOIC: u64 = 0x41C; // Interrupt clear
const GPIOAFSEL: u64 = 0x420; // Alternate function select

pub struct Pl061 {
    dev_name: String,
    region: MemRegion,
    data: u8,
    dir: u8,      // 0=input, 1=output
    is: u8,       // interrupt sense
    ibe: u8,      // interrupt both edges
    iev: u8,      // interrupt event
    ie: u8,       // interrupt mask enable
    ris: u8,      // raw interrupt status
    afsel: u8,    // alternate function
    old_data: u8, // previous data for edge detection
}

impl Pl061 {
    pub fn new(name: impl Into<String>) -> Self {
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
            data: 0,
            dir: 0,
            is: 0,
            ibe: 0,
            iev: 0,
            ie: 0,
            ris: 0,
            afsel: 0,
            old_data: 0,
        }
    }

    /// Set a pin from external input (for simulation).
    pub fn set_input(&mut self, pin: u8, high: bool) {
        if pin >= 8 {
            return;
        }
        if self.dir & (1 << pin) != 0 {
            return;
        } // output pin, ignore
        if high {
            self.data |= 1 << pin;
        } else {
            self.data &= !(1 << pin);
        }
    }

    /// Read an output pin value.
    pub fn get_output(&self, pin: u8) -> bool {
        pin < 8 && self.dir & (1 << pin) != 0 && self.data & (1 << pin) != 0
    }

    fn handle_read(&self, offset: u64) -> u32 {
        match offset {
            GPIODATA..=0x3FC => {
                // Bits [9:2] of the address select which data bits are returned
                let mask = ((offset >> 2) & 0xFF) as u8;
                (self.data & mask) as u32
            }
            GPIODIR => self.dir as u32,
            GPIOIS => self.is as u32,
            GPIOIBE => self.ibe as u32,
            GPIOIEV => self.iev as u32,
            GPIOIE => self.ie as u32,
            GPIORIS => self.ris as u32,
            GPIOMIS => (self.ris & self.ie) as u32,
            GPIOAFSEL => self.afsel as u32,
            // PrimeCell ID (PL061)
            0xFE0 => 0x61,
            0xFE4 => 0x10,
            0xFE8 => 0x04,
            0xFEC => 0x00,
            0xFF0 => 0x0D,
            0xFF4 => 0xF0,
            0xFF8 => 0x05,
            0xFFC => 0xB1,
            _ => 0,
        }
    }

    fn handle_write(&mut self, offset: u64, value: u32) {
        let v = value as u8;
        match offset {
            GPIODATA..=0x3FC => {
                let mask = ((offset >> 2) & 0xFF) as u8;
                self.data = (self.data & !mask) | (v & mask);
            }
            GPIODIR => self.dir = v,
            GPIOIS => self.is = v,
            GPIOIBE => self.ibe = v,
            GPIOIEV => self.iev = v,
            GPIOIE => self.ie = v,
            GPIOIC => self.ris &= !v,
            GPIOAFSEL => self.afsel = v,
            _ => {}
        }
    }
}

impl Device for Pl061 {
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
        self.data = 0;
        self.dir = 0;
        self.is = 0;
        self.ibe = 0;
        self.iev = 0;
        self.ie = 0;
        self.ris = 0;
        self.afsel = 0;
        self.old_data = 0;
        Ok(())
    }

    fn tick(&mut self, _cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        // Edge detection
        let changed = self.data ^ self.old_data;
        let rising = changed & self.data;
        let falling = changed & self.old_data;

        for i in 0..8u8 {
            let bit = 1u8 << i;
            if changed & bit == 0 {
                continue;
            }
            let triggered = if self.is & bit != 0 {
                // Level-sensitive
                if self.iev & bit != 0 {
                    self.data & bit != 0
                } else {
                    self.data & bit == 0
                }
            } else {
                // Edge-sensitive
                if self.ibe & bit != 0 {
                    true // both edges
                } else if self.iev & bit != 0 {
                    rising & bit != 0
                } else {
                    falling & bit != 0
                }
            };
            if triggered {
                self.ris |= bit;
            }
        }
        self.old_data = self.data;

        if self.ris & self.ie != 0 {
            Ok(vec![DeviceEvent::Irq {
                line: 0,
                assert: true,
            }])
        } else {
            Ok(vec![])
        }
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
}

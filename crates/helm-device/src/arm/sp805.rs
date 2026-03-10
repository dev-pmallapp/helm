//! SP805 Watchdog — ARM DDI0270.
//!
//! Countdown watchdog timer. If not kicked before reaching zero,
//! asserts an interrupt (and optionally resets the system).

use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;

const WDOGLOAD: u64 = 0x000;
const WDOGVALUE: u64 = 0x004;
const WDOGCONTROL: u64 = 0x008;
const WDOGINTCLR: u64 = 0x00C;
const WDOGRIS: u64 = 0x010;
const WDOGMIS: u64 = 0x014;
const WDOGLOCK: u64 = 0xC00;

const CTRL_INTEN: u32 = 1 << 0;
const CTRL_RESEN: u32 = 1 << 1;
const LOCK_MAGIC: u32 = 0x1ACC_E551;

pub struct Sp805 {
    dev_name: String,
    region: MemRegion,
    load: u32,
    value: u32,
    control: u32,
    raw_irq: bool,
    locked: bool,
}

impl Sp805 {
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
            load: 0xFFFF_FFFF,
            value: 0xFFFF_FFFF,
            control: 0,
            raw_irq: false,
            locked: false,
        }
    }

    fn handle_read(&self, offset: u64) -> u32 {
        match offset {
            WDOGLOAD => self.load,
            WDOGVALUE => self.value,
            WDOGCONTROL => self.control,
            WDOGRIS => self.raw_irq as u32,
            WDOGMIS => (self.raw_irq && (self.control & CTRL_INTEN != 0)) as u32,
            WDOGLOCK => self.locked as u32,
            0xFE0 => 0x05,
            0xFE4 => 0x18,
            0xFE8 => 0x14,
            0xFEC => 0x00,
            0xFF0 => 0x0D,
            0xFF4 => 0xF0,
            0xFF8 => 0x05,
            0xFFC => 0xB1,
            _ => 0,
        }
    }

    fn handle_write(&mut self, offset: u64, value: u32) {
        if self.locked && offset != WDOGLOCK {
            return;
        }
        match offset {
            WDOGLOAD => {
                self.load = value;
                self.value = value;
            }
            WDOGCONTROL => self.control = value & 3,
            WDOGINTCLR => {
                self.raw_irq = false;
                self.value = self.load;
            }
            WDOGLOCK => self.locked = value != LOCK_MAGIC,
            _ => {}
        }
    }
}

impl Device for Sp805 {
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
        self.load = 0xFFFF_FFFF;
        self.value = 0xFFFF_FFFF;
        self.control = 0;
        self.raw_irq = false;
        self.locked = false;
        Ok(())
    }

    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        if self.control & CTRL_INTEN == 0 {
            return Ok(vec![]);
        }
        let dec = cycles as u32;
        if self.value <= dec {
            self.raw_irq = true;
            self.value = 0;
            let mut events = vec![DeviceEvent::Irq {
                line: 0,
                assert: true,
            }];
            if self.control & CTRL_RESEN != 0 {
                events.push(DeviceEvent::Log {
                    level: crate::device::LogLevel::Error,
                    message: "watchdog reset triggered".into(),
                });
            }
            return Ok(events);
        }
        self.value -= dec;
        Ok(vec![])
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

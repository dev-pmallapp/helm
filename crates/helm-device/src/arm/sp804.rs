//! SP804 Dual Timer — ARM DDI0271.
//!
//! Two independent 32-bit countdown timers with interrupt generation.
//! Each timer has a 32-bit load value, current value, control, and
//! interrupt registers.

use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;

// Timer offsets (each timer is 0x20 apart)
const TIMER_LOAD: u64 = 0x00;
const TIMER_VALUE: u64 = 0x04;
const TIMER_CONTROL: u64 = 0x08;
const TIMER_INTCLR: u64 = 0x0C;
const TIMER_RIS: u64 = 0x10;
const TIMER_MIS: u64 = 0x14;
const TIMER_BGLOAD: u64 = 0x18;

// Control register bits
const CTRL_ENABLE: u32 = 1 << 7;
const CTRL_PERIODIC: u32 = 1 << 6;
const CTRL_INTEN: u32 = 1 << 5;
const CTRL_PRESCALE_MASK: u32 = 3 << 2;
const CTRL_32BIT: u32 = 1 << 1;
const CTRL_ONESHOT: u32 = 1 << 0;

#[derive(Debug, Clone)]
struct TimerUnit {
    load: u32,
    value: u32,
    control: u32,
    bg_load: u32,
    raw_irq: bool,
}

impl Default for TimerUnit {
    fn default() -> Self {
        Self {
            load: 0,
            value: 0xFFFF_FFFF,
            control: 0x20, // 32-bit mode by default
            bg_load: 0,
            raw_irq: false,
        }
    }
}

impl TimerUnit {
    fn enabled(&self) -> bool {
        self.control & CTRL_ENABLE != 0
    }

    fn periodic(&self) -> bool {
        self.control & CTRL_PERIODIC != 0
    }

    fn irq_enabled(&self) -> bool {
        self.control & CTRL_INTEN != 0
    }

    fn prescale_shift(&self) -> u32 {
        match (self.control & CTRL_PRESCALE_MASK) >> 2 {
            1 => 4, // divide by 16
            2 => 8, // divide by 256
            _ => 0, // no prescale
        }
    }

    fn tick(&mut self, cycles: u64) -> bool {
        if !self.enabled() {
            return false;
        }
        let shift = self.prescale_shift();
        let decrements = (cycles >> shift) as u32;
        if decrements == 0 {
            return false;
        }

        if self.value <= decrements {
            self.raw_irq = true;
            if self.periodic() {
                self.value = self.load.wrapping_sub(decrements - self.value);
            } else if self.control & CTRL_ONESHOT != 0 {
                self.value = 0;
                self.control &= !CTRL_ENABLE;
            } else {
                let wrap = if self.control & CTRL_32BIT != 0 {
                    0xFFFF_FFFF_u32
                } else {
                    0xFFFF_u32
                };
                self.value = wrap.wrapping_sub(decrements - self.value);
            }
            true
        } else {
            self.value -= decrements;
            false
        }
    }
}

/// SP804 Dual Timer device.
pub struct Sp804 {
    dev_name: String,
    region: MemRegion,
    timers: [TimerUnit; 2],
}

impl Sp804 {
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
            timers: [TimerUnit::default(), TimerUnit::default()],
        }
    }

    fn timer_for_offset(&self, offset: u64) -> (usize, u64) {
        if offset < 0x20 {
            (0, offset)
        } else {
            (1, offset - 0x20)
        }
    }

    fn handle_read(&self, offset: u64) -> u32 {
        // PrimeCell ID registers at top of page
        match offset {
            0xFE0 => return 0x04,
            0xFE4 => return 0x18,
            0xFE8 => return 0x14,
            0xFEC => return 0x00,
            0xFF0 => return 0x0D,
            0xFF4 => return 0xF0,
            0xFF8 => return 0x05,
            0xFFC => return 0xB1,
            _ => {}
        }
        let (idx, reg) = self.timer_for_offset(offset);
        let t = &self.timers[idx];
        match reg {
            TIMER_LOAD => t.load,
            TIMER_VALUE => t.value,
            TIMER_CONTROL => t.control,
            TIMER_RIS => t.raw_irq as u32,
            TIMER_MIS => (t.raw_irq && t.irq_enabled()) as u32,
            TIMER_BGLOAD => t.bg_load,
            _ => 0,
        }
    }

    fn handle_write(&mut self, offset: u64, value: u32) {
        let (idx, reg) = self.timer_for_offset(offset);
        let t = &mut self.timers[idx];
        match reg {
            TIMER_LOAD => {
                t.load = value;
                t.value = value;
            }
            TIMER_CONTROL => t.control = value,
            TIMER_INTCLR => t.raw_irq = false,
            TIMER_BGLOAD => {
                t.bg_load = value;
                t.load = value;
            }
            _ => {}
        }
    }
}

impl Device for Sp804 {
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
        self.timers = [TimerUnit::default(), TimerUnit::default()];
        Ok(())
    }

    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        let mut events = Vec::new();
        for (i, t) in self.timers.iter_mut().enumerate() {
            if t.tick(cycles) && t.irq_enabled() {
                events.push(DeviceEvent::Irq {
                    line: i as u32,
                    assert: true,
                });
            }
        }
        Ok(events)
    }

    fn read_fast(&mut self, offset: Addr, _size: usize) -> HelmResult<u64> {
        Ok(self.handle_read(offset) as u64)
    }

    fn write_fast(&mut self, offset: Addr, _size: usize, value: u64) -> HelmResult<()> {
        self.handle_write(offset, value as u32);
        Ok(())
    }

    fn name(&self) -> &str {
        &self.dev_name
    }
}

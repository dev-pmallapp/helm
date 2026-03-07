//! PL031 Real Time Clock — ARM DDI0224.
//!
//! Simple RTC with a 32-bit counter that increments once per second,
//! match register for alarm, and interrupt generation.

use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;

const RTCDR: u64 = 0x000;   // Data register (current time)
const RTCMR: u64 = 0x004;   // Match register
const RTCLR: u64 = 0x008;   // Load register
const RTCCR: u64 = 0x00C;   // Control register
const RTCIMSC: u64 = 0x010; // Interrupt mask
const RTCRIS: u64 = 0x014;  // Raw interrupt status
const RTCMIS: u64 = 0x018;  // Masked interrupt status
const RTCICR: u64 = 0x01C;  // Interrupt clear

pub struct Pl031 {
    dev_name: String,
    region: MemRegion,
    /// Current RTC value (seconds since epoch).
    data: u32,
    /// Match register for alarm.
    match_val: u32,
    /// Load register.
    load: u32,
    /// Control: bit 0 = RTC enable.
    control: u32,
    /// Interrupt mask.
    imsc: u32,
    /// Raw interrupt status.
    ris: u32,
    /// Ticks accumulated (to count seconds from cycles).
    tick_accumulator: u64,
    /// Clock frequency assumption (cycles per second).
    clock_hz: u64,
}

impl Pl031 {
    pub fn new(name: impl Into<String>) -> Self {
        let n = name.into();
        Self {
            region: MemRegion {
                name: n.clone(), base: 0, size: 0x1000,
                kind: crate::region::RegionKind::Io, priority: 0,
            },
            dev_name: n,
            data: 0,
            match_val: 0,
            load: 0,
            control: 1, // enabled by default
            imsc: 0,
            ris: 0,
            tick_accumulator: 0,
            clock_hz: 1_000_000, // 1 MHz default
        }
    }

    /// Set initial RTC time (seconds since epoch).
    pub fn set_time(mut self, seconds: u32) -> Self {
        self.data = seconds;
        self.load = seconds;
        self
    }

    fn handle_read(&self, offset: u64) -> u32 {
        match offset {
            RTCDR => self.data,
            RTCMR => self.match_val,
            RTCLR => self.load,
            RTCCR => self.control,
            RTCIMSC => self.imsc,
            RTCRIS => self.ris,
            RTCMIS => self.ris & self.imsc,
            // PrimeCell ID (PL031)
            0xFE0 => 0x31, 0xFE4 => 0x10, 0xFE8 => 0x04, 0xFEC => 0x00,
            0xFF0 => 0x0D, 0xFF4 => 0xF0, 0xFF8 => 0x05, 0xFFC => 0xB1,
            _ => 0,
        }
    }

    fn handle_write(&mut self, offset: u64, value: u32) {
        match offset {
            RTCMR => self.match_val = value,
            RTCLR => { self.load = value; self.data = value; }
            RTCCR => self.control = value & 1,
            RTCIMSC => self.imsc = value & 1,
            RTCICR => self.ris &= !value,
            _ => {}
        }
    }
}

impl Device for Pl031 {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if txn.is_write { self.handle_write(txn.offset, txn.data_u32()); }
        else { txn.set_data_u32(self.handle_read(txn.offset)); }
        txn.stall_cycles += 1;
        Ok(())
    }

    fn regions(&self) -> &[MemRegion] { std::slice::from_ref(&self.region) }

    fn reset(&mut self) -> HelmResult<()> {
        self.data = self.load;
        self.ris = 0;
        self.imsc = 0;
        self.control = 1;
        self.tick_accumulator = 0;
        Ok(())
    }

    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        if self.control & 1 == 0 { return Ok(vec![]); }
        self.tick_accumulator += cycles;
        let mut events = Vec::new();
        while self.tick_accumulator >= self.clock_hz {
            self.tick_accumulator -= self.clock_hz;
            self.data = self.data.wrapping_add(1);
            if self.data == self.match_val {
                self.ris |= 1;
                if self.imsc & 1 != 0 {
                    events.push(DeviceEvent::Irq { line: 0, assert: true });
                }
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

    fn name(&self) -> &str { &self.dev_name }
}

//! BCM2837 System Timer — BCM2835 ARM Peripherals §12.
//!
//! 64-bit free-running counter with 4 compare channels (C0-C3).
//! C0/C2 are used by the GPU, C1/C3 by the ARM.

use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;

const CS: u64 = 0x00;   // Control/Status
const CLO: u64 = 0x04;  // Counter lower 32 bits
const CHI: u64 = 0x08;  // Counter upper 32 bits
const C0: u64 = 0x0C;   // Compare 0
const C1: u64 = 0x10;   // Compare 1
const C2: u64 = 0x14;   // Compare 2
const C3: u64 = 0x18;   // Compare 3

pub struct BcmSysTimer {
    dev_name: String,
    region: MemRegion,
    counter: u64,
    compare: [u32; 4],
    /// Match flags (bits 0-3 of CS).
    cs: u32,
}

impl BcmSysTimer {
    pub fn new(name: impl Into<String>) -> Self {
        let n = name.into();
        Self {
            region: MemRegion {
                name: n.clone(), base: 0, size: 0x1000,
                kind: crate::region::RegionKind::Io, priority: 0,
            },
            dev_name: n,
            counter: 0,
            compare: [0; 4],
            cs: 0,
        }
    }

    fn handle_read(&self, offset: u64) -> u32 {
        match offset {
            CS => self.cs,
            CLO => self.counter as u32,
            CHI => (self.counter >> 32) as u32,
            C0 => self.compare[0],
            C1 => self.compare[1],
            C2 => self.compare[2],
            C3 => self.compare[3],
            _ => 0,
        }
    }

    fn handle_write(&mut self, offset: u64, value: u32) {
        match offset {
            CS => self.cs &= !value, // Write 1 to clear match bits
            C0 => self.compare[0] = value,
            C1 => self.compare[1] = value,
            C2 => self.compare[2] = value,
            C3 => self.compare[3] = value,
            _ => {}
        }
    }
}

impl Device for BcmSysTimer {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if txn.is_write { self.handle_write(txn.offset, txn.data_u32()); }
        else { txn.set_data_u32(self.handle_read(txn.offset)); }
        txn.stall_cycles += 1;
        Ok(())
    }

    fn regions(&self) -> &[MemRegion] { std::slice::from_ref(&self.region) }

    fn reset(&mut self) -> HelmResult<()> {
        self.counter = 0; self.cs = 0; self.compare = [0; 4];
        Ok(())
    }

    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        self.counter = self.counter.wrapping_add(cycles);
        let clo = self.counter as u32;
        let mut events = Vec::new();
        for i in 0..4 {
            if clo == self.compare[i] && self.cs & (1 << i) == 0 {
                self.cs |= 1 << i;
                events.push(DeviceEvent::Irq { line: i as u32, assert: true });
            }
        }
        Ok(events)
    }

    fn read_fast(&mut self, offset: Addr, _s: usize) -> HelmResult<u64> {
        Ok(self.handle_read(offset) as u64)
    }
    fn write_fast(&mut self, offset: Addr, _s: usize, v: u64) -> HelmResult<()> {
        self.handle_write(offset, v as u32); Ok(())
    }

    fn name(&self) -> &str { &self.dev_name }
}

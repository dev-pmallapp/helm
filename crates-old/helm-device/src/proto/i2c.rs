//! I2C bus protocol — 7-bit addressing, ACK/NAK.

use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::HelmResult;

/// I2C bus with 7-bit device addressing and protocol overhead.
pub struct I2cBus {
    name: String,
    devices: Vec<(u8, Box<dyn Device>)>, // (7-bit address, device)
    /// Cycles per byte transferred (includes clock stretching).
    pub cycles_per_byte: u64,
    region: MemRegion,
}

impl I2cBus {
    pub fn new(name: impl Into<String>) -> Self {
        let n = name.into();
        let region = MemRegion {
            name: n.clone(),
            base: 0,
            size: 128, // 7-bit address space
            kind: crate::region::RegionKind::Io,
            priority: 0,
        };
        Self {
            name: n,
            devices: Vec::new(),
            cycles_per_byte: 20,
            region,
        }
    }

    /// Attach a device at a 7-bit I2C address.
    pub fn attach(&mut self, addr: u8, device: Box<dyn Device>) {
        assert!(addr < 128, "I2C address must be 7-bit (0-127)");
        self.devices.push((addr, device));
    }
}

impl Device for I2cBus {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        let i2c_addr = txn.offset as u8;
        for (addr, dev) in &mut self.devices {
            if *addr == i2c_addr {
                txn.offset = 0;
                dev.transact(txn)?;
                // I2C overhead: start + addr + ack + data bytes + stop
                txn.stall_cycles += self.cycles_per_byte * (txn.size as u64 + 2);
                return Ok(());
            }
        }
        Err(helm_core::HelmError::Memory {
            addr: txn.addr,
            reason: format!("no I2C device at address {:#x}", i2c_addr),
        })
    }

    fn regions(&self) -> &[MemRegion] {
        std::slice::from_ref(&self.region)
    }

    fn reset(&mut self) -> HelmResult<()> {
        for (_, dev) in &mut self.devices {
            dev.reset()?;
        }
        Ok(())
    }

    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        let mut events = Vec::new();
        for (_, dev) in &mut self.devices {
            events.extend(dev.tick(cycles)?);
        }
        Ok(events)
    }

    fn name(&self) -> &str {
        &self.name
    }
}

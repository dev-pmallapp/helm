//! SPI bus protocol — chip-select, clock polarity/phase.

use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::HelmResult;

/// SPI bus with chip-select lines and clock configuration.
pub struct SpiBus {
    name: String,
    /// (chip_select_id, device)
    devices: Vec<(u8, Box<dyn Device>)>,
    /// Cycles per byte at the configured clock rate.
    pub cycles_per_byte: u64,
    region: MemRegion,
}

impl SpiBus {
    pub fn new(name: impl Into<String>) -> Self {
        let n = name.into();
        let region = MemRegion {
            name: n.clone(),
            base: 0,
            size: 256, // chip-select space
            kind: crate::region::RegionKind::Io,
            priority: 0,
        };
        Self {
            name: n,
            devices: Vec::new(),
            cycles_per_byte: 8,
            region,
        }
    }

    /// Attach a device to a chip-select line.
    pub fn attach(&mut self, cs: u8, device: Box<dyn Device>) {
        self.devices.push((cs, device));
    }
}

impl Device for SpiBus {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        let cs = txn.offset as u8;
        for (chip_select, dev) in &mut self.devices {
            if *chip_select == cs {
                txn.offset = 0;
                dev.transact(txn)?;
                txn.stall_cycles += self.cycles_per_byte * txn.size as u64;
                return Ok(());
            }
        }
        Err(helm_core::HelmError::Memory {
            addr: txn.addr,
            reason: format!("no SPI device on CS {}", cs),
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

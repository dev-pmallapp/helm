//! USB bus protocol — endpoint addressing, packet types.

use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::HelmResult;

/// USB bus with endpoint addressing and protocol overhead.
pub struct UsbBus {
    name: String,
    /// (endpoint_addr, device)
    devices: Vec<(u8, Box<dyn Device>)>,
    /// Fixed protocol overhead per transaction in cycles.
    pub protocol_overhead: u64,
    region: MemRegion,
}

impl UsbBus {
    pub fn new(name: impl Into<String>) -> Self {
        let n = name.into();
        let region = MemRegion {
            name: n.clone(),
            base: 0,
            size: 0x100_0000,
            kind: crate::region::RegionKind::Io,
            priority: 0,
        };
        Self {
            name: n,
            devices: Vec::new(),
            protocol_overhead: 10,
            region,
        }
    }

    pub fn attach(&mut self, endpoint: u8, device: Box<dyn Device>) {
        self.devices.push((endpoint, device));
    }
}

impl Device for UsbBus {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        // Upper bits of offset select endpoint, lower bits are device offset.
        let endpoint = (txn.offset >> 16) as u8;
        let dev_offset = txn.offset & 0xFFFF;
        for (ep, dev) in &mut self.devices {
            if *ep == endpoint {
                txn.offset = dev_offset;
                dev.transact(txn)?;
                txn.stall_cycles += self.protocol_overhead;
                return Ok(());
            }
        }
        Err(helm_core::HelmError::Memory {
            addr: txn.addr,
            reason: format!("no USB device at endpoint {}", endpoint),
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

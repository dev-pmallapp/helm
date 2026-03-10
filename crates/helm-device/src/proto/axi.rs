//! AXI4 bus protocol — burst transfers with configurable beat count.

use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;

/// AXI4 bus with burst protocol and configurable data width.
pub struct AxiBus {
    name: String,
    devices: Vec<(Addr, u64, Box<dyn Device>)>, // (base, size, device)
    /// Data bus width in bytes (4, 8, 16, 32, 64, 128).
    pub data_width: usize,
    /// Cycles per beat.
    pub cycles_per_beat: u64,
    /// Additional address-phase overhead.
    pub addr_phase_cycles: u64,
    region: MemRegion,
}

impl AxiBus {
    pub fn new(name: impl Into<String>, window_size: u64) -> Self {
        let n = name.into();
        let region = MemRegion {
            name: n.clone(),
            base: 0,
            size: window_size,
            kind: crate::region::RegionKind::Io,
            priority: 0,
        };
        Self {
            name: n,
            devices: Vec::new(),
            data_width: 8,
            cycles_per_beat: 1,
            addr_phase_cycles: 1,
            region,
        }
    }

    pub fn attach(&mut self, base: Addr, size: u64, device: Box<dyn Device>) {
        self.devices.push((base, size, device));
    }
}

impl Device for AxiBus {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        let local_addr = txn.offset;
        for (base, size, dev) in &mut self.devices {
            if local_addr >= *base && local_addr < *base + *size {
                txn.offset = local_addr - *base;
                dev.transact(txn)?;
                // AXI timing: address phase + data beats
                let beats = ((txn.size + self.data_width - 1) / self.data_width) as u64;
                txn.stall_cycles += self.addr_phase_cycles + beats * self.cycles_per_beat;
                return Ok(());
            }
        }
        Err(helm_core::HelmError::Memory {
            addr: txn.addr,
            reason: "no AXI device at this address".into(),
        })
    }

    fn regions(&self) -> &[MemRegion] {
        std::slice::from_ref(&self.region)
    }

    fn reset(&mut self) -> HelmResult<()> {
        for (_, _, dev) in &mut self.devices {
            dev.reset()?;
        }
        Ok(())
    }

    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        let mut events = Vec::new();
        for (_, _, dev) in &mut self.devices {
            events.extend(dev.tick(cycles)?);
        }
        Ok(events)
    }

    fn name(&self) -> &str {
        &self.name
    }
}

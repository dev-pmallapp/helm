//! PCI bus protocol — config space, BARs, INTx routing.

use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;
use serde::{Deserialize, Serialize};

/// PCI Base Address Register configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PciBar {
    /// BAR is not in use.
    Unused,
    /// Memory-mapped BAR.
    Mmio { size: u64 },
    /// I/O-space BAR.
    Io { size: u32 },
}

/// A PCI device with config space and BARs wrapping an inner device.
pub struct PciDevice {
    pub vendor_id: u16,
    pub device_id: u16,
    pub bars: [PciBar; 6],
    pub config_space: [u8; 256],
    pub inner: Box<dyn Device>,
    region: MemRegion,
}

impl PciDevice {
    pub fn new(vendor_id: u16, device_id: u16, inner: Box<dyn Device>) -> Self {
        let mut config_space = [0u8; 256];
        // Vendor ID at offset 0x00
        config_space[0] = vendor_id as u8;
        config_space[1] = (vendor_id >> 8) as u8;
        // Device ID at offset 0x02
        config_space[2] = device_id as u8;
        config_space[3] = (device_id >> 8) as u8;

        let size = inner.regions().first().map_or(0x1000, |r| r.size);
        let region = MemRegion {
            name: format!("pci-{:04x}:{:04x}", vendor_id, device_id),
            base: 0,
            size,
            kind: crate::region::RegionKind::Io,
            priority: 0,
        };

        Self {
            vendor_id,
            device_id,
            bars: [
                PciBar::Unused,
                PciBar::Unused,
                PciBar::Unused,
                PciBar::Unused,
                PciBar::Unused,
                PciBar::Unused,
            ],
            config_space,
            inner,
            region,
        }
    }

    /// Read from PCI config space.
    pub fn read_config(&self, offset: u8) -> u32 {
        let o = offset as usize;
        if o + 4 <= 256 {
            u32::from_le_bytes([
                self.config_space[o],
                self.config_space[o + 1],
                self.config_space[o + 2],
                self.config_space[o + 3],
            ])
        } else {
            0
        }
    }

    /// Write to PCI config space.
    pub fn write_config(&mut self, offset: u8, value: u32) {
        let o = offset as usize;
        if o + 4 <= 256 {
            let bytes = value.to_le_bytes();
            self.config_space[o..o + 4].copy_from_slice(&bytes);
        }
    }
}

impl Device for PciDevice {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        self.inner.transact(txn)
    }

    fn regions(&self) -> &[MemRegion] {
        std::slice::from_ref(&self.region)
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.inner.reset()
    }

    fn checkpoint(&self) -> HelmResult<serde_json::Value> {
        self.inner.checkpoint()
    }

    fn restore(&mut self, state: &serde_json::Value) -> HelmResult<()> {
        self.inner.restore(state)
    }

    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        self.inner.tick(cycles)
    }

    fn name(&self) -> &str {
        self.inner.name()
    }
}

/// PCI bus with config space access and bridge latency.
pub struct PciBus {
    name: String,
    devices: Vec<(Addr, PciDevice)>,
    bridge_latency: u64,
    region: MemRegion,
}

impl PciBus {
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
            bridge_latency: 1,
            region,
        }
    }

    pub fn attach(&mut self, base: Addr, device: PciDevice) {
        self.devices.push((base, device));
    }

    pub fn set_bridge_latency(&mut self, latency: u64) {
        self.bridge_latency = latency;
    }
}

impl Device for PciBus {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        let local_addr = txn.offset;
        for (base, dev) in &mut self.devices {
            let size = dev.regions().first().map_or(0, |r| r.size);
            if local_addr >= *base && local_addr < *base + size {
                txn.offset = local_addr - *base;
                dev.transact(txn)?;
                txn.stall_cycles += self.bridge_latency;
                return Ok(());
            }
        }
        Err(helm_core::HelmError::Memory {
            addr: txn.addr,
            reason: "no PCI device at this address".into(),
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

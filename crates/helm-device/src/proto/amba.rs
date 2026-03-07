//! AMBA bus protocols — APB (Advanced Peripheral Bus) and AHB
//! (Advanced High-performance Bus).
//!
//! ARM's AMBA specification defines a hierarchy of buses:
//! - **AHB**: High-bandwidth, pipelined. Connects CPU, memory, DMA.
//! - **APB**: Low-power, simple. Connects low-bandwidth peripherals
//!   (UART, GPIO, timers) via an AHB-to-APB bridge.
//!
//! ```text
//! CPU ─── AHB ───┬──── SRAM
//!                ├──── DMA
//!                └── AHB-APB Bridge (1 cycle)
//!                      ├── PL011 UART @ 0x0900_0000
//!                      ├── SP804 Timer @ 0x0901_0000
//!                      └── GPIO @ 0x0902_0000
//! ```

use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;

/// AMBA AHB bus — high-performance, pipelined.
///
/// Typical use: connects CPU, memory controller, DMA engines.
/// 0 or 1 wait-state per access.
pub struct AhbBus {
    name: String,
    devices: Vec<(Addr, u64, Box<dyn Device>)>,
    /// Wait states per access (pipeline overhead).
    pub wait_states: u64,
    window_size: u64,
    region: MemRegion,
}

impl AhbBus {
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
            wait_states: 0,
            window_size,
            region,
        }
    }

    /// Attach a device at the given base address.
    pub fn attach(&mut self, base: Addr, size: u64, device: Box<dyn Device>) {
        self.devices.push((base, size, device));
    }

    /// List attached devices.
    pub fn devices(&self) -> Vec<(&str, Addr, u64)> {
        self.devices
            .iter()
            .map(|(base, size, dev)| (dev.name(), *base, *size))
            .collect()
    }
}

impl Device for AhbBus {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        let local_addr = txn.offset;
        for (base, size, dev) in &mut self.devices {
            if local_addr >= *base && local_addr < *base + *size {
                txn.offset = local_addr - *base;
                dev.transact(txn)?;
                txn.stall_cycles += self.wait_states;
                return Ok(());
            }
        }
        Err(helm_core::HelmError::Memory {
            addr: txn.addr,
            reason: "no AHB device at this address".into(),
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

/// AMBA APB bus — low-power peripheral bus.
///
/// Attached to an AHB via a bridge. All accesses take 2 APB clock cycles
/// (setup + access phase) plus the bridge latency.
pub struct ApbBus {
    name: String,
    devices: Vec<(Addr, u64, Box<dyn Device>)>,
    /// Bridge latency from AHB → APB crossing.
    pub bridge_latency: u64,
    /// APB access overhead (typically 2 cycles for setup + access).
    pub access_cycles: u64,
    window_size: u64,
    region: MemRegion,
}

impl ApbBus {
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
            access_cycles: 2,
            window_size,
            region,
        }
    }

    /// Attach a peripheral at the given base address.
    pub fn attach(&mut self, base: Addr, size: u64, device: Box<dyn Device>) {
        self.devices.push((base, size, device));
    }

    /// List attached peripherals.
    pub fn peripherals(&self) -> Vec<(&str, Addr, u64)> {
        self.devices
            .iter()
            .map(|(base, size, dev)| (dev.name(), *base, *size))
            .collect()
    }
}

impl Device for ApbBus {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        let local_addr = txn.offset;
        for (base, size, dev) in &mut self.devices {
            if local_addr >= *base && local_addr < *base + *size {
                txn.offset = local_addr - *base;
                dev.transact(txn)?;
                txn.stall_cycles += self.bridge_latency + self.access_cycles;
                return Ok(());
            }
        }
        Err(helm_core::HelmError::Memory {
            addr: txn.addr,
            reason: "no APB peripheral at this address".into(),
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

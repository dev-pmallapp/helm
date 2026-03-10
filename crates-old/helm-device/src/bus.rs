//! Device bus — routes transactions to the correct device by address.
//!
//! A `DeviceBus` is itself a [`Device`], so buses can nest to model
//! hierarchical topologies (system bus → PCI root → endpoints).
//! Each bus level adds its `bridge_latency` to the transaction's stall cycles.
//!
//! The bus also implements [`MemoryMappedDevice`] for backward compatibility
//! with code that uses the legacy read/write API.

use crate::device::{Device, DeviceEvent};
use crate::mmio::{DeviceAccess, MemoryMappedDevice};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;

/// A device mapped at a specific base address on the bus.
pub struct DeviceSlot {
    pub name: String,
    pub base: Addr,
    pub size: u64,
    pub device: Box<dyn Device>,
}

/// Hierarchical device bus that dispatches transactions to devices.
///
/// Because `DeviceBus` implements [`Device`], buses can be attached
/// to other buses to form a tree:
///
/// ```text
/// system_bus (0 latency)
///   ├── uart @ 0x4000_0000
///   └── pci_bus @ 0xC000_0000 (1 cycle crossing)
///       ├── gpu @ 0x0000
///       └── nic @ 0x1000
/// ```
pub struct DeviceBus {
    name: String,
    slots: Vec<DeviceSlot>,
    /// Address window this bus covers.
    window_size: u64,
    /// Stall cycles added per bus crossing (bridge/protocol overhead).
    bridge_latency: u64,
    /// Cached region for the Device trait.
    region: MemRegion,
}

impl DeviceBus {
    /// Create a bus with custom name, window size, and bridge latency.
    pub fn new(name: impl Into<String>, window_size: u64, bridge_latency: u64) -> Self {
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
            slots: Vec::new(),
            window_size,
            bridge_latency,
            region,
        }
    }

    /// System bus: 0 crossing latency, full 64-bit address space.
    pub fn system() -> Self {
        Self::new("system", u64::MAX, 0)
    }

    /// PCI root complex with configurable window size and 1-cycle crossing.
    pub fn pci(name: impl Into<String>, window_size: u64) -> Self {
        Self::new(name, window_size, 1)
    }

    /// USB host controller: 10-cycle protocol overhead, 16 MB window.
    pub fn usb(name: impl Into<String>) -> Self {
        Self::new(name, 0x100_0000, 10)
    }

    /// Attach a new-style [`Device`] at the given base address.
    pub fn attach_device(&mut self, name: impl Into<String>, base: Addr, device: Box<dyn Device>) {
        let size = device.regions().first().map_or(0, |r| r.size);
        self.slots.push(DeviceSlot {
            name: name.into(),
            base,
            size,
            device,
        });
    }

    /// Attach a legacy [`MemoryMappedDevice`] at the given base address.
    ///
    /// Wraps it in a [`LegacyWrapper`](crate::device::LegacyWrapper) automatically.
    pub fn attach(
        &mut self,
        name: impl Into<String>,
        base: Addr,
        device: Box<dyn MemoryMappedDevice>,
    ) {
        let size = device.region_size();
        let wrapper = LegacyMmioWrapper { inner: device };
        let n = name.into();
        self.slots.push(DeviceSlot {
            name: n,
            base,
            size,
            device: Box::new(wrapper),
        });
    }

    /// Route a transaction through the bus. Finds the target device,
    /// adjusts the offset, adds bridge latency, and dispatches.
    pub fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        for slot in &mut self.slots {
            if txn.addr >= slot.base && txn.addr < slot.base + slot.size {
                txn.offset = txn.addr - slot.base;
                slot.device.transact(txn)?;
                txn.stall_cycles += self.bridge_latency;
                return Ok(());
            }
        }
        Err(helm_core::HelmError::Memory {
            addr: txn.addr,
            reason: "no device mapped at this address".into(),
        })
    }

    /// Read from the bus (legacy API). Routes to the device whose region
    /// contains `addr`. Adds `bridge_latency` to the returned stall cycles.
    pub fn bus_read(&mut self, addr: Addr, size: usize) -> HelmResult<DeviceAccess> {
        let mut txn = Transaction::read(addr, size);
        self.transact(&mut txn)?;
        Ok(DeviceAccess {
            data: txn.data_u64(),
            stall_cycles: txn.stall_cycles,
        })
    }

    /// Write to the bus (legacy API). Adds `bridge_latency` to the returned stall.
    pub fn bus_write(&mut self, addr: Addr, size: usize, value: u64) -> HelmResult<u64> {
        let mut txn = Transaction::write(addr, size, value);
        self.transact(&mut txn)?;
        Ok(txn.stall_cycles)
    }

    /// List all attached devices.
    pub fn devices(&self) -> Vec<(&str, Addr, u64)> {
        self.slots
            .iter()
            .map(|s| (s.name.as_str(), s.base, s.size))
            .collect()
    }

    /// Reset all devices.
    pub fn reset_all(&mut self) -> HelmResult<()> {
        for slot in &mut self.slots {
            slot.device.reset()?;
        }
        Ok(())
    }

    /// Bridge latency for this bus level.
    pub fn bridge_latency(&self) -> u64 {
        self.bridge_latency
    }

    /// Check if an address maps to any device on this bus.
    pub fn contains(&self, addr: Addr) -> bool {
        self.slots
            .iter()
            .any(|s| addr >= s.base && addr < s.base + s.size)
    }

    /// Tick all devices on this bus. Returns accumulated events.
    pub fn tick_all(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        let mut events = Vec::new();
        for slot in &mut self.slots {
            events.extend(slot.device.tick(cycles)?);
        }
        Ok(events)
    }

    // ── Fast functional path (FE mode) ──────────────────────────────────

    /// Fast-path read — no Transaction, no stall accumulation.
    /// Used in FE mode for maximum throughput.
    pub fn read_fast(&mut self, addr: Addr, size: usize) -> HelmResult<u64> {
        for slot in &mut self.slots {
            if addr >= slot.base && addr < slot.base + slot.size {
                return slot.device.read_fast(addr - slot.base, size);
            }
        }
        Err(helm_core::HelmError::Memory {
            addr,
            reason: "no device mapped at this address".into(),
        })
    }

    /// Fast-path write — no Transaction, no stall accumulation.
    pub fn write_fast(&mut self, addr: Addr, size: usize, value: u64) -> HelmResult<()> {
        for slot in &mut self.slots {
            if addr >= slot.base && addr < slot.base + slot.size {
                return slot.device.write_fast(addr - slot.base, size, value);
            }
        }
        Err(helm_core::HelmError::Memory {
            addr,
            reason: "no device mapped at this address".into(),
        })
    }
}

impl Default for DeviceBus {
    fn default() -> Self {
        Self::system()
    }
}

// ── DeviceBus implements Device (for nesting) ───────────────────────────────

impl Device for DeviceBus {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        // When nested, txn.addr is relative to this bus's window.
        // Route using offset as the address within this bus.
        let local_addr = txn.offset;
        for slot in &mut self.slots {
            if local_addr >= slot.base && local_addr < slot.base + slot.size {
                txn.offset = local_addr - slot.base;
                slot.device.transact(txn)?;
                txn.stall_cycles += self.bridge_latency;
                return Ok(());
            }
        }
        Err(helm_core::HelmError::Memory {
            addr: txn.addr,
            reason: "no device mapped at this address".into(),
        })
    }

    fn regions(&self) -> &[MemRegion] {
        std::slice::from_ref(&self.region)
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.reset_all()
    }

    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        self.tick_all(cycles)
    }

    fn name(&self) -> &str {
        &self.name
    }
}

// ── DeviceBus implements MemoryMappedDevice (backward compat) ───────────────

impl MemoryMappedDevice for DeviceBus {
    fn read(&mut self, offset: Addr, size: usize) -> HelmResult<DeviceAccess> {
        self.bus_read(offset, size)
    }

    fn write(&mut self, offset: Addr, size: usize, value: u64) -> HelmResult<u64> {
        self.bus_write(offset, size, value)
    }

    fn region_size(&self) -> u64 {
        self.window_size
    }

    fn device_name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.reset_all()
    }
}

// ── Internal wrapper for legacy devices ─────────────────────────────────────

/// Internal wrapper that adapts a `Box<dyn MemoryMappedDevice>` to `Device`.
struct LegacyMmioWrapper {
    inner: Box<dyn MemoryMappedDevice>,
}

// Safety: MemoryMappedDevice already requires Send + Sync
unsafe impl Send for LegacyMmioWrapper {}
unsafe impl Sync for LegacyMmioWrapper {}

impl Device for LegacyMmioWrapper {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if txn.is_write {
            let value = txn.data_u64();
            let stall = self.inner.write(txn.offset, txn.size, value)?;
            txn.stall_cycles += stall;
        } else {
            let access = self.inner.read(txn.offset, txn.size)?;
            txn.set_data_u64(access.data);
            txn.stall_cycles += access.stall_cycles;
        }
        Ok(())
    }

    fn regions(&self) -> &[MemRegion] {
        // Single-region device; base is set by the bus, not the device.
        &[]
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.inner.reset()
    }

    fn name(&self) -> &str {
        self.inner.device_name()
    }
}

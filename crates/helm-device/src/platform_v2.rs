//! Platform V2 — wires AddressMap, IrqRouter, CoopScheduler, and
//! DeviceRegistry into a complete simulated machine with lifecycle management.
//!
//! Runs alongside the existing [`Platform`](crate::platform::Platform) —
//! no migration required yet. Devices can be added and removed at runtime
//! (hot-plug), and the address map uses O(log n) dispatch.

use crate::address_map::AddressMap;
use crate::coop_scheduler::CoopScheduler;
use crate::device::{Device, DeviceEvent, DeviceId};
use crate::irq::IrqRouter;
use crate::loader::{DeviceConfig, DeviceLoadError, DynamicDeviceLoader};
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;

/// A complete platform with AddressMap-based dispatch and CoopScheduler.
pub struct PlatformV2 {
    /// Human-readable platform name.
    pub name: String,
    /// O(log n) address map.
    pub address_map: AddressMap,
    /// IRQ routing table.
    pub irq_router: IrqRouter,
    /// Cooperative multi-clock scheduler.
    pub scheduler: CoopScheduler,
    /// Total ticks advanced (for simple tick mode).
    total_ticks: u64,
}

impl PlatformV2 {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            address_map: AddressMap::new(),
            irq_router: IrqRouter::new(),
            scheduler: CoopScheduler::new(),
            total_ticks: 0,
        }
    }

    /// Add a device: attach to address map, map its regions, commit.
    ///
    /// The device's `regions()` are used to determine the MMIO mapping.
    /// If the device has `clock_hz() > 0`, it is registered with the scheduler.
    pub fn add_device(
        &mut self,
        name: impl Into<String>,
        base: Addr,
        device: Box<dyn Device>,
    ) -> HelmResult<DeviceId> {
        let region_size = device.regions().first().map_or(0, |r| r.size);
        let clock = device.clock_hz();

        let id = self.address_map.attach(name, device);

        if region_size > 0 {
            self.address_map.map_region(id, base, region_size, 0);
        }

        self.address_map.commit();

        if clock > 0 {
            self.scheduler.register(id, clock);
        }

        Ok(id)
    }

    /// Add a device from a registry config.
    pub fn add_device_from_config(
        &mut self,
        config: &DeviceConfig,
        base: Addr,
        registry: &DynamicDeviceLoader,
    ) -> Result<DeviceId, DeviceLoadError> {
        let device = registry.create_from_config(config)?;
        self.add_device(&config.instance_name, base, device)
            .map_err(|e| DeviceLoadError::CreateFailed(e.to_string()))
    }

    /// Remove a device by ID: unregister from scheduler, detach, commit.
    /// Also removes any IRQ routes for this device.
    pub fn remove_device(&mut self, id: DeviceId) -> Option<Box<dyn Device>> {
        self.scheduler.unregister(id);
        self.irq_router.remove_routes_for_device(id);
        let device = self.address_map.detach(id);
        self.address_map.commit();
        device
    }

    /// Dispatch a transaction through the address map.
    pub fn dispatch(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        self.address_map.dispatch(txn)
    }

    /// Fast-path read.
    pub fn read_fast(&mut self, addr: Addr, size: usize) -> HelmResult<u64> {
        self.address_map.read_fast(addr, size)
    }

    /// Fast-path write.
    pub fn write_fast(&mut self, addr: Addr, size: usize, value: u64) -> HelmResult<()> {
        self.address_map.write_fast(addr, size, value)
    }

    /// Tick all time-driven devices via the cooperative scheduler.
    ///
    /// Advances the scheduler by `cycles` steps — each step ticks the
    /// device with the smallest timestamp.
    pub fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        self.total_ticks += cycles;
        if self.scheduler.num_entries() > 0 {
            self.scheduler.run_steps(cycles, &mut self.address_map)
        } else {
            Ok(vec![])
        }
    }

    /// Reset all devices.
    pub fn reset(&mut self) -> HelmResult<()> {
        let mut seen = Vec::new();
        for entry in self.address_map.flat_view() {
            if !seen.contains(&entry.device_id) {
                seen.push(entry.device_id);
            }
        }
        for id in seen {
            if let Some(dev) = self.address_map.device_mut(id) {
                dev.reset()?;
            }
        }
        Ok(())
    }

    /// Total ticks advanced so far.
    pub fn total_ticks(&self) -> u64 {
        self.total_ticks
    }

    /// Number of devices currently attached.
    pub fn num_devices(&self) -> usize {
        self.address_map.num_devices()
    }
}

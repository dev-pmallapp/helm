//! Device context — passed to `realize()`/`unrealize()` during lifecycle.
//!
//! [`DeviceCtx`] is the only way a device can register MMIO regions, connect
//! IRQ wires, or access the address map during its lifecycle transitions.
//! This follows the QEMU two-phase lifecycle pattern (construct + realize).

use crate::address_map::{AddressMap, RegionHandle};
use crate::device::DeviceId;
use crate::irq::IrqRouter;
use helm_core::types::Addr;

/// Context passed to a device during `realize()` and `unrealize()`.
///
/// Provides controlled access to the platform's address map and IRQ router
/// so the device can register its MMIO regions and connect interrupts.
pub struct DeviceCtx<'a> {
    /// The device's own ID.
    pub device_id: DeviceId,
    /// The platform's address map (for mapping/unmapping regions).
    pub address_map: &'a mut AddressMap,
    /// The platform's IRQ router (for connecting/disconnecting IRQs).
    pub irq_router: &'a mut IrqRouter,
}

impl<'a> DeviceCtx<'a> {
    /// Create a new device context.
    pub fn new(
        device_id: DeviceId,
        address_map: &'a mut AddressMap,
        irq_router: &'a mut IrqRouter,
    ) -> Self {
        Self {
            device_id,
            address_map,
            irq_router,
        }
    }

    /// Map an MMIO region for this device.
    pub fn map_region(&mut self, base: Addr, size: u64, priority: i32) -> RegionHandle {
        self.address_map
            .map_region(self.device_id, base, size, priority)
    }

    /// Unmap a previously mapped region.
    pub fn unmap_region(&mut self, handle: RegionHandle) {
        self.address_map.unmap_region(handle);
    }

    /// Connect this device's IRQ output to an interrupt controller.
    /// Returns the route index for later disconnection.
    pub fn connect_irq(
        &mut self,
        source_line: u32,
        dest_controller: usize,
        dest_irq: u32,
    ) -> usize {
        self.irq_router.add_route(crate::irq::IrqRoute {
            source_device: self.device_id,
            source_line,
            dest_controller,
            dest_irq,
        })
    }

    /// Disconnect an IRQ route by index.
    pub fn disconnect_irq(&mut self, route_index: usize) {
        self.irq_router.remove_route(route_index);
    }

    /// Disconnect all IRQ routes for this device.
    pub fn disconnect_all_irqs(&mut self) {
        self.irq_router
            .remove_routes_for_device(self.device_id);
    }
}

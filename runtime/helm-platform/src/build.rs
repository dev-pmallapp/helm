//! Frozen platform build metadata.
//!
//! These types describe what a platform contributes to a simulation build
//! without constructing engine-owned runtime objects directly.

use crate::topology::DeviceTopology;
use crate::AttachableSlot;

/// Classification for one addressable region in a platform build plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    /// Guest RAM window.
    Ram,
    /// Fixed MMIO mapping owned by the platform.
    Mmio,
    /// Address window reserved for dynamic attachment before `run()`.
    AttachmentWindow,
}

/// One named address range in a frozen platform build plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressRegionSpec {
    /// Human-readable region name.
    pub name: &'static str,
    /// Start address of the region.
    pub base: u64,
    /// Size in bytes.
    pub size: u64,
    /// Semantic kind for this region.
    pub kind: RegionKind,
}

/// One interrupt route contributed by a platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptRouteSpec {
    /// Source device instance name.
    pub source: &'static str,
    /// Signal line or interrupt number at the sink.
    pub line: u32,
    /// Sink device instance name.
    pub sink: &'static str,
}

/// Frozen, inspectable output of platform construction.
#[derive(Debug, Clone)]
pub struct PlatformBuildPlan {
    /// Human-readable platform name.
    pub platform_name: &'static str,
    /// Slots available for runtime attachment before execution begins.
    pub attachment_slots: Vec<AttachableSlot>,
    /// Fixed address layout contributed by the platform.
    pub address_regions: Vec<AddressRegionSpec>,
    /// Fixed interrupt routes contributed by the platform.
    pub interrupt_routes: Vec<InterruptRouteSpec>,
    /// Topology tree for diagnostics and tooling.
    pub topology: DeviceTopology,
}

impl PlatformBuildPlan {
    /// Return a named address region, if present.
    pub fn region_named(&self, name: &str) -> Option<&AddressRegionSpec> {
        self.address_regions
            .iter()
            .find(|region| region.name == name)
    }

    /// Return the first interrupt route emitted by the given source device.
    pub fn route_from_source(&self, source: &str) -> Option<&InterruptRouteSpec> {
        self.interrupt_routes
            .iter()
            .find(|route| route.source == source)
    }

    /// Return the attachment window region that contains `addr`, if any.
    pub fn attachment_window_for(&self, addr: u64, size: u64) -> Option<&AddressRegionSpec> {
        self.address_regions.iter().find(|region| {
            if region.kind != RegionKind::AttachmentWindow {
                return false;
            }
            let start = u128::from(region.base);
            let end = start + u128::from(region.size);
            let addr_start = u128::from(addr);
            let addr_end = addr_start + u128::from(size);
            addr_start >= start && addr_end <= end
        })
    }
}

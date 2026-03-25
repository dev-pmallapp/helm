//! ARM virt platform — QEMU-compatible address map.
//!
//! This module defines the address constants and device topology for the
//! ARM virt platform. The actual device construction and system memory
//! wiring remains in `helm-engine` for now (it depends on engine-internal
//! types like `HelmAddressSpace` and `FsState`).

use crate::topology::{DeviceNode, DeviceTopology};
use crate::{
    AddressRegionSpec, AttachableSlot, InterruptRouteSpec, Platform, PlatformBuildPlan, RegionKind,
    SlotType,
};

// ── Address constants (QEMU virt compatible) ────────────────────────────────

/// GIC Distributor base address.
pub const GICD_BASE: u64 = 0x0800_0000;
/// GIC CPU Interface base address.
pub const GICC_BASE: u64 = 0x0801_0000;
/// PL011 UART base address.
pub const UART_BASE: u64 = 0x0900_0000;
/// MMIO device region start (for runtime-attached devices).
pub const MMIO_BASE: u64 = 0x0A00_0000;
/// MMIO device region end.
pub const MMIO_END: u64 = 0x0AFF_FFFF;
/// RAM base address.
pub const RAM_BASE: u64 = 0x4000_0000;
/// Standard GIC region size (4 KiB).
pub const GIC_REGION_SIZE: u64 = 0x1000;
/// UART SPI interrupt number (QEMU virt).
pub const UART_IRQ: u32 = 33;

// ── ArmVirtPlatform ─────────────────────────────────────────────────────────

/// ARM virt platform descriptor.
///
/// Mirrors QEMU's `-machine virt` address map. The platform itself is
/// stateless — it describes *what* to build, not *how* to build it.
pub struct ArmVirtPlatform;

impl Platform for ArmVirtPlatform {
    fn name(&self) -> &str {
        "arm-virt"
    }

    fn attachment_slots(&self) -> &[AttachableSlot] {
        &[AttachableSlot {
            name: "mmio",
            slot_type: SlotType::Mmio {
                base_range: (MMIO_BASE, MMIO_END),
            },
            max_devices: 16,
        }]
    }

    fn build_plan(&self) -> PlatformBuildPlan {
        PlatformBuildPlan {
            platform_name: "arm-virt",
            attachment_slots: self.attachment_slots().to_vec(),
            address_regions: vec![
                AddressRegionSpec {
                    name: "gic-dist",
                    base: GICD_BASE,
                    size: GIC_REGION_SIZE,
                    kind: RegionKind::Mmio,
                },
                AddressRegionSpec {
                    name: "gic-cpu",
                    base: GICC_BASE,
                    size: GIC_REGION_SIZE,
                    kind: RegionKind::Mmio,
                },
                AddressRegionSpec {
                    name: "uart0",
                    base: UART_BASE,
                    size: GIC_REGION_SIZE,
                    kind: RegionKind::Mmio,
                },
                AddressRegionSpec {
                    name: "mmio",
                    base: MMIO_BASE,
                    size: MMIO_END - MMIO_BASE + 1,
                    kind: RegionKind::AttachmentWindow,
                },
                AddressRegionSpec {
                    name: "ram",
                    base: RAM_BASE,
                    size: 0,
                    kind: RegionKind::Ram,
                },
            ],
            interrupt_routes: vec![InterruptRouteSpec {
                source: "uart0",
                line: UART_IRQ,
                sink: "gic-dist",
            }],
            topology: self.topology(),
        }
    }
}

impl ArmVirtPlatform {
    /// Build the default device topology for this platform.
    ///
    /// This is a static description of the platform's device tree,
    /// suitable for `print_topology()`.
    pub fn topology(&self) -> DeviceTopology {
        DeviceTopology::new(
            DeviceNode::new("soc", "ArmVirt")
                .with_child(
                    DeviceNode::new("gic-dist", "GICv2Distributor")
                        .with_base(GICD_BASE)
                        .with_size(GIC_REGION_SIZE),
                )
                .with_child(
                    DeviceNode::new("gic-cpu", "GICv2CpuInterface")
                        .with_base(GICC_BASE)
                        .with_size(GIC_REGION_SIZE),
                )
                .with_child(
                    DeviceNode::new("uart0", "PL011")
                        .with_base(UART_BASE)
                        .with_size(GIC_REGION_SIZE)
                        .with_irq(UART_IRQ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_name() {
        let p = ArmVirtPlatform;
        assert_eq!(p.name(), "arm-virt");
    }

    #[test]
    fn attachment_slots() {
        let p = ArmVirtPlatform;
        let slots = p.attachment_slots();
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].name, "mmio");
        assert_eq!(slots[0].max_devices, 16);
    }

    #[test]
    fn topology_prints() {
        let p = ArmVirtPlatform;
        let topo = p.topology();
        let output = topo.print();
        assert!(output.contains("arm-virt") || output.contains("ArmVirt"));
        assert!(output.contains("gic-dist"));
        assert!(output.contains("uart0"));
    }

    #[test]
    fn build_plan_captures_layout_and_routes() {
        let p = ArmVirtPlatform;
        let plan = p.build_plan();

        assert_eq!(plan.platform_name, "arm-virt");
        assert!(plan
            .address_regions
            .iter()
            .any(|r| r.name == "gic-dist" && r.base == GICD_BASE));
        assert!(plan
            .address_regions
            .iter()
            .any(|r| r.name == "mmio" && r.kind == RegionKind::AttachmentWindow));
        assert!(plan
            .interrupt_routes
            .iter()
            .any(|r| r.source == "uart0" && r.line == UART_IRQ && r.sink == "gic-dist"));
    }
}

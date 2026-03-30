//! ARM virt platform — QEMU-compatible address map.
//!
//! This module defines the address constants and device topology for the
//! ARM virt platform. The actual device construction and system memory
//! wiring remains in `helm-engine` for now (it depends on engine-internal
//! types like `HelmAddressSpace` and `FsState`).

use crate::topology::{DeviceNode, DeviceTopology};
use crate::{
    AddressRegionSpec, AttachableSlot, BoardQuirk, InterruptRouteSpec, Platform, PlatformBuildPlan,
    PlatformQuirk, QuirkKey, QuirkSpec, RegionKind, SlotType,
};

// ── Address constants (QEMU virt compatible) ────────────────────────────────

/// GIC Distributor base address (shared by GICv2 and GICv3).
pub const GICD_BASE: u64 = 0x0800_0000;
/// GICv2 CPU Interface base address (unused when GICv3 is selected).
pub const GICC_BASE: u64 = 0x0801_0000;
/// GICv3 Redistributor base address (QEMU virt-compatible).
/// Each PE occupies 128KB (0x20000): `GICR_BASE + cpu_idx * 0x20000`.
pub const GICR_BASE: u64 = 0x080A_0000;
/// GICv3 redistributor stride per PE.
pub const GICR_STRIDE: u64 = 0x2_0000;
/// PL011 UART base address.
pub const UART_BASE: u64 = 0x0900_0000;
/// PL031 RTC base address.
pub const RTC_BASE: u64 = 0x0901_0000;
/// MMIO device region start (for runtime-attached devices).
pub const MMIO_BASE: u64 = 0x0A00_0000;
/// MMIO device region end.
pub const MMIO_END: u64 = 0x0AFF_FFFF;
/// RAM base address.
pub const RAM_BASE: u64 = 0x4000_0000;
/// GIC distributor region size.
pub const GICD_REGION_SIZE: u64 = 0x1_0000;
/// One redistributor frame per processing element.
pub const GICR_REGION_SIZE: u64 = GICR_STRIDE;
/// UART SPI interrupt number (QEMU virt).
pub const UART_IRQ: u32 = 33;
/// RTC SPI interrupt number (QEMU virt).
pub const RTC_IRQ: u32 = 34;

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
                    size: GICD_REGION_SIZE,
                    kind: RegionKind::Mmio,
                },
                AddressRegionSpec {
                    name: "gic-redist",
                    base: GICR_BASE,
                    size: GICR_REGION_SIZE,
                    kind: RegionKind::Mmio,
                },
                AddressRegionSpec {
                    name: "gic-cpu",
                    base: GICC_BASE,
                    size: 0x1000,
                    kind: RegionKind::Mmio,
                },
                AddressRegionSpec {
                    name: "uart0",
                    base: UART_BASE,
                    size: 0x1000,
                    kind: RegionKind::Mmio,
                },
                AddressRegionSpec {
                    name: "rtc0",
                    base: RTC_BASE,
                    size: 0x1000,
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
            interrupt_routes: vec![
                InterruptRouteSpec {
                    source: "uart0",
                    line: UART_IRQ,
                    sink: "gic-dist",
                },
                InterruptRouteSpec {
                    source: "rtc0",
                    line: RTC_IRQ,
                    sink: "gic-dist",
                },
            ],
            quirks: vec![
                QuirkSpec {
                    key: QuirkKey::Platform(PlatformQuirk::ArmVirtPl031Rtc),
                    summary: "Expose a PL031 RTC at 0x0901_0000, wired to SPI 34.",
                    default_enabled: true,
                },
                QuirkSpec {
                    key: QuirkKey::Board(BoardQuirk::PsciViaEngine),
                    summary: "Route PSCI power-management calls through the engine.",
                    default_enabled: true,
                },
            ],
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
                    DeviceNode::new("gic-dist", "GICv3Distributor")
                        .with_base(GICD_BASE)
                        .with_size(GICD_REGION_SIZE),
                )
                .with_child(
                    DeviceNode::new("gic-redist", "GICv3Redistributor")
                        .with_base(GICR_BASE)
                        .with_size(GICR_REGION_SIZE),
                )
                .with_child(
                    DeviceNode::new("uart0", "PL011")
                        .with_base(UART_BASE)
                        .with_size(0x1000)
                        .with_irq(UART_IRQ),
                )
                .with_child(
                    DeviceNode::new("rtc0", "PL031")
                        .with_base(RTC_BASE)
                        .with_size(0x1000)
                        .with_irq(RTC_IRQ),
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
        assert!(output.contains("gic-redist"));
        assert!(output.contains("uart0"));
    }

    #[test]
    fn build_plan_captures_layout_and_routes() {
        let p = ArmVirtPlatform;
        let plan = p.build_plan();
        let quirks = plan.default_quirks();

        assert_eq!(plan.platform_name, "arm-virt");
        assert!(plan
            .address_regions
            .iter()
            .any(|r| r.name == "gic-dist" && r.base == GICD_BASE));
        assert!(plan
            .address_regions
            .iter()
            .any(|r| r.name == "gic-redist" && r.base == GICR_BASE));
        assert!(plan
            .address_regions
            .iter()
            .any(|r| r.name == "mmio" && r.kind == RegionKind::AttachmentWindow));
        assert!(plan
            .interrupt_routes
            .iter()
            .any(|r| r.source == "uart0" && r.line == UART_IRQ && r.sink == "gic-dist"));
        assert!(plan.supports_quirk(QuirkKey::Platform(PlatformQuirk::ArmVirtPl031Rtc)));
        assert!(plan.supports_quirk(QuirkKey::Board(BoardQuirk::PsciViaEngine)));
        assert!(quirks.contains(QuirkKey::Platform(PlatformQuirk::ArmVirtPl031Rtc)));
        assert!(quirks.contains(QuirkKey::Board(BoardQuirk::PsciViaEngine)));
    }
}

//! ARM virt platform — QEMU-compatible address map.
//!
//! This module defines the address constants and device topology for the
//! ARM virt platform. The actual device construction and system memory
//! wiring remains in `helm-engine` for now (it depends on engine-internal
//! types like `HelmAddressSpace` and `FsState`).

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use helm_devices::{CharBackend, NullCharBackend};
use helm_hw_char::Pl011;
use helm_hw_firmware::FwCfgMmio;
use helm_hw_intc::{build_gicv2_mp, GicSharedState, GicV3SharedState, Gicv2CpuInterface};
use helm_hw_pci::PciBus;
use helm_hw_rtc::Pl031;
use helm_memory::{FlatMem, HelmAddressSpace};

use crate::topology::{DeviceNode, DeviceTopology};
use crate::{
    AddressRegionSpec, AttachableSlot, BoardQuirk, InterruptRouteSpec, Platform, PlatformBuildPlan,
    PlatformError, PlatformQuirk, QuirkKey, QuirkSpec, RegionKind, SlotType,
};
use crate::{BuiltInMappedDevice, BuiltInMappedDeviceKind};

// ── Address constants (QEMU virt compatible) ────────────────────────────────

/// GIC Distributor base address (shared by `GICv2` and `GICv3`).
pub const GICD_BASE: u64 = 0x0800_0000;
/// Reserved low-memory flash window base address.
pub const FLASH_BASE: u64 = 0x0000_0000;
/// Reserved low-memory flash window size (QEMU virt-compatible first 128 MiB).
pub const FLASH_SIZE: u64 = 0x0800_0000;
/// `GICv2` CPU Interface base address (unused when `GICv3` is selected).
pub const GICC_BASE: u64 = 0x0801_0000;
/// `GICv2` CPU Interface region size.
pub const GICC_REGION_SIZE: u64 = 0x1_0000;
/// `GICv2m` MSI frame base address.
pub const GICV2M_BASE: u64 = 0x0802_0000;
/// `GICv2m` MSI frame region size.
pub const GICV2M_REGION_SIZE: u64 = 0x1_000;
/// `GICv3` ITS window base address.
pub const GITS_BASE: u64 = 0x0808_0000;
/// `GICv3` ITS window region size.
pub const GITS_REGION_SIZE: u64 = 0x2_0000;
/// `GICv3` Redistributor base address (QEMU virt-compatible).
/// Each PE occupies 128KB (0x20000): `GICR_BASE + cpu_idx * 0x20000`.
pub const GICR_BASE: u64 = 0x080A_0000;
/// `GICv3` redistributor stride per PE.
pub const GICR_STRIDE: u64 = 0x2_0000;
/// PL011 UART base address.
pub const UART_BASE: u64 = 0x0900_0000;
/// PL031 RTC base address.
pub const RTC_BASE: u64 = 0x0901_0000;
/// fw_cfg (QEMU firmware configuration device) base address.
pub const FW_CFG_BASE: u64 = 0x0902_0000;
/// GPIO controller base address -- reserved, not yet modeled.
pub const GPIO_BASE: u64 = 0x0903_0000;
/// Secure UART (second PL011) base address -- reserved, not yet modeled.
pub const SECURE_UART_BASE: u64 = 0x0904_0000;
/// SMMUv3 base address.
pub const SMMU_BASE: u64 = 0x0905_0000;
/// MMIO device region start (for runtime-attached devices).
pub const MMIO_BASE: u64 = 0x0A00_0000;
/// MMIO device region end.
pub const MMIO_END: u64 = 0x0AFF_FFFF;
/// PCI MMIO aperture base address used for live BAR mappings on arm-virt.
pub const PCIE_MMIO_BASE: u64 = 0x1000_0000;
/// PCI MMIO aperture size.
pub const PCIE_MMIO_SIZE: u64 = 0x1EFF_0000;
/// PCI PIO aperture base address.
pub const PCIE_PIO_BASE: u64 = 0x2FFF_0000;
/// PCI PIO aperture size.
pub const PCIE_PIO_SIZE: u64 = 0x1_0000;
/// PCI ECAM window base address.
pub const PCIE_ECAM_BASE: u64 = 0x3000_0000;
/// PCI ECAM window size (256 buses * 32 devices * 8 functions * 4 KiB).
pub const PCIE_ECAM_SIZE: u64 = 0x1000_0000;
/// Synthetic MSI target address used by the current built-in arm-virt PCI path.
pub const PCIE_MSI_ADDR: u64 = 0xFEE0_0000;
/// First PCI INTx interrupt on the GIC.
pub const PCIE_INTX_IRQ_BASE: u32 = 35;
/// Number of PCI INTx interrupt lines.
pub const PCIE_INTX_IRQ_COUNT: u32 = 4;
/// Number of ECAM buses covered by the arm-virt PCI host bridge.
pub const PCIE_BUS_COUNT: u32 = (PCIE_ECAM_SIZE / (32 * 8 * 4096)) as u32;
/// RAM base address.
pub const RAM_BASE: u64 = 0x4000_0000;
/// Baseline interrupt capacity for the built-in arm-virt GIC profiles.
pub const ARM_VIRT_GIC_IRQ_CAPACITY: u32 = 256;
/// GIC distributor region size.
pub const GICD_REGION_SIZE: u64 = 0x1_0000;
/// Reserved low-memory `GICv3` redistributor aperture from the QEMU virt map.
pub const GICR_REGION_SIZE: u64 = 0x00F6_0000;
/// UART SPI interrupt number (QEMU virt).
pub const UART_IRQ: u32 = 33;
/// RTC SPI interrupt number (QEMU virt).
pub const RTC_IRQ: u32 = 34;
/// GPIO SPI interrupt number (QEMU virt).
pub const GPIO_IRQ: u32 = 39;
/// Secondary UART SPI interrupt number (QEMU virt).
pub const SECURE_UART_IRQ: u32 = 40;

/// Current platform-owned PCI MSI routing contract for built-in arm-virt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArmVirtPciMsiRoute {
    /// Synthetic MSI doorbell address accepted by the current platform path.
    pub target_addr: u64,
    /// First routable SPI INTID.
    pub spi_base: u32,
    /// Number of routable SPIs behind this contract.
    pub spi_count: u32,
}

impl ArmVirtPciMsiRoute {
    /// Translate one MSI `(addr, data)` pair into a routable SPI INTID.
    pub fn translate(&self, addr: u64, data: u32) -> Option<u32> {
        if addr != self.target_addr || data < self.spi_base {
            return None;
        }
        let offset = data - self.spi_base;
        if offset >= self.spi_count {
            return None;
        }
        Some(self.spi_base + offset)
    }
}

/// Interrupt-controller profile intentionally described by the arm-virt machine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ArmVirtGicProfile {
    /// `GICv2` distributor + CPU interface profile.
    V2,
    /// `GICv3` distributor + redistributor profile.
    #[default]
    V3,
}

impl ArmVirtGicProfile {
    /// Baseline interrupt capacity for the selected profile.
    pub const fn baseline_irq_capacity(self) -> u32 {
        ARM_VIRT_GIC_IRQ_CAPACITY
    }
}

// ── ArmVirtPlatform ─────────────────────────────────────────────────────────

/// Device indices in the built arm-virt address space.
pub struct ArmVirtDevices {
    /// Index of the mapped GIC distributor device in the built address space.
    pub gicd_idx: usize,
    /// Index of the mapped GIC CPU interface device for `GICv2`.
    pub gicc_idx: usize,
    /// Index of the mapped PL011 UART device.
    pub uart_idx: usize,
    /// Index of the mapped fw_cfg device.
    pub fw_cfg_idx: usize,
    /// Optional index of the mapped PL031 RTC device when that quirk is enabled.
    pub rtc_idx: Option<usize>,
    /// Optional index of the mapped secondary PL011 UART when that quirk is enabled.
    pub secure_uart_idx: Option<usize>,
}

/// Built interrupt-controller state for one raw arm-virt platform realization.
pub enum BuiltArmVirtGic {
    /// `GICv2` shared state for the built machine.
    V2(Arc<Mutex<GicSharedState>>),
    /// `GICv3` shared state for the built machine.
    V3(Arc<Mutex<GicV3SharedState>>),
}

/// Built raw arm-virt platform artifact.
pub struct BuiltArmVirtPlatform {
    /// Fully populated system address space containing RAM and fixed devices.
    pub sys_mem: HelmAddressSpace,
    /// Device indices for fixed built-in peripherals.
    pub devs: ArmVirtDevices,
    /// IRQ lines wired from fixed devices into the interrupt controller.
    pub irq_lines: Vec<Arc<AtomicBool>>,
    /// Active board/platform quirks used when building this realization.
    pub quirks: crate::QuirkSet,
    /// Built interrupt-controller state for the selected GIC version.
    pub gic: BuiltArmVirtGic,
}

/// ARM virt platform descriptor.
///
/// Mirrors QEMU's `-machine virt` address map. The platform itself is
/// stateless — it describes *what* to build, not *how* to build it.
pub struct ArmVirtPlatform;

impl Platform for ArmVirtPlatform {
    fn name(&self) -> &'static str {
        "arm-virt"
    }

    fn attachment_slots(&self) -> &[AttachableSlot] {
        &[
            AttachableSlot {
                name: "mmio",
                slot_type: SlotType::Mmio {
                    base_range: (MMIO_BASE, MMIO_END),
                },
                max_devices: 16,
            },
            AttachableSlot {
                name: "pci0",
                slot_type: SlotType::Pci,
                max_devices: 1,
            },
        ]
    }

    fn build_plan(&self) -> PlatformBuildPlan {
        self.build_plan_with_gic_profile(ArmVirtGicProfile::default())
    }
}

fn arm_virt_common_address_regions() -> Vec<AddressRegionSpec> {
    vec![
        AddressRegionSpec {
            name: "flash",
            base: FLASH_BASE,
            size: FLASH_SIZE,
            kind: RegionKind::Reserved,
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
            name: "fw-cfg",
            base: FW_CFG_BASE,
            size: 0x18,
            kind: RegionKind::Mmio,
        },
        AddressRegionSpec {
            name: "gpio0",
            base: GPIO_BASE,
            size: 0x1000,
            kind: RegionKind::Reserved,
        },
        AddressRegionSpec {
            name: "uart1",
            base: SECURE_UART_BASE,
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
            name: "pci-mmio",
            base: PCIE_MMIO_BASE,
            size: PCIE_MMIO_SIZE,
            kind: RegionKind::AttachmentWindow,
        },
        AddressRegionSpec {
            name: "pci-pio",
            base: PCIE_PIO_BASE,
            size: PCIE_PIO_SIZE,
            kind: RegionKind::Reserved,
        },
        AddressRegionSpec {
            name: "pci-ecam",
            base: PCIE_ECAM_BASE,
            size: PCIE_ECAM_SIZE,
            kind: RegionKind::Mmio,
        },
        AddressRegionSpec {
            name: "ram",
            base: RAM_BASE,
            size: 0,
            kind: RegionKind::Ram,
        },
    ]
}

fn arm_virt_address_regions(profile: ArmVirtGicProfile) -> Vec<AddressRegionSpec> {
    let mut regions = vec![AddressRegionSpec {
        name: "gic-dist",
        base: GICD_BASE,
        size: GICD_REGION_SIZE,
        kind: RegionKind::Mmio,
    }];

    match profile {
        ArmVirtGicProfile::V2 => {
            regions.push(AddressRegionSpec {
                name: "gic-cpu",
                base: GICC_BASE,
                size: GICC_REGION_SIZE,
                kind: RegionKind::Mmio,
            });
            regions.push(AddressRegionSpec {
                name: "gic-v2m",
                base: GICV2M_BASE,
                size: GICV2M_REGION_SIZE,
                kind: RegionKind::Reserved,
            });
        }
        ArmVirtGicProfile::V3 => {
            regions.push(AddressRegionSpec {
                name: "gic-its",
                base: GITS_BASE,
                size: GITS_REGION_SIZE,
                kind: RegionKind::Reserved,
            });
            regions.push(AddressRegionSpec {
                name: "gic-redist",
                base: GICR_BASE,
                size: GICR_REGION_SIZE,
                kind: RegionKind::Mmio,
            });
        }
    }

    regions.extend(arm_virt_common_address_regions());
    regions
}

fn arm_virt_topology(profile: ArmVirtGicProfile) -> DeviceTopology {
    let mut root = DeviceNode::new("soc", "ArmVirt").with_child(match profile {
        ArmVirtGicProfile::V2 => DeviceNode::new("gic-dist", "GICv2Distributor")
            .with_base(GICD_BASE)
            .with_size(GICD_REGION_SIZE),
        ArmVirtGicProfile::V3 => DeviceNode::new("gic-dist", "GICv3Distributor")
            .with_base(GICD_BASE)
            .with_size(GICD_REGION_SIZE),
    });

    root = match profile {
        ArmVirtGicProfile::V2 => root
            .with_child(
                DeviceNode::new("gic-cpu", "GICv2CpuInterface")
                    .with_base(GICC_BASE)
                    .with_size(GICC_REGION_SIZE),
            )
            .with_child(
                DeviceNode::new("gic-v2m", "ReservedGicV2MFrame")
                    .with_base(GICV2M_BASE)
                    .with_size(GICV2M_REGION_SIZE),
            ),
        ArmVirtGicProfile::V3 => root
            .with_child(
                DeviceNode::new("gic-redist", "GICv3Redistributor")
                    .with_base(GICR_BASE)
                    .with_size(GICR_REGION_SIZE),
            )
            .with_child(
                DeviceNode::new("gic-its", "ReservedGicItsWindow")
                    .with_base(GITS_BASE)
                    .with_size(GITS_REGION_SIZE),
            ),
    };

    DeviceTopology::new(
        root.with_child(
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
        )
        .with_child(
            DeviceNode::new("fw-cfg", "FwCfgMmio")
                .with_base(FW_CFG_BASE)
                .with_size(0x18),
        )
        .with_child(
            DeviceNode::new("gpio0", "ReservedPl061")
                .with_base(GPIO_BASE)
                .with_size(0x1000)
                .with_irq(GPIO_IRQ),
        )
        .with_child(
            DeviceNode::new("uart1", "PL011")
                .with_base(SECURE_UART_BASE)
                .with_size(0x1000)
                .with_irq(SECURE_UART_IRQ),
        )
        .with_child(
            DeviceNode::new("pcie", "PciHostEcam")
                .with_base(PCIE_ECAM_BASE)
                .with_size(PCIE_ECAM_SIZE),
        )
        .with_child(
            DeviceNode::new("flash0", "ReservedFlashWindow")
                .with_base(FLASH_BASE)
                .with_size(FLASH_SIZE),
        ),
    )
}

fn infer_arm_virt_gic_profile(mappings: &[BuiltInMappedDevice]) -> ArmVirtGicProfile {
    if mappings
        .iter()
        .any(|mapping| matches!(mapping.kind, BuiltInMappedDeviceKind::GicV2 { .. }))
    {
        ArmVirtGicProfile::V2
    } else {
        ArmVirtGicProfile::V3
    }
}

impl ArmVirtPlatform {
    /// Return a frozen build plan for one intentional arm-virt GIC profile.
    pub fn build_plan_with_gic_profile(&self, profile: ArmVirtGicProfile) -> PlatformBuildPlan {
        PlatformBuildPlan {
            platform_name: "arm-virt",
            attachment_slots: self.attachment_slots().to_vec(),
            address_regions: arm_virt_address_regions(profile),
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
                InterruptRouteSpec {
                    source: "uart1",
                    line: SECURE_UART_IRQ,
                    sink: "gic-dist",
                },
                InterruptRouteSpec {
                    source: "pci0-inta",
                    line: PCIE_INTX_IRQ_BASE,
                    sink: "gic-dist",
                },
                InterruptRouteSpec {
                    source: "pci0-intb",
                    line: PCIE_INTX_IRQ_BASE + 1,
                    sink: "gic-dist",
                },
                InterruptRouteSpec {
                    source: "pci0-intc",
                    line: PCIE_INTX_IRQ_BASE + 2,
                    sink: "gic-dist",
                },
                InterruptRouteSpec {
                    source: "pci0-intd",
                    line: PCIE_INTX_IRQ_BASE + 3,
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
                    key: QuirkKey::Platform(PlatformQuirk::ArmVirtSecondPl011Uart),
                    summary: "Expose a second PL011 UART at 0x0904_0000, wired to SPI 40.",
                    default_enabled: false,
                },
                QuirkSpec {
                    key: QuirkKey::Platform(PlatformQuirk::ArmVirtDtbRandomness),
                    summary: "Populate /chosen with kaslr-seed and rng-seed properties.",
                    default_enabled: false,
                },
                QuirkSpec {
                    key: QuirkKey::Board(BoardQuirk::PsciViaEngine),
                    summary: "Route PSCI power-management calls through the engine.",
                    default_enabled: true,
                },
            ],
            topology: self.topology_with_gic_profile(profile),
        }
    }

    /// Return the default quirk selection for arm-virt.
    pub fn default_quirks(&self) -> crate::QuirkSet {
        self.build_plan_with_gic_profile(ArmVirtGicProfile::default())
            .default_quirks()
    }

    /// Return the default RAM base for the arm-virt platform.
    pub fn default_ram_base(&self) -> u64 {
        self.build_plan()
            .region_named("ram")
            .map(|region| region.base)
            .unwrap_or(RAM_BASE)
    }

    /// Return the current platform-owned PCI MSI routing contract.
    pub fn pci_msi_route(&self, num_irqs: u32) -> ArmVirtPciMsiRoute {
        ArmVirtPciMsiRoute {
            target_addr: PCIE_MSI_ADDR,
            spi_base: 32,
            spi_count: num_irqs.saturating_sub(32),
        }
    }

    /// Validate discovered mappings against the arm-virt system-mode layout.
    pub fn validate_system_mappings(
        &self,
        mappings: &[BuiltInMappedDevice],
    ) -> Result<(), PlatformError> {
        let plan = self.build_plan_with_gic_profile(infer_arm_virt_gic_profile(mappings));
        for mapping in mappings {
            validate_system_mapping(&plan, mapping)?;
        }
        Ok(())
    }

    /// Build the raw arm-virt platform with GICv2 device wiring.
    pub fn build_raw_gicv2(
        &self,
        mem_mib: usize,
        num_cpus: usize,
        uart_backend: Box<dyn CharBackend>,
    ) -> BuiltArmVirtPlatform {
        let quirks = self.default_quirks();
        build_raw_gicv2_with_quirks(mem_mib, num_cpus, uart_backend, &quirks)
    }

    /// Build the raw arm-virt platform with GICv3 device wiring.
    pub fn build_raw_gicv3(
        &self,
        mem_mib: usize,
        num_cpus: usize,
        uart_backend: Box<dyn CharBackend>,
    ) -> BuiltArmVirtPlatform {
        let quirks = self.default_quirks();
        build_raw_gicv3_with_quirks(mem_mib, num_cpus, uart_backend, &quirks)
    }

    /// Build the default device topology for this platform.
    ///
    /// This is a static description of the platform's device tree,
    /// suitable for `print_topology()`.
    pub fn topology(&self) -> DeviceTopology {
        self.topology_with_gic_profile(ArmVirtGicProfile::default())
    }

    /// Build a static topology description for one intentional GIC profile.
    pub fn topology_with_gic_profile(&self, profile: ArmVirtGicProfile) -> DeviceTopology {
        arm_virt_topology(profile)
    }
}

fn install_default_arm_virt_pci_bus(sys_mem: &mut HelmAddressSpace) {
    let _ = sys_mem.add_device(PCIE_ECAM_BASE, Box::new(PciBus::new("pci0")));
}

fn build_raw_gicv2_with_quirks(
    mem_mib: usize,
    num_cpus: usize,
    uart_backend: Box<dyn CharBackend>,
    quirks: &crate::QuirkSet,
) -> BuiltArmVirtPlatform {
    let ram = FlatMem::new(RAM_BASE, mem_mib * 1024 * 1024);
    let mut sys_mem = HelmAddressSpace::new(ram);

    let (gicd, _giccs, irq_lines, gic_state) =
        build_gicv2_mp(ArmVirtGicProfile::V2.baseline_irq_capacity(), num_cpus);
    let gicc = Gicv2CpuInterface::from_banked_shared(Arc::clone(&gic_state));
    let gicd_idx = sys_mem.add_device(GICD_BASE, Box::new(gicd));
    let gicc_idx = sys_mem.add_device(GICC_BASE, Box::new(gicc));

    let mut uart = Pl011::new(uart_backend);
    {
        use helm_devices::WireId;
        use helm_hw_intc::GicSink;
        let sink = Arc::new(GicSink::new(Arc::clone(&gic_state), UART_IRQ));
        uart.irq_out.wire(WireId::from(UART_IRQ), sink);
    }
    let uart_idx = sys_mem.add_device(UART_BASE, Box::new(uart));
    let fw_cfg_idx = sys_mem.add_device(FW_CFG_BASE, Box::new(FwCfgMmio::new(num_cpus)));

    let rtc_idx = if quirks.contains(QuirkKey::Platform(PlatformQuirk::ArmVirtPl031Rtc)) {
        let mut rtc = Pl031::new(0);
        {
            use helm_devices::WireId;
            use helm_hw_intc::GicSink;
            let sink = Arc::new(GicSink::new(Arc::clone(&gic_state), RTC_IRQ));
            rtc.irq_out.wire(WireId::from(RTC_IRQ), sink);
        }
        Some(sys_mem.add_device(RTC_BASE, Box::new(rtc)))
    } else {
        None
    };

    let secure_uart_idx =
        if quirks.contains(QuirkKey::Platform(PlatformQuirk::ArmVirtSecondPl011Uart)) {
            let mut uart1 = Pl011::new(Box::new(NullCharBackend));
            {
                use helm_devices::WireId;
                use helm_hw_intc::GicSink;
                let sink = Arc::new(GicSink::new(Arc::clone(&gic_state), SECURE_UART_IRQ));
                uart1.irq_out.wire(WireId::from(SECURE_UART_IRQ), sink);
            }
            Some(sys_mem.add_device(SECURE_UART_BASE, Box::new(uart1)))
        } else {
            None
        };

    install_default_arm_virt_pci_bus(&mut sys_mem);

    BuiltArmVirtPlatform {
        sys_mem,
        devs: ArmVirtDevices {
            gicd_idx,
            gicc_idx,
            uart_idx,
            fw_cfg_idx,
            rtc_idx,
            secure_uart_idx,
        },
        irq_lines,
        quirks: quirks.clone(),
        gic: BuiltArmVirtGic::V2(gic_state),
    }
}

fn build_raw_gicv3_with_quirks(
    mem_mib: usize,
    num_cpus: usize,
    uart_backend: Box<dyn CharBackend>,
    quirks: &crate::QuirkSet,
) -> BuiltArmVirtPlatform {
    let ram = FlatMem::new(RAM_BASE, mem_mib * 1024 * 1024);
    let mut sys_mem = HelmAddressSpace::new(ram);

    let affinities: Vec<u64> = (0..num_cpus).map(|i| i as u64).collect();
    let (gicd, gicrs, irq_lines, gicv3_state) = helm_hw_intc::build_gicv3_mp(
        ArmVirtGicProfile::V3.baseline_irq_capacity(),
        num_cpus,
        &affinities,
    );

    let gicd_idx = sys_mem.add_device(GICD_BASE, Box::new(gicd));
    let mut first_gicr_idx = 0;
    for (i, gicr) in gicrs.into_iter().enumerate() {
        let idx = sys_mem.add_device(GICR_BASE + (i as u64) * GICR_STRIDE, Box::new(gicr));
        if i == 0 {
            first_gicr_idx = idx;
        }
    }

    let mut uart = Pl011::new(uart_backend);
    {
        use helm_devices::WireId;
        use helm_hw_intc::GicV3Sink;
        let sink = Arc::new(GicV3Sink::new(Arc::clone(&gicv3_state), UART_IRQ));
        uart.irq_out.wire(WireId::from(UART_IRQ), sink);
    }
    let uart_idx = sys_mem.add_device(UART_BASE, Box::new(uart));
    let fw_cfg_idx = sys_mem.add_device(FW_CFG_BASE, Box::new(FwCfgMmio::new(num_cpus)));

    let rtc_idx = if quirks.contains(QuirkKey::Platform(PlatformQuirk::ArmVirtPl031Rtc)) {
        let mut rtc = Pl031::new(0);
        {
            use helm_devices::WireId;
            use helm_hw_intc::GicV3Sink;
            let sink = Arc::new(GicV3Sink::new(Arc::clone(&gicv3_state), RTC_IRQ));
            rtc.irq_out.wire(WireId::from(RTC_IRQ), sink);
        }
        Some(sys_mem.add_device(RTC_BASE, Box::new(rtc)))
    } else {
        None
    };

    let secure_uart_idx =
        if quirks.contains(QuirkKey::Platform(PlatformQuirk::ArmVirtSecondPl011Uart)) {
            let mut uart1 = Pl011::new(Box::new(NullCharBackend));
            {
                use helm_devices::WireId;
                use helm_hw_intc::GicV3Sink;
                let sink = Arc::new(GicV3Sink::new(Arc::clone(&gicv3_state), SECURE_UART_IRQ));
                uart1.irq_out.wire(WireId::from(SECURE_UART_IRQ), sink);
            }
            Some(sys_mem.add_device(SECURE_UART_BASE, Box::new(uart1)))
        } else {
            None
        };

    install_default_arm_virt_pci_bus(&mut sys_mem);

    BuiltArmVirtPlatform {
        sys_mem,
        devs: ArmVirtDevices {
            gicd_idx,
            gicc_idx: first_gicr_idx,
            uart_idx,
            fw_cfg_idx,
            rtc_idx,
            secure_uart_idx,
        },
        irq_lines,
        quirks: quirks.clone(),
        gic: BuiltArmVirtGic::V3(gicv3_state),
    }
}

fn validate_system_mapping(
    plan: &PlatformBuildPlan,
    mapping: &BuiltInMappedDevice,
) -> Result<(), PlatformError> {
    match &mapping.kind {
        BuiltInMappedDeviceKind::Ram => {
            let ram = plan
                .region_named("ram")
                .expect("arm-virt RAM region missing");
            if mapping.base != ram.base {
                return Err(PlatformError::other(format!(
                    "system-mode RAM must start at {:#x}, got {:#x}",
                    ram.base, mapping.base
                )));
            }
        }
        BuiltInMappedDeviceKind::GicV2 { .. } => {
            let region_name = match mapping.bank {
                0 => "gic-dist",
                1 => "gic-cpu",
                other => {
                    return Err(PlatformError::other(format!(
                        "system-mode GicV2 bank must be 0 or 1, got {other}"
                    )));
                }
            };
            validate_exact_region(plan, region_name, mapping)?;
        }
        BuiltInMappedDeviceKind::Pl011 => {
            if mapping.bank != 0 {
                return Err(PlatformError::other(format!(
                    "system-mode Pl011 bank must be 0, got {}",
                    mapping.bank
                )));
            }
            validate_exact_region(plan, "uart0", mapping)?;
        }
        BuiltInMappedDeviceKind::Unknown { python_type } => {
            if plan
                .attachment_window_for(mapping.base, mapping.size as u64)
                .is_none()
            {
                return Err(PlatformError::other(format!(
                    "system-mode mapping for unknown device type '{python_type}' must fit an attachment window"
                )));
            }
        }
    }

    Ok(())
}

fn validate_exact_region(
    plan: &PlatformBuildPlan,
    region_name: &str,
    mapping: &BuiltInMappedDevice,
) -> Result<(), PlatformError> {
    let region = plan
        .region_named(region_name)
        .ok_or_else(|| PlatformError::other(format!("missing platform region '{region_name}'")))?;

    if mapping.base != region.base || mapping.size as u64 != region.size {
        return Err(PlatformError::other(format!(
            "system-mode mapping for '{region_name}' must be [{:#x}, {:#x}), got [{:#x}, {:#x})",
            region.base,
            region.base.saturating_add(region.size),
            mapping.base,
            mapping.base.saturating_add(mapping.size as u64),
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_core::{AccessType, MemInterface};
    use helm_devices::NullCharBackend;
    use helm_hw_firmware::fw_cfg::{FW_CFG_NB_CPUS, FW_CFG_SIGNATURE};

    #[test]
    fn platform_name() {
        let p = ArmVirtPlatform;
        assert_eq!(p.name(), "arm-virt");
    }

    #[test]
    fn attachment_slots() {
        let p = ArmVirtPlatform;
        let slots = p.attachment_slots();
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].name, "mmio");
        assert_eq!(slots[0].max_devices, 16);
        assert_eq!(slots[1].name, "pci0");
        assert!(matches!(slots[1].slot_type, SlotType::Pci));
    }

    #[test]
    fn topology_prints() {
        let p = ArmVirtPlatform;
        let topo = p.topology();
        let output = topo.print();
        assert!(output.contains("arm-virt") || output.contains("ArmVirt"));
        assert!(output.contains("gic-dist"));
        assert!(output.contains("gic-redist"));
        assert!(output.contains("gic-its"));
        assert!(output.contains("pcie"));
        assert!(output.contains("gpio0"));
        assert!(output.contains("uart1"));
        assert!(output.contains("uart0"));
    }

    #[test]
    fn build_plan_captures_layout_and_routes() {
        let p = ArmVirtPlatform;
        let plan = p.build_plan();
        let quirks = plan.default_quirks();

        assert_eq!(plan.platform_name, "arm-virt");
        assert!(plan.address_regions.iter().any(|r| r.name == "flash"
            && r.base == FLASH_BASE
            && r.size == FLASH_SIZE
            && r.kind == RegionKind::Reserved));
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
            .any(|r| r.name == "gic-its" && r.base == GITS_BASE && r.kind == RegionKind::Reserved));
        assert!(!plan
            .address_regions
            .iter()
            .any(|r| r.name == "gic-cpu" || r.name == "gic-v2m"));
        assert!(plan
            .address_regions
            .iter()
            .any(|r| r.name == "fw-cfg" && r.base == FW_CFG_BASE && r.kind == RegionKind::Mmio));
        assert!(plan
            .address_regions
            .iter()
            .any(|r| r.name == "gpio0" && r.base == GPIO_BASE && r.kind == RegionKind::Reserved));
        assert!(plan.address_regions.iter().any(|r| r.name == "uart1"
            && r.base == SECURE_UART_BASE
            && r.kind == RegionKind::Mmio));
        assert!(plan
            .address_regions
            .iter()
            .any(|r| r.name == "mmio" && r.kind == RegionKind::AttachmentWindow));
        assert!(plan
            .address_regions
            .iter()
            .any(|r| r.name == "pci-mmio" && r.base == PCIE_MMIO_BASE && r.size == PCIE_MMIO_SIZE));
        assert!(plan
            .address_regions
            .iter()
            .any(|r| r.name == "pci-pio" && r.base == PCIE_PIO_BASE && r.size == PCIE_PIO_SIZE));
        assert!(plan
            .address_regions
            .iter()
            .any(|r| r.name == "pci-ecam" && r.base == PCIE_ECAM_BASE && r.size == PCIE_ECAM_SIZE));
        assert!(plan
            .interrupt_routes
            .iter()
            .any(|r| r.source == "uart0" && r.line == UART_IRQ && r.sink == "gic-dist"));
        assert!(plan
            .interrupt_routes
            .iter()
            .any(|r| r.source == "uart1" && r.line == SECURE_UART_IRQ && r.sink == "gic-dist"));
        assert!(plan.interrupt_routes.iter().any(|r| r.source == "pci0-inta"
            && r.line == PCIE_INTX_IRQ_BASE
            && r.sink == "gic-dist"));
        assert!(plan.supports_quirk(QuirkKey::Platform(PlatformQuirk::ArmVirtPl031Rtc)));
        assert!(plan.supports_quirk(QuirkKey::Platform(PlatformQuirk::ArmVirtSecondPl011Uart)));
        assert!(plan.supports_quirk(QuirkKey::Platform(PlatformQuirk::ArmVirtDtbRandomness)));
        assert!(plan.supports_quirk(QuirkKey::Board(BoardQuirk::PsciViaEngine)));
        assert!(quirks.contains(QuirkKey::Platform(PlatformQuirk::ArmVirtPl031Rtc)));
        assert!(!quirks.contains(QuirkKey::Platform(PlatformQuirk::ArmVirtSecondPl011Uart)));
        assert!(!quirks.contains(QuirkKey::Platform(PlatformQuirk::ArmVirtDtbRandomness)));
        assert!(quirks.contains(QuirkKey::Board(BoardQuirk::PsciViaEngine)));
    }

    #[test]
    fn build_plan_for_gicv2_profile_uses_cpu_interface_and_v2m_reservation() {
        let p = ArmVirtPlatform;
        let plan = p.build_plan_with_gic_profile(ArmVirtGicProfile::V2);

        assert!(plan
            .address_regions
            .iter()
            .any(|r| r.name == "gic-cpu" && r.base == GICC_BASE && r.size == GICC_REGION_SIZE));
        assert!(plan.address_regions.iter().any(|r| {
            r.name == "gic-v2m"
                && r.base == GICV2M_BASE
                && r.size == GICV2M_REGION_SIZE
                && r.kind == RegionKind::Reserved
        }));
        assert!(!plan
            .address_regions
            .iter()
            .any(|r| r.name == "gic-redist" || r.name == "gic-its"));
    }

    #[test]
    fn topology_for_gicv2_profile_prints_v2_specific_nodes() {
        let p = ArmVirtPlatform;
        let output = p.topology_with_gic_profile(ArmVirtGicProfile::V2).print();

        assert!(output.contains("gic-dist [GICv2Distributor]"));
        assert!(output.contains("gic-cpu [GICv2CpuInterface]"));
        assert!(output.contains("gic-v2m [ReservedGicV2MFrame]"));
        assert!(!output.contains("gic-redist [GICv3Redistributor]"));
    }

    #[test]
    fn default_ram_base_matches_ram_region() {
        let p = ArmVirtPlatform;
        assert_eq!(p.default_ram_base(), RAM_BASE);
    }

    #[test]
    fn raw_platform_realization_can_enable_second_uart() {
        let mut quirks = ArmVirtPlatform.default_quirks();
        quirks.enable(QuirkKey::Platform(PlatformQuirk::ArmVirtSecondPl011Uart));

        let built = build_raw_gicv2_with_quirks(256, 1, Box::new(NullCharBackend), &quirks);

        assert!(built.devs.secure_uart_idx.is_some());
        assert!(built.sys_mem.address_map.lookup(SECURE_UART_BASE).is_some());
    }

    #[test]
    fn raw_platform_realization_installs_default_pci_bus() {
        let built = ArmVirtPlatform.build_raw_gicv3(256, 1, Box::new(NullCharBackend));
        assert!(built.sys_mem.address_map.lookup(PCIE_ECAM_BASE).is_some());
        assert_eq!(built.sys_mem.devices.len(), 6);
        let BuiltArmVirtGic::V3(gic) = built.gic else {
            panic!("expected GICv3 platform state");
        };
        assert_eq!(
            gic.lock().unwrap().dist.num_irqs,
            ArmVirtGicProfile::V3.baseline_irq_capacity()
        );
    }

    #[test]
    fn raw_platform_realization_installs_fw_cfg_device() {
        let mut built = ArmVirtPlatform.build_raw_gicv2(256, 3, Box::new(NullCharBackend));

        built
            .sys_mem
            .write(
                FW_CFG_BASE + 0x08,
                2,
                u64::from(FW_CFG_SIGNATURE),
                AccessType::Store,
            )
            .unwrap();
        assert_eq!(
            built
                .sys_mem
                .read(FW_CFG_BASE, 1, AccessType::Load)
                .unwrap(),
            u64::from(b'Q')
        );
        assert_eq!(
            built
                .sys_mem
                .read(FW_CFG_BASE, 1, AccessType::Load)
                .unwrap(),
            u64::from(b'E')
        );

        built
            .sys_mem
            .write(
                FW_CFG_BASE + 0x08,
                2,
                u64::from(FW_CFG_NB_CPUS),
                AccessType::Store,
            )
            .unwrap();
        assert_eq!(
            built
                .sys_mem
                .read(FW_CFG_BASE, 4, AccessType::Load)
                .unwrap(),
            3
        );
        let BuiltArmVirtGic::V2(gic) = built.gic else {
            panic!("expected GICv2 platform state");
        };
        assert_eq!(
            gic.lock().unwrap().dist.num_irqs,
            ArmVirtGicProfile::V2.baseline_irq_capacity()
        );
        assert_eq!(built.devs.secure_uart_idx, None);
    }

    #[test]
    fn pci_msi_route_translates_only_routable_spis() {
        let route = ArmVirtPlatform.pci_msi_route(128);
        assert_eq!(route.translate(PCIE_MSI_ADDR, 32), Some(32));
        assert_eq!(route.translate(PCIE_MSI_ADDR, 65), Some(65));
        assert_eq!(route.translate(PCIE_MSI_ADDR, 31), None);
        assert_eq!(route.translate(PCIE_MSI_ADDR, 128), None);
        assert_eq!(route.translate(0, 65), None);
    }

    #[test]
    fn validate_system_mappings_accepts_fixed_platform_regions() {
        let p = ArmVirtPlatform;
        let mappings = vec![
            BuiltInMappedDevice {
                base: RAM_BASE,
                size: 128 * 1024 * 1024,
                bank: 0,
                kind: BuiltInMappedDeviceKind::Ram,
            },
            BuiltInMappedDevice {
                base: GICD_BASE,
                size: GICD_REGION_SIZE as usize,
                bank: 0,
                kind: BuiltInMappedDeviceKind::GicV2 { num_irqs: 96 },
            },
            BuiltInMappedDevice {
                base: UART_BASE,
                size: 0x1000,
                bank: 0,
                kind: BuiltInMappedDeviceKind::Pl011,
            },
            BuiltInMappedDevice {
                base: MMIO_BASE,
                size: 0x1000,
                bank: 0,
                kind: BuiltInMappedDeviceKind::Unknown {
                    python_type: "CustomDevice".to_string(),
                },
            },
        ];

        assert!(p.validate_system_mappings(&mappings).is_ok());
    }

    #[test]
    fn validate_system_mappings_rejects_unknown_device_outside_attachment_window() {
        let p = ArmVirtPlatform;
        let mappings = vec![BuiltInMappedDevice {
            base: UART_BASE + 0x1000,
            size: 0x1000,
            bank: 0,
            kind: BuiltInMappedDeviceKind::Unknown {
                python_type: "CustomDevice".to_string(),
            },
        }];

        let err = p
            .validate_system_mappings(&mappings)
            .expect_err("mapping should be rejected");
        assert!(err.to_string().contains("attachment window"));
    }
}

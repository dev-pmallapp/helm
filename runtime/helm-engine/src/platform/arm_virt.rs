//! ARM virt platform — QEMU-compatible address map and device wiring.
//!
//! Devices:
//! - GICv2 Distributor at 0x0800_0000  (4 KiB)
//! - GICv2 CPU Interface at 0x0801_0000 (4 KiB)
//! - PL011 UART at 0x0900_0000         (4 KiB)
//! - RAM at 0x4000_0000
//!
//! The GIC distributor and CPU interface share state via `Arc<Mutex<>>` and
//! a single `Arc<AtomicBool>` IRQ line that the FS step loop polls each step.

use std::sync::atomic::AtomicBool;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use helm_arch::Aarch64ArchState;
use helm_devices::{
    CharBackend, Device, MessageInterrupt, MessageInterruptEmitter, MessageInterruptSink,
};
use helm_hw_char::Pl011;
use helm_hw_iommu::smmu::SmmuState;
use helm_hw_intc::build_gicv2_mp;
use helm_hw_intc::Gicv2CpuInterface;
use helm_hw_pci::{build_pci_bar0_endpoint, build_pci_ram_bar_pair, Bdf, PciBuildError, PciBus};
use helm_hw_rtc::Pl031;
use helm_hw_virtio::blk::VirtioBlk;
use helm_hw_virtio::console::VirtioConsole;
use helm_hw_virtio::net::VirtioNet;
use helm_hw_virtio::pci::{build_virtio_pci_rng_pair, VirtioPciBuildError};
use helm_hw_virtio::proto::transport::VirtioMmioTransport;
use helm_hw_virtio::proto::virtqueue::RamBlockBackend;
use helm_hw_virtio::rng::VirtioRng;
use helm_platform::aarch64::virt::{
    ArmVirtPciMsiRoute, ArmVirtPlatform, GICC_BASE, GICD_BASE, GICR_BASE, GICR_STRIDE, MMIO_BASE,
    MMIO_END, PCIE_ECAM_BASE, RAM_BASE, RTC_BASE, RTC_IRQ, SMMU_BASE, UART_BASE, UART_IRQ,
};
use helm_platform::{BoardQuirk, Platform, PlatformQuirk, QuirkKey, QuirkSet};

use crate::address_space::HelmAddressSpace;
use crate::fs;
use crate::fs::FsState;
use crate::loader::{load_arm64_kernel, load_arm64_kernel_with_dtb_bytes};
use crate::platform::arm_virt_dtb::build_baseline_arm_virt_dtb;
use crate::session::{BuiltAarch64System, BuiltSystem, HelmBoard, HelmGic, HelmVcpu};
use crate::FlatMem;
use helm_core::{ByteMem, MemFault, MemInterface};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmVirtGicVersion {
    V2,
    V3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmVirtBootPolicy {
    ImageDefault,
    El1,
    El2,
    El3,
}

impl ArmVirtBootPolicy {
    fn resolve(self, image_boot_el: u8) -> u8 {
        match self {
            Self::ImageDefault => image_boot_el,
            Self::El1 => 1,
            Self::El2 => 2,
            Self::El3 => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ArmVirtBootPolicyError {
    #[error("unsupported arm-virt boot EL {0} (expected 1, 2, or 3)")]
    UnsupportedBootEl(u8),
}

pub fn arm_virt_boot_policy_from_override(
    boot_el: Option<u8>,
) -> Result<ArmVirtBootPolicy, ArmVirtBootPolicyError> {
    match boot_el {
        None => Ok(ArmVirtBootPolicy::ImageDefault),
        Some(1) => Ok(ArmVirtBootPolicy::El1),
        Some(2) => Ok(ArmVirtBootPolicy::El2),
        Some(3) => Ok(ArmVirtBootPolicy::El3),
        Some(other) => Err(ArmVirtBootPolicyError::UnsupportedBootEl(other)),
    }
}

const ARM_VIRT_SMMU_GERROR_IRQ: u32 = 106;
const ARM_VIRT_SMMU_EVTQ_IRQ: u32 = 108;

struct LiveFlatMemByteMem {
    ram: NonNull<FlatMem>,
}

impl LiveFlatMemByteMem {
    fn new(ram: &mut FlatMem) -> Self {
        Self {
            ram: NonNull::from(ram),
        }
    }

    #[allow(unsafe_code)]
    fn ram_mut(&mut self) -> &mut FlatMem {
        // Safety: the adapter is only created after the system memory is boxed,
        // so the FlatMem pointee remains stable for the lifetime of the SMMU.
        unsafe { self.ram.as_mut() }
    }
}

#[allow(unsafe_code)]
unsafe impl Send for LiveFlatMemByteMem {}

impl ByteMem for LiveFlatMemByteMem {
    fn read_bytes(&mut self, addr: u64, buf: &mut [u8]) -> Result<(), MemFault> {
        for (offset, byte) in buf.iter_mut().enumerate() {
            *byte = self
                .ram_mut()
                .read(addr + offset as u64, 1, helm_core::AccessType::Load)? as u8;
        }
        Ok(())
    }

    fn write_bytes(&mut self, addr: u64, data: &[u8]) -> Result<(), MemFault> {
        for (offset, byte) in data.iter().enumerate() {
            self.ram_mut().write(
                addr + offset as u64,
                1,
                u64::from(*byte),
                helm_core::AccessType::Store,
            )?;
        }
        Ok(())
    }
}

// ── Timer PPI injection ───────────────────────────────────────────────────────

/// Inject ARM generic timer PPIs into the GIC via `GICD_ISPENDR`/`GICD_ICPENDR`.
///
/// **Why MMIO and not `assert_irq`/`deassert_irq`?**
///
/// `GicState::assert_irq()` sets `physical_level[]`, which causes `cpu_eoi()` to
/// re-pend the interrupt immediately after the kernel's EOI write — even after
/// the kernel reprogrammed `CVAL` to a future deadline. The result is an infinite
/// timer ISR storm that locks up the boot.
///
/// Writing to `GICD_ISPENDR`/`ICPENDR` only touches `pending[]`, not
/// `physical_level[]`. After EOI the interrupt is fully quiesced; it re-fires
/// only when `check_timers` next finds the condition met. This matches the
/// approach used by the `../helm.git` reference implementation.
pub fn inject_timers_gicv2(
    a64: &mut helm_arch::Aarch64ArchState,
    fs: &mut FsState,
    sys_mem: &mut HelmAddressSpace,
) {
    const PTIMER_BIT: u64 = 1 << 30; // INTID 30 = PPI 14 (non-secure phys timer)
    const VTIMER_BIT: u64 = 1 << 27; // INTID 27 = PPI 11 (virtual timer)
    const HTIMER_BIT: u64 = 1 << 26; // INTID 26 = PPI 10 (hypervisor phys timer)
    const GICD_ISPENDR0: u64 = GICD_BASE + 0x200; // set-pending register[0]
    const GICD_ICPENDR0: u64 = GICD_BASE + 0x280; // clear-pending register[0]

    let (p_fire, v_fire, h_fire) = fs::check_timers(a64, fs);

    let ispendr = (if p_fire { PTIMER_BIT } else { 0 })
        | (if v_fire { VTIMER_BIT } else { 0 })
        | (if h_fire { HTIMER_BIT } else { 0 });
    let icpendr = (if p_fire { 0 } else { PTIMER_BIT })
        | (if v_fire { 0 } else { VTIMER_BIT })
        | (if h_fire { 0 } else { HTIMER_BIT });

    if ispendr != 0 {
        let _ = sys_mem.write(GICD_ISPENDR0, 4, ispendr, helm_core::AccessType::Store);
    }
    if icpendr != 0 {
        let _ = sys_mem.write(GICD_ICPENDR0, 4, icpendr, helm_core::AccessType::Store);
    }
}

/// Inject ARM generic timer PPIs into the vCPU-local GICv3 redistributor.
pub fn inject_timers_gicv3(
    a64: &mut helm_arch::Aarch64ArchState,
    fs: &mut FsState,
    gicv3: &Arc<Mutex<helm_hw_intc::GicV3SharedState>>,
    vcpu_idx: usize,
) {
    const PTIMER_BIT: u32 = 1 << 30;
    const VTIMER_BIT: u32 = 1 << 27;
    const HTIMER_BIT: u32 = 1 << 26;

    let (p_fire, v_fire, h_fire) = fs::check_timers(a64, fs);
    let mut shared = gicv3.lock().unwrap();
    let Some(redist) = shared.redists.get_mut(vcpu_idx) else {
        return;
    };

    if p_fire {
        redist.sgi_ppi_pending |= PTIMER_BIT;
    } else {
        redist.sgi_ppi_pending &= !PTIMER_BIT;
    }
    if v_fire {
        redist.sgi_ppi_pending |= VTIMER_BIT;
    } else {
        redist.sgi_ppi_pending &= !VTIMER_BIT;
    }
    if h_fire {
        redist.sgi_ppi_pending |= HTIMER_BIT;
    } else {
        redist.sgi_ppi_pending &= !HTIMER_BIT;
    }

    let _ = redist;
    shared.update_irq_line(vcpu_idx);
}
// ── Device index bookkeeping ──────────────────────────────────────────────────

/// Device indices in HelmAddressSpace.devices.
pub struct ArmVirtDevices {
    /// GIC distributor device index.
    pub gicd_idx: usize,
    /// GIC CPU interface or first redistributor device index.
    pub gicc_idx: usize,
    /// PL011 UART device index.
    pub uart_idx: usize,
    /// Optional PL031 RTC device index when enabled.
    pub rtc_idx: Option<usize>,
    /// Optional SMMUv3 MMIO device index when the boxed board path installs it.
    pub smmu_idx: Option<usize>,
}

fn install_default_arm_virt_pci_bus(sys_mem: &mut HelmAddressSpace) {
    let _ = install_arm_virt_pci_bus(sys_mem, PciBus::new("pci0"));
}

fn install_live_arm_virt_smmuv3(sys_mem: &mut Box<HelmAddressSpace>, gic: &HelmGic) -> usize {
    let mut smmu = SmmuState::new(LiveFlatMemByteMem::new(&mut sys_mem.ram));
    match gic {
        HelmGic::V2(shared) => {
            use helm_devices::WireId;
            use helm_hw_intc::GicSink;
            smmu.gerror_irq.wire(
                WireId::from(ARM_VIRT_SMMU_GERROR_IRQ),
                Arc::new(GicSink::new(shared.clone(), ARM_VIRT_SMMU_GERROR_IRQ)),
            );
            smmu.evtq_irq.wire(
                WireId::from(ARM_VIRT_SMMU_EVTQ_IRQ),
                Arc::new(GicSink::new(shared.clone(), ARM_VIRT_SMMU_EVTQ_IRQ)),
            );
        }
        HelmGic::V3(shared) => {
            use helm_devices::WireId;
            use helm_hw_intc::GicV3Sink;
            smmu.gerror_irq.wire(
                WireId::from(ARM_VIRT_SMMU_GERROR_IRQ),
                Arc::new(GicV3Sink::new(shared.clone(), ARM_VIRT_SMMU_GERROR_IRQ)),
            );
            smmu.evtq_irq.wire(
                WireId::from(ARM_VIRT_SMMU_EVTQ_IRQ),
                Arc::new(GicV3Sink::new(shared.clone(), ARM_VIRT_SMMU_EVTQ_IRQ)),
            );
        }
    }
    sys_mem.add_device(SMMU_BASE, Box::new(smmu))
}

fn finalize_arm_virt_board(
    sys_mem: HelmAddressSpace,
    vcpus: Vec<HelmVcpu>,
    mut devs: ArmVirtDevices,
    quirks: QuirkSet,
    irq_lines: Vec<Arc<AtomicBool>>,
    gic: HelmGic,
    pci_msi: MessageInterruptEmitter,
) -> HelmBoard {
    let mut sys_mem = Box::new(sys_mem);
    devs.smmu_idx = Some(install_live_arm_virt_smmuv3(&mut sys_mem, &gic));
    HelmBoard {
        sys_mem,
        vcpus,
        next_vcpu: 0,
        devs,
        quirks,
        irq_lines,
        gic: Some(gic),
        pci_msi: Some(pci_msi),
    }
}

struct ArmVirtGicv2PciMsiSink {
    gic: Arc<Mutex<helm_hw_intc::GicSharedState>>,
    route: ArmVirtPciMsiRoute,
}

impl MessageInterruptSink for ArmVirtGicv2PciMsiSink {
    fn on_message(&self, message: MessageInterrupt) {
        let Some(intid) = self.route.translate(message.addr, message.data) else {
            return;
        };
        self.gic.lock().unwrap().pend_irq_edge(intid);
    }
}

struct ArmVirtGicv3PciMsiSink {
    gic: Arc<Mutex<helm_hw_intc::GicV3SharedState>>,
    route: ArmVirtPciMsiRoute,
}

impl MessageInterruptSink for ArmVirtGicv3PciMsiSink {
    fn on_message(&self, message: MessageInterrupt) {
        let Some(intid) = self.route.translate(message.addr, message.data) else {
            return;
        };
        self.gic.lock().unwrap().pend_spi_edge(intid);
    }
}

pub(crate) fn build_arm_virt_gicv2_pci_msi_emitter(
    gic: Arc<Mutex<helm_hw_intc::GicSharedState>>,
) -> MessageInterruptEmitter {
    let num_irqs = gic.lock().unwrap().dist.num_irqs;
    let route = ArmVirtPlatform.pci_msi_route(num_irqs);
    MessageInterruptEmitter::wired(Arc::new(ArmVirtGicv2PciMsiSink { gic, route }))
}

pub(crate) fn build_arm_virt_gicv3_pci_msi_emitter(
    gic: Arc<Mutex<helm_hw_intc::GicV3SharedState>>,
) -> MessageInterruptEmitter {
    let num_irqs = gic.lock().unwrap().dist.num_irqs;
    let route = ArmVirtPlatform.pci_msi_route(num_irqs);
    MessageInterruptEmitter::wired(Arc::new(ArmVirtGicv3PciMsiSink { gic, route }))
}

pub(crate) fn default_arm_virt_quirks() -> QuirkSet {
    ArmVirtPlatform.build_plan().default_quirks()
}

/// Install a PCI ECAM bus on the built-in arm-virt machine layout.
pub fn install_arm_virt_pci_bus(sys_mem: &mut HelmAddressSpace, bus: PciBus) -> usize {
    sys_mem.add_device(PCIE_ECAM_BASE, Box::new(bus))
}

/// Install a BAR-backed MMIO device on the built-in arm-virt machine layout.
///
/// The BAR window itself lives in the normal arm-virt MMIO attachment range.
/// This helper registers both the mapped device and the authoritative BAR
/// metadata needed for later remap projection.
pub fn install_arm_virt_pci_bar_device(
    sys_mem: &mut HelmAddressSpace,
    bdf: Bdf,
    bar_idx: u8,
    base: u64,
    priority: i32,
    device: Box<dyn Device>,
) -> Option<usize> {
    if !(MMIO_BASE..=MMIO_END).contains(&base) {
        return None;
    }
    let size = device.region_size();
    let idx = sys_mem.add_device(base, device);
    if sys_mem.register_pci_bar_region(
        bdf.bus,
        bdf.device,
        bdf.function,
        bar_idx,
        idx,
        base,
        size,
        priority,
    ) {
        Some(idx)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ArmVirtPciInstallError {
    #[error("built-in platform does not expose a live PCI bus")]
    NoLivePciBus,
    #[error("built-in PCI bus is not available")]
    PciBusUnavailable,
    #[error("failed to attach PCI function at {bus:02x}:{device:02x}.{function}: {reason}")]
    PciEndpointAttach {
        bus: u8,
        device: u8,
        function: u8,
        reason: &'static str,
    },
    #[error("{0}")]
    PciBuild(#[from] PciBuildError),
    #[error("{0}")]
    VirtioPciBuild(#[from] VirtioPciBuildError),
    #[error("failed to register {device} BAR{bar_idx} window at {base:#x}")]
    BarRegistration {
        device: &'static str,
        bar_idx: u8,
        base: u64,
    },
    #[error("unknown PciVirtioConsole serial backend '{backend}' (expected 'null' or 'stdio')")]
    InvalidConsoleBackend { backend: String },
}

fn register_arm_virt_pci_bar_device(
    sys_mem: &mut HelmAddressSpace,
    bdf: Bdf,
    bar_idx: u8,
    base: u64,
    device_name: &'static str,
    device: Box<dyn Device>,
) -> Result<(), ArmVirtPciInstallError> {
    install_arm_virt_pci_bar_device(sys_mem, bdf, bar_idx, base, 0, device).ok_or(
        ArmVirtPciInstallError::BarRegistration {
            device: device_name,
            bar_idx,
            base,
        },
    )
    .map(|_| ())
}

/// Install a synthetic PCI endpoint with one RAM-backed BAR0 MMIO region.
pub fn install_arm_virt_pci_ram_bar(
    sys_mem: &mut HelmAddressSpace,
    bus: u8,
    slot: u8,
    function: u8,
    vendor_id: u16,
    device_id: u16,
    class_code: u32,
    base: u64,
    size: u64,
) -> Result<(), ArmVirtPciInstallError> {
    let bdf = Bdf::new(bus, slot, function);
    let pci_idx = find_arm_virt_pci_bus_index(sys_mem).ok_or(ArmVirtPciInstallError::NoLivePciBus)?;
    let (endpoint, device) = build_pci_ram_bar_pair(vendor_id, device_id, class_code, base, size)?;
    attach_pci_endpoint(sys_mem, pci_idx, bdf, Box::new(endpoint))?;
    register_arm_virt_pci_bar_device(sys_mem, bdf, 0, base, "PCI BAR0 MMIO", Box::new(device))?;
    Ok(())
}

fn find_arm_virt_pci_bus_index(sys_mem: &mut HelmAddressSpace) -> Option<usize> {
    (0..sys_mem.devices.len()).find(|&idx| sys_mem.device_as_mut::<PciBus>(idx).is_some())
}

fn attach_pci_endpoint(
    sys_mem: &mut HelmAddressSpace,
    pci_idx: usize,
    bdf: Bdf,
    endpoint: Box<dyn helm_hw_pci::PciEndpoint>,
) -> Result<(), ArmVirtPciInstallError> {
    let attach = sys_mem
        .with_device_mut::<PciBus, _>(pci_idx, |bus| bus.attach_endpoint(bdf, endpoint))
        .ok_or(ArmVirtPciInstallError::PciBusUnavailable)?;
    attach.map_err(|reason| ArmVirtPciInstallError::PciEndpointAttach {
        bus: bdf.bus,
        device: bdf.device,
        function: bdf.function,
        reason,
    })
}

/// Install a legacy single-BAR PCI function exposing a VirtIO MMIO RNG transport.
pub fn install_arm_virt_pci_virtio_rng_mmio(
    sys_mem: &mut HelmAddressSpace,
    bus: u8,
    slot: u8,
    function: u8,
    vendor_id: u16,
    device_id: u16,
    class_code: u32,
    base: u64,
    seed: u64,
) -> Result<(), ArmVirtPciInstallError> {
    let bdf = Bdf::new(bus, slot, function);
    let pci_idx = find_arm_virt_pci_bus_index(sys_mem).ok_or(ArmVirtPciInstallError::NoLivePciBus)?;
    let endpoint = build_pci_bar0_endpoint(vendor_id, device_id, class_code, base, 0x200)?;
    attach_pci_endpoint(sys_mem, pci_idx, bdf, Box::new(endpoint))?;

    let transport = VirtioMmioTransport::new(Box::new(VirtioRng::with_seed(seed)));
    register_arm_virt_pci_bar_device(sys_mem, bdf, 0, base, "PciVirtioRngMmio", Box::new(transport))?;
    Ok(())
}

/// Install a standard PCI VirtIO RNG function with BAR0 common config and BAR4 MSI-X.
pub fn install_arm_virt_pci_virtio_rng(
    sys_mem: &mut HelmAddressSpace,
    bus: u8,
    slot: u8,
    function: u8,
    base: u64,
    seed: u64,
) -> Result<(), ArmVirtPciInstallError> {
    let bdf = Bdf::new(bus, slot, function);
    let pci_idx = find_arm_virt_pci_bus_index(sys_mem).ok_or(ArmVirtPciInstallError::NoLivePciBus)?;
    let (endpoint, bar0, bar4) = build_virtio_pci_rng_pair(base, seed)?;
    attach_pci_endpoint(sys_mem, pci_idx, bdf, Box::new(endpoint))?;

    register_arm_virt_pci_bar_device(sys_mem, bdf, 0, base, "PciVirtioRng", Box::new(bar0))?;
    let bar4_base = base + 0x1000;
    register_arm_virt_pci_bar_device(sys_mem, bdf, 4, bar4_base, "PciVirtioRng MSI-X", Box::new(bar4))?;
    Ok(())
}

/// Install a standard PCI VirtIO block function with BAR0 common config and BAR4 MSI-X.
pub fn install_arm_virt_pci_virtio_blk(
    sys_mem: &mut HelmAddressSpace,
    bus: u8,
    slot: u8,
    function: u8,
    base: u64,
    capacity_bytes: usize,
    read_only: bool,
) -> Result<(), ArmVirtPciInstallError> {
    let bdf = Bdf::new(bus, slot, function);
    let pci_idx = find_arm_virt_pci_bus_index(sys_mem).ok_or(ArmVirtPciInstallError::NoLivePciBus)?;
    let disk = RamBlockBackend::zeroed(capacity_bytes);
    let (endpoint, bar0, bar4) = helm_hw_virtio::pci::build_virtio_pci_pair(
        Box::new(VirtioBlk::new(Box::new(disk), read_only)),
        base,
    )?;
    attach_pci_endpoint(sys_mem, pci_idx, bdf, Box::new(endpoint))?;

    register_arm_virt_pci_bar_device(sys_mem, bdf, 0, base, "PciVirtioBlk", Box::new(bar0))?;
    let bar4_base = base + 0x1000;
    register_arm_virt_pci_bar_device(sys_mem, bdf, 4, bar4_base, "PciVirtioBlk MSI-X", Box::new(bar4))?;
    Ok(())
}

/// Install a standard PCI VirtIO network function with BAR0 common config and BAR4 MSI-X.
pub fn install_arm_virt_pci_virtio_net(
    sys_mem: &mut HelmAddressSpace,
    bus: u8,
    slot: u8,
    function: u8,
    base: u64,
    mac: [u8; 6],
) -> Result<(), ArmVirtPciInstallError> {
    let bdf = Bdf::new(bus, slot, function);
    let pci_idx = find_arm_virt_pci_bus_index(sys_mem).ok_or(ArmVirtPciInstallError::NoLivePciBus)?;
    let (endpoint, bar0, bar4) =
        helm_hw_virtio::pci::build_virtio_pci_pair(Box::new(VirtioNet::new(mac)), base)?;
    attach_pci_endpoint(sys_mem, pci_idx, bdf, Box::new(endpoint))?;

    register_arm_virt_pci_bar_device(sys_mem, bdf, 0, base, "PciVirtioNet", Box::new(bar0))?;
    let bar4_base = base + 0x1000;
    register_arm_virt_pci_bar_device(sys_mem, bdf, 4, bar4_base, "PciVirtioNet MSI-X", Box::new(bar4))?;
    Ok(())
}

fn make_arm_virt_console_backend(
    serial: &str,
) -> Result<Box<dyn CharBackend>, ArmVirtPciInstallError> {
    match serial {
        "null" => Ok(Box::new(helm_devices::NullCharBackend)),
        "stdio" => Ok(Box::new(StdioCharBackend)),
        other => Err(ArmVirtPciInstallError::InvalidConsoleBackend {
            backend: other.to_string(),
        }),
    }
}

/// Install a standard PCI VirtIO console function with BAR0 common config and BAR4 MSI-X.
pub fn install_arm_virt_pci_virtio_console(
    sys_mem: &mut HelmAddressSpace,
    bus: u8,
    slot: u8,
    function: u8,
    base: u64,
    serial: &str,
    cols: u16,
    rows: u16,
) -> Result<(), ArmVirtPciInstallError> {
    let bdf = Bdf::new(bus, slot, function);
    let pci_idx = find_arm_virt_pci_bus_index(sys_mem).ok_or(ArmVirtPciInstallError::NoLivePciBus)?;
    let backend = make_arm_virt_console_backend(serial)?;
    let console = VirtioConsole::with_size(backend, cols, rows);
    let (endpoint, bar0, bar4) = helm_hw_virtio::pci::build_virtio_pci_pair(Box::new(console), base)?;
    attach_pci_endpoint(sys_mem, pci_idx, bdf, Box::new(endpoint))?;

    register_arm_virt_pci_bar_device(sys_mem, bdf, 0, base, "PciVirtioConsole", Box::new(bar0))?;
    let bar4_base = base + 0x1000;
    register_arm_virt_pci_bar_device(
        sys_mem,
        bdf,
        4,
        bar4_base,
        "PciVirtioConsole MSI-X",
        Box::new(bar4),
    )?;
    Ok(())
}

fn build_arm_virt_with_cpus_and_quirks(
    mem_mib: usize,
    num_cpus: usize,
    uart_backend: Box<dyn CharBackend>,
    quirks: &QuirkSet,
) -> (
    HelmAddressSpace,
    ArmVirtDevices,
    Vec<Arc<AtomicBool>>,
    Arc<std::sync::Mutex<helm_hw_intc::GicSharedState>>,
) {
    let ram = FlatMem::new(RAM_BASE, mem_mib * 1024 * 1024);
    let mut sys_mem = HelmAddressSpace::new(ram);

    // GICv2: distributor + CPU interface share state; irq_line goes to the CPU,
    // gic_state allows the step loop to assert device/timer IRQs.
    let (gicd, _giccs, irq_lines, gic_state) = build_gicv2_mp(128, num_cpus);
    let gicc = Gicv2CpuInterface::from_banked_shared(Arc::clone(&gic_state));
    let gicd_idx = sys_mem.add_device(GICD_BASE, Box::new(gicd));
    let gicc_idx = sys_mem.add_device(GICC_BASE, Box::new(gicc));

    // PL011 UART — wire its interrupt using the platform build plan so the
    // fixed routing contract lives in helm-platform rather than here.
    let mut uart = Pl011::new(uart_backend);
    {
        use helm_devices::WireId;
        use helm_hw_intc::GicSink;
        let sink = std::sync::Arc::new(GicSink::new(Arc::clone(&gic_state), UART_IRQ));
        uart.irq_out.wire(WireId::from(UART_IRQ), sink);
    }
    let uart_idx = sys_mem.add_device(UART_BASE, Box::new(uart));

    let rtc_idx = if quirks.contains(QuirkKey::Platform(PlatformQuirk::ArmVirtPl031Rtc)) {
        let mut rtc = Pl031::new(0);
        {
            use helm_devices::WireId;
            use helm_hw_intc::GicSink;
            let sink = std::sync::Arc::new(GicSink::new(Arc::clone(&gic_state), RTC_IRQ));
            rtc.irq_out.wire(WireId::from(RTC_IRQ), sink);
        }
        Some(sys_mem.add_device(RTC_BASE, Box::new(rtc)))
    } else {
        None
    };

    install_default_arm_virt_pci_bus(&mut sys_mem);

    let devs = ArmVirtDevices {
        gicd_idx,
        gicc_idx,
        uart_idx,
        rtc_idx,
        smmu_idx: None,
    };
    (sys_mem, devs, irq_lines, gic_state)
}

fn build_arm_virt_gicv3_with_quirks(
    mem_mib: usize,
    num_cpus: usize,
    uart_backend: Box<dyn CharBackend>,
    quirks: &QuirkSet,
) -> (
    HelmAddressSpace,
    ArmVirtDevices,
    Vec<Arc<AtomicBool>>,
    Arc<std::sync::Mutex<helm_hw_intc::GicV3SharedState>>,
) {
    let ram = FlatMem::new(RAM_BASE, mem_mib * 1024 * 1024);
    let mut sys_mem = HelmAddressSpace::new(ram);

    // Build GICv3 with MPIDR-derived affinities: Aff0 = cpu_idx
    let affinities: Vec<u64> = (0..num_cpus).map(|i| i as u64).collect();
    let (gicd, gicrs, irq_lines, gicv3_state) =
        helm_hw_intc::build_gicv3_mp(256, num_cpus, &affinities);

    let gicd_idx = sys_mem.add_device(GICD_BASE, Box::new(gicd));
    // Map redistributors contiguously starting at GICR_BASE
    let mut first_gicr_idx = 0;
    for (i, gicr) in gicrs.into_iter().enumerate() {
        let idx = sys_mem.add_device(GICR_BASE + (i as u64) * GICR_STRIDE, Box::new(gicr));
        if i == 0 {
            first_gicr_idx = idx;
        }
    }

    // PL011 UART — wire interrupt to GICv3 via GicV3Sink
    let mut uart = Pl011::new(uart_backend);
    {
        use helm_devices::WireId;
        use helm_hw_intc::GicV3Sink;
        let sink = std::sync::Arc::new(GicV3Sink::new(Arc::clone(&gicv3_state), UART_IRQ));
        uart.irq_out.wire(WireId::from(UART_IRQ), sink);
    }
    let uart_idx = sys_mem.add_device(UART_BASE, Box::new(uart));

    let rtc_idx = if quirks.contains(QuirkKey::Platform(PlatformQuirk::ArmVirtPl031Rtc)) {
        let mut rtc = Pl031::new(0);
        {
            use helm_devices::WireId;
            use helm_hw_intc::GicV3Sink;
            let sink = std::sync::Arc::new(GicV3Sink::new(Arc::clone(&gicv3_state), RTC_IRQ));
            rtc.irq_out.wire(WireId::from(RTC_IRQ), sink);
        }
        Some(sys_mem.add_device(RTC_BASE, Box::new(rtc)))
    } else {
        None
    };

    install_default_arm_virt_pci_bus(&mut sys_mem);

    let devs = ArmVirtDevices {
        gicd_idx,
        gicc_idx: first_gicr_idx, // repurpose gicc_idx for first redistributor index
        uart_idx,
        rtc_idx,
        smmu_idx: None,
    };
    (sys_mem, devs, irq_lines, gicv3_state)
}

fn build_boot_vcpu(
    cpu_idx: usize,
    entry: u64,
    dtb_addr: u64,
    boot_el: u8,
    mem_mib: usize,
    gic_version: ArmVirtGicVersion,
    quirks: &QuirkSet,
) -> (Aarch64ArchState, FsState) {
    let mut cpu = Aarch64ArchState::new();
    let cpu_slot_top =
        RAM_BASE + (mem_mib as u64 * 1024 * 1024) - 0x1000 - (cpu_idx as u64 * 0x10000);
    let initial_sp = cpu_slot_top - 0x1000;
    let initial_tpidrro = cpu_slot_top;
    cpu.current_el = boot_el;
    cpu.spsel = true;
    cpu.pc = entry;
    cpu.x[0] = dtb_addr;
    cpu.x[1] = 0;
    cpu.x[2] = 0;
    cpu.x[3] = 0;
    cpu.tpidrro_el0 = initial_tpidrro;
    match boot_el {
        3 => {
            cpu.sp_el3 = initial_sp;
            cpu.sctlr_el3 = 0x0000_0800;
            cpu.id_aa64pfr0_el1 = (cpu.id_aa64pfr0_el1 & !0xFF00) | 0x1100;
        }
        2 => {
            cpu.sp_el2 = initial_sp;
            cpu.sctlr_el2 = 0x0000_0800;
            cpu.id_aa64pfr0_el1 = (cpu.id_aa64pfr0_el1 & !0xF00) | 0x100;
        }
        _ => {
            cpu.sp_el1 = initial_sp;
            cpu.sctlr_el1 = 0x0000_0800;
        }
    }
    if matches!(gic_version, ArmVirtGicVersion::V3) {
        cpu.id_aa64pfr0_el1 |= 1 << 24;
    }
    cpu.daif = 0xF;
    cpu.mpidr_el1 = 0x8000_0000 | cpu_idx as u64;
    cpu.psci_via_engine = quirks.contains(QuirkKey::Board(BoardQuirk::PsciViaEngine));
    (cpu, FsState::new())
}

fn build_idle_vcpu(cpu_idx: usize, gic_version: ArmVirtGicVersion) -> HelmVcpu {
    let mut cpu = Aarch64ArchState::new();
    cpu.current_el = 1;
    cpu.spsel = true;
    cpu.mpidr_el1 = 0x8000_0000 | cpu_idx as u64;
    if matches!(gic_version, ArmVirtGicVersion::V3) {
        cpu.id_aa64pfr0_el1 |= 1 << 24;
    }
    HelmVcpu {
        arch: cpu,
        fs: FsState::new(),
        powered_on: cpu_idx == 0,
    }
}

pub(crate) fn build_arm_virt_system(
    mem_mib: usize,
    num_cpus: usize,
    gic_version: ArmVirtGicVersion,
    uart_backend: Box<dyn CharBackend>,
) -> BuiltSystem {
    let quirks = default_arm_virt_quirks();
    let (sys_mem, devs, irq_lines, gic) = match gic_version {
        ArmVirtGicVersion::V2 => {
            let (sys_mem, devs, irq_lines, gic_state) =
                build_arm_virt_with_cpus(mem_mib, num_cpus, uart_backend);
            (sys_mem, devs, irq_lines, HelmGic::V2(gic_state))
        }
        ArmVirtGicVersion::V3 => {
            let (sys_mem, devs, irq_lines, gic_state) =
                build_arm_virt_gicv3(mem_mib, num_cpus, uart_backend);
            (sys_mem, devs, irq_lines, HelmGic::V3(gic_state))
        }
    };

    let pci_msi = match &gic {
        HelmGic::V2(shared) => build_arm_virt_gicv2_pci_msi_emitter(shared.clone()),
        HelmGic::V3(shared) => build_arm_virt_gicv3_pci_msi_emitter(shared.clone()),
    };

    BuiltSystem::Aarch64(BuiltAarch64System {
        board: finalize_arm_virt_board(
            sys_mem,
            (0..num_cpus.max(1))
                .map(|cpu_idx| build_idle_vcpu(cpu_idx, gic_version))
                .collect(),
            devs,
            quirks,
            irq_lines,
            gic,
            pci_msi,
        ),
    })
}

fn setup_arm_virt_boot_with_cpus_and_quirks(
    kernel_path: &str,
    dtb_path: &str,
    initrd_path: Option<&str>,
    append: Option<&str>,
    mem_mib: usize,
    num_cpus: usize,
    gic_version: ArmVirtGicVersion,
    boot_policy: ArmVirtBootPolicy,
    uart_backend: Box<dyn CharBackend>,
    quirks: QuirkSet,
) -> Result<
    (
        Vec<(Aarch64ArchState, FsState)>,
        HelmAddressSpace,
        ArmVirtDevices,
        Vec<Arc<AtomicBool>>,
        crate::session::HelmGic,
        QuirkSet,
    ),
    crate::loader::arm64_image::Arm64KernelLoadError,
> {
    let (mut sys_mem, devs, irq_lines, gic_state) = match gic_version {
        ArmVirtGicVersion::V2 => {
            let (sys_mem, devs, irq_lines, gic_state) =
                build_arm_virt_with_cpus_and_quirks(mem_mib, num_cpus, uart_backend, &quirks);
            (
                sys_mem,
                devs,
                irq_lines,
                crate::session::HelmGic::V2(gic_state),
            )
        }
        ArmVirtGicVersion::V3 => {
            let (sys_mem, devs, irq_lines, gic_state) =
                build_arm_virt_gicv3_with_quirks(mem_mib, num_cpus, uart_backend, &quirks);
            (
                sys_mem,
                devs,
                irq_lines,
                crate::session::HelmGic::V3(gic_state),
            )
        }
    };

    // Load kernel, DTB, initramfs into RAM; optionally override bootargs.
    let loaded = load_arm64_kernel(
        kernel_path,
        dtb_path,
        initrd_path,
        append,
        &mut sys_mem.ram,
        RAM_BASE,
    )?;

    let cpu_count = num_cpus.max(1);
    let mut boot_vcpus = Vec::with_capacity(cpu_count);
    let boot_el = boot_policy.resolve(loaded.boot_el);
    for cpu_idx in 0..cpu_count {
        boot_vcpus.push(build_boot_vcpu(
            cpu_idx,
            loaded.entry,
            loaded.dtb_addr,
            boot_el,
            mem_mib,
            gic_version,
            &quirks,
        ));
    }

    Ok((boot_vcpus, sys_mem, devs, irq_lines, gic_state, quirks))
}

fn build_default_arm_virt_dtb_bytes(
    append: Option<&str>,
    initrd_path: Option<&str>,
    mem_mib: usize,
    num_cpus: usize,
    gic_version: ArmVirtGicVersion,
    quirks: &QuirkSet,
) -> Result<Vec<u8>, crate::loader::arm64_image::Arm64KernelLoadError> {
    let initrd_size = match initrd_path {
        Some(path) => Some(
            std::fs::metadata(path)
                .map_err(|source| crate::loader::arm64_image::Arm64KernelLoadError::ReadInitrd {
                    path: path.to_string(),
                    source,
                })?
                .len(),
        ),
        None => None,
    };
    Ok(build_baseline_arm_virt_dtb(
        mem_mib,
        num_cpus,
        gic_version,
        append.unwrap_or(""),
        initrd_size,
        quirks.contains(QuirkKey::Platform(PlatformQuirk::ArmVirtPl031Rtc)),
    ))
}

fn setup_arm_virt_boot_with_cpus_dtb_bytes_and_quirks(
    kernel_path: &str,
    dtb_data: &[u8],
    initrd_path: Option<&str>,
    append: Option<&str>,
    mem_mib: usize,
    num_cpus: usize,
    gic_version: ArmVirtGicVersion,
    boot_policy: ArmVirtBootPolicy,
    uart_backend: Box<dyn CharBackend>,
    quirks: QuirkSet,
) -> Result<
    (
        Vec<(Aarch64ArchState, FsState)>,
        HelmAddressSpace,
        ArmVirtDevices,
        Vec<Arc<AtomicBool>>,
        crate::session::HelmGic,
        QuirkSet,
    ),
    crate::loader::arm64_image::Arm64KernelLoadError,
> {
    let (mut sys_mem, devs, irq_lines, gic_state) = match gic_version {
        ArmVirtGicVersion::V2 => {
            let (sys_mem, devs, irq_lines, gic_state) =
                build_arm_virt_with_cpus_and_quirks(mem_mib, num_cpus, uart_backend, &quirks);
            (
                sys_mem,
                devs,
                irq_lines,
                crate::session::HelmGic::V2(gic_state),
            )
        }
        ArmVirtGicVersion::V3 => {
            let (sys_mem, devs, irq_lines, gic_state) =
                build_arm_virt_gicv3_with_quirks(mem_mib, num_cpus, uart_backend, &quirks);
            (
                sys_mem,
                devs,
                irq_lines,
                crate::session::HelmGic::V3(gic_state),
            )
        }
    };

    let loaded = load_arm64_kernel_with_dtb_bytes(
        kernel_path,
        dtb_data,
        initrd_path,
        append,
        &mut sys_mem.ram,
        RAM_BASE,
    )?;

    let cpu_count = num_cpus.max(1);
    let mut boot_vcpus = Vec::with_capacity(cpu_count);
    let boot_el = boot_policy.resolve(loaded.boot_el);
    for cpu_idx in 0..cpu_count {
        boot_vcpus.push(build_boot_vcpu(
            cpu_idx,
            loaded.entry,
            loaded.dtb_addr,
            boot_el,
            mem_mib,
            gic_version,
            &quirks,
        ));
    }

    Ok((boot_vcpus, sys_mem, devs, irq_lines, gic_state, quirks))
}

// ── build_arm_virt ────────────────────────────────────────────────────────────

/// Build the ARM virt platform.
///
/// Returns `(sys_mem, device_indices, irq_lines, shared_gic_state)`.
pub fn build_arm_virt(
    mem_mib: usize,
    uart_backend: Box<dyn CharBackend>,
) -> (
    HelmAddressSpace,
    ArmVirtDevices,
    Vec<Arc<AtomicBool>>,
    Arc<std::sync::Mutex<helm_hw_intc::GicSharedState>>,
) {
    build_arm_virt_with_cpus(mem_mib, 1, uart_backend)
}

/// Multicore-ready arm-virt platform constructor.
pub fn build_arm_virt_with_cpus(
    mem_mib: usize,
    num_cpus: usize,
    uart_backend: Box<dyn CharBackend>,
) -> (
    HelmAddressSpace,
    ArmVirtDevices,
    Vec<Arc<AtomicBool>>,
    Arc<std::sync::Mutex<helm_hw_intc::GicSharedState>>,
) {
    let quirks = default_arm_virt_quirks();
    build_arm_virt_with_cpus_and_quirks(mem_mib, num_cpus, uart_backend, &quirks)
}

// ── GICv3 platform builder ────────────────────────────────────────────────────

/// Build the ARM virt platform with GICv3 (distributor + per-PE redistributors).
///
/// Returns `(sys_mem, device_indices, irq_lines, shared_gicv3_state)`.
pub fn build_arm_virt_gicv3(
    mem_mib: usize,
    num_cpus: usize,
    uart_backend: Box<dyn CharBackend>,
) -> (
    HelmAddressSpace,
    ArmVirtDevices,
    Vec<Arc<AtomicBool>>,
    Arc<std::sync::Mutex<helm_hw_intc::GicV3SharedState>>,
) {
    let quirks = default_arm_virt_quirks();
    build_arm_virt_gicv3_with_quirks(mem_mib, num_cpus, uart_backend, &quirks)
}

/// Multicore-ready boot setup. Scheduling still defaults to stepping vCPU0.
pub(crate) fn setup_arm_virt_boot_with_cpus(
    kernel_path: &str,
    dtb_path: &str,
    initrd_path: Option<&str>,
    append: Option<&str>,
    mem_mib: usize,
    num_cpus: usize,
    gic_version: ArmVirtGicVersion,
    boot_policy: ArmVirtBootPolicy,
    uart_backend: Box<dyn CharBackend>,
) -> Result<
    (
        Vec<(Aarch64ArchState, FsState)>,
        HelmAddressSpace,
        ArmVirtDevices,
        Vec<Arc<AtomicBool>>,
        crate::session::HelmGic,
        QuirkSet,
    ),
    crate::loader::arm64_image::Arm64KernelLoadError,
> {
    setup_arm_virt_boot_with_cpus_and_quirks(
        kernel_path,
        dtb_path,
        initrd_path,
        append,
        mem_mib,
        num_cpus,
        gic_version,
        boot_policy,
        uart_backend,
        default_arm_virt_quirks(),
    )
}

pub(crate) fn build_loaded_arm_virt_system(
    kernel_path: &str,
    dtb_path: &str,
    initrd_path: Option<&str>,
    append: Option<&str>,
    mem_mib: usize,
    num_cpus: usize,
    gic_version: ArmVirtGicVersion,
    boot_policy: ArmVirtBootPolicy,
    uart_backend: Box<dyn CharBackend>,
) -> Result<BuiltSystem, crate::loader::arm64_image::Arm64KernelLoadError> {
    let (boot_vcpus, sys_mem, devs, irq_lines, gic_state, quirks) = setup_arm_virt_boot_with_cpus(
        kernel_path,
        dtb_path,
        initrd_path,
        append,
        mem_mib,
        num_cpus,
        gic_version,
        boot_policy,
        uart_backend,
    )?;

    let pci_msi = match &gic_state {
        crate::session::HelmGic::V2(shared) => build_arm_virt_gicv2_pci_msi_emitter(shared.clone()),
        crate::session::HelmGic::V3(shared) => build_arm_virt_gicv3_pci_msi_emitter(shared.clone()),
    };

    Ok(BuiltSystem::Aarch64(BuiltAarch64System {
        board: finalize_arm_virt_board(
            sys_mem,
            boot_vcpus
                .into_iter()
                .enumerate()
                .map(|(idx, (arch, fs))| HelmVcpu {
                    arch,
                    fs,
                    powered_on: idx == 0,
                })
                .collect(),
            devs,
            quirks,
            irq_lines,
            gic_state,
            pci_msi,
        ),
    }))
}

pub(crate) fn build_loaded_arm_virt_system_auto_dtb(
    kernel_path: &str,
    initrd_path: Option<&str>,
    append: Option<&str>,
    mem_mib: usize,
    num_cpus: usize,
    gic_version: ArmVirtGicVersion,
    boot_policy: ArmVirtBootPolicy,
    uart_backend: Box<dyn CharBackend>,
) -> Result<BuiltSystem, crate::loader::arm64_image::Arm64KernelLoadError> {
    let quirks = default_arm_virt_quirks();
    let dtb = build_default_arm_virt_dtb_bytes(
        append,
        initrd_path,
        mem_mib,
        num_cpus,
        gic_version,
        &quirks,
    )?;
    let (boot_vcpus, sys_mem, devs, irq_lines, gic_state, quirks) =
        setup_arm_virt_boot_with_cpus_dtb_bytes_and_quirks(
            kernel_path,
            &dtb,
            initrd_path,
            append,
            mem_mib,
            num_cpus,
            gic_version,
            boot_policy,
            uart_backend,
            quirks,
        )?;

    let pci_msi = match &gic_state {
        crate::session::HelmGic::V2(shared) => build_arm_virt_gicv2_pci_msi_emitter(shared.clone()),
        crate::session::HelmGic::V3(shared) => build_arm_virt_gicv3_pci_msi_emitter(shared.clone()),
    };

    Ok(BuiltSystem::Aarch64(BuiltAarch64System {
        board: finalize_arm_virt_board(
            sys_mem,
            boot_vcpus
                .into_iter()
                .enumerate()
                .map(|(idx, (arch, fs))| HelmVcpu {
                    arch,
                    fs,
                    powered_on: idx == 0,
                })
                .collect(),
            devs,
            quirks,
            irq_lines,
            gic_state,
            pci_msi,
        ),
    }))
}

/// Multicore-ready boot setup using an in-memory DTB blob.
pub(crate) fn setup_arm_virt_boot_with_cpus_dtb_bytes(
    kernel_path: &str,
    dtb_data: &[u8],
    initrd_path: Option<&str>,
    append: Option<&str>,
    mem_mib: usize,
    num_cpus: usize,
    gic_version: ArmVirtGicVersion,
    boot_policy: ArmVirtBootPolicy,
    uart_backend: Box<dyn CharBackend>,
) -> Result<
    (
        Vec<(Aarch64ArchState, FsState)>,
        HelmAddressSpace,
        ArmVirtDevices,
        Vec<Arc<AtomicBool>>,
        crate::session::HelmGic,
        QuirkSet,
    ),
    crate::loader::arm64_image::Arm64KernelLoadError,
> {
    setup_arm_virt_boot_with_cpus_dtb_bytes_and_quirks(
        kernel_path,
        dtb_data,
        initrd_path,
        append,
        mem_mib,
        num_cpus,
        gic_version,
        boot_policy,
        uart_backend,
        default_arm_virt_quirks(),
    )
}

pub(crate) fn build_loaded_arm_virt_system_dtb_bytes(
    kernel_path: &str,
    dtb_data: &[u8],
    initrd_path: Option<&str>,
    append: Option<&str>,
    mem_mib: usize,
    num_cpus: usize,
    gic_version: ArmVirtGicVersion,
    boot_policy: ArmVirtBootPolicy,
    uart_backend: Box<dyn CharBackend>,
) -> Result<BuiltSystem, crate::loader::arm64_image::Arm64KernelLoadError> {
    let (boot_vcpus, sys_mem, devs, irq_lines, gic_state, quirks) =
        setup_arm_virt_boot_with_cpus_dtb_bytes(
            kernel_path,
            dtb_data,
            initrd_path,
            append,
            mem_mib,
            num_cpus,
            gic_version,
            boot_policy,
            uart_backend,
        )?;

    let pci_msi = match &gic_state {
        crate::session::HelmGic::V2(shared) => build_arm_virt_gicv2_pci_msi_emitter(shared.clone()),
        crate::session::HelmGic::V3(shared) => build_arm_virt_gicv3_pci_msi_emitter(shared.clone()),
    };

    Ok(BuiltSystem::Aarch64(BuiltAarch64System {
        board: finalize_arm_virt_board(
            sys_mem,
            boot_vcpus
                .into_iter()
                .enumerate()
                .map(|(idx, (arch, fs))| HelmVcpu {
                    arch,
                    fs,
                    powered_on: idx == 0,
                })
                .collect(),
            devs,
            quirks,
            irq_lines,
            gic_state,
            pci_msi,
        ),
    }))
}

// ── StdioCharBackend ──────────────────────────────────────────────────────────

/// Character backend that writes guest UART output to host stdout.
pub struct StdioCharBackend;

impl CharBackend for StdioCharBackend {
    fn write(&mut self, data: &[u8]) -> usize {
        use std::io::Write;
        let _ = std::io::stdout().write_all(data);
        let _ = std::io::stdout().flush();
        data.len()
    }
    fn read(&mut self) -> Option<u8> {
        None
    }
    fn can_write(&self) -> bool {
        true
    }
    fn can_read(&self) -> bool {
        false
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address_space::drain_pci_bus_remaps;
    use helm_core::{AccessType, MemInterface};
    use helm_devices::NullCharBackend;
    use helm_hw_pci::{config::PciConfigSpace, PciEndpoint};

    struct TestPciEndpoint {
        config: PciConfigSpace,
        vendor: u16,
        device: u16,
        class: u32,
    }

    impl TestPciEndpoint {
        fn new(vendor_id: u16, device_id: u16, class_code: u32) -> Self {
            Self {
                config: PciConfigSpace::new(vendor_id, device_id, class_code, 0x00),
                vendor: vendor_id,
                device: device_id,
                class: class_code,
            }
        }

        fn with_bar0(mut self, base: u32, size: u32) -> Self {
            self.config.set_bar_size(0, size);
            self.config.write(0x10, 4, base);
            self
        }
    }

    impl PciEndpoint for TestPciEndpoint {
        fn config_read(&self, offset: u16, size: usize) -> u32 {
            let off = offset as usize;
            match size {
                1 => self.config.data_ref().get(off).copied().unwrap_or(0) as u32,
                2 => self
                    .config
                    .data_ref()
                    .get(off..off + 2)
                    .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]) as u32)
                    .unwrap_or(0),
                4 => self
                    .config
                    .data_ref()
                    .get(off..off + 4)
                    .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                    .unwrap_or(0),
                _ => 0,
            }
        }

        fn config_write(&mut self, offset: u16, size: usize, val: u32) {
            self.config.write(offset, size, val);
        }

        fn vendor_id(&self) -> u16 {
            self.vendor
        }

        fn device_id(&self) -> u16 {
            self.device
        }

        fn class_code(&self) -> u32 {
            self.class
        }

        fn bar_base(&self, bar_index: u8) -> Option<u64> {
            self.config.bar_address(bar_index as usize)
        }

        fn bar_size(&self, bar_index: u8) -> Option<u64> {
            self.config.bar_size(bar_index as usize)
        }
    }

    struct MockBarDevice {
        last_write_offset: u64,
        last_write_val: u64,
    }

    impl MockBarDevice {
        fn new() -> Self {
            Self {
                last_write_offset: u64::MAX,
                last_write_val: 0,
            }
        }
    }

    impl Device for MockBarDevice {
        fn read(&mut self, _offset: u64, _size: usize) -> u64 {
            0
        }

        fn write(&mut self, offset: u64, _size: usize, val: u64) {
            self.last_write_offset = offset;
            self.last_write_val = val;
        }

        fn region_size(&self) -> u64 {
            0x1000
        }
    }

    #[test]
    fn build_arm_virt_creates_devices() {
        let (sys_mem, devs, _irqs, _gic) = build_arm_virt(256, Box::new(NullCharBackend));
        assert_eq!(devs.gicd_idx, 0);
        assert_eq!(devs.gicc_idx, 1);
        assert_eq!(devs.uart_idx, 2);
        assert_eq!(devs.rtc_idx, Some(3));
        assert_eq!(devs.smmu_idx, None);
        assert_eq!(sys_mem.devices.len(), 5);
        assert!(sys_mem.address_map.lookup(PCIE_ECAM_BASE).is_some());
    }

    #[test]
    fn built_arm_virt_system_installs_live_smmu_mmio_device() {
        use helm_core::{AccessType, MemInterface};

        let built = build_arm_virt_system(256, 1, ArmVirtGicVersion::V3, Box::new(NullCharBackend));
        let BuiltSystem::Aarch64(BuiltAarch64System { mut board }) = built;

        assert!(board.devs.smmu_idx.is_some());
        assert_eq!(
            board.sys_mem.read(SMMU_BASE, 4, AccessType::Load).unwrap(),
            0x0000_0001
        );
    }

    #[test]
    fn built_arm_virt_smmu_cmdq_uses_live_board_ram() {
        use helm_core::{AccessType, MemInterface};

        const CMDQ_BASE_ADDR: u64 = RAM_BASE + 0x2000;
        let built = build_arm_virt_system(256, 1, ArmVirtGicVersion::V3, Box::new(NullCharBackend));
        let BuiltSystem::Aarch64(BuiltAarch64System { mut board }) = built;

        board
            .sys_mem
            .ram
            .load_bytes(CMDQ_BASE_ADDR, &0x46u64.to_le_bytes());
        board
            .sys_mem
            .ram
            .load_bytes(CMDQ_BASE_ADDR + 8, &0u64.to_le_bytes());

        board
            .sys_mem
            .write(SMMU_BASE + 0x20, 4, 0x7, AccessType::Store)
            .unwrap();
        board
            .sys_mem
            .write(
                SMMU_BASE + 0x90,
                4,
                (CMDQ_BASE_ADDR & 0xFFFF_FFFF) | 2,
                AccessType::Store,
            )
            .unwrap();
        board
            .sys_mem
            .write(
                SMMU_BASE + 0x94,
                4,
                CMDQ_BASE_ADDR >> 32,
                AccessType::Store,
            )
            .unwrap();
        board
            .sys_mem
            .write(SMMU_BASE + 0x98, 4, 1, AccessType::Store)
            .unwrap();

        assert_eq!(
            board
                .sys_mem
                .read(SMMU_BASE + 0x9C, 4, AccessType::Load)
                .unwrap(),
            1
        );
    }

    #[test]
    fn uart_at_correct_address() {
        let (mut sys_mem, _devs, _irqs, _gic) = build_arm_virt(256, Box::new(NullCharBackend));
        use helm_core::{AccessType, MemInterface};
        // UARTFR at offset 0x18 — TXFE and RXFE should be set on reset
        let val = sys_mem.read(UART_BASE + 0x18, 4, AccessType::Load).unwrap();
        assert_ne!(val & 0x90, 0);
    }

    #[test]
    fn rtc_at_correct_address() {
        let (mut sys_mem, _devs, _irqs, _gic) = build_arm_virt(256, Box::new(NullCharBackend));
        use helm_core::{AccessType, MemInterface};
        let val = sys_mem.read(RTC_BASE + 0xFE0, 4, AccessType::Load).unwrap();
        assert_eq!(val, 0x31);
    }

    #[test]
    fn platform_quirk_can_disable_rtc_installation() {
        let mut quirks = default_arm_virt_quirks();
        quirks.disable(QuirkKey::Platform(PlatformQuirk::ArmVirtPl031Rtc));

        let (sys_mem, devs, _irqs, _gic) =
            build_arm_virt_with_cpus_and_quirks(256, 1, Box::new(NullCharBackend), &quirks);

        assert_eq!(devs.rtc_idx, None);
        assert_eq!(sys_mem.devices.len(), 4);
        assert!(sys_mem.address_map.lookup(PCIE_ECAM_BASE).is_some());
    }

    #[test]
    fn board_quirk_controls_psci_via_engine_boot_flag() {
        let mut quirks = default_arm_virt_quirks();
        quirks.disable(QuirkKey::Board(BoardQuirk::PsciViaEngine));

        let (cpu, _fs) = build_boot_vcpu(1, 0x1000, 0x2000, 1, 256, ArmVirtGicVersion::V2, &quirks);

        assert_eq!(cpu.pc, 0x1000);
        assert_eq!(cpu.x[0], 0x2000);
        assert_eq!(cpu.mpidr_el1, 0x8000_0001);
        assert!(!cpu.psci_via_engine);
    }

    #[test]
    fn build_boot_vcpu_can_start_at_el2() {
        let quirks = default_arm_virt_quirks();

        let (cpu, _fs) = build_boot_vcpu(0, 0x4100_0000, 0x4240_0000, 2, 512, ArmVirtGicVersion::V2, &quirks);

        assert_eq!(cpu.current_el, 2);
        assert_eq!(cpu.pc, 0x4100_0000);
        assert_eq!(cpu.x[0], 0x4240_0000);
        assert_ne!(cpu.sp_el2, 0);
        assert_ne!(cpu.tpidrro_el0, 0);
        assert_eq!(cpu.tpidrro_el0 - cpu.sp_el2, 0x1000);
        assert_eq!((cpu.id_aa64pfr0_el1 >> 8) & 0xF, 1);
    }

    #[test]
    fn build_boot_vcpu_can_start_at_el3() {
        let quirks = default_arm_virt_quirks();

        let (cpu, _fs) =
            build_boot_vcpu(0, 0x4300_0000, 0x4240_0000, 3, 512, ArmVirtGicVersion::V2, &quirks);

        assert_eq!(cpu.current_el, 3);
        assert_eq!(cpu.pc, 0x4300_0000);
        assert_eq!(cpu.x[0], 0x4240_0000);
        assert_ne!(cpu.sp_el3, 0);
        assert_eq!((cpu.id_aa64pfr0_el1 >> 8) & 0xF, 1);
        assert_eq!((cpu.id_aa64pfr0_el1 >> 12) & 0xF, 1);
    }

    #[test]
    fn boot_policy_override_accepts_supported_levels() {
        assert_eq!(
            arm_virt_boot_policy_from_override(None).unwrap(),
            ArmVirtBootPolicy::ImageDefault
        );
        assert_eq!(
            arm_virt_boot_policy_from_override(Some(1)).unwrap(),
            ArmVirtBootPolicy::El1
        );
        assert_eq!(
            arm_virt_boot_policy_from_override(Some(2)).unwrap(),
            ArmVirtBootPolicy::El2
        );
        assert_eq!(
            arm_virt_boot_policy_from_override(Some(3)).unwrap(),
            ArmVirtBootPolicy::El3
        );
    }

    #[test]
    fn boot_policy_override_rejects_unsupported_levels() {
        assert!(arm_virt_boot_policy_from_override(Some(0)).is_err());
        assert!(arm_virt_boot_policy_from_override(Some(4)).is_err());
    }

    #[test]
    fn built_arm_virt_can_host_pci_ecam_and_bar_remap() {
        const BAR0_BASE: u64 = MMIO_BASE;
        const NEW_BAR0_BASE: u64 = MMIO_BASE + 0x10000;

        let (mut sys_mem, _devs, _irqs, _gic) = build_arm_virt(256, Box::new(NullCharBackend));

        let endpoint =
            TestPciEndpoint::new(0x1AF4, 0x1001, 0x010000).with_bar0(BAR0_BASE as u32, 0x1000);
        let pci_idx = sys_mem
            .devices
            .iter()
            .position(|dev| (&**dev as &dyn std::any::Any).is::<PciBus>())
            .expect("default arm-virt PCI bus should be installed");
        sys_mem
            .with_device_mut::<PciBus, _>(pci_idx, |bus| {
                bus.attach_endpoint(Bdf::new(0, 1, 0), Box::new(endpoint))
                    .unwrap();
            })
            .expect("default PCI bus should be mutable");

        let bar_idx = install_arm_virt_pci_bar_device(
            &mut sys_mem,
            Bdf::new(0, 1, 0),
            0,
            BAR0_BASE,
            0,
            Box::new(MockBarDevice::new()),
        )
        .expect("bar device should install");

        let ecam_bar0 = PCIE_ECAM_BASE + (1u64 << 15) + 0x10;
        sys_mem
            .write(ecam_bar0, 4, NEW_BAR0_BASE, AccessType::Store)
            .unwrap();

        let result = drain_pci_bus_remaps(&mut sys_mem, pci_idx);
        assert_eq!(result.drained, 1);
        assert_eq!(result.applied, 1);
        assert!(sys_mem.address_map.lookup(BAR0_BASE).is_none());
        assert!(sys_mem.address_map.lookup(NEW_BAR0_BASE).is_some());

        sys_mem
            .write(NEW_BAR0_BASE + 0x20, 4, 0x5A, AccessType::Store)
            .unwrap();
        let bar_dev = sys_mem.device_as_mut::<MockBarDevice>(bar_idx).unwrap();
        assert_eq!(bar_dev.last_write_offset, 0x20);
        assert_eq!(bar_dev.last_write_val, 0x5A);
    }

    #[test]
    fn arm_virt_helper_installs_pci_ram_bar() {
        let (mut sys_mem, _devs, _irqs, _gic) = build_arm_virt(256, Box::new(NullCharBackend));

        install_arm_virt_pci_ram_bar(
            &mut sys_mem,
            0,
            1,
            0,
            0xCAFE,
            0x0001,
            0xFF0000,
            MMIO_BASE,
            0x1000,
        )
        .expect("pci ram bar helper should install");

        let vendor_device = sys_mem
            .read(PCIE_ECAM_BASE + (1u64 << 15), 4, AccessType::Load)
            .unwrap() as u32;
        assert_eq!(vendor_device, 0x0001_CAFE);
        assert!(sys_mem.address_map.lookup(MMIO_BASE).is_some());
    }

    #[test]
    fn arm_virt_pci_ram_bar_requires_live_pci_bus() {
        let mut sys_mem = HelmAddressSpace::new(FlatMem::new(RAM_BASE, 0x1000));

        let err = install_arm_virt_pci_ram_bar(
            &mut sys_mem,
            0,
            1,
            0,
            0xCAFE,
            0x0001,
            0xFF0000,
            MMIO_BASE,
            0x1000,
        )
        .unwrap_err();

        assert_eq!(err, ArmVirtPciInstallError::NoLivePciBus);
    }

    #[test]
    fn arm_virt_helper_installs_standard_pci_virtio_rng() {
        let (mut sys_mem, _devs, _irqs, _gic) = build_arm_virt(256, Box::new(NullCharBackend));

        install_arm_virt_pci_virtio_rng(
            &mut sys_mem,
            0,
            3,
            0,
            MMIO_BASE + 0x3000,
            0x1234_5678,
        )
        .expect("virtio rng helper should install");

        let vendor_device = sys_mem
            .read(PCIE_ECAM_BASE + (3u64 << 15), 4, AccessType::Load)
            .unwrap() as u32;
        assert_eq!(vendor_device, 0x1044_1AF4);
        assert!(sys_mem.address_map.lookup(MMIO_BASE + 0x3000).is_some());
        assert!(sys_mem.address_map.lookup(MMIO_BASE + 0x4000).is_some());
    }

    #[test]
    fn arm_virt_helper_installs_mmio_rng_transport() {
        let (mut sys_mem, _devs, _irqs, _gic) = build_arm_virt(256, Box::new(NullCharBackend));

        install_arm_virt_pci_virtio_rng_mmio(
            &mut sys_mem,
            0,
            2,
            0,
            0xCAFE,
            0x1004,
            0xFF0000,
            MMIO_BASE + 0x1000,
            0x1234_5678,
        )
        .expect("virtio rng mmio helper should install");

        let vendor_device = sys_mem
            .read(PCIE_ECAM_BASE + (2u64 << 15), 4, AccessType::Load)
            .unwrap() as u32;
        assert_eq!(vendor_device, 0x1004_CAFE);
        assert!(sys_mem.address_map.lookup(MMIO_BASE + 0x1000).is_some());
    }

    #[test]
    fn arm_virt_helper_installs_standard_pci_virtio_blk() {
        let (mut sys_mem, _devs, _irqs, _gic) = build_arm_virt(256, Box::new(NullCharBackend));

        install_arm_virt_pci_virtio_blk(
            &mut sys_mem,
            0,
            4,
            0,
            MMIO_BASE + 0x5000,
            4096,
            false,
        )
        .expect("virtio blk helper should install");

        let vendor_device = sys_mem
            .read(PCIE_ECAM_BASE + (4u64 << 15), 4, AccessType::Load)
            .unwrap() as u32;
        assert_eq!(vendor_device, 0x1042_1AF4);
        assert!(sys_mem.address_map.lookup(MMIO_BASE + 0x5000).is_some());
        assert!(sys_mem.address_map.lookup(MMIO_BASE + 0x6000).is_some());
    }

    #[test]
    fn arm_virt_helper_installs_standard_pci_virtio_net() {
        let (mut sys_mem, _devs, _irqs, _gic) = build_arm_virt(256, Box::new(NullCharBackend));

        install_arm_virt_pci_virtio_net(
            &mut sys_mem,
            0,
            5,
            0,
            MMIO_BASE + 0x8000,
            [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
        )
        .expect("virtio net helper should install");

        let vendor_device = sys_mem
            .read(PCIE_ECAM_BASE + (5u64 << 15), 4, AccessType::Load)
            .unwrap() as u32;
        assert_eq!(vendor_device, 0x1041_1AF4);
        assert!(sys_mem.address_map.lookup(MMIO_BASE + 0x8000).is_some());
        assert!(sys_mem.address_map.lookup(MMIO_BASE + 0x9000).is_some());
    }

    #[test]
    fn arm_virt_console_helper_rejects_unknown_backend() {
        let (mut sys_mem, _devs, _irqs, _gic) = build_arm_virt(256, Box::new(NullCharBackend));

        let err = install_arm_virt_pci_virtio_console(
            &mut sys_mem,
            0,
            6,
            0,
            MMIO_BASE + 0xB000,
            "bogus",
            132,
            50,
        )
        .unwrap_err();

        assert_eq!(
            err,
            ArmVirtPciInstallError::InvalidConsoleBackend {
                backend: "bogus".to_string()
            }
        );
    }

    #[test]
    fn arm_virt_helper_installs_standard_pci_virtio_console() {
        let (mut sys_mem, _devs, _irqs, _gic) = build_arm_virt(256, Box::new(NullCharBackend));

        install_arm_virt_pci_virtio_console(
            &mut sys_mem,
            0,
            6,
            0,
            MMIO_BASE + 0xB000,
            "null",
            132,
            50,
        )
        .expect("virtio console helper should install");

        let vendor_device = sys_mem
            .read(PCIE_ECAM_BASE + (6u64 << 15), 4, AccessType::Load)
            .unwrap() as u32;
        assert_eq!(vendor_device, 0x1043_1AF4);
        assert!(sys_mem.address_map.lookup(MMIO_BASE + 0xB000).is_some());
        assert!(sys_mem.address_map.lookup(MMIO_BASE + 0xC000).is_some());
    }

    #[test]
    fn auto_dtb_boot_path_builds_valid_fdt_for_loaded_kernel() {
        let tmp_path = std::env::temp_dir().join(format!(
            "helm-ng-arm-virt-auto-dtb-{}-{}.bin",
            std::process::id(),
            std::thread::current().name().unwrap_or("unnamed")
        ));

        let mut elf = vec![0u8; 0x2000];
        elf[0..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[5] = 1;
        elf[6] = 1;
        elf[16..18].copy_from_slice(&2u16.to_le_bytes());
        elf[18..20].copy_from_slice(&183u16.to_le_bytes());
        elf[20..24].copy_from_slice(&1u32.to_le_bytes());
        elf[24..32].copy_from_slice(&0x4100_0000u64.to_le_bytes());
        elf[32..40].copy_from_slice(&64u64.to_le_bytes());
        elf[52..54].copy_from_slice(&64u16.to_le_bytes());
        elf[54..56].copy_from_slice(&56u16.to_le_bytes());
        elf[56..58].copy_from_slice(&1u16.to_le_bytes());
        let ph = 64usize;
        elf[ph..ph + 4].copy_from_slice(&1u32.to_le_bytes());
        elf[ph + 4..ph + 8].copy_from_slice(&5u32.to_le_bytes());
        elf[ph + 8..ph + 16].copy_from_slice(&0x1000u64.to_le_bytes());
        elf[ph + 16..ph + 24].copy_from_slice(&0x4100_0000u64.to_le_bytes());
        elf[ph + 24..ph + 32].copy_from_slice(&0x4100_0000u64.to_le_bytes());
        elf[ph + 32..ph + 40].copy_from_slice(&4u64.to_le_bytes());
        elf[ph + 40..ph + 48].copy_from_slice(&0x1000u64.to_le_bytes());
        elf[ph + 48..ph + 56].copy_from_slice(&0x1000u64.to_le_bytes());
        elf[0x1000..0x1004].copy_from_slice(&0xD503_201Fu32.to_le_bytes());
        std::fs::write(&tmp_path, &elf).unwrap();

        let built = build_loaded_arm_virt_system_auto_dtb(
            tmp_path.to_str().unwrap(),
            None,
            Some("console=ttyAMA0"),
            512,
            2,
            ArmVirtGicVersion::V3,
            ArmVirtBootPolicy::El1,
            Box::new(NullCharBackend),
        )
        .unwrap();

        let BuiltSystem::Aarch64(BuiltAarch64System { mut board }) = built;
        assert_eq!(board.vcpus.len(), 2);
        assert_eq!(board.vcpus[0].arch.x[0], board.vcpus[1].arch.x[0]);
        let dtb_addr = board.vcpus[0].arch.x[0];
        assert_ne!(dtb_addr, 0);
        assert_eq!(
            board
                .sys_mem
                .read(dtb_addr, 4, AccessType::Load)
                .expect("DTB header should be readable"),
            u64::from(u32::from_be_bytes(0xD00D_FEEDu32.to_ne_bytes()))
        );

        let _ = std::fs::remove_file(tmp_path);
    }

    #[test]
    fn uart_irq_out_is_wired_to_gic() {
        // Confirm that the PL011's irq_out is connected; a TX write must NOT
        // produce the "unconnected pin" warning. Instead it should call the
        // GicSink, which asserts/deasserts INTID 33 on the shared GicState.
        use helm_core::{AccessType, MemInterface};
        let (mut sys_mem, devs, irqs, _gic) = build_arm_virt(256, Box::new(NullCharBackend));
        // Enable GICD + GICC + unmask UART SPI 1 (INTID 33)
        sys_mem.write(GICD_BASE, 4, 1, AccessType::Store).unwrap();
        sys_mem
            .write(GICD_BASE + 0x104, 4, 0x2, AccessType::Store)
            .unwrap(); // ISENABLER1 bit1=INTID33
        sys_mem.write(GICC_BASE, 4, 1, AccessType::Store).unwrap();
        sys_mem
            .write(GICC_BASE + 0x4, 4, 0xFF, AccessType::Store)
            .unwrap(); // PMR=all
                       // Enable UART TX interrupt in IMSC
        sys_mem
            .write(UART_BASE + 0x3C, 4, 1 << 5, AccessType::Store)
            .unwrap(); // UARTIMSC (0x03C) TX bit
                       // Write a byte to UART DR — triggers ris|=INT_TX -> update_irq -> assert
        sys_mem
            .write(UART_BASE, 4, b'A' as u64, AccessType::Store)
            .unwrap();
        // GIC irq_line should now be raised
        use std::sync::atomic::Ordering;
        assert!(
            irqs[0].load(Ordering::Relaxed),
            "UART TX interrupt must propagate through GicSink -> GIC -> irq_line"
        );
        let _ = devs.uart_idx;
    }

    #[test]
    fn gic_irq_line_not_raised_on_reset() {
        use std::sync::atomic::Ordering;
        let (_sys_mem, _devs, irqs, _gic) = build_arm_virt(256, Box::new(NullCharBackend));
        assert!(
            !irqs[0].load(Ordering::Relaxed),
            "IRQ line should be low on reset"
        );
    }

    #[test]
    fn gic_irq_line_raised_when_enabled_and_pending() {
        use helm_core::{AccessType, MemInterface};
        use std::sync::atomic::Ordering;
        let (mut sys_mem, devs, irqs, _gic) = build_arm_virt(256, Box::new(NullCharBackend));

        // Enable GICD (GICD_CTLR = 1)
        sys_mem.write(GICD_BASE, 4, 1, AccessType::Store).unwrap();
        // Enable IRQ 32 (GICD_ISENABLER1 bit 0)
        sys_mem
            .write(GICD_BASE + 0x104, 4, 0x1, AccessType::Store)
            .unwrap();
        // Set pending IRQ 32 (GICD_ISPENDR1 bit 0)
        sys_mem
            .write(GICD_BASE + 0x204, 4, 0x1, AccessType::Store)
            .unwrap();
        // Enable GICC (GICC_CTLR = 1)
        sys_mem.write(GICC_BASE, 4, 1, AccessType::Store).unwrap();
        // Set GICC PMR = 0xFF (allow all)
        sys_mem
            .write(GICC_BASE + 0x4, 4, 0xFF, AccessType::Store)
            .unwrap();

        assert!(irqs[0].load(Ordering::Relaxed), "IRQ line should be raised");

        // ACK via GICC_IAR
        let iar = sys_mem.read(GICC_BASE + 0xC, 4, AccessType::Load).unwrap();
        assert_eq!(iar, 32, "IAR should return IRQ 32");
        assert!(
            !irqs[0].load(Ordering::Relaxed),
            "IRQ line should drop after ACK"
        );

        // EOI
        sys_mem
            .write(GICC_BASE + 0x10, 4, 32, AccessType::Store)
            .unwrap();
        let _ = devs.gicc_idx; // suppress unused warning
    }
}

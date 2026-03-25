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
use std::sync::Arc;

use helm_arch::Aarch64ArchState;
use helm_devices::CharBackend;
use helm_hw_char::Pl011;
use helm_hw_intc::build_gicv2_mp;
use helm_hw_intc::Gicv2CpuInterface;
use helm_platform::aarch64::virt::{ArmVirtPlatform, GICD_BASE, RAM_BASE};
#[cfg(test)]
use helm_platform::aarch64::virt::{GICC_BASE, UART_BASE};
use helm_platform::Platform;

use crate::fs;
use crate::fs::FsState;
use crate::loader::{load_arm64_kernel, load_arm64_kernel_with_dtb_bytes};
use crate::address_space::HelmAddressSpace;
use crate::FlatMem;
use helm_core::MemInterface;

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
pub fn inject_timers(
    a64: &mut helm_arch::Aarch64ArchState,
    fs: &mut FsState,
    sys_mem: &mut HelmAddressSpace,
) {
    const PTIMER_BIT: u64 = 1 << 30; // INTID 30 = PPI 14 (non-secure phys timer)
    const VTIMER_BIT: u64 = 1 << 27; // INTID 27 = PPI 11 (virtual timer)
    const GICD_ISPENDR0: u64 = GICD_BASE + 0x200; // set-pending register[0]
    const GICD_ICPENDR0: u64 = GICD_BASE + 0x280; // clear-pending register[0]

    let (p_fire, v_fire) = fs::check_timers(a64, fs);

    let ispendr = (if p_fire { PTIMER_BIT } else { 0 }) | (if v_fire { VTIMER_BIT } else { 0 });
    let icpendr = (if p_fire { 0 } else { PTIMER_BIT }) | (if v_fire { 0 } else { VTIMER_BIT });

    if ispendr != 0 {
        let _ = sys_mem.write(GICD_ISPENDR0, 4, ispendr, helm_core::AccessType::Store);
    }
    if icpendr != 0 {
        let _ = sys_mem.write(GICD_ICPENDR0, 4, icpendr, helm_core::AccessType::Store);
    }
}
// ── Device index bookkeeping ──────────────────────────────────────────────────

/// Device indices in HelmAddressSpace.devices.
pub struct ArmVirtDevices {
    pub gicd_idx: usize,
    pub gicc_idx: usize,
    pub uart_idx: usize,
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
    let plan = ArmVirtPlatform.build_plan();
    let ram_base = plan
        .region_named("ram")
        .expect("arm-virt plan missing RAM region")
        .base;
    let gicd_base = plan
        .region_named("gic-dist")
        .expect("arm-virt plan missing GIC distributor region")
        .base;
    let gicc_base = plan
        .region_named("gic-cpu")
        .expect("arm-virt plan missing GIC CPU interface region")
        .base;
    let uart_region = plan
        .region_named("uart0")
        .expect("arm-virt plan missing UART region");
    let uart_irq = plan
        .route_from_source("uart0")
        .expect("arm-virt plan missing UART interrupt route")
        .line;

    let ram = FlatMem::new(ram_base, mem_mib * 1024 * 1024);
    let mut sys_mem = HelmAddressSpace::new(ram);

    // GICv2: distributor + CPU interface share state; irq_line goes to the CPU,
    // gic_state allows the step loop to assert device/timer IRQs.
    let (gicd, _giccs, irq_lines, gic_state) = build_gicv2_mp(128, num_cpus);
    let gicc = Gicv2CpuInterface::from_banked_shared(Arc::clone(&gic_state));
    let gicd_idx = sys_mem.add_device(gicd_base, Box::new(gicd));
    let gicc_idx = sys_mem.add_device(gicc_base, Box::new(gicc));

    // PL011 UART — wire its interrupt using the platform build plan so the
    // fixed routing contract lives in helm-platform rather than here.
    let mut uart = Pl011::new(uart_backend);
    {
        use helm_devices::WireId;
        use helm_hw_intc::GicSink;
        let sink = std::sync::Arc::new(GicSink::new(Arc::clone(&gic_state), uart_irq));
        uart.irq_out.wire(WireId::from(uart_irq), sink);
    }
    let uart_idx = sys_mem.add_device(uart_region.base, Box::new(uart));

    let devs = ArmVirtDevices {
        gicd_idx,
        gicc_idx,
        uart_idx,
    };
    (sys_mem, devs, irq_lines, gic_state)
}

// ── setup_arm_virt_boot ───────────────────────────────────────────────────────

/// Load a kernel and set up AArch64 state for FS boot on the ARM virt platform.
///
/// Returns `(boot_vcpus, sys_mem, devices, irq_lines, shared_gic_state)`.
pub fn setup_arm_virt_boot(
    kernel_path: &str,
    dtb_path: &str,
    initrd_path: Option<&str>,
    append: Option<&str>,
    mem_mib: usize,
    uart_backend: Box<dyn CharBackend>,
) -> Result<
    (
        Vec<(Aarch64ArchState, FsState)>,
        HelmAddressSpace,
        ArmVirtDevices,
        Vec<Arc<AtomicBool>>,
        Arc<std::sync::Mutex<helm_hw_intc::GicSharedState>>,
    ),
    String,
> {
    setup_arm_virt_boot_with_cpus(
        kernel_path,
        dtb_path,
        initrd_path,
        append,
        mem_mib,
        1,
        uart_backend,
    )
}

/// Multicore-ready boot setup. Scheduling still defaults to stepping vCPU0.
pub fn setup_arm_virt_boot_with_cpus(
    kernel_path: &str,
    dtb_path: &str,
    initrd_path: Option<&str>,
    append: Option<&str>,
    mem_mib: usize,
    num_cpus: usize,
    uart_backend: Box<dyn CharBackend>,
) -> Result<
    (
        Vec<(Aarch64ArchState, FsState)>,
        HelmAddressSpace,
        ArmVirtDevices,
        Vec<Arc<AtomicBool>>,
        Arc<std::sync::Mutex<helm_hw_intc::GicSharedState>>,
    ),
    String,
> {
    let (mut sys_mem, devs, irq_lines, gic_state) =
        build_arm_virt_with_cpus(mem_mib, num_cpus, uart_backend);

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
    for cpu_idx in 0..cpu_count {
        let mut cpu = Aarch64ArchState::new();
        cpu.current_el = 1;
        cpu.spsel = true;
        cpu.pc = loaded.entry;
        cpu.x[0] = loaded.dtb_addr;
        cpu.x[1] = 0;
        cpu.x[2] = 0;
        cpu.x[3] = 0;
        cpu.sp_el1 =
            RAM_BASE + (mem_mib as u64 * 1024 * 1024) - 0x1000 - (cpu_idx as u64 * 0x10000);
        cpu.sctlr_el1 = 0x0000_0800;
        cpu.daif = 0xF;
        cpu.mpidr_el1 = 0x8000_0000 | cpu_idx as u64;
        cpu.psci_via_engine = true;
        boot_vcpus.push((cpu, FsState::new()));
    }

    Ok((boot_vcpus, sys_mem, devs, irq_lines, gic_state))
}

/// Multicore-ready boot setup using an in-memory DTB blob.
pub fn setup_arm_virt_boot_with_cpus_dtb_bytes(
    kernel_path: &str,
    dtb_data: &[u8],
    initrd_path: Option<&str>,
    append: Option<&str>,
    mem_mib: usize,
    num_cpus: usize,
    uart_backend: Box<dyn CharBackend>,
) -> Result<
    (
        Vec<(Aarch64ArchState, FsState)>,
        HelmAddressSpace,
        ArmVirtDevices,
        Vec<Arc<AtomicBool>>,
        Arc<std::sync::Mutex<helm_hw_intc::GicSharedState>>,
    ),
    String,
> {
    let (mut sys_mem, devs, irq_lines, gic_state) =
        build_arm_virt_with_cpus(mem_mib, num_cpus, uart_backend);

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
    for cpu_idx in 0..cpu_count {
        let mut cpu = Aarch64ArchState::new();
        cpu.current_el = 1;
        cpu.spsel = true;
        cpu.pc = loaded.entry;
        cpu.x[0] = loaded.dtb_addr;
        cpu.x[1] = 0;
        cpu.x[2] = 0;
        cpu.x[3] = 0;
        cpu.sp_el1 =
            RAM_BASE + (mem_mib as u64 * 1024 * 1024) - 0x1000 - (cpu_idx as u64 * 0x10000);
        cpu.sctlr_el1 = 0x0000_0800;
        cpu.daif = 0xF;
        cpu.mpidr_el1 = 0x8000_0000 | cpu_idx as u64;
        cpu.psci_via_engine = true;
        boot_vcpus.push((cpu, FsState::new()));
    }

    Ok((boot_vcpus, sys_mem, devs, irq_lines, gic_state))
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
    use helm_devices::NullCharBackend;

    #[test]
    fn build_arm_virt_creates_devices() {
        let (sys_mem, devs, _irqs, _gic) = build_arm_virt(256, Box::new(NullCharBackend));
        assert_eq!(devs.gicd_idx, 0);
        assert_eq!(devs.gicc_idx, 1);
        assert_eq!(devs.uart_idx, 2);
        assert_eq!(sys_mem.devices.len(), 3);
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

//! Platform builder — wires buses, devices, and IRQ routes into a
//! complete simulated machine.
//!
//! Inspired by gem5's `fs.py` and QEMU's machine types. A platform
//! describes the entire device topology:
//!
//! ```text
//! Platform("arm-virt")
//!   ├── system_bus (AHB, 0 latency)
//!   │   ├── memory @ 0x4000_0000 (2 GB RAM)
//!   │   ├── apb_bridge @ 0x0900_0000 (APB, 1 cycle bridge)
//!   │   │   ├── pl011 @ 0x0000 (UART0)
//!   │   │   └── pl011 @ 0x1000 (UART1)
//!   │   └── virtio_mmio @ 0x0A00_0000 (virtio-blk)
//!   └── irq_router
//!       ├── uart0:0 → gic:33
//!       └── uart1:0 → gic:34
//! ```
//!
//! Platforms can be built programmatically in Rust or configured from
//! Python via helm-python bindings.

use crate::bus::DeviceBus;
use crate::device::Device;
use crate::irq::{IrqRoute, IrqRouter};
use helm_core::types::Addr;

use crate::fdt::{DtbConfig, RuntimeDtb};

/// A complete platform description — buses, devices, and IRQ wiring.
pub struct Platform {
    /// Human-readable platform name (e.g. "arm-virt").
    pub name: String,
    /// Top-level system bus.
    pub system_bus: DeviceBus,
    /// IRQ routing table.
    pub irq_router: IrqRouter,
    /// Named device references for easy access.
    device_map: Vec<(String, Addr)>,
    /// Whether this platform uses a device tree (ARM=true, x86 ACPI=false).
    pub uses_dtb: bool,
}

impl Platform {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            system_bus: DeviceBus::system(),
            irq_router: IrqRouter::new(),
            device_map: Vec::new(),
            uses_dtb: true,
        }
    }

    /// Attach a device to the system bus at the given address.
    pub fn add_device(&mut self, name: impl Into<String>, base: Addr, device: Box<dyn Device>) {
        let n = name.into();
        self.device_map.push((n.clone(), base));
        self.system_bus.attach_device(&n, base, device);
    }

    /// Attach a sub-bus (APB, AHB, PCI) to the system bus.
    pub fn add_bus(&mut self, name: impl Into<String>, base: Addr, bus: Box<dyn Device>) {
        let n = name.into();
        self.device_map.push((n.clone(), base));
        self.system_bus.attach_device(&n, base, bus);
    }

    /// Add an IRQ route.
    pub fn add_irq_route(&mut self, route: IrqRoute) {
        self.irq_router.add_route(route);
    }

    /// List all registered devices with their base addresses.
    pub fn device_map(&self) -> &[(String, Addr)] {
        &self.device_map
    }

    /// Reset the entire platform.
    pub fn reset(&mut self) -> helm_core::HelmResult<()> {
        self.system_bus.reset_all()
    }

    /// Tick all devices on the platform.
    pub fn tick(&mut self, cycles: u64) -> helm_core::HelmResult<Vec<crate::device::DeviceEvent>> {
        self.system_bus.tick_all(cycles)
    }

    /// Create a [`RuntimeDtb`] from this platform, generating a fresh
    /// skeleton DTB from `config`.
    pub fn create_dtb(&self, config: &DtbConfig) -> RuntimeDtb {
        RuntimeDtb::new(self, config, None)
    }

    /// Create a [`RuntimeDtb`] by parsing an existing DTB blob and
    /// overlaying this platform's devices plus CLI extras from `config`.
    pub fn patch_dtb(&self, base_blob: &[u8], config: &DtbConfig) -> RuntimeDtb {
        RuntimeDtb::new(self, config, Some(base_blob))
    }
}

// ── Pre-built platform configurations ───────────────────────────────────────

/// Build an ARM "virt" platform similar to QEMU's virt machine.
///
/// Memory map (subset):
/// - `0x0900_0000`: PL011 UART0 (on APB)
/// - `0x0900_1000`: PL011 UART1 (on APB)
/// - `0x0A00_0000`: VirtIO MMIO slot 0
/// - `0x0A00_0200`: VirtIO MMIO slot 1
/// - `0x4000_0000`: DRAM base
///
/// This function creates the platform with UARTs pre-wired. VirtIO
/// devices and other peripherals can be added afterwards.
pub fn arm_virt_platform(
    uart0_backend: Box<dyn crate::backend::CharBackend>,
    uart1_backend: Box<dyn crate::backend::CharBackend>,
    irq_signal: Option<helm_core::IrqSignal>,
) -> Platform {
    use crate::arm::gic::Gic;
    use crate::arm::pl011::Pl011;
    use crate::proto::amba::ApbBus;

    let mut platform = Platform::new("arm-virt");

    // GIC at 0x0800_0000 (dist) + 0x0801_0000 (CPU interface)
    // The Gic device model has dist at offset 0 and CPU iface at offset 0x1000.
    // We place it at 0x0800_0000 so MMIO accesses at 0x0801_0000 hit offset 0x1000.
    let mut gic = Gic::new("gic", 256);
    if let Some(sig) = irq_signal {
        gic.set_irq_signal(sig);
    }
    platform.add_device("gic", 0x0800_0000, Box::new(gic));

    // APB bus for peripherals at 0x0900_0000
    let mut apb = ApbBus::new("apb", 0x10_0000); // 1 MB window
    apb.attach(0x0000, 0x1000, Box::new(Pl011::new("uart0", uart0_backend)));
    apb.attach(0x1000, 0x1000, Box::new(Pl011::new("uart1", uart1_backend)));

    platform.add_bus("apb", 0x0900_0000, Box::new(apb));

    platform
}

/// Build an ARM RealView Platform Baseboard for Cortex-A8.
///
/// Memory map per ARM DUI0417D:
/// - `0x1000_0000`: System registers
/// - `0x1000_1000`: SP804 dual timer
/// - `0x1000_6000`: PL031 RTC
/// - `0x1000_9000`–`0x1000_C000`: PL011 UART0–UART3
/// - `0x1000_F000`: SP805 watchdog
/// - `0x1001_3000`–`0x1001_5000`: PL061 GPIO0–GPIO2
/// - `0x1F00_0000`: GIC (dist + CPU interface)
pub fn realview_pb_platform(
    uart0_backend: Box<dyn crate::backend::CharBackend>,
) -> Platform {
    use crate::arm::gic::Gic;
    use crate::arm::pl011::Pl011;
    use crate::arm::pl031::Pl031;
    use crate::arm::pl061::Pl061;
    use crate::arm::sp804::Sp804;
    use crate::arm::sp805::Sp805;
    use crate::arm::sysregs::RealViewSysRegs;

    let mut platform = Platform::new("realview-pb-a8");

    // System registers
    platform.add_device("sysregs", 0x1000_0000,
        Box::new(RealViewSysRegs::realview_pb_a8()));

    // Timers
    platform.add_device("timer01", 0x1000_1000, Box::new(Sp804::new("timer01")));

    // RTC
    platform.add_device("rtc", 0x1000_6000, Box::new(Pl031::new("rtc")));

    // UARTs
    platform.add_device("uart0", 0x1000_9000,
        Box::new(Pl011::new("uart0", uart0_backend)));
    platform.add_device("uart1", 0x1000_A000,
        Box::new(Pl011::new("uart1", Box::new(crate::backend::NullCharBackend))));
    platform.add_device("uart2", 0x1000_B000,
        Box::new(Pl011::new("uart2", Box::new(crate::backend::NullCharBackend))));
    platform.add_device("uart3", 0x1000_C000,
        Box::new(Pl011::new("uart3", Box::new(crate::backend::NullCharBackend))));

    // Watchdog
    platform.add_device("watchdog", 0x1000_F000, Box::new(Sp805::new("watchdog")));

    // GPIO
    platform.add_device("gpio0", 0x1001_3000, Box::new(Pl061::new("gpio0")));
    platform.add_device("gpio1", 0x1001_4000, Box::new(Pl061::new("gpio1")));
    platform.add_device("gpio2", 0x1001_5000, Box::new(Pl061::new("gpio2")));

    // GIC (distributor at base, CPU interface at base + 0x1000)
    platform.add_device("gic", 0x1F00_0000, Box::new(Gic::new("gic", 96)));

    platform
}

/// Build a Raspberry Pi 3 (BCM2837) platform.
///
/// Memory map per BCM2835 ARM Peripherals:
/// - `0x3F00_3000`: System timer
/// - `0x3F00_B880`: Mailbox
/// - `0x3F20_0000`: GPIO
/// - `0x3F20_1000`: PL011 UART0
/// - `0x3F21_5000`: Mini UART (UART1)
pub fn rpi3_platform(
    uart0_backend: Box<dyn crate::backend::CharBackend>,
    uart1_backend: Box<dyn crate::backend::CharBackend>,
) -> Platform {
    use crate::arm::bcm_gpio::BcmGpio;
    use crate::arm::bcm_mailbox::BcmMailbox;
    use crate::arm::bcm_mini_uart::BcmMiniUart;
    use crate::arm::bcm_sys_timer::BcmSysTimer;
    use crate::arm::pl011::Pl011;

    let mut platform = Platform::new("rpi3");

    // System timer
    platform.add_device("sys-timer", 0x3F00_3000,
        Box::new(BcmSysTimer::new("sys-timer")));

    // Mailbox
    platform.add_device("mailbox", 0x3F00_B880,
        Box::new(BcmMailbox::rpi3()));

    // GPIO
    platform.add_device("gpio", 0x3F20_0000,
        Box::new(BcmGpio::new("gpio")));

    // PL011 UART0 (full UART)
    platform.add_device("uart0", 0x3F20_1000,
        Box::new(Pl011::new("uart0", uart0_backend)));

    // Mini UART (UART1)
    platform.add_device("uart1", 0x3F21_5000,
        Box::new(BcmMiniUart::new("uart1", uart1_backend)));

    platform
}

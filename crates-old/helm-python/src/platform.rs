//! PyO3 wrappers for Platform, device creation, and IRQ wiring.
//!
//! These types let Python scripts build complete device topologies
//! in a gem5-like style, then pass them to `FsSession.from_platform()`.

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use helm_core::IrqSignal;
use helm_device::backend::{NullCharBackend, StdioCharBackend};
use helm_device::device::Device;
use helm_device::platform::Platform;
use helm_device::proto::amba::ApbBus;

// ---------------------------------------------------------------------------
// IrqSignal wrapper
// ---------------------------------------------------------------------------

/// Shared IRQ signal for GIC → CPU communication.
///
/// Create one `IrqSignal`, pass it to the GIC device via `create_device("gic", irq_signal=sig)`,
/// and pass the same signal to `FsSession.from_platform()`.
#[pyclass(name = "IrqSignal")]
#[derive(Clone)]
pub struct PyIrqSignal {
    pub(crate) inner: IrqSignal,
}

#[pymethods]
impl PyIrqSignal {
    #[new]
    fn new() -> Self {
        Self {
            inner: IrqSignal::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// DeviceHandle — wraps a Box<dyn Device> or a special bus type
// ---------------------------------------------------------------------------

/// Opaque handle to a device created by `create_device()`.
///
/// Devices are consumed (moved) when added to a Platform or an APB bus.
/// After adding, the handle is empty and cannot be reused.
#[pyclass(name = "DeviceHandle", unsendable)]
pub struct PyDeviceHandle {
    inner: Option<DeviceInner>,
}

enum DeviceInner {
    /// Regular device (Gic, Pl011, Sp804, VirtIO, etc.)
    Boxed(Box<dyn Device>),
    /// APB bus — special because it supports `attach_child()`.
    Apb(ApbBus),
    /// PCI function — must be attached to a PCI host bridge via `attach()`.
    PciFunc(Box<dyn helm_device::pci::PciFunction>),
    /// PCI host bridge — placed directly on the platform via `add_device()`.
    PciHost(helm_device::pci::PciHostBridge),
}

impl PyDeviceHandle {
    fn take_device(&mut self) -> PyResult<Box<dyn Device>> {
        match self.inner.take() {
            Some(DeviceInner::Boxed(d)) => Ok(d),
            Some(DeviceInner::Apb(apb)) => Ok(Box::new(apb)),
            Some(DeviceInner::PciHost(host)) => Ok(Box::new(host)),
            Some(DeviceInner::PciFunc(_)) => Err(PyRuntimeError::new_err(
                "PCI function must be attached to a PCI host bridge, not directly to a platform",
            )),
            None => Err(PyRuntimeError::new_err(
                "device already consumed (moved to Platform or bus)",
            )),
        }
    }
}

#[pymethods]
impl PyDeviceHandle {
    /// Attach a child device to an APB bus at the given offset.
    ///
    /// Only valid for APB bus handles created with `create_device("apb-bus", ...)`.
    #[pyo3(signature = (offset, size, child))]
    fn attach_child(&mut self, offset: u64, size: u64, child: &mut PyDeviceHandle) -> PyResult<()> {
        let child_dev = child.take_device()?;
        match &mut self.inner {
            Some(DeviceInner::Apb(apb)) => {
                apb.attach(offset, size, child_dev);
                Ok(())
            }
            _ => Err(PyRuntimeError::new_err(
                "attach_child() is only valid for APB bus devices",
            )),
        }
    }

    /// Attach a PCI function to a PCI host bridge at the given device slot.
    ///
    /// Only valid for PCI host bridge handles created with `create_device("pci-host", ...)`.
    /// The `device` argument must be a PCI function handle (e.g. from
    /// `create_device("virtio-blk", transport="pci")`).
    #[pyo3(signature = (slot, device))]
    fn attach(&mut self, slot: u8, device: &mut PyDeviceHandle) -> PyResult<()> {
        match &mut self.inner {
            Some(DeviceInner::PciHost(host)) => match device.inner.take() {
                Some(DeviceInner::PciFunc(func)) => {
                    host.attach(slot, 0, func);
                    Ok(())
                }
                Some(other) => {
                    device.inner = Some(other);
                    Err(PyRuntimeError::new_err(
                        "only PCI functions can be attached to a PCI host bridge",
                    ))
                }
                None => Err(PyRuntimeError::new_err("device already consumed")),
            },
            _ => Err(PyRuntimeError::new_err(
                "attach() is only valid for PCI host bridge devices",
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Platform wrapper
// ---------------------------------------------------------------------------

/// A complete device topology — buses, devices, and IRQ wiring.
///
/// Build one from Python, then pass it to `FsSession.from_platform()`.
///
/// ```python
/// platform = _helm_core.Platform("arm-virt")
/// gic = _helm_core.create_device("gic", max_irqs=256)
/// platform.add_device("gic", 0x0800_0000, gic)
/// ```
#[pyclass(name = "Platform", unsendable)]
pub struct PyPlatform {
    pub(crate) inner: Option<Platform>,
}

#[pymethods]
impl PyPlatform {
    #[new]
    fn new(name: &str) -> Self {
        Self {
            inner: Some(Platform::new(name)),
        }
    }

    /// Add a device to the platform at the given base address.
    ///
    /// The device handle is consumed (moved) and cannot be reused.
    fn add_device(
        &mut self,
        name: &str,
        base_addr: u64,
        device: &mut PyDeviceHandle,
    ) -> PyResult<()> {
        let dev = device.take_device()?;
        let platform = self
            .inner
            .as_mut()
            .ok_or_else(|| PyRuntimeError::new_err("platform already consumed"))?;
        platform.add_device(name, base_addr, dev);
        Ok(())
    }

    /// Return a list of (name, base_addr) for all registered devices.
    fn device_list(&self) -> PyResult<Vec<(String, u64)>> {
        let platform = self
            .inner
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("platform already consumed"))?;
        Ok(platform
            .device_map()
            .iter()
            .map(|(n, a)| (n.clone(), *a))
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Device factory
// ---------------------------------------------------------------------------

/// Create a device by type name.
///
/// Supported types:
/// - `"gic"` — GICv2 interrupt controller.
///   kwargs: `max_irqs` (int, default 256), `irq_signal` (IrqSignal, optional)
/// - `"pl011"` — PL011 UART.
///   kwargs: `name` (str), `serial` (str: "stdio" or "null", default "null")
/// - `"apb-bus"` — APB peripheral bus.
///   kwargs: `name` (str), `window` (int, byte size of address window)
/// - `"sp804"` — SP804 dual timer. kwargs: `name` (str)
/// - `"pl031"` — PL031 RTC. kwargs: `name` (str)
/// - `"pl061"` — PL061 GPIO. kwargs: `name` (str)
/// - `"sp805"` — SP805 watchdog. kwargs: `name` (str)
/// - `"bcm-sys-timer"` — BCM2837 system timer. kwargs: `name` (str)
/// - `"bcm-mailbox"` — BCM2837 mailbox. kwargs: `name` (str)
/// - `"bcm-gpio"` — BCM2837 GPIO. kwargs: `name` (str)
/// - `"bcm-mini-uart"` — BCM2837 mini UART.
///   kwargs: `name` (str), `serial` (str: "stdio" or "null", default "null")
/// - `"virtio-blk"` — VirtIO block device.
///   kwargs: `capacity` (int, bytes, default 0)
/// - `"virtio-net"` — VirtIO network device.
/// - `"virtio-rng"` — VirtIO entropy device.
/// - `"virtio-console"` — VirtIO console.
#[pyfunction]
#[pyo3(signature = (device_type, **kwargs))]
pub fn create_device(
    device_type: &str,
    kwargs: Option<&Bound<'_, PyDict>>,
) -> PyResult<PyDeviceHandle> {
    let get_str = |key: &str, default: &str| -> String {
        kwargs
            .and_then(|d| d.get_item(key).ok().flatten())
            .and_then(|v| v.extract::<String>().ok())
            .unwrap_or_else(|| default.to_string())
    };
    let get_u64 = |key: &str, default: u64| -> u64 {
        kwargs
            .and_then(|d| d.get_item(key).ok().flatten())
            .and_then(|v| v.extract::<u64>().ok())
            .unwrap_or(default)
    };
    let get_u32 = |key: &str, default: u32| -> u32 {
        kwargs
            .and_then(|d| d.get_item(key).ok().flatten())
            .and_then(|v| v.extract::<u32>().ok())
            .unwrap_or(default)
    };

    fn make_char_backend(serial: &str) -> Box<dyn helm_device::backend::CharBackend> {
        match serial {
            "stdio" => Box::new(StdioCharBackend),
            _ => Box::new(NullCharBackend),
        }
    }

    let inner = match device_type {
        "gic" => {
            use helm_device::arm::gic::Gic;
            let max_irqs = get_u32("max_irqs", 256);
            let name = get_str("name", "gic");
            let mut gic = Gic::new(&name, max_irqs);
            // Wire IRQ signal if provided
            if let Some(d) = kwargs {
                if let Ok(Some(sig_any)) = d.get_item("irq_signal") {
                    if let Ok(sig) = sig_any.extract::<PyIrqSignal>() {
                        gic.set_irq_signal(sig.inner);
                    }
                }
            }
            DeviceInner::Boxed(Box::new(gic))
        }

        "pl011" => {
            use helm_device::arm::pl011::Pl011;
            let name = get_str("name", "uart");
            let serial = get_str("serial", "null");
            DeviceInner::Boxed(Box::new(Pl011::new(&name, make_char_backend(&serial))))
        }

        "apb-bus" => {
            let name = get_str("name", "apb");
            let window = get_u64("window", 0x10_0000);
            DeviceInner::Apb(ApbBus::new(&name, window))
        }

        "sp804" => {
            use helm_device::arm::sp804::Sp804;
            let name = get_str("name", "timer");
            DeviceInner::Boxed(Box::new(Sp804::new(&name)))
        }

        "pl031" => {
            use helm_device::arm::pl031::Pl031;
            let name = get_str("name", "rtc");
            DeviceInner::Boxed(Box::new(Pl031::new(&name)))
        }

        "pl061" => {
            use helm_device::arm::pl061::Pl061;
            let name = get_str("name", "gpio");
            DeviceInner::Boxed(Box::new(Pl061::new(&name)))
        }

        "sp805" => {
            use helm_device::arm::sp805::Sp805;
            let name = get_str("name", "watchdog");
            DeviceInner::Boxed(Box::new(Sp805::new(&name)))
        }

        "bcm-sys-timer" => {
            use helm_device::arm::bcm_sys_timer::BcmSysTimer;
            let name = get_str("name", "sys-timer");
            DeviceInner::Boxed(Box::new(BcmSysTimer::new(&name)))
        }

        "bcm-mailbox" => {
            use helm_device::arm::bcm_mailbox::BcmMailbox;
            DeviceInner::Boxed(Box::new(BcmMailbox::rpi3()))
        }

        "bcm-gpio" => {
            use helm_device::arm::bcm_gpio::BcmGpio;
            let name = get_str("name", "gpio");
            DeviceInner::Boxed(Box::new(BcmGpio::new(&name)))
        }

        "bcm-mini-uart" => {
            use helm_device::arm::bcm_mini_uart::BcmMiniUart;
            let name = get_str("name", "mini-uart");
            let serial = get_str("serial", "null");
            DeviceInner::Boxed(Box::new(BcmMiniUart::new(
                &name,
                make_char_backend(&serial),
            )))
        }

        "virtio-blk" => {
            let transport = get_str("transport", "mmio");
            let capacity = get_u64("capacity", 0);
            if transport == "pci" {
                use helm_device::pci::VirtioPciTransport;
                use helm_device::virtio::blk::VirtioBlk;
                DeviceInner::PciFunc(Box::new(VirtioPciTransport::new(Box::new(VirtioBlk::new(
                    capacity,
                )))))
            } else {
                use helm_device::virtio::blk::VirtioBlk;
                use helm_device::virtio::transport::VirtioMmioTransport;
                DeviceInner::Boxed(Box::new(VirtioMmioTransport::new(Box::new(
                    VirtioBlk::new(capacity),
                ))))
            }
        }

        "virtio-net" => {
            let transport = get_str("transport", "mmio");
            if transport == "pci" {
                use helm_device::pci::VirtioPciTransport;
                use helm_device::virtio::net::VirtioNet;
                DeviceInner::PciFunc(Box::new(VirtioPciTransport::new(
                    Box::new(VirtioNet::new()),
                )))
            } else {
                use helm_device::virtio::net::VirtioNet;
                use helm_device::virtio::transport::VirtioMmioTransport;
                DeviceInner::Boxed(Box::new(VirtioMmioTransport::new(Box::new(
                    VirtioNet::new(),
                ))))
            }
        }

        "virtio-rng" => {
            let transport = get_str("transport", "mmio");
            if transport == "pci" {
                use helm_device::pci::VirtioPciTransport;
                use helm_device::virtio::rng::VirtioRng;
                DeviceInner::PciFunc(Box::new(VirtioPciTransport::new(
                    Box::new(VirtioRng::new()),
                )))
            } else {
                use helm_device::virtio::rng::VirtioRng;
                use helm_device::virtio::transport::VirtioMmioTransport;
                DeviceInner::Boxed(Box::new(VirtioMmioTransport::new(Box::new(
                    VirtioRng::new(),
                ))))
            }
        }

        "virtio-console" => {
            let transport = get_str("transport", "mmio");
            if transport == "pci" {
                use helm_device::pci::VirtioPciTransport;
                use helm_device::virtio::console::VirtioConsole;
                DeviceInner::PciFunc(Box::new(VirtioPciTransport::new(Box::new(
                    VirtioConsole::new(),
                ))))
            } else {
                use helm_device::virtio::console::VirtioConsole;
                use helm_device::virtio::transport::VirtioMmioTransport;
                DeviceInner::Boxed(Box::new(VirtioMmioTransport::new(Box::new(
                    VirtioConsole::new(),
                ))))
            }
        }

        "pci-host" => {
            let ecam_base = get_u64("ecam_base", 0x3F00_0000);
            let ecam_size = get_u64("ecam_size", 0x0100_0000);
            let mmio_base = get_u64("mmio_base", 0x1000_0000);
            let mmio_size = get_u64("mmio_size", 0x2EFF_0000);
            DeviceInner::PciHost(helm_device::pci::PciHostBridge::new(
                ecam_base, ecam_size, mmio_base, mmio_size,
            ))
        }

        "accel-pci" => {
            let ir_file = get_str("ir_file", "");
            let ir_string = get_str("ir", "");
            if !ir_file.is_empty() {
                DeviceInner::PciFunc(Box::new(helm_llvm::AcceleratorPciFunction::from_file(
                    &ir_file,
                )))
            } else if !ir_string.is_empty() {
                DeviceInner::PciFunc(Box::new(helm_llvm::AcceleratorPciFunction::from_string(
                    &ir_string,
                )))
            } else {
                return Err(PyRuntimeError::new_err(
                    "accel-pci requires either ir_file= or ir= argument",
                ));
            }
        }

        other => {
            return Err(PyRuntimeError::new_err(format!(
                "unknown device type: {other}"
            )));
        }
    };

    Ok(PyDeviceHandle { inner: Some(inner) })
}

#![allow(missing_docs)]

use crate::port::PortRef;
use crate::simobject::SimObject;
use crate::system::HelmSystem;
use pyo3::prelude::*;

/// GICv2 interrupt controller.
///
/// Before `instantiate()`: configuration descriptor with `num_irqs`.
/// After `instantiate()`: provides read-only state inspection of live GIC
/// state via back-reference to the parent [`HelmSystem`].
///
/// # Introspection examples (after instantiate)
///
/// ```python
/// print(system.gic.pending_mask(cpu=0, reg=1))  # pending IRQs 32-63
/// print(system.gic.enabled_mask(reg=1))          # enabled IRQs 32-63
/// print(system.gic.active_mask(reg=2))           # active  IRQs 64-95
/// ```
#[pyclass(name = "GicV2", extends = SimObject)]
pub struct GicV2 {
    #[pyo3(get, set)]
    pub num_irqs: u32,
    /// Back-reference to the parent system (set during instantiate).
    pub(crate) system_ref: Option<Py<HelmSystem>>,
}

#[pymethods]
impl GicV2 {
    #[new]
    #[pyo3(signature = (name, *, num_irqs = 96))]
    fn new(name: &str, num_irqs: u32) -> (Self, SimObject) {
        (
            GicV2 {
                num_irqs,
                system_ref: None,
            },
            SimObject::new(name),
        )
    }

    /// Return a PortRef for SPI interrupt number `n`.
    fn spi(&self, py: Python<'_>, n: u32) -> PyResult<PortRef> {
        // The GicV2 doesn't know its own SimObject name at construction time
        // (it's set by the parent via __setattr__). Use empty string — resolved
        // at instantiate() from the child hierarchy.
        let _ = py;
        Ok(PortRef {
            target_name: String::new(),
            port_name: "spi".to_string(),
            port_index: Some(n),
        })
    }

    // ── Live GIC state introspection (read-only) ────────────────────────

    /// Return the pending interrupt mask for a 32-IRQ bank.
    ///
    /// `cpu` selects the CPU index (relevant for banked IRQs 0-31 when `reg=0`).
    /// `reg` selects the register bank: 0 = IRQs 0-31, 1 = IRQs 32-63, etc.
    ///
    /// Returns `None` if not instantiated or no GICv2 is present.
    #[pyo3(signature = (cpu=0, reg=1))]
    fn pending_mask(&self, py: Python<'_>, cpu: usize, reg: usize) -> PyResult<Option<u32>> {
        let sys = self.require_system(py)?;
        let system = sys.borrow(py);
        Ok(system.sim.as_ref().and_then(|s| s.gic_pending_mask(cpu, reg)))
    }

    /// Return the enabled interrupt mask for a 32-IRQ bank.
    ///
    /// `cpu` selects the CPU index (relevant for banked IRQs 0-31 when `reg=0`).
    /// `reg` selects the register bank: 0 = IRQs 0-31, 1 = IRQs 32-63, etc.
    ///
    /// Returns `None` if not instantiated or no GICv2 is present.
    #[pyo3(signature = (cpu=0, reg=1))]
    fn enabled_mask(&self, py: Python<'_>, cpu: usize, reg: usize) -> PyResult<Option<u32>> {
        let sys = self.require_system(py)?;
        let system = sys.borrow(py);
        Ok(system.sim.as_ref().and_then(|s| s.gic_enabled_mask(cpu, reg)))
    }

    /// Return the active interrupt mask for a 32-IRQ bank.
    ///
    /// `cpu` selects the CPU index (relevant for banked IRQs 0-31 when `reg=0`).
    /// `reg` selects the register bank: 0 = IRQs 0-31, 1 = IRQs 32-63, etc.
    ///
    /// Returns `None` if not instantiated or no GICv2 is present.
    #[pyo3(signature = (cpu=0, reg=1))]
    fn active_mask(&self, py: Python<'_>, cpu: usize, reg: usize) -> PyResult<Option<u32>> {
        let sys = self.require_system(py)?;
        let system = sys.borrow(py);
        Ok(system.sim.as_ref().and_then(|s| s.gic_active_mask(cpu, reg)))
    }

    fn __traverse__(&self, visit: pyo3::PyVisit<'_>) -> Result<(), pyo3::PyTraverseError> {
        if let Some(ref sys) = self.system_ref {
            visit.call(sys)?;
        }
        Ok(())
    }

    fn __clear__(&mut self) {
        self.system_ref = None;
    }
}

impl GicV2 {
    fn require_system(&self, _py: Python<'_>) -> PyResult<&Py<HelmSystem>> {
        self.system_ref.as_ref().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "GIC state inspection requires an instantiated system \
                 (call system.instantiate() first)",
            )
        })
    }
}

/// PL011 UART device.
///
/// Before `instantiate()`: configuration descriptor.
/// After `instantiate()`: provides read-only state inspection of the live
/// UART state.
///
/// # Introspection examples (after instantiate)
///
/// ```python
/// print(system.uart.tx_count)    # total bytes transmitted
/// print(system.uart.rx_count)    # total bytes received
/// print(system.uart.is_tx_full)  # always False in simulation
/// print(system.uart.is_rx_empty) # True when RX FIFO is empty
/// ```
#[pyclass(name = "Pl011", extends = SimObject)]
pub struct Pl011 {
    pub(crate) irq: Option<PortRef>,
    /// Back-reference to the parent system (set during instantiate).
    pub(crate) system_ref: Option<Py<HelmSystem>>,
}

#[pymethods]
impl Pl011 {
    #[new]
    fn new(name: &str) -> (Self, SimObject) {
        (
            Pl011 {
                irq: None,
                system_ref: None,
            },
            SimObject::new(name),
        )
    }

    /// Get the IRQ port reference.
    #[getter]
    fn irq(&self) -> Option<PortRef> {
        self.irq.clone()
    }

    /// Set the IRQ port reference.
    #[setter]
    fn set_irq(&mut self, value: Option<PortRef>) {
        self.irq = value;
    }

    // ── Live UART state introspection (read-only) ───────────────────────
    //
    // The PL011 is embedded deep inside the HelmAddressSpace device list.
    // Rather than threading a typed reference, these accessors use the MMIO
    // flag register read path which is safe and accurate.

    /// Total bytes transmitted through this UART.
    ///
    /// Returns `None` if the system is not instantiated.
    #[getter]
    fn tx_count(&self, py: Python<'_>) -> PyResult<Option<u64>> {
        let sys = self.require_system(py)?;
        let system = sys.borrow(py);
        Ok(system.sim.as_ref().and_then(|s| s.uart_tx_count()))
    }

    /// Total bytes read from the RX FIFO of this UART.
    ///
    /// Returns `None` if the system is not instantiated.
    #[getter]
    fn rx_count(&self, py: Python<'_>) -> PyResult<Option<u64>> {
        let sys = self.require_system(py)?;
        let system = sys.borrow(py);
        Ok(system.sim.as_ref().and_then(|s| s.uart_rx_count()))
    }

    /// Whether the transmit FIFO is full.
    ///
    /// In simulation, TX is always instant so this is always `False`.
    /// Returns `None` if the system is not instantiated.
    #[getter]
    fn is_tx_full(&self, py: Python<'_>) -> PyResult<Option<bool>> {
        let sys = self.require_system(py)?;
        let system = sys.borrow(py);
        Ok(system.sim.as_ref().and_then(|s| s.uart_is_tx_full()))
    }

    /// Whether the receive FIFO is empty.
    ///
    /// Returns `None` if the system is not instantiated.
    #[getter]
    fn is_rx_empty(&self, py: Python<'_>) -> PyResult<Option<bool>> {
        let sys = self.require_system(py)?;
        let system = sys.borrow(py);
        Ok(system.sim.as_ref().and_then(|s| s.uart_is_rx_empty()))
    }

    fn __traverse__(&self, visit: pyo3::PyVisit<'_>) -> Result<(), pyo3::PyTraverseError> {
        if let Some(ref sys) = self.system_ref {
            visit.call(sys)?;
        }
        Ok(())
    }

    fn __clear__(&mut self) {
        self.system_ref = None;
    }
}

impl Pl011 {
    fn require_system(&self, _py: Python<'_>) -> PyResult<&Py<HelmSystem>> {
        self.system_ref.as_ref().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "UART state inspection requires an instantiated system \
                 (call system.instantiate() first)",
            )
        })
    }
}

/// Minimal PCI function with a BAR0-backed RAM window.
#[pyclass(name = "PciRamBar", extends = SimObject)]
pub struct PciRamBar {
    #[pyo3(get, set)]
    pub vendor_id: u16,
    #[pyo3(get, set)]
    pub device_id: u16,
    #[pyo3(get, set)]
    pub class_code: u32,
    #[pyo3(get, set)]
    pub bus: u8,
    #[pyo3(get, set)]
    pub slot: u8,
    #[pyo3(get, set)]
    pub function: u8,
}

#[pymethods]
impl PciRamBar {
    #[new]
    #[pyo3(signature = (
        name,
        *,
        vendor_id = 0xCAFE,
        device_id = 0x0001,
        class_code = 0xFF0000,
        bus = 0,
        slot = 1,
        function = 0
    ))]
    fn new(
        name: &str,
        vendor_id: u16,
        device_id: u16,
        class_code: u32,
        bus: u8,
        slot: u8,
        function: u8,
    ) -> (Self, SimObject) {
        (
            PciRamBar {
                vendor_id,
                device_id,
                class_code,
                bus,
                slot,
                function,
            },
            SimObject::new(name),
        )
    }
}

/// PCI-attached VirtIO RNG MMIO bridge compatibility shim.
///
/// Prefer the standard modern [`PciVirtioRng`] transport for new machine
/// definitions. This wrapper exists to keep the earlier BAR-exposed MMIO path
/// available while the standard `virtio-pci` transport supersedes it.
#[pyclass(name = "PciVirtioRngMmio", extends = SimObject)]
pub struct PciVirtioRngMmio {
    #[pyo3(get, set)]
    pub vendor_id: u16,
    #[pyo3(get, set)]
    pub device_id: u16,
    #[pyo3(get, set)]
    pub class_code: u32,
    #[pyo3(get, set)]
    pub bus: u8,
    #[pyo3(get, set)]
    pub slot: u8,
    #[pyo3(get, set)]
    pub function: u8,
    #[pyo3(get, set)]
    pub seed: u64,
}

#[pymethods]
impl PciVirtioRngMmio {
    #[new]
    #[pyo3(signature = (
        name,
        *,
        vendor_id = 0xCAFE,
        device_id = 0x1004,
        class_code = 0xFF0000,
        bus = 0,
        slot = 2,
        function = 0,
        seed = 0x0123_4567_89AB_CDEF
    ))]
    fn new(
        name: &str,
        vendor_id: u16,
        device_id: u16,
        class_code: u32,
        bus: u8,
        slot: u8,
        function: u8,
        seed: u64,
    ) -> (Self, SimObject) {
        (
            PciVirtioRngMmio {
                vendor_id,
                device_id,
                class_code,
                bus,
                slot,
                function,
                seed,
            },
            SimObject::new(name),
        )
    }
}

/// Standard modern `virtio-pci` RNG function.
#[pyclass(name = "PciVirtioRng", extends = SimObject)]
pub struct PciVirtioRng {
    #[pyo3(get, set)]
    pub bus: u8,
    #[pyo3(get, set)]
    pub slot: u8,
    #[pyo3(get, set)]
    pub function: u8,
    #[pyo3(get, set)]
    pub seed: u64,
}

#[pymethods]
impl PciVirtioRng {
    #[new]
    #[pyo3(signature = (
        name,
        *,
        bus = 0,
        slot = 3,
        function = 0,
        seed = 0x0123_4567_89AB_CDEF
    ))]
    fn new(name: &str, bus: u8, slot: u8, function: u8, seed: u64) -> (Self, SimObject) {
        (
            PciVirtioRng {
                bus,
                slot,
                function,
                seed,
            },
            SimObject::new(name),
        )
    }
}

/// Standard modern `virtio-pci` block function.
#[pyclass(name = "PciVirtioBlk", extends = SimObject)]
pub struct PciVirtioBlk {
    #[pyo3(get, set)]
    pub bus: u8,
    #[pyo3(get, set)]
    pub slot: u8,
    #[pyo3(get, set)]
    pub function: u8,
    #[pyo3(get, set)]
    pub capacity_bytes: usize,
    #[pyo3(get, set)]
    pub read_only: bool,
}

#[pymethods]
impl PciVirtioBlk {
    #[new]
    #[pyo3(signature = (
        name,
        *,
        bus = 0,
        slot = 4,
        function = 0,
        capacity_bytes = 4096,
        read_only = false
    ))]
    fn new(
        name: &str,
        bus: u8,
        slot: u8,
        function: u8,
        capacity_bytes: usize,
        read_only: bool,
    ) -> (Self, SimObject) {
        (
            PciVirtioBlk {
                bus,
                slot,
                function,
                capacity_bytes,
                read_only,
            },
            SimObject::new(name),
        )
    }
}

/// Standard modern `virtio-pci` network function.
#[pyclass(name = "PciVirtioNet", extends = SimObject)]
pub struct PciVirtioNet {
    #[pyo3(get, set)]
    pub bus: u8,
    #[pyo3(get, set)]
    pub slot: u8,
    #[pyo3(get, set)]
    pub function: u8,
    #[pyo3(get, set)]
    pub mac: String,
}

#[pymethods]
impl PciVirtioNet {
    #[new]
    #[pyo3(signature = (
        name,
        *,
        bus = 0,
        slot = 5,
        function = 0,
        mac = "52:54:00:12:34:56"
    ))]
    fn new(name: &str, bus: u8, slot: u8, function: u8, mac: &str) -> (Self, SimObject) {
        (
            PciVirtioNet {
                bus,
                slot,
                function,
                mac: mac.to_string(),
            },
            SimObject::new(name),
        )
    }
}

/// Standard modern `virtio-pci` console function.
#[pyclass(name = "PciVirtioConsole", extends = SimObject)]
pub struct PciVirtioConsole {
    #[pyo3(get, set)]
    pub bus: u8,
    #[pyo3(get, set)]
    pub slot: u8,
    #[pyo3(get, set)]
    pub function: u8,
    #[pyo3(get, set)]
    pub serial: String,
    #[pyo3(get, set)]
    pub cols: u16,
    #[pyo3(get, set)]
    pub rows: u16,
}

#[pymethods]
impl PciVirtioConsole {
    #[new]
    #[pyo3(signature = (
        name,
        *,
        bus = 0,
        slot = 6,
        function = 0,
        serial = "null",
        cols = 80,
        rows = 24
    ))]
    fn new(
        name: &str,
        bus: u8,
        slot: u8,
        function: u8,
        serial: &str,
        cols: u16,
        rows: u16,
    ) -> (Self, SimObject) {
        (
            PciVirtioConsole {
                bus,
                slot,
                function,
                serial: serial.to_string(),
                cols,
                rows,
            },
            SimObject::new(name),
        )
    }
}

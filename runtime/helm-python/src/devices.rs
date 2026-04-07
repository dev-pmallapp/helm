#![allow(missing_docs)]

use crate::port::PortRef;
use crate::simobject::SimObject;
use pyo3::prelude::*;

/// GICv2 interrupt controller.
#[pyclass(name = "GicV2", extends = SimObject)]
pub struct GicV2 {
    #[pyo3(get, set)]
    pub num_irqs: u32,
}

#[pymethods]
impl GicV2 {
    #[new]
    #[pyo3(signature = (name, *, num_irqs = 96))]
    fn new(name: &str, num_irqs: u32) -> (Self, SimObject) {
        (GicV2 { num_irqs }, SimObject::new(name))
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
}

/// PL011 UART device.
#[pyclass(name = "Pl011", extends = SimObject)]
pub struct Pl011 {
    pub(crate) irq: Option<PortRef>,
}

#[pymethods]
impl Pl011 {
    #[new]
    fn new(name: &str) -> (Self, SimObject) {
        (Pl011 { irq: None }, SimObject::new(name))
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

/// PCI-attached VirtIO RNG MMIO bridge.
///
/// This exposes the existing VirtIO MMIO RNG transport behind PCI BAR0 so the
/// function can be discovered through the built-in `pci0` bus while still
/// using the current MMIO transport model.
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

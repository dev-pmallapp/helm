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

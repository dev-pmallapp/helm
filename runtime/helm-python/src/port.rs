#![allow(missing_docs)]

use pyo3::prelude::*;

/// Interrupt port connection descriptor, resolved at instantiate().
#[pyclass(name = "PortRef")]
#[derive(Clone)]
pub struct PortRef {
    #[pyo3(get)]
    pub target_name: String,
    #[pyo3(get)]
    pub port_name: String,
}

#[pymethods]
impl PortRef {
    fn __repr__(&self) -> String {
        format!("PortRef({}.{})", self.target_name, self.port_name)
    }
}

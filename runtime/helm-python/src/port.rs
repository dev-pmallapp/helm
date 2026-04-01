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
    /// Create a new port reference, resolved at instantiate() time.
    #[new]
    fn new(target_name: &str, port_name: &str) -> Self {
        Self {
            target_name: target_name.to_string(),
            port_name: port_name.to_string(),
        }
    }

    fn __repr__(&self) -> String {
        format!("PortRef({}.{})", self.target_name, self.port_name)
    }
}

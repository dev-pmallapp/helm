#![allow(missing_docs)]

use pyo3::prelude::*;

/// Interrupt port connection descriptor, resolved at instantiate().
#[pyclass(name = "PortRef")]
#[derive(Clone, Debug)]
pub struct PortRef {
    #[pyo3(get)]
    pub target_name: String,
    #[pyo3(get)]
    pub port_name: String,
    #[pyo3(get)]
    pub port_index: Option<u32>,
}

#[pymethods]
impl PortRef {
    /// Create a new port reference, resolved at instantiate() time.
    #[new]
    #[pyo3(signature = (target_name, port_name, port_index=None))]
    fn new(target_name: String, port_name: String, port_index: Option<u32>) -> Self {
        Self {
            target_name,
            port_name,
            port_index,
        }
    }

    fn __repr__(&self) -> String {
        match self.port_index {
            Some(idx) => format!("PortRef({}.{}[{}])", self.target_name, self.port_name, idx),
            None => format!("PortRef({}.{})", self.target_name, self.port_name),
        }
    }
}

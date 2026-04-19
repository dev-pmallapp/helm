#![allow(missing_docs)]

use indexmap::IndexMap;
use pyo3::prelude::*;

use crate::port::PortRef;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SimObjectState {
    Pending,
    Instantiated,
}

/// Base class for all simulatable components.
///
/// Manages the child hierarchy, port references, and tracks instantiation state.
#[pyclass(subclass, name = "SimObject")]
pub struct SimObject {
    pub name: String,
    pub children: IndexMap<String, PyObject>,
    /// Port wiring descriptors set via `device.irq = gic.spi(N)`.
    /// Resolved to actual interrupt wiring at `instantiate()` time.
    pub port_refs: IndexMap<String, PortRef>,
    pub state: SimObjectState,
}

#[pymethods]
impl SimObject {
    #[new]
    pub fn new(name: &str) -> Self {
        SimObject {
            name: name.to_string(),
            children: IndexMap::new(),
            port_refs: IndexMap::new(),
            state: SimObjectState::Pending,
        }
    }

    #[getter]
    fn name(&self) -> &str {
        &self.name
    }

    #[getter]
    fn instantiated(&self) -> bool {
        self.state == SimObjectState::Instantiated
    }

    fn __setattr__(&mut self, py: Python<'_>, name: &str, value: PyObject) -> PyResult<()> {
        // If value is a SimObject subclass, store as child
        if value.downcast_bound::<SimObject>(py).is_ok() {
            self.require_pending()?;
            self.children.insert(name.to_string(), value);
            return Ok(());
        }
        // If value is a PortRef, store as a port wiring descriptor
        if let Ok(port_ref) = value.extract::<PortRef>(py) {
            self.require_pending()?;
            self.port_refs.insert(name.to_string(), port_ref);
            return Ok(());
        }
        // Otherwise, delegate to normal attribute setting via Python
        Err(PyErr::new::<pyo3::exceptions::PyAttributeError, _>(
            format!("cannot set '{name}' on SimObject '{}'", self.name),
        ))
    }

    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        // Check children first
        if let Some(obj) = self.children.get(name) {
            return Ok(obj.clone_ref(py));
        }
        // Then check port refs
        if let Some(pref) = self.port_refs.get(name) {
            let py_ref = Py::new(py, pref.clone())
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;
            return Ok(py_ref.into_any());
        }
        Err(PyErr::new::<pyo3::exceptions::PyAttributeError, _>(
            format!("'{}' has no child or port '{name}'", self.name),
        ))
    }

    fn __traverse__(&self, visit: pyo3::PyVisit<'_>) -> Result<(), pyo3::PyTraverseError> {
        for child in self.children.values() {
            visit.call(child)?;
        }
        Ok(())
    }

    fn __clear__(&mut self) {
        self.children.clear();
        self.port_refs.clear();
    }
}

impl SimObject {
    pub fn require_pending(&self) -> PyResult<()> {
        if self.state == SimObjectState::Instantiated {
            return Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "cannot modify SimObject after instantiate()",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_refs_map_starts_empty() {
        let obj = SimObject::new("test");
        assert!(obj.port_refs.is_empty());
    }

    #[test]
    fn port_refs_stored_directly() {
        let mut obj = SimObject::new("device0");
        let pref = PortRef {
            target_name: "gic".into(),
            port_name: "spi".into(),
            port_index: Some(5),
        };
        obj.port_refs.insert("irq".into(), pref.clone());
        assert_eq!(obj.port_refs.len(), 1);
        let stored = obj.port_refs.get("irq").unwrap();
        assert_eq!(stored.target_name, "gic");
        assert_eq!(stored.port_name, "spi");
        assert_eq!(stored.port_index, Some(5));
    }

    #[test]
    fn port_refs_cleared_on_clear() {
        let mut obj = SimObject::new("device0");
        obj.port_refs.insert(
            "irq".into(),
            PortRef {
                target_name: "gic".into(),
                port_name: "spi".into(),
                port_index: Some(5),
            },
        );
        assert!(!obj.port_refs.is_empty());
        obj.port_refs.clear();
        assert!(obj.port_refs.is_empty());
    }

    #[test]
    fn setattr_stores_port_ref_via_python() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let pref = Py::new(
                py,
                PortRef {
                    target_name: "gic".into(),
                    port_name: "spi".into(),
                    port_index: Some(33),
                },
            )
            .unwrap();

            let mut obj = SimObject::new("uart0");
            obj.__setattr__(py, "irq", pref.into_any()).unwrap();

            assert_eq!(obj.port_refs.len(), 1);
            let stored = obj.port_refs.get("irq").unwrap();
            assert_eq!(stored.target_name, "gic");
            assert_eq!(stored.port_name, "spi");
            assert_eq!(stored.port_index, Some(33));
        });
    }

    #[test]
    fn getattr_returns_stored_port_ref() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let mut obj = SimObject::new("uart0");
            obj.port_refs.insert(
                "irq".into(),
                PortRef {
                    target_name: "gic".into(),
                    port_name: "spi".into(),
                    port_index: Some(7),
                },
            );

            let result = obj.__getattr__(py, "irq").unwrap();
            let pref: PortRef = result.extract(py).unwrap();
            assert_eq!(pref.target_name, "gic");
            assert_eq!(pref.port_name, "spi");
            assert_eq!(pref.port_index, Some(7));
        });
    }

    #[test]
    fn setattr_rejects_port_ref_after_instantiate() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let pref = Py::new(
                py,
                PortRef {
                    target_name: "gic".into(),
                    port_name: "spi".into(),
                    port_index: Some(33),
                },
            )
            .unwrap();

            let mut obj = SimObject::new("uart0");
            obj.state = SimObjectState::Instantiated;

            let result = obj.__setattr__(py, "irq", pref.into_any());
            assert!(result.is_err());
        });
    }
}

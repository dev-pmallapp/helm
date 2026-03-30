#![allow(missing_docs)]

use indexmap::IndexMap;
use pyo3::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SimObjectState {
    Pending,
    Instantiated,
}

/// Base class for all simulatable components.
///
/// Manages the child hierarchy and tracks instantiation state.
#[pyclass(subclass, name = "SimObject")]
pub struct SimObject {
    pub name: String,
    pub children: IndexMap<String, PyObject>,
    pub state: SimObjectState,
}

#[pymethods]
impl SimObject {
    #[new]
    pub fn new(name: &str) -> Self {
        SimObject {
            name: name.to_string(),
            children: IndexMap::new(),
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
        // Otherwise, delegate to normal attribute setting via Python
        Err(PyErr::new::<pyo3::exceptions::PyAttributeError, _>(
            format!("cannot set '{name}' on SimObject '{}'", self.name),
        ))
    }

    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        self.children
            .get(name)
            .map(|obj| obj.clone_ref(py))
            .ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyAttributeError, _>(format!(
                    "'{}' has no child '{name}'",
                    self.name
                ))
            })
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

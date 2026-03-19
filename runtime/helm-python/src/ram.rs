#![allow(missing_docs)]

use crate::simobject::SimObject;
use pyo3::prelude::*;

/// RAM — size descriptor (base address assigned by MemorySpace).
#[pyclass(name = "Ram", extends = SimObject)]
pub struct Ram {
    #[pyo3(get, set)]
    pub size: String,
}

#[pymethods]
impl Ram {
    #[new]
    #[pyo3(signature = (name, *, size = "512MiB"))]
    fn new(name: &str, size: &str) -> (Self, SimObject) {
        (Ram { size: size.into() }, SimObject::new(name))
    }
}

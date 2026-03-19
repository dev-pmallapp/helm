#![allow(missing_docs)]

use crate::simobject::SimObject;
use pyo3::prelude::*;

/// Cache level descriptor (consumed by timing model, no live Rust backing).
#[pyclass(name = "Cache", extends = SimObject)]
pub struct Cache {
    #[pyo3(get, set)]
    pub size: String,
    #[pyo3(get, set)]
    pub assoc: u32,
    #[pyo3(get, set)]
    pub latency: u32,
    #[pyo3(get, set)]
    pub line_size: u32,
}

#[pymethods]
impl Cache {
    #[new]
    #[pyo3(signature = (name, *, size = "32KiB", assoc = 8, latency = 4, line_size = 64))]
    fn new(name: &str, size: &str, assoc: u32, latency: u32, line_size: u32) -> (Self, SimObject) {
        (
            Cache {
                size: size.into(),
                assoc,
                latency,
                line_size,
            },
            SimObject::new(name),
        )
    }
}

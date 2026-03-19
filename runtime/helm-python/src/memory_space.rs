#![allow(missing_docs)]

use crate::simobject::SimObject;
use pyo3::prelude::*;

/// A single entry in the memory map.
#[pyclass(name = "MapEntry")]
#[allow(dead_code)]
pub struct MapEntry {
    #[pyo3(get)]
    pub base: u64,
    pub(crate) device: PyObject,
    #[pyo3(get)]
    pub size: u64,
    #[pyo3(get)]
    pub bank: u32,
}

/// Physical memory space with device mapping.
#[pyclass(name = "MemorySpace", extends = SimObject)]
pub struct MemorySpace {
    pub(crate) entries: Vec<MapEntry>,
}

#[pymethods]
impl MemorySpace {
    #[new]
    fn new(name: &str) -> (Self, SimObject) {
        (
            MemorySpace {
                entries: Vec::new(),
            },
            SimObject::new(name),
        )
    }

    /// Add a device to the memory map.
    #[pyo3(signature = (base, device, size, *, bank = 0))]
    fn add_map(
        &mut self,
        base: u64,
        device: PyObject,
        size: u64,
        bank: u32,
    ) -> PyResult<()> {
        self.entries.push(MapEntry {
            base,
            device,
            size,
            bank,
        });
        Ok(())
    }
}

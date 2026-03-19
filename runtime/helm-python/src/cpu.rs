#![allow(missing_docs)]

use crate::simobject::SimObject;
use pyo3::prelude::*;

/// CPU core configuration and register access.
#[pyclass(name = "Cpu", extends = SimObject)]
pub struct Cpu {
    #[pyo3(get, set)]
    pub isa: String,
    #[pyo3(get, set)]
    pub model: String,
    #[pyo3(get, set)]
    pub width: u32,
    #[pyo3(get, set)]
    pub rob_size: u32,
    #[pyo3(get, set)]
    pub iq_size: u32,
    #[pyo3(get, set)]
    pub lq_size: u32,
    #[pyo3(get, set)]
    pub sq_size: u32,
}

#[pymethods]
impl Cpu {
    #[new]
    #[pyo3(signature = (name, *, isa="aarch64", model="cortex-a55",
                        width=4, rob_size=128, iq_size=64, lq_size=32, sq_size=32))]
    fn new(
        name: &str,
        isa: &str,
        model: &str,
        width: u32,
        rob_size: u32,
        iq_size: u32,
        lq_size: u32,
        sq_size: u32,
    ) -> (Self, SimObject) {
        (
            Cpu {
                isa: isa.into(),
                model: model.into(),
                width,
                rob_size,
                iq_size,
                lq_size,
                sq_size,
            },
            SimObject::new(name),
        )
    }
}

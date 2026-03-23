//! `helm-python` — PyO3 bindings for the helm-ng simulator.
//!
//! Exposes the `_helm_ng` module to Python with a first-class SimObject hierarchy:
//!
//! - `SimObject` — base class for all simulatable components
//! - `System` — top-level container wrapping `HelmSim`
//! - `Cpu`, `Ram`, `MemorySpace`, `Cache` — config descriptors
//! - `GicV2`, `Pl011` — device wrappers
//! - `SpySession` — standalone observation session
//! - `PortRef`, `MapEntry` — connection and mapping descriptors
//! - `build_simulation()` — backward-compatible factory

#![allow(missing_docs)]

mod cache;
mod compat;
mod cpu;
mod devices;
mod instantiate;
mod memory_space;
mod port;
mod ram;
mod simobject;
mod spy;
mod system;

use pyo3::prelude::*;

#[pymodule]
pub fn _helm_ng(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // SimObject hierarchy
    m.add_class::<simobject::SimObject>()?;
    m.add_class::<system::System>()?;
    m.add_class::<cpu::Cpu>()?;
    m.add_class::<ram::Ram>()?;
    m.add_class::<memory_space::MemorySpace>()?;
    m.add_class::<memory_space::MapEntry>()?;
    m.add_class::<cache::Cache>()?;

    // Devices
    m.add_class::<devices::GicV2>()?;
    m.add_class::<devices::Pl011>()?;

    // Support classes
    m.add_class::<port::PortRef>()?;

    // Standalone observer
    m.add_class::<spy::PySpySession>()?;

    // Backward-compat factory + diagnostics
    m.add_function(wrap_pyfunction!(compat::build_simulation, m)?)?;
    m.add_function(wrap_pyfunction!(compat::set_sim_trace, m)?)?;

    Ok(())
}

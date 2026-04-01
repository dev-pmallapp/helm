//! `helm-python` — PyO3 bindings for the helm-ng simulator.
//!
//! Exposes the `_helm_ng` module to Python with a first-class SimObject hierarchy:
//!
//! - `SimObject` — base class for all simulatable components
//! - `HelmSystem` — top-level container wrapping `HelmSim`
//! - `Cpu`, `Ram`, `MemorySpace`, `Cache` — config descriptors
//! - `GicV2`, `Pl011` — device wrappers
//! - `HelmSpy` — standalone observation session
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

/// Version of the helm-ng Python API (from Cargo.toml).
const HELM_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Return a dict of per-surface version strings.
#[pyfunction]
fn version_manifest() -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    m.insert("helm_ng".to_string(), HELM_VERSION.to_string());
    m.insert(
        "device_sdk".to_string(),
        helm_devices::framework::sdk::SDK_VERSION.to_string(),
    );
    m.insert(
        "device_abi".to_string(),
        format!(
            "{}.{}",
            helm_devices::framework::sdk::SDK_VERSION_MAJOR,
            helm_devices::framework::sdk::SDK_VERSION_MINOR,
        ),
    );
    m
}

#[pymodule]
pub fn _helm_ng(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Version
    m.add("__version__", HELM_VERSION)?;

    // SimObject hierarchy
    m.add_class::<simobject::SimObject>()?;
    m.add_class::<system::HelmSystem>()?;
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
    m.add_class::<spy::HelmSpy>()?;

    // Backward-compat factory + diagnostics
    m.add_function(wrap_pyfunction!(compat::build_simulation, m)?)?;
    m.add_function(wrap_pyfunction!(compat::set_sim_trace, m)?)?;
    m.add_function(wrap_pyfunction!(compat::list_cpu_models, m)?)?;
    m.add_function(wrap_pyfunction!(compat::list_platforms, m)?)?;

    // Version manifest
    m.add_function(wrap_pyfunction!(version_manifest, m)?)?;

    Ok(())
}

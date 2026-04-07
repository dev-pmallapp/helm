#![allow(missing_docs)]

use pyo3::prelude::*;

use crate::instantiate::freeze_explicit_system_config;
use crate::simobject::{SimObject, SimObjectState};
use crate::system::HelmSystem;
use helm_engine::build_simulator_from_request;

/// Backward-compatible factory — creates a HelmSystem and instantiates it.
#[pyfunction]
#[pyo3(signature = (
    isa      = "aarch64",
    mode     = "se",
    timing   = "virtual",
    mem_base = 0u64,
    mem_mib  = 512usize,
    ipc      = 4.0f64,
))]
pub fn build_simulation(
    py: Python<'_>,
    isa: &str,
    mode: &str,
    timing: &str,
    mem_base: u64,
    mem_mib: usize,
    ipc: f64,
) -> PyResult<Py<HelmSystem>> {
    let mem_size = mem_mib * 1024 * 1024;
    let request =
        freeze_explicit_system_config(isa, mode, timing, mem_base, mem_size, ipc)?.request;
    let sim = build_simulator_from_request(request);

    let system = HelmSystem {
        timing: timing.into(),
        mode: mode.into(),
        ipc,
        sim: Some(sim),
        exited: false,
        exit_code_val: 0,
        plugins: Vec::new(),
    };
    let mut base = SimObject::new("default");
    base.state = SimObjectState::Instantiated;

    Py::new(py, (system, base))
}

/// Install a sim-trace monitor on the current Python thread.
#[pyfunction]
#[pyo3(signature = (uri = "stderr:"))]
pub fn set_sim_trace(uri: &str) -> PyResult<String> {
    use helm_diag::{install_monitor, DiagSink};
    let (sink, monitor) = DiagSink::open(uri).map_err(|e| {
        pyo3::exceptions::PyValueError::new_err(format!(
            "cannot open sim-trace backend '{uri}': {e}"
        ))
    })?;
    install_monitor(monitor);
    std::mem::forget(sink);
    Ok(uri.to_string())
}

/// List available CPU models as `[(name, description), ...]`.
#[pyfunction]
pub fn list_cpu_models() -> Vec<(String, String)> {
    helm_engine::helm_arch::ArmCoreModel::list_models()
        .into_iter()
        .map(|(n, d)| (n.to_string(), d.to_string()))
        .collect()
}

/// List available machine/platform types as `[(name, description, isa), ...]`.
#[pyfunction]
pub fn list_platforms() -> Vec<(String, String, String)> {
    helm_platform::list_platforms()
        .into_iter()
        .map(|p| {
            (
                p.name.to_string(),
                p.description.to_string(),
                p.isa.to_string(),
            )
        })
        .collect()
}

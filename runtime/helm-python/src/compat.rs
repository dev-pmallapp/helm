#![allow(missing_docs)]

use std::cell::RefCell;

use helm_diag::{install_monitor, uninstall_monitor, DiagSink};
use pyo3::prelude::*;

use crate::instantiate::freeze_explicit_system_config;
use crate::simobject::{SimObject, SimObjectState};
use crate::system::HelmSystem;
use helm_engine::build_simulator_from_request;

thread_local! {
    static COMPAT_SIM_TRACE_SINK: RefCell<Option<DiagSink>> = const { RefCell::new(None) };
}

pub fn clear_sim_trace_for_host() {
    uninstall_monitor();
    COMPAT_SIM_TRACE_SINK.with(|slot| {
        drop(slot.borrow_mut().take());
    });
}

#[pyfunction]
pub fn clear_sim_trace() {
    clear_sim_trace_for_host();
}

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
        num_cpus: 1,
        gic_version: "v3".into(),
        sim: Some(sim),
        exited: false,
        exit_code_val: 0,
        plugins: Vec::new(),
        breakpoints: None,
        watchpoints: None,
        native_trigger_state: None,
        last_stop_default: helm_debug::RuntimeStopState::default(),
        last_stop_by_runtime: std::collections::HashMap::new(),
        cut_points_default: Vec::new(),
        cut_points_by_runtime: std::collections::HashMap::new(),
        segment_history_default: Vec::new(),
        segment_history_by_runtime: std::collections::HashMap::new(),
    };
    let mut base = SimObject::new("default");
    base.state = SimObjectState::Instantiated;

    Py::new(py, (system, base))
}

/// Install a sim-trace monitor on the current Python thread.
#[pyfunction]
#[pyo3(signature = (uri = "stderr:"))]
pub fn set_sim_trace(uri: &str) -> PyResult<String> {
    clear_sim_trace_for_host();

    let (sink, monitor) = DiagSink::open(uri).map_err(|e| {
        pyo3::exceptions::PyValueError::new_err(format!(
            "cannot open sim-trace backend '{uri}': {e}"
        ))
    })?;
    install_monitor(monitor);
    COMPAT_SIM_TRACE_SINK.with(|slot| {
        *slot.borrow_mut() = Some(sink);
    });
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

#[cfg(test)]
mod tests {
    use super::{clear_sim_trace_for_host, set_sim_trace};
    use helm_diag::DIAG_MONITOR;

    #[test]
    fn set_sim_trace_installs_and_clear_sim_trace_removes_monitor() {
        clear_sim_trace_for_host();
        assert!(DIAG_MONITOR.with(|cell| cell.borrow().is_none()));

        set_sim_trace("null:").expect("install sim-trace");
        assert!(DIAG_MONITOR.with(|cell| cell.borrow().is_some()));

        clear_sim_trace_for_host();
        assert!(DIAG_MONITOR.with(|cell| cell.borrow().is_none()));
    }
}

#![allow(missing_docs)]

use helm_engine::{build_simulator, Isa};
use pyo3::prelude::*;

use crate::simobject::{SimObject, SimObjectState};
use crate::system::{parse_mode, parse_timing, System};

/// Backward-compatible factory — creates a System and instantiates it.
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
) -> PyResult<Py<System>> {
    let isa_val = match isa {
        "aarch64" | "arm64" => Isa::AArch64,
        "riscv" | "riscv64" | "rv64" => Isa::RiscV,
        "aarch32" | "arm32" => Isa::AArch32,
        other => {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "unknown ISA '{other}'"
            )))
        }
    };
    let mode_val = parse_mode(mode)?;
    let timing_val = parse_timing(timing, ipc)?;

    let mem_size = mem_mib * 1024 * 1024;
    let sim = build_simulator(isa_val, mode_val, timing_val, mem_base, mem_size);

    let system = System {
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

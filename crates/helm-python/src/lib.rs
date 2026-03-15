//! `helm-python` — PyO3 bindings for the helm_ng Python package.
//!
//! Exposes `HelmSim` as `PySimulation` to Python.
//! All Python interaction goes through this thin layer; no logic lives here.
//!
//! # Phase 2
//! Full Python DSL (Cpu, Memory, Board component classes) implemented in
//! `python/helm_ng/` on top of these raw bindings.

use helm_engine::{build_simulator, ExecMode, Isa, StopReason, TimingChoice};
use pyo3::prelude::*;

// ── PySimulation ──────────────────────────────────────────────────────────────

/// Python-facing simulation handle.
#[pyclass(name = "Simulation")]
pub struct PySimulation {
    inner: helm_engine::HelmSim,
}

#[pymethods]
impl PySimulation {
    /// Run up to `max_insns` instructions.
    /// Returns `"quantum"`, `"exit:<code>"`, or `"exception:<msg>"`.
    fn run(&mut self, max_insns: u64) -> String {
        match self.inner.run(max_insns) {
            StopReason::Quantum      => "quantum".to_string(),
            StopReason::Exit { code } => format!("exit:{code}"),
            StopReason::Exception(e)  => format!("exception:{e}"),
            StopReason::Unsupported   => "unsupported".to_string(),
        }
    }

    fn set_pc(&mut self, pc: u64) { self.inner.set_pc(pc); }

    fn insns_retired(&self) -> u64 { self.inner.insns_retired() }

    fn load_bytes(&mut self, addr: u64, data: Vec<u8>) {
        self.inner.load_bytes(addr, &data);
    }
}

// ── Module entry point ────────────────────────────────────────────────────────

/// `build_simulator(isa, mode, timing, mem_base, mem_size) -> Simulation`
#[pyfunction]
#[pyo3(signature = (
    isa      = "riscv",
    mode     = "syscall",
    timing   = "virtual",
    mem_base = 0x8000_0000u64,
    mem_size = 512 * 1024 * 1024usize,
    ipc      = 1.0f64,
))]
fn new_simulation(
    isa: &str,
    mode: &str,
    timing: &str,
    mem_base: u64,
    mem_size: usize,
    ipc: f64,
) -> PyResult<PySimulation> {
    let isa = match isa {
        "riscv" | "riscv64" => Isa::RiscV,
        "aarch64"           => Isa::AArch64,
        "aarch32"           => Isa::AArch32,
        other => return Err(pyo3::exceptions::PyValueError::new_err(
            format!("unknown ISA '{other}'; expected riscv64, aarch64, or aarch32"),
        )),
    };
    let mode = match mode {
        "functional" => ExecMode::Functional,
        "syscall" | "se" => ExecMode::Syscall,
        "system"  | "fs" => ExecMode::System,
        other => return Err(pyo3::exceptions::PyValueError::new_err(
            format!("unknown mode '{other}'; expected functional, syscall, or system"),
        )),
    };
    let timing = match timing {
        "virtual"  => TimingChoice::Virtual { ipc },
        "interval" => TimingChoice::Interval { ipc, interval_len: 10_000 },
        "accurate" => TimingChoice::Accurate,
        other => return Err(pyo3::exceptions::PyValueError::new_err(
            format!("unknown timing '{other}'; expected virtual, interval, or accurate"),
        )),
    };

    Ok(PySimulation {
        inner: build_simulator(isa, mode, timing, mem_base, mem_size),
    })
}

#[pymodule]
fn helm_ng(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PySimulation>()?;
    m.add_function(wrap_pyfunction!(new_simulation, m)?)?;
    Ok(())
}

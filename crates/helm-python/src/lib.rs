//! # helm-python
//!
//! PyO3 extension module that exposes HELM's Rust simulation engine to
//! Python.  The Python-side `helm` package calls into these bindings to
//! launch simulations and retrieve results.

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use helm_core::config::{
    BranchPredictorConfig, CacheConfig, CoreConfig, MemoryConfig, PlatformConfig,
};
use helm_core::types::{ExecMode, IsaKind};
use helm_engine::Simulation;

// ---------------------------------------------------------------------------
// Python-visible configuration classes
// ---------------------------------------------------------------------------

/// Cache configuration exposed to Python.
#[pyclass(name = "CacheConfig")]
#[derive(Clone)]
struct PyCacheConfig {
    inner: CacheConfig,
}

#[pymethods]
impl PyCacheConfig {
    #[new]
    #[pyo3(signature = (size, associativity, latency_cycles, line_size=64))]
    fn new(size: String, associativity: u32, latency_cycles: u64, line_size: u32) -> Self {
        Self {
            inner: CacheConfig {
                size,
                associativity,
                latency_cycles,
                line_size,
            },
        }
    }
}

/// Branch predictor configuration exposed to Python.
#[pyclass(name = "BranchPredictorConfig")]
#[derive(Clone)]
struct PyBranchPredictorConfig {
    inner: BranchPredictorConfig,
}

#[pymethods]
impl PyBranchPredictorConfig {
    #[staticmethod]
    fn static_pred() -> Self {
        Self {
            inner: BranchPredictorConfig::Static,
        }
    }

    #[staticmethod]
    fn bimodal(table_size: u32) -> Self {
        Self {
            inner: BranchPredictorConfig::Bimodal { table_size },
        }
    }

    #[staticmethod]
    fn gshare(history_bits: u32) -> Self {
        Self {
            inner: BranchPredictorConfig::GShare { history_bits },
        }
    }

    #[staticmethod]
    fn tage(history_length: u32) -> Self {
        Self {
            inner: BranchPredictorConfig::TAGE { history_length },
        }
    }

    #[staticmethod]
    fn tournament() -> Self {
        Self {
            inner: BranchPredictorConfig::Tournament,
        }
    }
}

/// Core configuration exposed to Python.
#[pyclass(name = "CoreConfig")]
#[derive(Clone)]
struct PyCoreConfig {
    inner: CoreConfig,
}

#[pymethods]
impl PyCoreConfig {
    #[new]
    #[pyo3(signature = (name, width=4, rob_size=128, iq_size=64, lq_size=32, sq_size=32, branch_predictor=None))]
    fn new(
        name: String,
        width: u32,
        rob_size: u32,
        iq_size: u32,
        lq_size: u32,
        sq_size: u32,
        branch_predictor: Option<PyBranchPredictorConfig>,
    ) -> Self {
        let bp = branch_predictor
            .map(|b| b.inner)
            .unwrap_or(BranchPredictorConfig::Static);
        Self {
            inner: CoreConfig {
                name,
                width,
                rob_size,
                iq_size,
                lq_size,
                sq_size,
                branch_predictor: bp,
            },
        }
    }
}

/// Memory configuration exposed to Python.
#[pyclass(name = "MemoryConfig")]
#[derive(Clone)]
struct PyMemoryConfig {
    inner: MemoryConfig,
}

#[pymethods]
impl PyMemoryConfig {
    #[new]
    #[pyo3(signature = (dram_latency_cycles=100, l1i=None, l1d=None, l2=None, l3=None))]
    fn new(
        dram_latency_cycles: u64,
        l1i: Option<PyCacheConfig>,
        l1d: Option<PyCacheConfig>,
        l2: Option<PyCacheConfig>,
        l3: Option<PyCacheConfig>,
    ) -> Self {
        Self {
            inner: MemoryConfig {
                l1i: l1i.map(|c| c.inner),
                l1d: l1d.map(|c| c.inner),
                l2: l2.map(|c| c.inner),
                l3: l3.map(|c| c.inner),
                dram_latency_cycles,
            },
        }
    }
}

/// Platform configuration exposed to Python.
#[pyclass(name = "PlatformConfig")]
#[derive(Clone)]
struct PyPlatformConfig {
    inner: PlatformConfig,
}

#[pymethods]
impl PyPlatformConfig {
    #[new]
    #[pyo3(signature = (name, isa, exec_mode, cores, memory))]
    fn new(
        name: String,
        isa: &str,
        exec_mode: &str,
        cores: Vec<PyCoreConfig>,
        memory: PyMemoryConfig,
    ) -> PyResult<Self> {
        let isa_kind = match isa.to_lowercase().as_str() {
            "x86" | "x86_64" => IsaKind::X86_64,
            "riscv" | "riscv64" => IsaKind::RiscV64,
            "arm" | "aarch64" | "arm64" => IsaKind::Arm64,
            _ => return Err(PyRuntimeError::new_err(format!("Unknown ISA: {}", isa))),
        };
        let mode = match exec_mode.to_lowercase().as_str() {
            "se" | "syscall" | "syscall_emulation" => ExecMode::SyscallEmulation,
            "microarch" | "microarchitectural" | "detailed" => ExecMode::Microarchitectural,
            _ => {
                return Err(PyRuntimeError::new_err(format!(
                    "Unknown exec mode: {}",
                    exec_mode
                )))
            }
        };
        Ok(Self {
            inner: PlatformConfig {
                name,
                isa: isa_kind,
                exec_mode: mode,
                cores: cores.into_iter().map(|c| c.inner).collect(),
                memory: memory.inner,
            },
        })
    }

    /// Serialise to JSON.
    fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.inner).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Simulation entry point
// ---------------------------------------------------------------------------

/// Run a simulation from Python.
#[pyfunction]
#[pyo3(signature = (platform, binary, max_cycles=1_000_000))]
fn run_simulation(platform: PyPlatformConfig, binary: String, max_cycles: u64) -> PyResult<String> {
    let mut sim = Simulation::new(platform.inner, binary);
    let results = sim
        .run(max_cycles)
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
    Ok(results.to_json())
}

// ---------------------------------------------------------------------------
// Module registration
// ---------------------------------------------------------------------------

/// The native Python module (imported as `helm._helm_core`).
#[pymodule]
fn _helm_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyCacheConfig>()?;
    m.add_class::<PyBranchPredictorConfig>()?;
    m.add_class::<PyCoreConfig>()?;
    m.add_class::<PyMemoryConfig>()?;
    m.add_class::<PyPlatformConfig>()?;
    m.add_function(wrap_pyfunction!(run_simulation, m)?)?;
    Ok(())
}

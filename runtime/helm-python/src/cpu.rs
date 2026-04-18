#![allow(missing_docs)]

use crate::simobject::SimObject;
use crate::system::HelmSystem;
use pyo3::prelude::*;

/// CPU core configuration and register access.
///
/// Before `instantiate()`: holds configuration fields (ISA, model, widths).
/// After `instantiate()`: also provides live register read access via a
/// back-reference to the parent [`HelmSystem`].
///
/// # Examples (after instantiate)
///
/// ```python
/// system = System("sys")
/// system.cpu = Cpu("cpu0", isa="riscv64")
/// system.instantiate()
/// system.load_elf("hello")
/// system.run(100)
/// print(system.cpu.pc)       # current program counter
/// print(system.cpu.xn(1))    # general register x1
/// print(system.cpu.sp)       # stack pointer
/// ```
#[pyclass(name = "Cpu", extends = SimObject)]
pub struct Cpu {
    #[pyo3(get, set)]
    pub isa: String,
    #[pyo3(get, set)]
    pub model: String,
    /// Front-end / issue width hint stored on the Python descriptor.
    ///
    /// Current runtime execution paths do not consume this value.
    #[pyo3(get, set)]
    pub width: u32,
    /// Reserved reorder-buffer size hint stored on the Python descriptor.
    ///
    /// Current runtime execution paths do not consume this value.
    #[pyo3(get, set)]
    pub rob_size: u32,
    /// Reserved issue-queue size hint stored on the Python descriptor.
    ///
    /// Current runtime execution paths do not consume this value.
    #[pyo3(get, set)]
    pub iq_size: u32,
    /// Reserved load-queue size hint stored on the Python descriptor.
    ///
    /// Current runtime execution paths do not consume this value.
    #[pyo3(get, set)]
    pub lq_size: u32,
    /// Reserved store-queue size hint stored on the Python descriptor.
    ///
    /// Current runtime execution paths do not consume this value.
    #[pyo3(get, set)]
    pub sq_size: u32,

    /// Back-reference to the parent system (set during instantiate).
    pub(crate) system_ref: Option<Py<HelmSystem>>,
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
                system_ref: None,
            },
            SimObject::new(name),
        )
    }

    // ── Live register access (read-only, requires instantiate) ──────────

    /// Read general-purpose register `n`.
    ///
    /// AArch64: x0-x30 (n=31 returns SP).
    /// RISC-V: x0-x31 (x0 is always 0).
    ///
    /// Returns 0 if the system has not been instantiated.
    fn xn(&self, py: Python<'_>, n: usize) -> PyResult<u64> {
        let sys = self.require_system(py)?;
        let system = sys.borrow(py);
        match &system.sim {
            Some(sim) => Ok(sim.read_gpr(n).unwrap_or(0)),
            None => Ok(0),
        }
    }

    /// Program counter.
    #[getter]
    fn pc(&self, py: Python<'_>) -> PyResult<u64> {
        let sys = self.require_system(py)?;
        let system = sys.borrow(py);
        Ok(system.sim.as_ref().map_or(0, |s| s.pc()))
    }

    /// Stack pointer (alias for x31 on AArch64, x2 on RISC-V).
    #[getter]
    fn sp(&self, py: Python<'_>) -> PyResult<u64> {
        let sys = self.require_system(py)?;
        let system = sys.borrow(py);
        match &system.sim {
            Some(sim) => {
                if sim.a64_state().is_some() {
                    // AArch64: SP = x31
                    Ok(sim.read_gpr(31).unwrap_or(0))
                } else if sim.rv64_state().is_some() {
                    // RISC-V: SP = x2
                    Ok(sim.read_gpr(2).unwrap_or(0))
                } else {
                    Ok(0)
                }
            }
            None => Ok(0),
        }
    }

    /// Read SIMD/FP register `n` as a (low64, high64) tuple.
    ///
    /// AArch64 only. Returns (0, 0) for RISC-V or if not instantiated.
    fn vn(&self, py: Python<'_>, n: usize) -> PyResult<(u64, u64)> {
        let sys = self.require_system(py)?;
        let system = sys.borrow(py);
        match &system.sim {
            Some(sim) => Ok(sim
                .a64_state()
                .map_or((0, 0), |s| {
                    let val = s.v[n];
                    (val as u64, (val >> 64) as u64)
                })),
            None => Ok((0, 0)),
        }
    }

    /// NZCV condition flags (AArch64 only).
    #[getter]
    fn nzcv(&self, py: Python<'_>) -> PyResult<u32> {
        let sys = self.require_system(py)?;
        let system = sys.borrow(py);
        Ok(system
            .sim
            .as_ref()
            .and_then(|s| s.a64_state())
            .map_or(0, |s| s.nzcv))
    }

    /// Current exception level (AArch64 only).
    #[getter]
    fn current_el(&self, py: Python<'_>) -> PyResult<u8> {
        let sys = self.require_system(py)?;
        let system = sys.borrow(py);
        Ok(system
            .sim
            .as_ref()
            .and_then(|s| s.a64_state())
            .map_or(0, |s| s.current_el))
    }

    /// Total instructions retired.
    #[getter]
    fn insn_count(&self, py: Python<'_>) -> PyResult<u64> {
        let sys = self.require_system(py)?;
        let system = sys.borrow(py);
        Ok(system.sim.as_ref().map_or(0, |s| s.insns_retired()))
    }

    fn __traverse__(&self, visit: pyo3::PyVisit<'_>) -> Result<(), pyo3::PyTraverseError> {
        if let Some(ref sys) = self.system_ref {
            visit.call(sys)?;
        }
        Ok(())
    }

    fn __clear__(&mut self) {
        self.system_ref = None;
    }
}

impl Cpu {
    fn require_system(&self, _py: Python<'_>) -> PyResult<&Py<HelmSystem>> {
        self.system_ref.as_ref().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "CPU register access requires an instantiated system \
                 (call system.instantiate() first)",
            )
        })
    }
}

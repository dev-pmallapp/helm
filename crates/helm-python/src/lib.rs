//! `helm-python` — PyO3 bindings for the helm-ng simulator.
//!
//! Exposes the `_helm_ng` module to Python with:
//!
//! - `Simulation` — build a simulator, load ELF, run, inspect registers
//! - `build_simulation()` — constructor with keyword args

use helm_engine::{build_simulator, ExecMode, Isa, StopReason, TimingChoice};
use pyo3::prelude::*;

// ── Simulation ────────────────────────────────────────────────────────────────

/// Python-facing simulation handle.
///
/// Python usage::
///
///     import _helm_ng
///     sim = _helm_ng.build_simulation(isa="aarch64", mode="se")
///     sim.load_elf("./hello", ["hello"], ["HOME=/tmp"])
///     while not sim.has_exited:
///         sim.run(50_000_000)
///     print(sim.exit_code)
#[pyclass(name = "Simulation")]
pub struct PySimulation {
    inner: helm_engine::HelmSim,
    exited: bool,
    exit_code_val: i32,
}

#[pymethods]
impl PySimulation {
    /// Load a static AArch64 ELF binary and configure SE mode.
    ///
    /// Parameters
    /// ----------
    /// binary : str
    ///     Path to the AArch64 ELF.
    /// argv : list[str]
    ///     Argument vector.
    /// envp : list[str]
    ///     Environment variables.
    #[pyo3(signature = (binary, argv=None, envp=None))]
    fn load_elf(
        &mut self,
        binary: &str,
        argv: Option<Vec<String>>,
        envp: Option<Vec<String>>,
    ) -> PyResult<()> {
        let argv_strings = argv.unwrap_or_else(|| {
            vec![std::path::Path::new(binary)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()]
        });
        let envp_strings = envp.unwrap_or_else(|| {
            vec![
                "HOME=/tmp".into(), "TERM=dumb".into(),
                "PATH=/usr/bin:/bin".into(), "LANG=C".into(),
                "USER=helm".into(),
            ]
        });
        let argv_refs: Vec<&str> = argv_strings.iter().map(String::as_str).collect();
        let envp_refs: Vec<&str> = envp_strings.iter().map(String::as_str).collect();

        self.inner
            .load_aarch64_elf(binary, &argv_refs, &envp_refs)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e))
    }

    /// Run up to `max_insns` guest instructions.
    ///
    /// Returns a status string: ``"quantum"``, ``"exit:<code>"``,
    /// ``"exception:<msg>"``, or ``"unsupported"``.
    fn run(&mut self, max_insns: u64) -> String {
        if self.exited {
            return format!("exit:{}", self.exit_code_val);
        }
        match self.inner.run(max_insns) {
            StopReason::Exit { code } => {
                self.exited = true;
                self.exit_code_val = code;
                format!("exit:{code}")
            }
            StopReason::Quantum     => "quantum".to_string(),
            StopReason::Exception(e) => format!("exception:{e:?}"),
            StopReason::Unsupported => "unsupported".to_string(),
        }
    }

    /// Current program counter.
    #[getter]
    fn pc(&self) -> u64 {
        match &self.inner {
            helm_engine::HelmSim::Virtual(e)  => e.a64_state.as_ref().map_or(e.pc, |s| s.pc),
            helm_engine::HelmSim::Interval(e) => e.a64_state.as_ref().map_or(e.pc, |s| s.pc),
            helm_engine::HelmSim::Accurate(e) => e.a64_state.as_ref().map_or(e.pc, |s| s.pc),
        }
    }

    /// Total instructions retired.
    #[getter]
    fn insn_count(&self) -> u64 {
        self.inner.insns_retired()
    }

    /// True once the guest called ``exit()`` / ``exit_group()``.
    #[getter]
    fn has_exited(&self) -> bool {
        self.exited
    }

    /// Guest exit code (valid when ``has_exited`` is True).
    #[getter]
    fn exit_code(&self) -> i32 {
        self.exit_code_val
    }

    /// Read general-purpose register Xn (0-30) or SP (31).
    fn xn(&self, n: usize) -> u64 {
        let state = match &self.inner {
            helm_engine::HelmSim::Virtual(e)  => e.a64_state.as_ref(),
            helm_engine::HelmSim::Interval(e) => e.a64_state.as_ref(),
            helm_engine::HelmSim::Accurate(e) => e.a64_state.as_ref(),
        };
        state.map_or(0, |s| if n < 31 { s.x[n] } else { s.sp })
    }

    /// Stack pointer.
    #[getter]
    fn sp(&self) -> u64 {
        self.xn(31)
    }

    fn set_pc(&mut self, pc: u64) {
        self.inner.set_pc(pc);
    }

    fn load_bytes(&mut self, addr: u64, data: Vec<u8>) {
        self.inner.load_bytes(addr, &data);
    }
}

// ── build_simulation() ───────────────────────────────────────────────────────

/// Create a new simulation.
///
/// Parameters
/// ----------
/// isa : str
///     ``"aarch64"`` (default), ``"riscv64"``, or ``"aarch32"``.
/// mode : str
///     ``"se"`` (default), ``"functional"``, or ``"fs"``.
/// timing : str
///     ``"virtual"`` (default), ``"interval"``, or ``"accurate"``.
/// mem_base : int
///     Guest memory base address (default 0x0).
/// mem_mib : int
///     Guest memory in MiB (default 512).
/// ipc : float
///     Instructions-per-cycle for virtual/interval timing (default 4.0).
#[pyfunction]
#[pyo3(signature = (
    isa      = "aarch64",
    mode     = "se",
    timing   = "virtual",
    mem_base = 0x0u64,
    mem_mib  = 512usize,
    ipc      = 4.0f64,
))]
fn build_simulation(
    isa: &str,
    mode: &str,
    timing: &str,
    mem_base: u64,
    mem_mib: usize,
    ipc: f64,
) -> PyResult<PySimulation> {
    let isa = match isa {
        "aarch64" | "arm64"         => Isa::AArch64,
        "riscv" | "riscv64" | "rv64" => Isa::RiscV,
        "aarch32" | "arm32"         => Isa::AArch32,
        other => return Err(pyo3::exceptions::PyValueError::new_err(
            format!("unknown ISA '{other}'"),
        )),
    };
    let mode = match mode {
        "se" | "syscall"     => ExecMode::Syscall,
        "functional" | "fe"  => ExecMode::Functional,
        "fs" | "system"      => ExecMode::System,
        other => return Err(pyo3::exceptions::PyValueError::new_err(
            format!("unknown mode '{other}'"),
        )),
    };
    let timing = match timing {
        "virtual"  => TimingChoice::Virtual { ipc },
        "interval" => TimingChoice::Interval { ipc, interval_len: 10_000 },
        "accurate" => TimingChoice::Accurate,
        other => return Err(pyo3::exceptions::PyValueError::new_err(
            format!("unknown timing '{other}'"),
        )),
    };

    let mem_size = mem_mib * 1024 * 1024;
    Ok(PySimulation {
        inner: build_simulator(isa, mode, timing, mem_base, mem_size),
        exited: false,
        exit_code_val: 0,
    })
}

// ── Module ────────────────────────────────────────────────────────────────────

#[pymodule]
pub fn _helm_ng(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PySimulation>()?;
    m.add_function(wrap_pyfunction!(build_simulation, m)?)?;
    Ok(())
}

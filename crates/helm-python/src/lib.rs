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
use helm_plugin::api::ComponentRegistry;
use helm_plugin::runtime::{register_builtins, PluginComponentAdapter};
use helm_plugin::{PluginArgs, PluginRegistry};
use helm_timing::model::{ApeModelDetailed, FeModel};
use helm_timing::TimingModel;
use std::collections::HashMap;

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
            "se" | "syscall" | "syscall_emulation" => ExecMode::SE,
            "microarch" | "microarchitectural" | "detailed" => ExecMode::CAE,
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
// Simulation entry points
// ---------------------------------------------------------------------------

/// Python-visible timing model wrapper.
///
/// Constructed from ``TimingMode.to_dict()`` on the Python side.
/// The ``level`` key selects the model; remaining keys are params.
#[pyclass(name = "TimingModel")]
#[derive(Clone)]
struct PyTimingModel {
    level: String,
    params: HashMap<String, u64>,
}

#[pymethods]
impl PyTimingModel {
    #[new]
    #[pyo3(signature = (level="fe", **params))]
    fn new(level: &str, params: Option<HashMap<String, u64>>) -> Self {
        Self {
            level: level.to_lowercase(),
            params: params.unwrap_or_default(),
        }
    }
}

/// Build a `Box<dyn TimingModel>` from a [`PyTimingModel`].
fn build_timing_model(cfg: &PyTimingModel) -> Box<dyn TimingModel> {
    match cfg.level.as_str() {
        "ape" | "cae" => {
            let p = &cfg.params;
            Box::new(ApeModelDetailed {
                int_alu_latency: p.get("int_alu_latency").copied().unwrap_or(1),
                int_mul_latency: p.get("int_mul_latency").copied().unwrap_or(3),
                int_div_latency: p.get("int_div_latency").copied().unwrap_or(12),
                fp_alu_latency: p.get("fp_alu_latency").copied().unwrap_or(4),
                fp_mul_latency: p.get("fp_mul_latency").copied().unwrap_or(5),
                fp_div_latency: p.get("fp_div_latency").copied().unwrap_or(15),
                load_latency: p.get("load_latency").copied().unwrap_or(4),
                store_latency: p.get("store_latency").copied().unwrap_or(1),
                branch_penalty: p.get("branch_penalty").copied().unwrap_or(10),
                l1_latency: p.get("l1_latency").copied().unwrap_or(3),
                l2_latency: p.get("l2_latency").copied().unwrap_or(12),
                l3_latency: p.get("l3_latency").copied().unwrap_or(40),
                dram_latency: p.get("dram_latency").copied().unwrap_or(200),
            })
        }
        _ => Box::new(FeModel),
    }
}

/// Run a simulation from Python (no plugins).
#[pyfunction]
#[pyo3(signature = (platform, binary, max_cycles=1_000_000, timing=None))]
fn run_simulation(
    platform: PyPlatformConfig,
    binary: String,
    max_cycles: u64,
    timing: Option<PyTimingModel>,
) -> PyResult<String> {
    let model = timing.as_ref().map_or_else(
        || Box::new(FeModel) as Box<dyn TimingModel>,
        |t| build_timing_model(t),
    );
    let mut sim = Simulation::new(platform.inner, binary, model);
    let results = sim
        .run(max_cycles)
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
    Ok(results.to_json())
}

/// Run AArch64 SE mode with plugins enabled via a PluginManager.
///
/// ```python
/// pm = PluginManager()
/// pm.enable("plugin.trace.insn-count")
/// timing = TimingModelConfig("ape", int_mul_latency=3)
/// result = run_se("binary", ["binary", "-c", "echo hi"], [], 1000, pm, timing)
/// ```
#[pyfunction]
#[pyo3(signature = (binary, argv, envp, max_insns, plugin_manager=None, timing=None))]
fn run_se(
    binary: String,
    argv: Vec<String>,
    envp: Vec<String>,
    max_insns: u64,
    plugin_manager: Option<&PyPluginManager>,
    timing: Option<PyTimingModel>,
) -> PyResult<PySeResult> {
    let argv_refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    let envp_refs: Vec<&str> = envp.iter().map(|s| s.as_str()).collect();

    // Build a PluginRegistry from the enabled plugins
    let mut comp_reg = ComponentRegistry::new();
    register_builtins(&mut comp_reg);

    let mut plugin_reg = PluginRegistry::new();
    let mut adapters: Vec<PluginComponentAdapter> = Vec::new();

    if let Some(pm) = plugin_manager {
        for fqn in &pm.enabled {
            if let Some(comp) = comp_reg.create(fqn) {
                let raw = Box::into_raw(comp);
                // SAFETY: register_builtins only creates PluginComponentAdapter
                let mut adapter = unsafe { *Box::from_raw(raw as *mut PluginComponentAdapter) };
                adapter.install(&mut plugin_reg, &PluginArgs::new());
                adapters.push(adapter);
            }
        }
    }

    let plugins = if adapters.is_empty() {
        None
    } else {
        Some(&plugin_reg)
    };

    let mut model: Box<dyn TimingModel> = timing.as_ref().map_or_else(
        || Box::new(FeModel) as Box<dyn TimingModel>,
        |t| build_timing_model(t),
    );

    let mut backend = helm_engine::ExecBackend::interpretive();
    let result = helm_engine::run_aarch64_se_timed(
        &binary,
        &argv_refs,
        &envp_refs,
        max_insns,
        model.as_mut(),
        &mut backend,
        None,
        plugins,
        None,
    )
    .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;

    for adapter in &mut adapters {
        adapter.atexit();
    }

    Ok(PySeResult {
        exit_code: result.exit_code,
        instructions_executed: result.instructions_executed,
        virtual_cycles: result.virtual_cycles,
        hit_limit: result.hit_limit,
    })
}

// ---------------------------------------------------------------------------
// SE result wrapper
// ---------------------------------------------------------------------------

/// Result of an SE-mode run, returned to Python.
#[pyclass(name = "SeResult")]
struct PySeResult {
    #[pyo3(get)]
    exit_code: u64,
    #[pyo3(get)]
    instructions_executed: u64,
    #[pyo3(get)]
    virtual_cycles: u64,
    #[pyo3(get)]
    hit_limit: bool,
}

#[pymethods]
impl PySeResult {
    #[getter]
    fn ipc(&self) -> f64 {
        if self.virtual_cycles == 0 {
            return 0.0;
        }
        self.instructions_executed as f64 / self.virtual_cycles as f64
    }

    fn __repr__(&self) -> String {
        let ipc = if self.virtual_cycles > 0 {
            self.instructions_executed as f64 / self.virtual_cycles as f64
        } else {
            0.0
        };
        format!(
            "SeResult(exit_code={}, instructions={}, cycles={}, IPC={:.3}, hit_limit={})",
            self.exit_code, self.instructions_executed, self.virtual_cycles, ipc, self.hit_limit
        )
    }
}

// ---------------------------------------------------------------------------
// Plugin management
// ---------------------------------------------------------------------------

/// Python-visible plugin manager backed by `ComponentRegistry`.
#[pyclass(name = "PluginManager")]
struct PyPluginManager {
    registry: ComponentRegistry,
    enabled: Vec<String>,
}

#[pymethods]
impl PyPluginManager {
    #[new]
    fn new() -> Self {
        let mut registry = ComponentRegistry::new();
        register_builtins(&mut registry);
        Self {
            registry,
            enabled: Vec::new(),
        }
    }

    /// List all available plugin type names.
    fn available(&self) -> Vec<String> {
        self.registry.list().into_iter().map(String::from).collect()
    }

    /// List plugin type names that implement a given interface
    /// (e.g. "trace", "memory", "profiling").
    fn with_interface(&self, interface: &str) -> Vec<String> {
        self.registry
            .types_with_interface(interface)
            .into_iter()
            .map(String::from)
            .collect()
    }

    /// Enable a plugin by type name (e.g. "plugin.trace.execlog").
    /// Returns True if the plugin was found and enabled.
    fn enable(&mut self, type_name: String) -> bool {
        if self.registry.create(&type_name).is_some() {
            if !self.enabled.contains(&type_name) {
                self.enabled.push(type_name);
            }
            true
        } else {
            false
        }
    }

    /// Disable a previously enabled plugin.
    fn disable(&mut self, type_name: &str) {
        self.enabled.retain(|n| n != type_name);
    }

    /// Return the list of currently enabled plugin type names.
    fn enabled_plugins(&self) -> Vec<String> {
        self.enabled.clone()
    }
}

// ---------------------------------------------------------------------------
// Module registration
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Suspendable session wrappers
// ---------------------------------------------------------------------------

/// Result of a session run() call, returned to Python.
#[pyclass(name = "StopResult")]
struct PyStopResult {
    #[pyo3(get)]
    reason: String,
    #[pyo3(get)]
    pc: u64,
    #[pyo3(get)]
    exit_code: u64,
    #[pyo3(get)]
    message: String,
}

#[pymethods]
impl PyStopResult {
    fn __repr__(&self) -> String {
        match self.reason.as_str() {
            "exited" => format!("StopResult(EXITED, code={})", self.exit_code),
            "breakpoint" => format!("StopResult(BREAKPOINT, pc={:#x})", self.pc),
            "error" => format!("StopResult(ERROR, {:?})", self.message),
            _ => format!("StopResult(INSN_LIMIT, pc={:#x})", self.pc),
        }
    }
}

fn stop_reason_to_py(reason: &helm_engine::StopReason, pc: u64) -> PyStopResult {
    match reason {
        helm_engine::StopReason::InsnLimit => PyStopResult {
            reason: "insn_limit".into(),
            pc,
            exit_code: 0,
            message: String::new(),
        },
        helm_engine::StopReason::Breakpoint { pc: bp } => PyStopResult {
            reason: "breakpoint".into(),
            pc: *bp,
            exit_code: 0,
            message: String::new(),
        },
        helm_engine::StopReason::Exited { code } => PyStopResult {
            reason: "exited".into(),
            pc,
            exit_code: *code,
            message: String::new(),
        },
        helm_engine::StopReason::Error(msg) => PyStopResult {
            reason: "error".into(),
            pc,
            exit_code: 0,
            message: msg.clone(),
        },
    }
}

/// Suspendable SE-mode session exposed to Python.
#[pyclass(name = "SeSession", unsendable)]
struct PySeSession {
    inner: helm_engine::SeSession,
}

#[pymethods]
impl PySeSession {
    #[new]
    #[pyo3(signature = (binary, argv, envp=None))]
    fn new(binary: &str, argv: Vec<String>, envp: Option<Vec<String>>) -> PyResult<Self> {
        let envp = envp.unwrap_or_else(|| {
            vec![
                "HOME=/tmp".into(),
                "TERM=dumb".into(),
                "PATH=/usr/bin:/bin".into(),
            ]
        });
        let argv_refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        let envp_refs: Vec<&str> = envp.iter().map(|s| s.as_str()).collect();
        let inner = helm_engine::SeSession::new(binary, &argv_refs, &envp_refs)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(Self { inner })
    }

    fn run(&mut self, max_insns: u64) -> PyStopResult {
        let reason = self.inner.run(max_insns);
        stop_reason_to_py(&reason, self.inner.pc())
    }

    fn run_until_pc(&mut self, target: u64, max_insns: u64) -> PyStopResult {
        let reason = self.inner.run_until_pc(target, max_insns);
        stop_reason_to_py(&reason, self.inner.pc())
    }

    fn run_until_symbol(&mut self, sym: &str, max_insns: u64) -> PyStopResult {
        let reason = self.inner.run_until_symbol(sym, max_insns);
        stop_reason_to_py(&reason, self.inner.pc())
    }

    fn add_plugin(&mut self, name: &str, args: &str) -> bool {
        self.inner.add_plugin(name, args)
    }

    #[getter]
    fn pc(&self) -> u64 {
        self.inner.pc()
    }

    #[getter]
    fn insn_count(&self) -> u64 {
        self.inner.insn_count()
    }

    #[getter]
    fn virtual_cycles(&self) -> u64 {
        self.inner.virtual_cycles()
    }

    #[getter]
    fn has_exited(&self) -> bool {
        self.inner.has_exited()
    }

    #[getter]
    fn exit_code(&self) -> u64 {
        self.inner.exit_code()
    }

    fn xn(&self, n: u32) -> u64 {
        self.inner.xn(n)
    }

    fn regs(&self) -> HashMap<String, u64> {
        let mut m = HashMap::new();
        m.insert("pc".into(), self.inner.pc());
        for i in 0..31 {
            m.insert(format!("x{i}"), self.inner.xn(i));
        }
        m
    }

    fn finish(&mut self) {
        self.inner.finish();
    }
}

/// Suspendable FS-mode session exposed to Python.
#[pyclass(name = "FsSession", unsendable)]
struct PyFsSession {
    inner: helm_engine::FsSession,
}

#[pymethods]
impl PyFsSession {
    #[new]
    #[pyo3(signature = (
        kernel,
        machine="virt",
        append="",
        memory_size="256M",
        serial="stdio",
        timing="fe",
        backend="jit",
        dtb=None,
        initrd=None,
        sysmap=None,
    ))]
    fn new(
        kernel: &str,
        machine: &str,
        append: &str,
        memory_size: &str,
        serial: &str,
        timing: &str,
        backend: &str,
        dtb: Option<String>,
        initrd: Option<String>,
        sysmap: Option<String>,
    ) -> PyResult<Self> {
        let opts = helm_engine::FsOpts {
            machine: machine.to_string(),
            append: append.to_string(),
            memory_size: memory_size.to_string(),
            serial: serial.to_string(),
            timing: timing.to_string(),
            dtb,
            initrd,
            sysmap,
            backend: backend.to_string(),
            ..Default::default()
        };
        let inner = helm_engine::FsSession::new(kernel, &opts)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(Self { inner })
    }

    fn run(&mut self, max_insns: u64) -> PyStopResult {
        use helm_engine::MonitorTarget;
        let reason = self.inner.run(max_insns);
        stop_reason_to_py(&reason, self.inner.pc())
    }

    fn run_until_pc(&mut self, target: u64, max_insns: u64) -> PyStopResult {
        use helm_engine::MonitorTarget;
        let reason = self.inner.run_until_pc(target, max_insns);
        stop_reason_to_py(&reason, self.inner.pc())
    }

    fn run_forever(&mut self) -> PyStopResult {
        use helm_engine::MonitorTarget;
        let reason = self.inner.run_forever();
        stop_reason_to_py(&reason, self.inner.pc())
    }

    fn run_until_symbol(&mut self, sym: &str, max_insns: u64) -> PyStopResult {
        use helm_engine::MonitorTarget;
        let reason =
            helm_engine::fs::session::FsSession::run_until_symbol(&mut self.inner, sym, max_insns);
        stop_reason_to_py(&reason, self.inner.pc())
    }

    #[getter]
    fn pc(&self) -> u64 {
        use helm_engine::MonitorTarget;
        self.inner.pc()
    }

    #[getter]
    fn insn_count(&self) -> u64 {
        use helm_engine::MonitorTarget;
        self.inner.insn_count()
    }

    fn xn(&self, n: u32) -> u64 {
        use helm_engine::MonitorTarget;
        self.inner.xn(n)
    }

    #[getter]
    fn sp(&self) -> u64 {
        use helm_engine::MonitorTarget;
        self.inner.sp()
    }

    fn read_memory(&self, addr: u64, size: usize) -> Option<Vec<u8>> {
        use helm_engine::MonitorTarget;
        self.inner.read_memory(addr, size)
    }

    fn read_virtual(&mut self, va: u64, size: usize) -> Option<Vec<u8>> {
        self.inner.read_virtual(va, size)
    }

    fn regs(&self) -> HashMap<String, u64> {
        use helm_engine::MonitorTarget;
        let mut m = HashMap::new();
        m.insert("pc".into(), self.inner.pc());
        for i in 0..31 {
            m.insert(format!("x{i}"), self.inner.xn(i));
        }
        m.insert("sp".into(), self.inner.sp());
        m.insert("daif".into(), self.inner.daif() as u64);
        m.insert("current_el".into(), self.inner.current_el() as u64);
        m
    }

    /// Read a named system register (e.g. "sctlr_el1", "ttbr0_el1").
    fn sysreg(&self, name: &str) -> Option<u64> {
        use helm_engine::MonitorTarget;
        self.inner.sysreg(name)
    }

    #[getter]
    fn virtual_cycles(&self) -> u64 {
        use helm_engine::MonitorTarget;
        self.inner.virtual_cycles()
    }

    #[getter]
    fn current_el(&self) -> u8 {
        use helm_engine::MonitorTarget;
        self.inner.current_el()
    }

    #[getter]
    fn daif(&self) -> u32 {
        use helm_engine::MonitorTarget;
        self.inner.daif()
    }

    #[getter]
    fn irq_count(&self) -> u64 {
        use helm_engine::MonitorTarget;
        self.inner.irq_count()
    }

    #[getter]
    fn has_exited(&self) -> bool {
        use helm_engine::MonitorTarget;
        self.inner.has_exited()
    }

    /// Return session statistics as a dict.
    fn stats(&self) -> HashMap<String, u64> {
        let s = self.inner.stats();
        let mut m = HashMap::new();
        m.insert("insn_count".into(), s.insn_count);
        m.insert("virtual_cycles".into(), s.virtual_cycles);
        m.insert("irq_count".into(), s.irq_count);
        m.insert("isa_skip_count".into(), s.isa_skip_count);
        m
    }
}

/// The native Python module (imported as `helm._helm_core`).
///
/// When used as an extension module, PyO3 generates the init function.
/// When embedded, the binary calls `pyo3::append_to_inittab!(_helm_core)`
/// before `Python::with_gil()`.
#[pymodule]
pub fn _helm_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyCacheConfig>()?;
    m.add_class::<PyBranchPredictorConfig>()?;
    m.add_class::<PyCoreConfig>()?;
    m.add_class::<PyMemoryConfig>()?;
    m.add_class::<PyPlatformConfig>()?;
    m.add_class::<PyTimingModel>()?;
    m.add_class::<PyPluginManager>()?;
    m.add_class::<PySeResult>()?;
    m.add_class::<PyStopResult>()?;
    m.add_class::<PySeSession>()?;
    m.add_class::<PyFsSession>()?;
    m.add_function(wrap_pyfunction!(run_simulation, m)?)?;
    m.add_function(wrap_pyfunction!(run_se, m)?)?;
    m.add_function(wrap_pyfunction!(list_platforms, m)?)?;
    Ok(())
}

/// Return the list of built-in FS platform names.
#[pyfunction]
fn list_platforms() -> Vec<String> {
    vec![
        "virt".into(),
        "arm-virt".into(),
        "realview-pb".into(),
        "realview".into(),
        "rpi3".into(),
        "raspi3".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_timing_model_fe_returns_latency_one() {
        let cfg = PyTimingModel {
            level: "fe".to_string(),
            params: HashMap::new(),
        };
        let mut model = build_timing_model(&cfg);
        assert_eq!(
            model.instruction_latency_for_class(helm_timing::InsnClass::IntAlu),
            1
        );
    }

    #[test]
    fn build_timing_model_ape_uses_custom_params() {
        let mut params = HashMap::new();
        params.insert("int_mul_latency".to_string(), 7);
        let cfg = PyTimingModel {
            level: "ape".to_string(),
            params,
        };
        let mut model = build_timing_model(&cfg);
        assert_eq!(
            model.instruction_latency_for_class(helm_timing::InsnClass::IntMul),
            7
        );
    }

    #[test]
    fn build_timing_model_unknown_level_falls_back_to_fe() {
        let cfg = PyTimingModel {
            level: "unknown".to_string(),
            params: HashMap::new(),
        };
        let mut model = build_timing_model(&cfg);
        assert_eq!(
            model.instruction_latency_for_class(helm_timing::InsnClass::Load),
            1
        );
    }
}

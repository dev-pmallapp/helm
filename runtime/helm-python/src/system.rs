#![allow(missing_docs)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use helm_debug::Inspectable;
use helm_devices::DeviceParams;
#[cfg(test)]
use helm_engine::{build_simulator, Isa};
use helm_engine::{ExecMode, HelmSim, StopReason, TimingChoice, TimingMemModelConfig};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};

use crate::simobject::SimObject;
use crate::spy::HelmSpy;

// ── Shared parsing helpers ───────────────────────────────────────────────────

pub(crate) fn parse_mode(s: &str) -> PyResult<ExecMode> {
    match s {
        "se" | "syscall" => Ok(ExecMode::Syscall),
        "functional" | "fe" => Ok(ExecMode::Functional),
        "fs" | "system" => Ok(ExecMode::System),
        other => Err(pyo3::exceptions::PyValueError::new_err(format!(
            "unknown mode '{other}'"
        ))),
    }
}

pub(crate) fn parse_gic_version(
    s: &str,
) -> PyResult<helm_engine::platform::arm_virt::ArmVirtGicVersion> {
    match s {
        "v2" => Ok(helm_engine::platform::arm_virt::ArmVirtGicVersion::V2),
        "v3" => Ok(helm_engine::platform::arm_virt::ArmVirtGicVersion::V3),
        other => Err(pyo3::exceptions::PyValueError::new_err(format!(
            "unknown gic_version '{other}' (expected 'v2' or 'v3')"
        ))),
    }
}

pub(crate) fn parse_timing(s: &str, ipc: f64) -> PyResult<TimingChoice> {
    match s.strip_prefix("interval") {
        Some("") => Ok(TimingChoice::IntervalTiming {
            ipc,
            interval_len: 10_000,
            mem_model: TimingMemModelConfig::default(),
        }),
        Some(rest) => parse_interval_timing(rest, ipc),
        None => match s {
            "virtual" => Ok(TimingChoice::VirtualTiming { ipc }),
            "accurate" => Ok(TimingChoice::AccurateTiming),
            other => Err(pyo3::exceptions::PyValueError::new_err(format!(
                "unknown timing '{other}'"
            ))),
        },
    }
}

const CUT_POINT_HISTORY_LIMIT: usize = 8;
const SEGMENT_HISTORY_LIMIT: usize = 8;
const CHECKPOINT_HISTORY_LIMIT: usize = 8;

#[cfg(feature = "instrumentation")]
pub(crate) type NativeTriggerStateHandle = helm_debug::NativeTriggerState;

#[cfg(not(feature = "instrumentation"))]
pub(crate) struct NativeTriggerStateHandle;

#[derive(Clone, Copy)]
struct RunStartSnapshot {
    pc: u64,
    insn_count: u64,
    cycle_count: u64,
}

fn parse_interval_timing(rest: &str, ipc: f64) -> PyResult<TimingChoice> {
    let Some(params) = rest.strip_prefix(':') else {
        return Err(pyo3::exceptions::PyValueError::new_err(format!(
            "invalid interval timing suffix '{rest}'"
        )));
    };

    let mut interval_len = 10_000;
    let mut mem_model = TimingMemModelConfig::default();

    for entry in params.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((key, value)) = entry.split_once('=') else {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "invalid interval timing option '{entry}'"
            )));
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "interval_len" => interval_len = parse_u64_option(key, value)?,
            "l1d_size" => mem_model.l1d.size_bytes = parse_size_option(key, value)?,
            "l1d_assoc" => mem_model.l1d.assoc = parse_usize_option(key, value)?,
            "l1d_line" | "l1d_line_size" => {
                mem_model.l1d.line_size = parse_usize_option(key, value)?
            }
            "l2_size" => mem_model.l2.size_bytes = parse_size_option(key, value)?,
            "l2_assoc" => mem_model.l2.assoc = parse_usize_option(key, value)?,
            "l2_line" | "l2_line_size" => mem_model.l2.line_size = parse_usize_option(key, value)?,
            other => {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "unknown interval timing option '{other}'"
                )));
            }
        }
    }

    Ok(TimingChoice::IntervalTiming {
        ipc,
        interval_len,
        mem_model,
    })
}

fn parse_u64_option(key: &str, value: &str) -> PyResult<u64> {
    value.parse::<u64>().map_err(|e| {
        pyo3::exceptions::PyValueError::new_err(format!(
            "invalid interval timing value for {key}: '{value}' ({e})"
        ))
    })
}

fn parse_usize_option(key: &str, value: &str) -> PyResult<usize> {
    let parsed = value.parse::<usize>().map_err(|e| {
        pyo3::exceptions::PyValueError::new_err(format!(
            "invalid interval timing value for {key}: '{value}' ({e})"
        ))
    })?;
    Ok(parsed.max(1))
}

fn parse_size_option(key: &str, value: &str) -> PyResult<usize> {
    let bytes = DeviceParams::parse_memory_size(value).map_err(|e| {
        pyo3::exceptions::PyValueError::new_err(format!(
            "invalid interval timing size for {key}: '{value}' ({e})"
        ))
    })?;
    usize::try_from(bytes).map_err(|_| {
        pyo3::exceptions::PyValueError::new_err(format!(
            "interval timing size for {key} exceeds host usize capacity: '{value}'"
        ))
    })
}

fn warn_deprecated_api(py: Python<'_>, message: &str) -> PyResult<()> {
    let warnings = py.import_bound("warnings")?;
    warnings.call_method1(
        "warn",
        (
            message,
            py.get_type_bound::<pyo3::exceptions::PyDeprecationWarning>(),
        ),
    )?;
    Ok(())
}

/// Top-level simulation container.
///
/// Before `instantiate()`: holds config fields (timing, mode, ipc).
/// After `instantiate()`: wraps a live `HelmSim` with register access and run().
#[pyclass(name = "System", extends = SimObject)]
pub struct HelmSystem {
    #[pyo3(get, set)]
    pub timing: String,
    #[pyo3(get, set)]
    pub mode: String,
    #[pyo3(get, set)]
    pub ipc: f64,
    #[pyo3(get, set)]
    pub num_cpus: usize,
    #[pyo3(get, set)]
    pub gic_version: String,

    pub(crate) sim: Option<HelmSim>,
    pub(crate) exited: bool,
    pub(crate) exit_code_val: i32,
    pub(crate) plugins: Vec<Box<dyn helm_engine::helm_plugin::api::HelmPlugin>>,
    #[cfg_attr(not(feature = "instrumentation"), allow(dead_code))]
    pub(crate) breakpoints: Option<Arc<Mutex<helm_debug::BreakpointEngine>>>,
    #[cfg_attr(not(feature = "instrumentation"), allow(dead_code))]
    pub(crate) watchpoints: Option<Arc<Mutex<helm_debug::WatchpointEngine>>>,
    #[cfg_attr(not(feature = "instrumentation"), allow(dead_code))]
    pub(crate) native_trigger_state: Option<Arc<Mutex<NativeTriggerStateHandle>>>,
    pub(crate) last_stop_default: helm_debug::RuntimeStopState,
    pub(crate) last_stop_by_runtime: HashMap<usize, helm_debug::RuntimeStopState>,
    pub(crate) cut_points_default: Vec<helm_debug::ReplayCutPoint>,
    pub(crate) cut_points_by_runtime: HashMap<usize, Vec<helm_debug::ReplayCutPoint>>,
    pub(crate) segment_history_default: Vec<helm_debug::ReplaySegment>,
    pub(crate) segment_history_by_runtime: HashMap<usize, Vec<helm_debug::ReplaySegment>>,
    pub(crate) checkpoint_history_default: Vec<helm_debug::ReplayCheckpointRecord>,
    pub(crate) checkpoint_history_by_runtime:
        HashMap<usize, Vec<helm_debug::ReplayCheckpointRecord>>,
}

#[pymethods]
impl HelmSystem {
    #[new]
    #[pyo3(signature = (name, *, timing = "virtual", mode = "se", ipc = 4.0, num_cpus = 1, gic_version = "v3"))]
    fn new(
        name: &str,
        timing: &str,
        mode: &str,
        ipc: f64,
        num_cpus: usize,
        gic_version: &str,
    ) -> (Self, SimObject) {
        (
            HelmSystem {
                timing: timing.into(),
                mode: mode.into(),
                ipc,
                num_cpus: num_cpus.max(1),
                gic_version: gic_version.into(),
                sim: None,
                exited: false,
                exit_code_val: 0,
                plugins: Vec::new(),
                breakpoints: None,
                watchpoints: None,
                native_trigger_state: None,
                last_stop_default: helm_debug::RuntimeStopState::default(),
                last_stop_by_runtime: HashMap::new(),
                cut_points_default: Vec::new(),
                cut_points_by_runtime: HashMap::new(),
                segment_history_default: Vec::new(),
                segment_history_by_runtime: HashMap::new(),
                checkpoint_history_default: Vec::new(),
                checkpoint_history_by_runtime: HashMap::new(),
            },
            SimObject::new(name),
        )
    }

    /// Freeze config and create all Rust simulation objects.
    ///
    /// After building the simulator, wires back-references from child
    /// device pyclasses (Cpu, GicV2, Pl011) so that live state inspection
    /// methods like `system.cpu.xn(n)` work.
    fn instantiate(slf: Py<Self>, py: Python<'_>) -> PyResult<()> {
        // Phase 1: build the simulator (needs mutable borrow).
        {
            let system_ref = slf.borrow_mut(py);
            crate::instantiate::instantiate_system(system_ref, py)?;
        }
        // Phase 2: wire back-references (needs Py<HelmSystem> handle,
        // mutable borrow released by the block above).
        crate::instantiate::wire_device_back_refs(&slf, py)
    }

    // ── Simulation control ───────────────────────────────────────────────────

    /// Run up to `max_insns` guest instructions.
    fn run(&mut self, max_insns: u64) -> String {
        if self.exited {
            return format!("exit:{}", self.exit_code_val);
        }
        self.clear_native_trigger_hits();
        let start = self.capture_run_start();
        let sim = match self.sim.as_mut() {
            Some(s) => s,
            None => {
                let stop = helm_debug::RuntimeStopState {
                    stop: helm_debug::RuntimeStopView::ErrorNotInstantiated,
                    last_native_hit: None,
                };
                let rendered = stop.render();
                self.record_stop_state(stop);
                return rendered;
            }
        };
        let stop = match sim.run(max_insns) {
            StopReason::Exit { code } => {
                self.exited = true;
                self.exit_code_val = code;
                helm_debug::RuntimeStopView::Exit { code }
            }
            StopReason::Breakpoint => {
                helm_debug::RuntimeStopView::JitBreakpoint { pc: Some(sim.pc()) }
            }
            other => Self::stop_reason_view(&other),
        };
        let stop = helm_debug::RuntimeStopState {
            stop,
            last_native_hit: self.native_trigger_hit_snapshot(None),
        };
        let rendered = stop.render();
        self.record_execution_segment("run", max_insns, start, &stop);
        self.record_stop_state(stop);
        rendered
    }

    /// Enable or disable the JIT backend.
    #[cfg(feature = "jit")]
    fn set_jit(&mut self, enabled: bool) -> PyResult<()> {
        let sim = self.require_sim()?;
        sim.set_jit(enabled);
        Ok(())
    }

    /// Run up to `max_insns` using the JIT backend.
    #[cfg(feature = "jit")]
    fn run_jit(&mut self, max_insns: u64) -> String {
        if self.exited {
            return format!("exit:{}", self.exit_code_val);
        }
        self.clear_native_trigger_hits();
        let start = self.capture_run_start();
        let sim = match self.sim.as_mut() {
            Some(s) => s,
            None => {
                let stop = helm_debug::RuntimeStopState {
                    stop: helm_debug::RuntimeStopView::ErrorNotInstantiated,
                    last_native_hit: None,
                };
                let rendered = stop.render();
                self.record_stop_state(stop);
                return rendered;
            }
        };
        let stop = match sim.run_jit(max_insns) {
            StopReason::Exit { code } => {
                self.exited = true;
                self.exit_code_val = code;
                helm_debug::RuntimeStopView::Exit { code }
            }
            StopReason::Breakpoint => {
                helm_debug::RuntimeStopView::JitBreakpoint { pc: Some(sim.pc()) }
            }
            other => Self::stop_reason_view(&other),
        };
        let stop = helm_debug::RuntimeStopState {
            stop,
            last_native_hit: self.native_trigger_hit_snapshot(None),
        };
        let rendered = stop.render();
        self.record_execution_segment("run_jit", max_insns, start, &stop);
        self.record_stop_state(stop);
        rendered
    }

    // ── ELF / Kernel loading ─────────────────────────────────────────────────

    // ── JIT debug/trace ──────────────────────────────────────────────────

    /// Add a JIT breakpoint at `pc`. The JIT loop will stop with "breakpoint"
    /// when it reaches this address.
    #[cfg(feature = "jit")]
    fn add_jit_breakpoint(&mut self, pc: u64) -> PyResult<bool> {
        let sim = self.require_sim()?;
        Ok(sim.add_jit_breakpoint(pc))
    }

    /// Remove a JIT breakpoint at `pc`.
    #[cfg(feature = "jit")]
    fn remove_jit_breakpoint(&mut self, pc: u64) -> PyResult<bool> {
        let sim = self.require_sim()?;
        Ok(sim.remove_jit_breakpoint(pc))
    }

    /// Remove all JIT breakpoints.
    #[cfg(feature = "jit")]
    fn clear_jit_breakpoints(&mut self) -> PyResult<()> {
        let sim = self.require_sim()?;
        sim.clear_jit_breakpoints();
        Ok(())
    }

    /// Set a JIT trace window. Events are only emitted when the window is
    /// active. All parameters are optional.
    ///
    /// Args:
    ///     start_pc: Start emitting events when PC reaches this address.
    ///     stop_pc: Stop emitting events when PC reaches this address.
    ///     start_insn: Start after this many guest instructions retired.
    ///     stop_insn: Stop after this many guest instructions retired.
    ///     max_events: Maximum number of block-execute events to emit.
    #[cfg(feature = "jit")]
    #[pyo3(signature = (start_pc=None, stop_pc=None, start_insn=None, stop_insn=None, max_events=None))]
    fn set_jit_trace_window(
        &mut self,
        start_pc: Option<u64>,
        stop_pc: Option<u64>,
        start_insn: Option<u64>,
        stop_insn: Option<u64>,
        max_events: Option<u64>,
    ) -> PyResult<()> {
        let sim = self.require_sim()?;
        sim.set_jit_trace_window(helm_engine::JitTraceWindow {
            start_pc,
            start_insn,
            stop_pc,
            stop_insn,
            max_events,
        });
        Ok(())
    }

    /// Remove the JIT trace window (events always pass through).
    #[cfg(feature = "jit")]
    fn clear_jit_trace_window(&mut self) -> PyResult<()> {
        let sim = self.require_sim()?;
        sim.clear_jit_trace_window();
        Ok(())
    }

    /// Start a blocking GDB Remote Serial Protocol server on localhost.
    ///
    /// This serves the current simulator state directly. The initial bridge
    /// supports register/memory access, single-step, continue, and software
    /// breakpoint packets backed by the simulator's JIT breakpoint path.
    #[pyo3(signature = (port=9001))]
    fn serve_gdb(&mut self, port: u16) -> PyResult<()> {
        let sim = self.require_sim()?;
        sim.serve_gdb(port).map_err(crate::errors::debug_error)
    }

    /// List debug-connection-capable runtimes in the current machine/session.
    fn debug_connections(&self) -> Vec<(usize, String, String, Option<String>, String, u8, bool)> {
        self.sim.as_ref().map_or_else(Vec::new, |sim| {
            sim.debug_connections()
                .into_iter()
                .map(|conn| {
                    (
                        conn.runtime_id,
                        conn.label,
                        conn.arch,
                        conn.mode,
                        conn.role,
                        conn.domain,
                        conn.active,
                    )
                })
                .collect()
        })
    }

    /// Return the active debug connection/runtime, if any.
    fn active_debug_connection<'py>(&self, py: Python<'py>) -> Bound<'py, PyDict> {
        let out = PyDict::new_bound(py);
        if let Some(sim) = &self.sim {
            if let Some(conn) = sim.active_debug_connection() {
                let _ = out.set_item("runtime_id", conn.runtime_id);
                let _ = out.set_item("label", conn.label);
                let _ = out.set_item("arch", conn.arch);
                let _ = out.set_item("mode", conn.mode);
                let _ = out.set_item("role", conn.role);
                let _ = out.set_item("domain", conn.domain);
                let _ = out.set_item("active", conn.active);
            }
        }
        out
    }

    /// Select which runtime the generic debug connections should target.
    fn select_debug_connection(&mut self, runtime_id: usize) -> PyResult<bool> {
        let sim = self.require_sim()?;
        Ok(sim.select_debug_connection(runtime_id))
    }

    /// Force the JIT to use interpreter fallback for every block.
    /// This enables per-instruction plugin/probe delivery at the cost of
    /// JIT performance.
    #[cfg(feature = "jit")]
    fn set_jit_force_interpreter(&mut self, force: bool) -> PyResult<()> {
        let sim = self.require_sim()?;
        sim.set_jit_force_interpreter(force);
        Ok(())
    }
    /// Load a static AArch64 ELF binary and configure SE mode.
    #[pyo3(signature = (binary, argv=None, envp=None))]
    fn load_elf(
        &mut self,
        binary: &str,
        argv: Option<Vec<String>>,
        envp: Option<Vec<String>>,
    ) -> PyResult<()> {
        let sim = self.require_sim()?;
        let argv_strings = argv.unwrap_or_else(|| {
            vec![std::path::Path::new(binary)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()]
        });
        let envp_strings = envp.unwrap_or_else(|| {
            vec![
                "HOME=/tmp".into(),
                "TERM=dumb".into(),
                "PATH=/usr/bin:/bin".into(),
                "LANG=C".into(),
                "USER=helm".into(),
            ]
        });
        let argv_refs: Vec<&str> = argv_strings.iter().map(String::as_str).collect();
        let envp_refs: Vec<&str> = envp_strings.iter().map(String::as_str).collect();

        sim.load_aarch64_elf(binary, &argv_refs, &envp_refs)
            .map_err(|err| pyo3::exceptions::PyRuntimeError::new_err(err.to_string()))
    }

    /// Load an ARM64 Linux kernel Image and configure FS mode.
    #[pyo3(signature = (kernel, dtb=None, dtb_bytes=None, initrd=None, append=None, num_cpus=1, gic_version="v3", boot_el=None))]
    fn load_kernel(
        &mut self,
        kernel: &str,
        dtb: Option<&str>,
        dtb_bytes: Option<Vec<u8>>,
        initrd: Option<&str>,
        append: Option<&str>,
        num_cpus: usize,
        gic_version: &str,
        boot_el: Option<u8>,
    ) -> PyResult<()> {
        let sim = self.require_sim()?;
        let gic_version = parse_gic_version(gic_version)?;
        match (dtb, dtb_bytes) {
            (Some(path), None) => sim
                .load_aarch64_kernel(kernel, path, initrd, append, num_cpus, gic_version, boot_el)
                .map_err(|err| pyo3::exceptions::PyRuntimeError::new_err(err.to_string())),
            (None, None) => sim
                .load_aarch64_kernel_auto_dtb(
                    kernel,
                    initrd,
                    append,
                    num_cpus,
                    gic_version,
                    boot_el,
                )
                .map_err(|err| pyo3::exceptions::PyRuntimeError::new_err(err.to_string())),
            (None, Some(bytes)) => sim
                .load_aarch64_kernel_dtb_bytes(
                    kernel,
                    &bytes,
                    initrd,
                    append,
                    num_cpus,
                    gic_version,
                    boot_el,
                )
                .map_err(|err| pyo3::exceptions::PyRuntimeError::new_err(err.to_string())),
            (Some(_), Some(_)) => Err(pyo3::exceptions::PyValueError::new_err(
                "pass either dtb or dtb_bytes, not both",
            )),
        }
    }

    /// Set the ARM CPU core model.
    fn set_cpu_model(&mut self, model: &str) -> PyResult<()> {
        let sim = self.require_sim()?;
        sim.set_cpu_model(model).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!(
                "{e}. Valid: generic, cortex-a53, cortex-a55, cortex-a73, \
                 neoverse-n1, cortex-a78, cortex-x1, cortex-a510, cortex-a710"
            ))
        })
    }

    /// Set virtual-time scale factor (default 1). Higher values speed up
    /// delay loops by advancing the tick counter faster per instruction.
    fn set_tick_scale(&mut self, scale: u64) -> PyResult<()> {
        let sim = self.require_sim()?;
        sim.set_tick_scale(scale);
        Ok(())
    }

    // ── Register access ──────────────────────────────────────────────────────

    #[getter]
    fn pc(&self) -> u64 {
        self.sim.as_ref().map_or(0, helm_engine::HelmSim::debug_pc)
    }

    #[getter]
    fn sp(&self) -> u64 {
        self.xn(31)
    }

    #[getter]
    fn current_sp(&self) -> u64 {
        self.sim
            .as_ref()
            .and_then(helm_engine::HelmSim::debug_sp)
            .unwrap_or(0)
    }

    fn xn(&self, n: usize) -> u64 {
        self.sim
            .as_ref()
            .and_then(|s| s.debug_read_gpr(n))
            .unwrap_or(0)
    }

    fn vn(&self, n: usize) -> (u64, u64) {
        self.sim
            .as_ref()
            .and_then(|s| s.debug_vn(n))
            .unwrap_or((0, 0))
    }

    #[getter]
    fn nzcv(&self) -> u32 {
        self.sim
            .as_ref()
            .and_then(helm_engine::HelmSim::debug_nzcv)
            .unwrap_or(0)
    }

    #[getter]
    fn current_el(&self) -> u8 {
        self.sim
            .as_ref()
            .and_then(helm_engine::HelmSim::debug_current_el)
            .unwrap_or(0)
    }

    #[getter]
    fn daif(&self) -> u32 {
        self.sim
            .as_ref()
            .and_then(helm_engine::HelmSim::debug_daif)
            .unwrap_or(0)
    }

    #[getter]
    fn esr_el1(&self) -> u32 {
        self.sim
            .as_ref()
            .and_then(|s| s.a64_state())
            .map_or(0, |s| s.esr_el1)
    }

    #[getter]
    fn far_el1(&self) -> u64 {
        self.sim
            .as_ref()
            .and_then(|s| s.a64_state())
            .map_or(0, |s| s.far_el1)
    }

    #[getter]
    fn elr_el1(&self) -> u64 {
        self.sim
            .as_ref()
            .and_then(|s| s.a64_state())
            .map_or(0, |s| s.elr_el1)
    }

    // ── Counters and status ──────────────────────────────────────────────────

    #[getter]
    fn insn_count(&self) -> u64 {
        self.sim.as_ref().map_or(0, |s| s.insns_retired())
    }

    /// Return a small statistics dictionary for end-of-run reporting.
    fn stats(&self, py: Python<'_>) -> pyo3::PyObject {
        #[allow(deprecated)]
        let d = PyDict::new_bound(py);
        let insn_count = self.sim.as_ref().map_or(0, |s| s.insns_retired());
        let tick_count = self.sim.as_ref().map_or(0, |s| s.current_cycles());
        let jit_enabled = self.sim.as_ref().is_some_and(|s| s.jit_enabled());
        let jit_stats = self
            .sim
            .as_ref()
            .map_or_else(helm_engine::JitPerfStats::default, |s| s.jit_perf_stats());
        let ipc = if tick_count == 0 {
            0.0
        } else {
            insn_count as f64 / tick_count as f64
        };
        let _ = d.set_item("insn_count", insn_count);
        let _ = d.set_item("tick_count", tick_count);
        let _ = d.set_item("virtual_cycles", tick_count);
        let _ = d.set_item("sim_freq", 1_000_000_000u64);
        let _ = d.set_item("ipc", ipc);
        let _ = d.set_item("jit_enabled", jit_enabled);
        let _ = d.set_item("jit_block_cache_hits", jit_stats.block_cache_hits);
        let _ = d.set_item("jit_block_cache_misses", jit_stats.block_cache_misses);
        let _ = d.set_item("jit_blocks_compiled", jit_stats.blocks_compiled);
        let _ = d.set_item("jit_compiled_guest_insns", jit_stats.compiled_guest_insns);
        let _ = d.set_item("jit_blocks_executed", jit_stats.blocks_executed);
        let _ = d.set_item("jit_traces_compiled", jit_stats.traces_compiled);
        let _ = d.set_item("jit_trace_guest_insns", jit_stats.trace_guest_insns);
        let _ = d.set_item("jit_traces_executed", jit_stats.traces_executed);
        let _ = d.set_item("jit_trace_cache_hits", jit_stats.trace_cache_hits);
        let _ = d.set_item("jit_trace_cache_misses", jit_stats.trace_cache_misses);
        let _ = d.set_item("jit_trace_guard_exits", jit_stats.trace_guard_exits);
        let _ = d.set_item("jit_trace_retired", jit_stats.trace_retired);
        let _ = d.set_item("jit_fallback_count", jit_stats.fallback_count);
        let _ = d.set_item("jit_fallback_insns", jit_stats.fallback_insns);
        let _ = d.set_item(
            "jit_unsupported_block_starts",
            jit_stats.unsupported_block_starts,
        );
        let _ = d.set_item("jit_cache_entries", jit_stats.cache_entries);
        let _ = d.set_item("jit_trace_cache_entries", jit_stats.trace_cache_entries);
        let _ = d.set_item("jit_cache_promotions", jit_stats.cache_promotions);
        let _ = d.set_item("jit_cache_evictions", jit_stats.cache_evictions);
        let (user_stage2_events, user_stage2_repeats) = self
            .sim
            .as_ref()
            .and_then(|s| s.user_stage2_insn_abort_stats())
            .unwrap_or((0, 0));
        let mmu_stats = self.sim.as_ref().and_then(|s| s.aarch64_mmu_stats());
        let _ = d.set_item("user_stage2_insn_abort_events", user_stage2_events);
        let _ = d.set_item("user_stage2_insn_abort_repeats", user_stage2_repeats);
        let _ = d.set_item("mmu_tlb_hits", mmu_stats.map_or(0, |s| s.hits));
        let _ = d.set_item("mmu_tlb_misses", mmu_stats.map_or(0, |s| s.misses));
        let _ = d.set_item("mmu_stage1_walks", mmu_stats.map_or(0, |s| s.stage1_walks));
        let _ = d.set_item("mmu_stage2_walks", mmu_stats.map_or(0, |s| s.stage2_walks));
        #[allow(deprecated)]
        let unsupported = PyDict::new_bound(py);
        for (opcode, count) in jit_stats.unsupported_opcodes {
            let _ = unsupported.set_item(opcode, count);
        }
        let _ = d.set_item("jit_unsupported_opcodes", unsupported);
        d.into()
    }

    #[getter]
    fn current_cycles(&self) -> u64 {
        self.sim.as_ref().map_or(0, |s| s.current_cycles())
    }

    #[getter]
    fn has_unimplemented_instructions(&self) -> bool {
        self.sim
            .as_ref()
            .map_or(false, |s| s.has_unimplemented_instructions())
    }

    #[getter]
    fn unimplemented_instruction_count(&self) -> usize {
        self.sim
            .as_ref()
            .map_or(0, |s| s.unimplemented_instruction_count())
    }

    #[getter]
    fn has_exited(&self) -> bool {
        self.exited
    }

    #[getter]
    fn exit_code(&self) -> i32 {
        self.exit_code_val
    }

    // ── Plugins ──────────────────────────────────────────────────────────────

    /// Install a built-in plugin by name.
    #[pyo3(signature = (name, args=""))]
    fn add_plugin(&mut self, py: Python<'_>, name: &str, args: &str) -> PyResult<()> {
        use helm_engine::helm_plugin::api::{HelmPlugin, HelmPluginArgs};

        warn_deprecated_api(
            py,
            "add_plugin() is deprecated, use system.observe() API instead",
        )?;

        let pargs = HelmPluginArgs::parse(args);
        let sim = self.require_sim()?;
        let reg = sim.plugins_mut();

        let mut plugin: Box<dyn HelmPlugin> = match name {
            "stub-tracer" => Box::new(helm_engine::helm_plugin::builtins::debug::StubTracer::new()),
            "insn-count" => Box::new(helm_engine::helm_plugin::builtins::trace::InsnCount::new()),
            "syscall-trace" => {
                Box::new(helm_engine::helm_plugin::builtins::trace::SyscallTrace::new())
            }
            "hotblocks" => Box::new(helm_engine::helm_plugin::builtins::trace::HotBlocks::new()),
            "howvec" => Box::new(helm_engine::helm_plugin::builtins::trace::HowVec::new()),
            "execlog" => Box::new(helm_engine::helm_plugin::builtins::trace::ExecLog::new()),
            "fault-detect" => {
                Box::new(helm_engine::helm_plugin::builtins::debug::FaultDetect::new())
            }
            "pc-trace" => Box::new(helm_engine::helm_plugin::builtins::debug::PcTrace::new()),
            "hvc-trace" => Box::new(helm_engine::helm_plugin::builtins::debug::HvcTrace::new()),
            "register-dump" => {
                Box::new(helm_engine::helm_plugin::builtins::debug::RegisterDump::new())
            }
            "trace-window-fault" => {
                Box::new(helm_engine::helm_plugin::builtins::debug::TraceWindowFault::new())
            }
            "cache" => Box::new(helm_engine::helm_plugin::builtins::memory::CacheSim::new()),
            "mem-trace" => Box::new(helm_engine::helm_plugin::builtins::memory::MemTrace::new()),
            "branch-trace" => {
                Box::new(helm_engine::helm_plugin::builtins::trace::BranchTrace::new())
            }
            "watchpoint" => Box::new(helm_engine::helm_plugin::builtins::debug::Watchpoint::new()),
            "jit-execlog" => Box::new(helm_engine::helm_plugin::builtins::trace::JitExecLog::new()),
            other => {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "unknown plugin '{other}'"
                )))
            }
        };
        plugin.install(reg, &pargs);
        self.plugins.push(plugin);
        Ok(())
    }

    /// Create an observation session (backward-compat — prefer HelmSpy standalone).
    #[pyo3(signature = (
        cache_l1d_size=None,
        cache_l1d_ways=8,
        cache_l1d_line=64,
        predictor=None,
        predictor_bits=10,
        predictor_table_bits=None,
        start_insn=None,
        end_insn=None,
        filter_pc_start=None,
        filter_pc_end=None,
        filter_addr_start=None,
        filter_addr_end=None,
    ))]
    fn spy(
        slf: Py<Self>,
        py: Python<'_>,
        cache_l1d_size: Option<usize>,
        cache_l1d_ways: usize,
        cache_l1d_line: usize,
        predictor: Option<&str>,
        predictor_bits: u8,
        predictor_table_bits: Option<u8>,
        start_insn: Option<u64>,
        end_insn: Option<u64>,
        filter_pc_start: Option<u64>,
        filter_pc_end: Option<u64>,
        filter_addr_start: Option<u64>,
        filter_addr_end: Option<u64>,
    ) -> PyResult<HelmSpy> {
        warn_deprecated_api(
            py,
            "spy() is deprecated, use helm.HelmSpy(system, ...) or system.observe() instead",
        )?;
        let mut system = slf.borrow_mut(py);
        let sim = system.require_sim()?;
        crate::spy::build_spy_session(
            sim,
            cache_l1d_size,
            cache_l1d_ways,
            cache_l1d_line,
            predictor,
            predictor_bits,
            predictor_table_bits,
            start_insn,
            end_insn,
            filter_pc_start,
            filter_pc_end,
            filter_addr_start,
            filter_addr_end,
            Some(slf.clone_ref(py)),
        )
    }

    // ── Ergonomic tracing API ────────────────────────────────────────────────

    /// Legacy plugin-backed tracing helper. Prefer `HelmSpy` / `observe()`.
    #[pyo3(signature = (*, insn_count=None, pc=None, symbol=None, events=None, max=None, writes_only=false))]
    fn trace_after(
        &mut self,
        py: Python<'_>,
        insn_count: Option<u64>,
        pc: Option<u64>,
        symbol: Option<&str>,
        events: Option<Vec<String>>,
        max: Option<usize>,
        writes_only: bool,
    ) -> PyResult<()> {
        warn_deprecated_api(
            py,
            "trace_after() is deprecated legacy plugin instrumentation; prefer helm.HelmSpy(system, ...) or system.observe() for observation",
        )?;
        let sim = self.require_sim()?;

        let trigger_pc = match (insn_count, pc, symbol) {
            (Some(_), None, None) => None,
            (None, Some(addr), None) => Some(addr),
            (None, None, Some(sym)) => {
                let addr = sim.resolve_symbol(sym).ok_or_else(|| {
                    pyo3::exceptions::PyValueError::new_err(format!("symbol '{sym}' not found"))
                })?;
                Some(addr)
            }
            _ => {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "exactly one of insn_count, pc, or symbol must be specified",
                ))
            }
        };

        let events = events.unwrap_or_else(|| vec!["all".into()]);
        let want_mem = events.iter().any(|e| e == "mem" || e == "all");
        let want_branch = events.iter().any(|e| e == "branch" || e == "all");
        let want_insn = events.iter().any(|e| e == "insn" || e == "all");

        let active = Arc::new(AtomicBool::new(false));
        let max_events = max.unwrap_or(usize::MAX);

        let reg = sim.plugins_mut();

        if let Some(threshold) = insn_count {
            let flag = Arc::clone(&active);
            reg.on_timer(
                1,
                Box::new(move |_vcpu, count| {
                    if count >= threshold && !flag.load(Ordering::Relaxed) {
                        flag.store(true, Ordering::Relaxed);
                        eprintln!("[trace_after] activated at insn_count={count}");
                    }
                }),
            );
        } else if let Some(addr) = trigger_pc {
            let flag = Arc::clone(&active);
            reg.on_insn_exec(Box::new(move |_vcpu, insn| {
                if insn.pc == addr && !flag.load(Ordering::Relaxed) {
                    flag.store(true, Ordering::Relaxed);
                    eprintln!("[trace_after] activated at pc={:#x}", insn.pc);
                }
            }));
        }

        if want_mem {
            let flag = Arc::clone(&active);
            let counter = Arc::new(AtomicU64::new(0));
            let max_e = max_events as u64;
            let filter = if writes_only {
                helm_engine::helm_plugin::runtime::MemFilter::WritesOnly
            } else {
                helm_engine::helm_plugin::runtime::MemFilter::All
            };
            reg.on_mem_access(
                filter,
                Box::new(move |_vcpu, info| {
                    if !flag.load(Ordering::Relaxed) {
                        return;
                    }
                    let n = counter.fetch_add(1, Ordering::Relaxed);
                    if n >= max_e {
                        return;
                    }
                    let tag = if info.is_store { "W" } else { "R" };
                    let atomic = if info.is_atomic { " atomic" } else { "" };
                    eprintln!(
                        "[trace_after:mem] [{tag}] {:#018x} {}{}",
                        info.vaddr, info.size, atomic
                    );
                }),
            );
        }

        if want_branch {
            let flag = Arc::clone(&active);
            let counter = Arc::new(AtomicU64::new(0));
            let max_e = max_events as u64;
            reg.on_branch(Box::new(move |_vcpu, info| {
                if !flag.load(Ordering::Relaxed) {
                    return;
                }
                let n = counter.fetch_add(1, Ordering::Relaxed);
                if n >= max_e {
                    return;
                }
                let dir = if info.taken { "T" } else { "N" };
                eprintln!(
                    "[trace_after:branch] {:#018x} -> {:#018x} [{dir}] {:?}",
                    info.pc, info.target, info.kind
                );
            }));
        }

        if want_insn {
            let flag = Arc::clone(&active);
            let counter = Arc::new(AtomicU64::new(0));
            let max_e = max_events as u64;
            reg.on_insn_exec(Box::new(move |vcpu, insn| {
                if !flag.load(Ordering::Relaxed) {
                    return;
                }
                let n = counter.fetch_add(1, Ordering::Relaxed);
                if n >= max_e {
                    return;
                }
                eprintln!(
                    "[trace_after:insn] vcpu={vcpu} pc={:#018x} raw={:#010x}",
                    insn.pc, insn.raw
                );
            }));
        }

        Ok(())
    }

    /// Legacy watchpoint alias. Prefer `watchpoint()` for explicit debug intent.
    #[pyo3(signature = (addr, size=8, writes_only=true))]
    fn watch(&mut self, py: Python<'_>, addr: u64, size: u64, writes_only: bool) -> PyResult<()> {
        warn_deprecated_api(
            py,
            "watch() is deprecated; use watchpoint() for explicit debug intent",
        )?;
        self.install_watchpoint_plugin(addr, size, writes_only)
    }

    // ── Observation API (v2) ────────────────────────────────────────────────

    /// Create a HelmSpy observation session (preferred API).
    ///
    /// Returns a new HelmSpy attached to this system's probes.
    /// Equivalent to `HelmSpy(system)` but available as a method.
    #[pyo3(signature = (
        *,
        cache_l1d_size=None,
        cache_l1d_ways=8,
        cache_l1d_line=64,
        predictor=None,
        predictor_bits=10,
        predictor_table_bits=None,
        start_insn=None,
        end_insn=None,
        filter_pc_start=None,
        filter_pc_end=None,
        filter_addr_start=None,
        filter_addr_end=None,
    ))]
    fn observe(
        slf: Py<Self>,
        py: Python<'_>,
        cache_l1d_size: Option<usize>,
        cache_l1d_ways: usize,
        cache_l1d_line: usize,
        predictor: Option<&str>,
        predictor_bits: u8,
        predictor_table_bits: Option<u8>,
        start_insn: Option<u64>,
        end_insn: Option<u64>,
        filter_pc_start: Option<u64>,
        filter_pc_end: Option<u64>,
        filter_addr_start: Option<u64>,
        filter_addr_end: Option<u64>,
    ) -> PyResult<HelmSpy> {
        let mut system = slf.borrow_mut(py);
        let sim = system.require_sim()?;
        crate::spy::build_spy_session(
            sim,
            cache_l1d_size,
            cache_l1d_ways,
            cache_l1d_line,
            predictor,
            predictor_bits,
            predictor_table_bits,
            start_insn,
            end_insn,
            filter_pc_start,
            filter_pc_end,
            filter_addr_start,
            filter_addr_end,
            Some(slf.clone_ref(py)),
        )
    }

    /// Save a minimal architectural checkpoint of the active CPU state and
    /// native debug intent.
    ///
    /// This currently captures visible CPU architectural state only, not full
    /// device or machine state.
    fn save_checkpoint<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let mgr = helm_debug::CheckpointManager::new();

        #[cfg(feature = "instrumentation")]
        let debug_intent = {
            let bp_guard = self
                .breakpoints
                .as_ref()
                .map(|engine| engine.lock().unwrap());
            let wp_guard = self
                .watchpoints
                .as_ref()
                .map(|engine| engine.lock().unwrap());
            helm_debug::DebugIntentCheckpoint::capture(bp_guard.as_deref(), wp_guard.as_deref())
        };

        #[allow(unused_mut)]
        let (mut values, runtime_id, active_connection, insn_count, cycle_count): (
            Vec<(String, u64)>,
            Option<usize>,
            Option<helm_debug::DebugConnectionView>,
            u64,
            u64,
        ) = {
            let sim = self.require_sim()?;
            let values = if let Some(values) = sim.save_debug_checkpoint_values() {
                values
            } else {
                return Err(pyo3::exceptions::PyRuntimeError::new_err(
                    "checkpoint save requires an active debug connection/runtime",
                ));
            };
            let active_connection = sim.active_debug_connection();
            let runtime_id = active_connection.as_ref().map(|conn| conn.runtime_id);
            let insn_count = sim.insns_retired();
            let cycle_count = sim.current_cycles();
            (
                values,
                runtime_id,
                active_connection,
                insn_count,
                cycle_count,
            )
        };

        #[cfg(feature = "instrumentation")]
        debug_intent.append_values(&mut values);

        let refs: Vec<(&str, u64)> = values.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        let bytes = mgr.save_values(&refs);
        let record = helm_debug::ReplayCheckpointRecord::capture(
            runtime_id,
            active_connection,
            insn_count,
            cycle_count,
            bytes.clone(),
        )
        .map_err(crate::errors::debug_error)?;
        self.record_checkpoint(record);
        Ok(PyBytes::new_bound(py, &bytes))
    }

    /// Restore a checkpoint previously produced by `save_checkpoint()`.
    ///
    /// Returns the number of restored fields.
    fn restore_checkpoint(&mut self, data: &[u8]) -> PyResult<usize> {
        let sim = self.require_sim()?;
        let mgr = helm_debug::CheckpointManager::new();
        let restored = mgr
            .restore_values(data)
            .map_err(crate::errors::debug_error)?;

        if !sim.restore_debug_checkpoint_values(&restored) {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "checkpoint restore requires an active debug connection/runtime",
            ));
        }

        #[cfg(feature = "instrumentation")]
        {
            let debug_intent = helm_debug::DebugIntentCheckpoint::from_restored_values(&restored);

            if let Some(intents) = debug_intent.breakpoints {
                let engine = self.ensure_breakpoint_engine()?;
                engine.lock().unwrap().restore_intent(&intents);
            }

            if let Some(intents) = debug_intent.watchpoints {
                let engine = self.ensure_watchpoint_engine()?;
                engine.lock().unwrap().restore_intent(&intents);
            }
        }

        Ok(restored.len())
    }

    /// Build a replay/rewind planning artifact from a saved checkpoint and the
    /// current debug stop/inspection state.
    #[pyo3(signature = (data, runtime_id=None, segment_index=None))]
    fn replay_plan<'py>(
        &mut self,
        py: Python<'py>,
        data: &[u8],
        runtime_id: Option<usize>,
        segment_index: Option<usize>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let sim = self.sim.as_ref().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "operation requires an instantiated system (call instantiate() or build_simulation())",
            )
        })?;
        let inspection = sim.inspect();
        let selected_segment = match (
            segment_index,
            self.selected_segment_for_runtime(runtime_id, segment_index),
        ) {
            (Some(index), None) => {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "segment_index {index} is out of range for the selected replay history"
                )))
            }
            (_, segment) => segment.cloned(),
        };
        let active_connection = if segment_index.is_some() {
            selected_segment
                .as_ref()
                .and_then(|segment| segment.active_connection.clone())
                .or_else(|| sim.active_debug_connection())
        } else {
            sim.active_debug_connection()
        };
        let runtime_id = runtime_id
            .or_else(|| {
                selected_segment
                    .as_ref()
                    .and_then(|segment| segment.runtime_id)
            })
            .or_else(|| active_connection.as_ref().map(|conn| conn.runtime_id));
        let stop = if segment_index.is_some() {
            selected_segment
                .as_ref()
                .map(|segment| segment.stop.clone())
                .unwrap_or_else(|| self.stop_state_for_runtime(runtime_id).clone())
        } else {
            self.stop_state_for_runtime(runtime_id).clone()
        };
        let cut_point = if segment_index.is_some() {
            selected_segment
                .as_ref()
                .and_then(|segment| self.matching_cut_point_for_segment(runtime_id, segment))
                .or_else(|| self.latest_cut_point_for_runtime(runtime_id))
                .cloned()
        } else {
            self.latest_cut_point_for_runtime(runtime_id).cloned()
        };
        let segment =
            selected_segment.or_else(|| self.latest_segment_for_runtime(runtime_id).cloned());
        let plan = helm_debug::ReplayPlan::from_checkpoint_bytes(
            data,
            runtime_id,
            active_connection,
            &stop,
            cut_point.as_ref(),
            segment.as_ref(),
            &inspection,
        )
        .map_err(crate::errors::debug_error)?;

        let out = PyDict::new_bound(py);
        let _ = out.set_item("runtime_id", plan.runtime_id);
        let _ = out.set_item("target", plan.target);
        let _ = out.set_item("steps", plan.steps);

        let checkpoint = PyDict::new_bound(py);
        let _ = checkpoint.set_item("version", plan.checkpoint.version);
        let _ = checkpoint.set_item("entry_count", plan.checkpoint.entry_count);
        let _ = checkpoint.set_item("pc", plan.checkpoint.pc);
        let _ = checkpoint.set_item("insn_count", plan.checkpoint.insn_count);
        let _ = checkpoint.set_item("cycle_count", plan.checkpoint.cycle_count);
        let _ = checkpoint.set_item("breakpoint_count", plan.checkpoint.breakpoint_count);
        let _ = checkpoint.set_item("watchpoint_count", plan.checkpoint.watchpoint_count);
        let _ = out.set_item("checkpoint", checkpoint);

        if let Some(cut_point) = plan.cut_point {
            let cut = PyDict::new_bound(py);
            let _ = cut.set_item("runtime_id", cut_point.runtime_id);
            let _ = cut.set_item("pc", cut_point.pc);
            let _ = cut.set_item("insn_count", cut_point.insn_count);
            let _ = cut.set_item("cycle_count", cut_point.cycle_count);
            let _ = cut.set_item("target", cut_point.target);
            let _ = cut.set_item("rendered_stop", cut_point.rendered_stop);
            let _ = out.set_item("cut_point", cut);
        }

        if let Some(segment) = plan.segment {
            let seg = PyDict::new_bound(py);
            let _ = seg.set_item("runtime_id", segment.runtime_id);
            let _ = seg.set_item("kind", segment.kind);
            let _ = seg.set_item("requested_insns", segment.requested_insns);
            let _ = seg.set_item("start_pc", segment.start_pc);
            let _ = seg.set_item("end_pc", segment.end_pc);
            let _ = seg.set_item("insn_delta", segment.insn_delta);
            let _ = seg.set_item("cycle_delta", segment.cycle_delta);
            let _ = seg.set_item("target", segment.target);
            let _ = seg.set_item("rendered_stop", segment.rendered_stop);
            let _ = out.set_item("segment", seg);
        }

        let inspection_out = PyDict::new_bound(py);
        let _ = inspection_out.set_item("arch", plan.inspection.arch);
        let _ = inspection_out.set_item("pc", plan.inspection.pc);
        let _ = inspection_out.set_item("register_count", plan.inspection.register_count);
        let _ = inspection_out.set_item("symbol_count", plan.inspection.symbol_count);
        let _ = inspection_out.set_item("device_names", plan.inspection.device_names);
        let _ = out.set_item("inspection", inspection_out);

        let _ = out.set_item("stop_state", self.stop_state(py, runtime_id));
        let _ = out.set_item("active_connection", self.active_debug_connection(py));
        Ok(out)
    }

    /// Build a replay plan from a stored checkpoint-history entry.
    #[pyo3(signature = (checkpoint_index, runtime_id=None, segment_index=None))]
    fn replay_plan_from_history<'py>(
        &mut self,
        py: Python<'py>,
        checkpoint_index: usize,
        runtime_id: Option<usize>,
        segment_index: Option<usize>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let Some(record) = self
            .selected_checkpoint_for_runtime(runtime_id, Some(checkpoint_index))
            .cloned()
        else {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "checkpoint_index {checkpoint_index} is out of range for the selected checkpoint history"
            )));
        };
        let out = self.replay_plan(
            py,
            &record.bytes,
            runtime_id.or(record.runtime_id),
            segment_index,
        )?;
        Self::update_replay_plan_checkpoint_summary(&out, &record.checkpoint);
        Ok(out)
    }

    /// Build a replay plan for a selected execution segment using either an
    /// explicitly chosen checkpoint or the best stored checkpoint anchor.
    #[pyo3(signature = (segment_index, runtime_id=None, checkpoint_index=None))]
    fn replay_plan_for_segment<'py>(
        &mut self,
        py: Python<'py>,
        segment_index: usize,
        runtime_id: Option<usize>,
        checkpoint_index: Option<usize>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let Some(segment) = self
            .selected_segment_for_runtime(runtime_id, Some(segment_index))
            .cloned()
        else {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "segment_index {segment_index} is out of range for the selected replay history"
            )));
        };

        let checkpoint_record = if let Some(index) = checkpoint_index {
            let Some(record) = self
                .selected_checkpoint_for_runtime(runtime_id.or(segment.runtime_id), Some(index))
                .cloned()
            else {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "checkpoint_index {index} is out of range for the selected checkpoint history"
                )));
            };
            record
        } else {
            let Some(record) = self
                .recommended_anchor_for_segment(
                    runtime_id.or(segment.runtime_id),
                    segment_index,
                    &segment,
                )
                .map(|candidate| candidate.1.clone())
            else {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "no stored checkpoint precedes the selected replay segment",
                ));
            };
            record
        };

        let out = self.replay_plan(
            py,
            &checkpoint_record.bytes,
            runtime_id.or(segment.runtime_id),
            Some(segment_index),
        )?;
        Self::update_replay_plan_checkpoint_summary(&out, &checkpoint_record.checkpoint);
        Ok(out)
    }

    /// Save a durable replay-anchor decision for a selected execution segment.
    #[pyo3(signature = (segment_index, runtime_id=None, checkpoint_index=None))]
    fn save_replay_anchor<'py>(
        &mut self,
        py: Python<'py>,
        segment_index: usize,
        runtime_id: Option<usize>,
        checkpoint_index: Option<usize>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let Some(segment) = self
            .selected_segment_for_runtime(runtime_id, Some(segment_index))
            .cloned()
        else {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "segment_index {segment_index} is out of range for the selected replay history"
            )));
        };

        let checkpoint_record = if let Some(index) = checkpoint_index {
            let Some(record) = self
                .selected_checkpoint_for_runtime(runtime_id.or(segment.runtime_id), Some(index))
                .cloned()
            else {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "checkpoint_index {index} is out of range for the selected checkpoint history"
                )));
            };
            record
        } else {
            let Some(record) = self
                .recommended_anchor_for_segment(
                    runtime_id.or(segment.runtime_id),
                    segment_index,
                    &segment,
                )
                .map(|candidate| candidate.1.clone())
            else {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "no stored checkpoint precedes the selected replay segment",
                ));
            };
            record
        };

        let anchor = helm_debug::ReplayAnchorSelection::capture(
            runtime_id.or(segment.runtime_id),
            &checkpoint_record,
            &segment,
        );
        Ok(PyBytes::new_bound(py, &anchor.to_bytes()))
    }

    /// Build a replay plan from a previously saved replay-anchor decision.
    fn replay_plan_from_anchor<'py>(
        &mut self,
        py: Python<'py>,
        data: &[u8],
    ) -> PyResult<Bound<'py, PyDict>> {
        let anchor = helm_debug::ReplayAnchorSelection::from_bytes(data)
            .map_err(crate::errors::debug_error)?;
        let Some((checkpoint_index, _checkpoint)) =
            self.find_checkpoint_for_anchor(anchor.runtime_id, &anchor)
        else {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "saved replay anchor references a checkpoint that is not present in current history",
            ));
        };
        let Some((segment_index, _segment)) =
            self.find_segment_for_anchor(anchor.runtime_id, &anchor)
        else {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "saved replay anchor references a segment that is not present in current history",
            ));
        };
        self.replay_plan_from_history(py, checkpoint_index, anchor.runtime_id, Some(segment_index))
    }

    /// Return scored replay-anchor candidates for a selected execution segment.
    #[pyo3(signature = (segment_index, runtime_id=None))]
    fn replay_anchor_candidates<'py>(
        &self,
        py: Python<'py>,
        segment_index: usize,
        runtime_id: Option<usize>,
    ) -> PyResult<Bound<'py, PyList>> {
        let Some(segment) = self
            .selected_segment_for_runtime(runtime_id, Some(segment_index))
            .cloned()
        else {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "segment_index {segment_index} is out of range for the selected replay history"
            )));
        };

        let candidates = self.anchor_candidates_for_segment(
            runtime_id.or(segment.runtime_id),
            segment_index,
            &segment,
        );
        let out = PyList::empty_bound(py);
        for (candidate, _record) in candidates {
            let item = PyDict::new_bound(py);
            let _ = item.set_item("checkpoint_index", candidate.checkpoint_index);
            let _ = item.set_item("segment_index", candidate.segment_index);
            let _ = item.set_item("insn_gap", candidate.insn_gap);
            let _ = item.set_item("cycle_gap", candidate.cycle_gap);
            let _ = item.set_item("exact_pc_match", candidate.exact_pc_match);
            let _ = item.set_item("rationale", candidate.rationale);

            let checkpoint = PyDict::new_bound(py);
            let _ = checkpoint.set_item("pc", candidate.checkpoint.pc);
            let _ = checkpoint.set_item("insn_count", candidate.checkpoint.insn_count);
            let _ = checkpoint.set_item("cycle_count", candidate.checkpoint.cycle_count);
            let _ = checkpoint.set_item("version", candidate.checkpoint.version);
            let _ = item.set_item("checkpoint", checkpoint);

            let segment_out = PyDict::new_bound(py);
            let _ = segment_out.set_item("start_pc", candidate.segment.start_pc);
            let _ = segment_out.set_item("end_pc", candidate.segment.end_pc);
            let _ = segment_out.set_item("insn_delta", candidate.segment.insn_delta);
            let _ = segment_out.set_item("cycle_delta", candidate.segment.cycle_delta);
            let _ = segment_out.set_item("kind", candidate.segment.kind);
            let _ = item.set_item("segment", segment_out);

            let _ = out.append(item);
        }
        Ok(out)
    }

    /// Return the bounded checkpoint history for a selected runtime.
    #[pyo3(signature = (runtime_id=None))]
    fn checkpoints<'py>(&self, py: Python<'py>, runtime_id: Option<usize>) -> Bound<'py, PyList> {
        let out = PyList::empty_bound(py);
        for (history_index, record) in self
            .checkpoints_for_runtime(runtime_id)
            .into_iter()
            .enumerate()
        {
            let checkpoint = PyDict::new_bound(py);
            let _ = checkpoint.set_item("history_index", history_index);
            let _ = checkpoint.set_item("runtime_id", record.runtime_id);
            let _ = checkpoint.set_item("version", record.checkpoint.version);
            let _ = checkpoint.set_item("entry_count", record.checkpoint.entry_count);
            let _ = checkpoint.set_item("pc", record.checkpoint.pc);
            let _ = checkpoint.set_item("insn_count", record.checkpoint.insn_count);
            let _ = checkpoint.set_item("cycle_count", record.checkpoint.cycle_count);
            let _ = checkpoint.set_item("breakpoint_count", record.checkpoint.breakpoint_count);
            let _ = checkpoint.set_item("watchpoint_count", record.checkpoint.watchpoint_count);
            let _ = checkpoint.set_item("byte_len", record.bytes.len());
            if let Some(connection) = &record.active_connection {
                let _ = checkpoint.set_item(
                    "active_connection",
                    (
                        connection.runtime_id,
                        connection.label.clone(),
                        connection.arch.clone(),
                        connection.mode.clone(),
                        connection.role.clone(),
                        connection.domain,
                        connection.active,
                    ),
                );
            }
            let _ = out.append(checkpoint);
        }
        out
    }

    /// Return the bounded replay cut-point history for a selected runtime.
    #[pyo3(signature = (runtime_id=None))]
    fn cut_points<'py>(&self, py: Python<'py>, runtime_id: Option<usize>) -> Bound<'py, PyList> {
        let out = PyList::empty_bound(py);
        for (history_index, cut_point) in self
            .cut_points_for_runtime(runtime_id)
            .into_iter()
            .enumerate()
        {
            let cut = PyDict::new_bound(py);
            let _ = cut.set_item("history_index", history_index);
            let _ = cut.set_item("runtime_id", cut_point.runtime_id);
            let _ = cut.set_item("pc", cut_point.pc);
            let _ = cut.set_item("insn_count", cut_point.insn_count);
            let _ = cut.set_item("cycle_count", cut_point.cycle_count);
            let _ = cut.set_item("target", cut_point.target());
            let _ = cut.set_item("rendered_stop", cut_point.stop.render());
            if let Some(connection) = &cut_point.active_connection {
                let _ = cut.set_item(
                    "active_connection",
                    (
                        connection.runtime_id,
                        connection.label.clone(),
                        connection.arch.clone(),
                        connection.mode.clone(),
                        connection.role.clone(),
                        connection.domain,
                        connection.active,
                    ),
                );
            }
            let _ = out.append(cut);
        }
        out
    }

    /// Return the bounded execution-segment history for a selected runtime.
    #[pyo3(signature = (runtime_id=None))]
    fn execution_segments<'py>(
        &self,
        py: Python<'py>,
        runtime_id: Option<usize>,
    ) -> Bound<'py, PyList> {
        let out = PyList::empty_bound(py);
        for (history_index, segment) in self
            .segments_for_runtime(runtime_id)
            .into_iter()
            .enumerate()
        {
            let seg = PyDict::new_bound(py);
            let _ = seg.set_item("history_index", history_index);
            let _ = seg.set_item("runtime_id", segment.runtime_id);
            let _ = seg.set_item("kind", segment.kind.clone());
            let _ = seg.set_item("requested_insns", segment.requested_insns);
            let _ = seg.set_item("start_pc", segment.start_pc);
            let _ = seg.set_item("end_pc", segment.end_pc);
            let _ = seg.set_item(
                "insn_delta",
                segment
                    .end_insn_count
                    .saturating_sub(segment.start_insn_count),
            );
            let _ = seg.set_item(
                "cycle_delta",
                segment
                    .end_cycle_count
                    .saturating_sub(segment.start_cycle_count),
            );
            let _ = seg.set_item("target", segment.target());
            let _ = seg.set_item("rendered_stop", segment.stop.render());
            let _ = out.append(seg);
        }
        out
    }

    /// Set a PC breakpoint that stops execution.
    #[pyo3(signature = (pc, action="break"))]
    fn breakpoint(&mut self, pc: u64, action: &str) -> PyResult<()> {
        #[cfg(feature = "instrumentation")]
        {
            let action = Self::parse_break_action(action)?;
            let engine = self.ensure_breakpoint_engine()?;
            engine.lock().unwrap().add(pc, action);
            return Ok(());
        }

        #[cfg(not(feature = "instrumentation"))]
        {
            let sim = self.require_sim()?;
            let reg = sim.plugins_mut();
            let act_str = action.to_string();
            reg.on_insn_exec(Box::new(move |_vcpu, insn| {
                if insn.pc == pc {
                    match act_str.as_str() {
                        "log" => eprintln!("[breakpoint] hit at {:#x}", insn.pc),
                        _ => eprintln!("[breakpoint] break at {:#x}", insn.pc),
                    }
                }
            }));
            Ok(())
        }
    }

    /// List native probe-backed breakpoints as `(id, addr, action, enabled, hit_count)`.
    fn breakpoints(&self) -> Vec<(u32, u64, String, bool, u64)> {
        #[cfg(feature = "instrumentation")]
        {
            self.debug_state_snapshot()
                .breakpoints
                .into_iter()
                .map(|bp| (bp.id, bp.addr, bp.action, bp.enabled, bp.hit_count))
                .collect()
        }
        #[cfg(not(feature = "instrumentation"))]
        {
            Vec::new()
        }
    }

    /// Remove a native probe-backed breakpoint by id.
    #[cfg_attr(not(feature = "instrumentation"), allow(unused_variables))]
    fn remove_breakpoint(&mut self, id: u32) -> PyResult<bool> {
        #[cfg(feature = "instrumentation")]
        if let Some(engine) = &self.breakpoints {
            return Ok(engine.lock().unwrap().remove(id));
        }
        Ok(false)
    }

    /// Enable or disable a native probe-backed breakpoint by id.
    #[pyo3(signature = (id, enabled=true))]
    #[cfg_attr(not(feature = "instrumentation"), allow(unused_variables))]
    fn enable_breakpoint(&mut self, id: u32, enabled: bool) -> PyResult<bool> {
        #[cfg(feature = "instrumentation")]
        if let Some(engine) = &self.breakpoints {
            return Ok(engine.lock().unwrap().set_enabled(id, enabled));
        }
        Ok(false)
    }

    /// Clear all native probe-backed breakpoints.
    fn clear_breakpoints(&mut self) -> PyResult<()> {
        #[cfg(feature = "instrumentation")]
        if let Some(engine) = &self.breakpoints {
            engine.lock().unwrap().clear();
        }
        Ok(())
    }

    /// Set a memory watchpoint.
    #[pyo3(signature = (addr, size=8, kind="write"))]
    fn watchpoint(&mut self, addr: u64, size: u64, kind: &str) -> PyResult<()> {
        #[cfg(feature = "instrumentation")]
        {
            let kind = Self::parse_watch_kind(kind)?;
            let engine = self.ensure_watchpoint_engine()?;
            engine
                .lock()
                .unwrap()
                .add(addr, size, kind, helm_debug::WatchAction::Break);
            return Ok(());
        }

        #[cfg(not(feature = "instrumentation"))]
        {
            let writes_only = kind == "write";
            self.install_watchpoint_plugin(addr, size, writes_only)
        }
    }

    /// List native probe-backed watchpoints as `(id, start, size, kind, action, enabled)`.
    fn watchpoints(&self) -> Vec<(u32, u64, u64, String, String, bool)> {
        #[cfg(feature = "instrumentation")]
        {
            self.debug_state_snapshot()
                .watchpoints
                .into_iter()
                .map(|wp| (wp.id, wp.start, wp.size, wp.kind, wp.action, wp.enabled))
                .collect()
        }
        #[cfg(not(feature = "instrumentation"))]
        {
            Vec::new()
        }
    }

    /// Return current native debug trigger state as a structured dictionary.
    fn debug_state<'py>(&self, py: Python<'py>) -> Bound<'py, PyDict> {
        let snapshot = self.debug_state_snapshot();
        let out = PyDict::new_bound(py);
        let breakpoints = snapshot
            .breakpoints
            .into_iter()
            .map(|bp| (bp.id, bp.addr, bp.action, bp.enabled, bp.hit_count))
            .collect::<Vec<_>>();
        let watchpoints = snapshot
            .watchpoints
            .into_iter()
            .map(|wp| (wp.id, wp.start, wp.size, wp.kind, wp.action, wp.enabled))
            .collect::<Vec<_>>();
        let active_connection = self
            .sim
            .as_ref()
            .and_then(helm_engine::HelmSim::active_debug_connection)
            .map(|conn| {
                (
                    conn.runtime_id,
                    conn.label,
                    conn.arch,
                    conn.mode,
                    conn.role,
                    conn.domain,
                    conn.active,
                )
            });
        let _ = out.set_item("breakpoints", breakpoints);
        let _ = out.set_item("watchpoints", watchpoints);
        let _ = out.set_item("active_connection", active_connection);
        out
    }

    /// Return structured information about the most recent run stop and any
    /// native debug trigger hit observed for a selected debug connection.
    #[pyo3(signature = (runtime_id=None))]
    fn stop_state<'py>(&self, py: Python<'py>, runtime_id: Option<usize>) -> Bound<'py, PyDict> {
        let out = PyDict::new_bound(py);
        let stop = self.stop_state_for_runtime(runtime_id);
        let _ = out.set_item("kind", stop.stop.kind_label());
        let _ = out.set_item("rendered", stop.render());
        let _ = out.set_item(
            "runtime_id",
            runtime_id.or_else(|| self.current_debug_runtime_id()),
        );
        let _ = out.set_item(
            "active_connection",
            self.sim
                .as_ref()
                .and_then(helm_engine::HelmSim::active_debug_connection)
                .map(|conn| {
                    (
                        conn.runtime_id,
                        conn.label,
                        conn.arch,
                        conn.mode,
                        conn.role,
                        conn.domain,
                        conn.active,
                    )
                }),
        );
        match &stop.stop {
            helm_debug::RuntimeStopView::Exit { code } => {
                let _ = out.set_item("code", *code);
            }
            helm_debug::RuntimeStopView::Exception(err) => {
                let _ = out.set_item("message", err);
            }
            helm_debug::RuntimeStopView::JitBreakpoint { pc } => {
                if let Some(pc) = pc {
                    let _ = out.set_item("pc", *pc);
                }
            }
            helm_debug::RuntimeStopView::Quantum
            | helm_debug::RuntimeStopView::Unsupported
            | helm_debug::RuntimeStopView::ErrorNotInstantiated => {}
        }

        if let Some(hit) = &stop.last_native_hit {
            let native = PyDict::new_bound(py);
            let _ = native.set_item("kind", hit.kind_label());
            match hit {
                helm_debug::NativeTriggerHitView::Breakpoint(bp) => {
                    let _ = native.set_item("breakpoint_id", bp.breakpoint_id);
                    let _ = native.set_item("addr", bp.addr);
                    let _ = native.set_item("action", &bp.action);
                }
                helm_debug::NativeTriggerHitView::Watchpoint(wp) => {
                    let _ = native.set_item("watchpoint_id", wp.watchpoint_id);
                    let _ = native.set_item("addr", wp.addr);
                    let _ = native.set_item("size", wp.size);
                    let _ = native.set_item("access", &wp.access);
                    let _ = native.set_item("action", &wp.action);
                }
            }
            let _ = out.set_item("last_native_hit", native);
        }
        out
    }

    /// Return a structured inspection snapshot for the active debug target.
    fn inspect<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let inspection = {
            let sim = self.require_sim()?;
            sim.inspect()
        };

        let out = PyDict::new_bound(py);
        if let Some(arch) = inspection.arch {
            let _ = out.set_item("arch", arch);
        }
        let _ = out.set_item("pc", inspection.pc);

        let registers = PyDict::new_bound(py);
        for (name, value) in inspection.int_regs {
            let _ = registers.set_item(name, value);
        }
        let _ = out.set_item("registers", registers);

        let symbols = inspection
            .symbols
            .into_iter()
            .map(|symbol| (symbol.name, symbol.addr, symbol.size))
            .collect::<Vec<_>>();
        let _ = out.set_item("symbols", symbols);

        let devices = PyDict::new_bound(py);
        for device in inspection.devices {
            let fields = PyDict::new_bound(py);
            for (key, value) in device.fields {
                let _ = fields.set_item(key, value);
            }
            let _ = devices.set_item(device.name, fields);
        }
        let _ = out.set_item("devices", devices);

        let extras = PyDict::new_bound(py);
        for (key, value) in inspection.extras {
            let _ = extras.set_item(key, value);
        }
        let _ = out.set_item("extras", extras);
        let _ = out.set_item("debug_state", self.debug_state(py));
        let _ = out.set_item("active_connection", self.active_debug_connection(py));
        Ok(out)
    }

    /// Read an arbitrary byte range from guest memory for the active debug target.
    fn read_memory<'py>(
        &mut self,
        py: Python<'py>,
        addr: u64,
        len: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = {
            let sim = self.require_sim()?;
            sim.inspect_memory(addr, len).unwrap_or_default()
        };
        Ok(PyBytes::new_bound(py, &bytes))
    }

    /// Remove a native probe-backed watchpoint by id.
    #[cfg_attr(not(feature = "instrumentation"), allow(unused_variables))]
    fn remove_watchpoint(&mut self, id: u32) -> PyResult<bool> {
        #[cfg(feature = "instrumentation")]
        if let Some(engine) = &self.watchpoints {
            return Ok(engine.lock().unwrap().remove(id));
        }
        Ok(false)
    }

    /// Enable or disable a native probe-backed watchpoint by id.
    #[pyo3(signature = (id, enabled=true))]
    #[cfg_attr(not(feature = "instrumentation"), allow(unused_variables))]
    fn enable_watchpoint(&mut self, id: u32, enabled: bool) -> PyResult<bool> {
        #[cfg(feature = "instrumentation")]
        if let Some(engine) = &self.watchpoints {
            return Ok(engine.lock().unwrap().set_enabled(id, enabled));
        }
        Ok(false)
    }

    /// Clear all native probe-backed watchpoints.
    fn clear_watchpoints(&mut self) -> PyResult<()> {
        #[cfg(feature = "instrumentation")]
        if let Some(engine) = &self.watchpoints {
            engine.lock().unwrap().clear();
        }
        Ok(())
    }

    // ── Misc ─────────────────────────────────────────────────────────────────

    fn set_pc(&mut self, pc: u64) {
        if let Some(sim) = self.sim.as_mut() {
            sim.set_pc(pc);
        }
    }

    fn finish(&mut self) {
        for p in &mut self.plugins {
            p.atexit();
        }
        self.plugins.clear();
    }

    fn load_bytes(&mut self, addr: u64, data: Vec<u8>) {
        if let Some(sim) = self.sim.as_mut() {
            sim.load_bytes(addr, &data);
        }
    }

    fn read_mem(&mut self, addr: u64) -> u64 {
        self.sim.as_mut().map_or(0xDEAD, |s| s.read_mem(addr, 8))
    }

    fn resolve_symbol(&self, name: &str) -> Option<u64> {
        self.sim.as_ref().and_then(|s| s.resolve_symbol(name))
    }

    fn symbols(&self) -> Vec<(String, u64, u64)> {
        self.sim.as_ref().map_or_else(Vec::new, |s| {
            s.symbols()
                .iter()
                .map(|sym| (sym.name.clone(), sym.addr, sym.size))
                .collect()
        })
    }
}

impl HelmSystem {
    fn update_replay_plan_checkpoint_summary(
        out: &Bound<'_, PyDict>,
        checkpoint: &helm_debug::ReplayCheckpointSummary,
    ) {
        if let Some(checkpoint_any) = out.get_item("checkpoint").ok().flatten() {
            if let Ok(checkpoint_dict) = checkpoint_any.downcast::<PyDict>() {
                let _ = checkpoint_dict.set_item("insn_count", checkpoint.insn_count);
                let _ = checkpoint_dict.set_item("cycle_count", checkpoint.cycle_count);
            }
        }
    }

    fn stop_reason_view(stop: &StopReason) -> helm_debug::RuntimeStopView {
        match stop {
            StopReason::Exit { code } => helm_debug::RuntimeStopView::Exit { code: *code },
            StopReason::Quantum => helm_debug::RuntimeStopView::Quantum,
            StopReason::Exception(e) => helm_debug::RuntimeStopView::Exception(e.to_string()),
            StopReason::Unsupported => helm_debug::RuntimeStopView::Unsupported,
            StopReason::Breakpoint => helm_debug::RuntimeStopView::JitBreakpoint { pc: None },
        }
    }

    fn current_debug_runtime_id(&self) -> Option<usize> {
        self.sim
            .as_ref()
            .and_then(helm_engine::HelmSim::active_debug_connection)
            .map(|conn| conn.runtime_id)
    }

    fn push_cut_point(
        history: &mut Vec<helm_debug::ReplayCutPoint>,
        cut_point: helm_debug::ReplayCutPoint,
    ) {
        if history.len() >= CUT_POINT_HISTORY_LIMIT {
            history.remove(0);
        }
        history.push(cut_point);
    }

    fn push_segment(
        history: &mut Vec<helm_debug::ReplaySegment>,
        segment: helm_debug::ReplaySegment,
    ) {
        if history.len() >= SEGMENT_HISTORY_LIMIT {
            history.remove(0);
        }
        history.push(segment);
    }

    fn push_checkpoint_record(
        history: &mut Vec<helm_debug::ReplayCheckpointRecord>,
        checkpoint: helm_debug::ReplayCheckpointRecord,
    ) {
        if history.len() >= CHECKPOINT_HISTORY_LIMIT {
            history.remove(0);
        }
        history.push(checkpoint);
    }

    fn capture_run_start(&self) -> Option<RunStartSnapshot> {
        self.sim.as_ref().map(|sim| RunStartSnapshot {
            pc: sim.debug_pc(),
            insn_count: sim.insns_retired(),
            cycle_count: sim.current_cycles(),
        })
    }

    fn record_execution_segment(
        &mut self,
        kind: &str,
        requested_insns: u64,
        start: Option<RunStartSnapshot>,
        stop: &helm_debug::RuntimeStopState,
    ) {
        let Some(start) = start else {
            return;
        };
        let Some(sim) = self.sim.as_ref() else {
            return;
        };
        let runtime_id = self.current_debug_runtime_id();
        let segment = helm_debug::ReplaySegment::capture(
            runtime_id,
            sim.active_debug_connection(),
            kind,
            requested_insns,
            start.pc,
            sim.debug_pc(),
            start.insn_count,
            sim.insns_retired(),
            start.cycle_count,
            sim.current_cycles(),
            stop.clone(),
        );
        if let Some(runtime_id) = runtime_id {
            Self::push_segment(
                self.segment_history_by_runtime
                    .entry(runtime_id)
                    .or_default(),
                segment,
            );
        } else {
            Self::push_segment(&mut self.segment_history_default, segment);
        }
    }

    fn record_checkpoint(&mut self, checkpoint: helm_debug::ReplayCheckpointRecord) {
        if let Some(runtime_id) = checkpoint.runtime_id {
            Self::push_checkpoint_record(
                self.checkpoint_history_by_runtime
                    .entry(runtime_id)
                    .or_default(),
                checkpoint,
            );
        } else {
            Self::push_checkpoint_record(&mut self.checkpoint_history_default, checkpoint);
        }
    }

    fn record_stop_state(&mut self, stop: helm_debug::RuntimeStopState) {
        let runtime_id = self.current_debug_runtime_id();
        if let Some(sim) = self.sim.as_ref() {
            let cut_point = helm_debug::ReplayCutPoint::capture(
                runtime_id,
                sim.active_debug_connection(),
                sim.debug_pc(),
                sim.insns_retired(),
                sim.current_cycles(),
                stop.clone(),
            );
            if let Some(runtime_id) = runtime_id {
                Self::push_cut_point(
                    self.cut_points_by_runtime.entry(runtime_id).or_default(),
                    cut_point,
                );
            } else {
                Self::push_cut_point(&mut self.cut_points_default, cut_point);
            }
        }

        if let Some(runtime_id) = runtime_id {
            self.last_stop_by_runtime.insert(runtime_id, stop);
        } else {
            self.last_stop_default = stop;
        }
    }

    fn stop_state_for_runtime(&self, runtime_id: Option<usize>) -> &helm_debug::RuntimeStopState {
        match runtime_id.or_else(|| self.current_debug_runtime_id()) {
            Some(runtime_id) => self
                .last_stop_by_runtime
                .get(&runtime_id)
                .unwrap_or(&self.last_stop_default),
            None => &self.last_stop_default,
        }
    }

    fn cut_points_for_runtime(&self, runtime_id: Option<usize>) -> Vec<helm_debug::ReplayCutPoint> {
        match runtime_id.or_else(|| self.current_debug_runtime_id()) {
            Some(runtime_id) => self
                .cut_points_by_runtime
                .get(&runtime_id)
                .cloned()
                .unwrap_or_else(|| self.cut_points_default.clone()),
            None => self.cut_points_default.clone(),
        }
    }

    fn latest_cut_point_for_runtime(
        &self,
        runtime_id: Option<usize>,
    ) -> Option<&helm_debug::ReplayCutPoint> {
        match runtime_id.or_else(|| self.current_debug_runtime_id()) {
            Some(runtime_id) => self
                .cut_points_by_runtime
                .get(&runtime_id)
                .and_then(|history| history.last())
                .or_else(|| self.cut_points_default.last()),
            None => self.cut_points_default.last(),
        }
    }

    fn segments_for_runtime(&self, runtime_id: Option<usize>) -> Vec<helm_debug::ReplaySegment> {
        match runtime_id.or_else(|| self.current_debug_runtime_id()) {
            Some(runtime_id) => self
                .segment_history_by_runtime
                .get(&runtime_id)
                .cloned()
                .unwrap_or_else(|| self.segment_history_default.clone()),
            None => self.segment_history_default.clone(),
        }
    }

    fn latest_segment_for_runtime(
        &self,
        runtime_id: Option<usize>,
    ) -> Option<&helm_debug::ReplaySegment> {
        match runtime_id.or_else(|| self.current_debug_runtime_id()) {
            Some(runtime_id) => self
                .segment_history_by_runtime
                .get(&runtime_id)
                .and_then(|history| history.last())
                .or_else(|| self.segment_history_default.last()),
            None => self.segment_history_default.last(),
        }
    }

    fn checkpoints_for_runtime(
        &self,
        runtime_id: Option<usize>,
    ) -> Vec<helm_debug::ReplayCheckpointRecord> {
        match runtime_id.or_else(|| self.current_debug_runtime_id()) {
            Some(runtime_id) => self
                .checkpoint_history_by_runtime
                .get(&runtime_id)
                .cloned()
                .unwrap_or_else(|| self.checkpoint_history_default.clone()),
            None => self.checkpoint_history_default.clone(),
        }
    }

    fn selected_checkpoint_for_runtime(
        &self,
        runtime_id: Option<usize>,
        checkpoint_index: Option<usize>,
    ) -> Option<&helm_debug::ReplayCheckpointRecord> {
        let history = match runtime_id.or_else(|| self.current_debug_runtime_id()) {
            Some(runtime_id) => self
                .checkpoint_history_by_runtime
                .get(&runtime_id)
                .unwrap_or(&self.checkpoint_history_default),
            None => &self.checkpoint_history_default,
        };
        match checkpoint_index {
            Some(index) => history.get(index),
            None => history.last(),
        }
    }

    fn anchor_candidates_for_segment(
        &self,
        runtime_id: Option<usize>,
        segment_index: usize,
        segment: &helm_debug::ReplaySegment,
    ) -> Vec<(
        helm_debug::ReplayAnchorCandidate,
        &helm_debug::ReplayCheckpointRecord,
    )> {
        let history = match runtime_id
            .or_else(|| segment.runtime_id)
            .or_else(|| self.current_debug_runtime_id())
        {
            Some(runtime_id) => self
                .checkpoint_history_by_runtime
                .get(&runtime_id)
                .unwrap_or(&self.checkpoint_history_default),
            None => &self.checkpoint_history_default,
        };
        let candidates = helm_debug::ReplayAnchorCandidate::candidates_for_segment(
            segment_index,
            segment,
            history,
        );
        candidates
            .into_iter()
            .filter_map(|candidate| {
                history
                    .get(candidate.checkpoint_index)
                    .map(|record| (candidate, record))
            })
            .collect()
    }

    fn recommended_anchor_for_segment(
        &self,
        runtime_id: Option<usize>,
        segment_index: usize,
        segment: &helm_debug::ReplaySegment,
    ) -> Option<(
        helm_debug::ReplayAnchorCandidate,
        &helm_debug::ReplayCheckpointRecord,
    )> {
        self.anchor_candidates_for_segment(runtime_id, segment_index, segment)
            .into_iter()
            .next()
    }

    fn find_checkpoint_for_anchor(
        &self,
        runtime_id: Option<usize>,
        anchor: &helm_debug::ReplayAnchorSelection,
    ) -> Option<(usize, &helm_debug::ReplayCheckpointRecord)> {
        let history = match runtime_id.or_else(|| self.current_debug_runtime_id()) {
            Some(runtime_id) => self
                .checkpoint_history_by_runtime
                .get(&runtime_id)
                .unwrap_or(&self.checkpoint_history_default),
            None => &self.checkpoint_history_default,
        };
        history.iter().enumerate().find(|(_, record)| {
            record.checkpoint.pc == anchor.checkpoint.pc
                && record.checkpoint.insn_count == anchor.checkpoint.insn_count
                && record.checkpoint.cycle_count == anchor.checkpoint.cycle_count
                && record.checkpoint.entry_count == anchor.checkpoint.entry_count
        })
    }

    fn selected_segment_for_runtime(
        &self,
        runtime_id: Option<usize>,
        segment_index: Option<usize>,
    ) -> Option<&helm_debug::ReplaySegment> {
        let history = match runtime_id.or_else(|| self.current_debug_runtime_id()) {
            Some(runtime_id) => self
                .segment_history_by_runtime
                .get(&runtime_id)
                .unwrap_or(&self.segment_history_default),
            None => &self.segment_history_default,
        };
        match segment_index {
            Some(index) => history.get(index),
            None => history.last(),
        }
    }

    fn find_segment_for_anchor(
        &self,
        runtime_id: Option<usize>,
        anchor: &helm_debug::ReplayAnchorSelection,
    ) -> Option<(usize, &helm_debug::ReplaySegment)> {
        let history = match runtime_id.or_else(|| self.current_debug_runtime_id()) {
            Some(runtime_id) => self
                .segment_history_by_runtime
                .get(&runtime_id)
                .unwrap_or(&self.segment_history_default),
            None => &self.segment_history_default,
        };
        history.iter().enumerate().find(|(_, segment)| {
            segment.kind == anchor.segment_kind
                && segment.requested_insns == anchor.segment_requested_insns
                && segment.start_pc == anchor.segment_start_pc
                && segment.end_pc == anchor.segment_end_pc
                && segment.start_insn_count == anchor.segment_start_insn_count
                && segment.end_insn_count == anchor.segment_end_insn_count
                && segment.start_cycle_count == anchor.segment_start_cycle_count
                && segment.end_cycle_count == anchor.segment_end_cycle_count
        })
    }

    fn matching_cut_point_for_segment(
        &self,
        runtime_id: Option<usize>,
        segment: &helm_debug::ReplaySegment,
    ) -> Option<&helm_debug::ReplayCutPoint> {
        let history = match runtime_id
            .or_else(|| segment.runtime_id)
            .or_else(|| self.current_debug_runtime_id())
        {
            Some(runtime_id) => self
                .cut_points_by_runtime
                .get(&runtime_id)
                .unwrap_or(&self.cut_points_default),
            None => &self.cut_points_default,
        };
        history.iter().rev().find(|cut_point| {
            cut_point.pc == segment.end_pc
                && cut_point.insn_count == segment.end_insn_count
                && cut_point.cycle_count == segment.end_cycle_count
        })
    }

    #[cfg(feature = "instrumentation")]
    fn ensure_native_trigger_state(&mut self) -> Arc<Mutex<helm_debug::NativeTriggerState>> {
        if let Some(state) = &self.native_trigger_state {
            return Arc::clone(state);
        }
        let state = helm_debug::NativeTriggerState::shared();
        self.native_trigger_state = Some(Arc::clone(&state));
        state
    }

    #[cfg(feature = "instrumentation")]
    fn clear_native_trigger_hits(&mut self) {
        if let Some(state) = &self.native_trigger_state {
            if let Ok(mut guard) = state.lock() {
                guard.clear();
            }
        }
    }

    #[cfg(not(feature = "instrumentation"))]
    fn clear_native_trigger_hits(&mut self) {}

    #[cfg(feature = "instrumentation")]
    fn native_trigger_hit_snapshot(
        &self,
        runtime_id: Option<usize>,
    ) -> Option<helm_debug::NativeTriggerHitView> {
        let runtime_id = runtime_id.or_else(|| self.current_debug_runtime_id())?;
        self.native_trigger_state.as_ref().and_then(|state| {
            state
                .lock()
                .ok()
                .and_then(|guard| guard.snapshot_for_runtime(runtime_id))
        })
    }

    #[cfg(not(feature = "instrumentation"))]
    fn native_trigger_hit_snapshot(
        &self,
        _runtime_id: Option<usize>,
    ) -> Option<helm_debug::NativeTriggerHitView> {
        None
    }

    #[cfg(feature = "instrumentation")]
    fn debug_state_snapshot(&self) -> helm_debug::DebugStateSnapshot {
        let bp_guard = self
            .breakpoints
            .as_ref()
            .map(|engine| engine.lock().unwrap());
        let wp_guard = self
            .watchpoints
            .as_ref()
            .map(|engine| engine.lock().unwrap());
        helm_debug::DebugStateSnapshot::capture(bp_guard.as_deref(), wp_guard.as_deref())
    }

    #[cfg(not(feature = "instrumentation"))]
    fn debug_state_snapshot(&self) -> helm_debug::DebugStateSnapshot {
        helm_debug::DebugStateSnapshot::default()
    }

    #[cfg(feature = "instrumentation")]
    fn ensure_breakpoint_engine(&mut self) -> PyResult<Arc<Mutex<helm_debug::BreakpointEngine>>> {
        if let Some(engine) = &self.breakpoints {
            return Ok(Arc::clone(engine));
        }

        let trigger_state = self.ensure_native_trigger_state();
        let sim = self.require_sim()?;
        let engine =
            helm_debug::attach_breakpoint_engine(sim.probes_mut(), trigger_state, move |result| {
                match result {
                    helm_debug::BreakResult::Hit { addr, action, .. } => match action {
                        helm_debug::BreakAction::Log => eprintln!("[breakpoint] hit at {addr:#x}"),
                        helm_debug::BreakAction::Break => {
                            eprintln!("[breakpoint] break at {addr:#x}")
                        }
                        helm_debug::BreakAction::Callback(id) => {
                            eprintln!("[breakpoint] callback id={id} at {addr:#x}")
                        }
                    },
                    helm_debug::BreakResult::None => {}
                }
            });
        self.breakpoints = Some(Arc::clone(&engine));
        Ok(engine)
    }

    #[cfg(feature = "instrumentation")]
    fn ensure_watchpoint_engine(&mut self) -> PyResult<Arc<Mutex<helm_debug::WatchpointEngine>>> {
        if let Some(engine) = &self.watchpoints {
            return Ok(Arc::clone(engine));
        }

        let trigger_state = self.ensure_native_trigger_state();
        let sim = self.require_sim()?;
        let engine =
            helm_debug::attach_watchpoint_engine(sim.probes_mut(), trigger_state, move |result| {
                match result {
                    helm_debug::WatchResult::Hit {
                        addr,
                        size,
                        is_store,
                        action,
                        ..
                    } => match action {
                        helm_debug::WatchAction::Log => eprintln!(
                            "[watchpoint] {} at {addr:#x} size={size}",
                            if is_store { "write" } else { "read" }
                        ),
                        helm_debug::WatchAction::Break => eprintln!(
                            "[watchpoint] break on {} at {addr:#x} size={size}",
                            if is_store { "write" } else { "read" }
                        ),
                        helm_debug::WatchAction::Callback(id) => eprintln!(
                            "[watchpoint] callback id={id} on {} at {addr:#x} size={size}",
                            if is_store { "write" } else { "read" }
                        ),
                    },
                    helm_debug::WatchResult::None => {}
                }
            });
        self.watchpoints = Some(Arc::clone(&engine));
        Ok(engine)
    }

    #[cfg(feature = "instrumentation")]
    fn parse_break_action(action: &str) -> PyResult<helm_debug::BreakAction> {
        match action {
            "log" => Ok(helm_debug::BreakAction::Log),
            "break" => Ok(helm_debug::BreakAction::Break),
            other => Err(pyo3::exceptions::PyValueError::new_err(format!(
                "unknown breakpoint action '{other}' (expected 'break' or 'log')"
            ))),
        }
    }

    #[cfg(feature = "instrumentation")]
    fn parse_watch_kind(kind: &str) -> PyResult<helm_debug::WatchKind> {
        match kind {
            "read" => Ok(helm_debug::WatchKind::Read),
            "write" => Ok(helm_debug::WatchKind::Write),
            "rw" | "readwrite" | "read-write" => Ok(helm_debug::WatchKind::ReadWrite),
            other => Err(pyo3::exceptions::PyValueError::new_err(format!(
                "unknown watchpoint kind '{other}' (expected 'read', 'write', or 'rw')"
            ))),
        }
    }

    fn install_watchpoint_plugin(
        &mut self,
        addr: u64,
        size: u64,
        writes_only: bool,
    ) -> PyResult<()> {
        use helm_engine::helm_plugin::api::{HelmPlugin, HelmPluginArgs};

        let sim = self.require_sim()?;
        let mut plugin = Box::new(
            helm_engine::helm_plugin::builtins::debug::Watchpoint::with_addr(
                addr,
                size,
                writes_only,
                None,
            ),
        );
        let pargs = HelmPluginArgs::parse("");
        let reg = sim.plugins_mut();
        plugin.install(reg, &pargs);
        self.plugins.push(plugin);
        Ok(())
    }

    pub(crate) fn require_sim(&mut self) -> PyResult<&mut HelmSim> {
        self.sim.as_mut().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "operation requires an instantiated system (call instantiate() or build_simulation())",
            )
        })
    }
}

impl Drop for HelmSystem {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system_with_sim(sim: HelmSim) -> HelmSystem {
        HelmSystem {
            timing: "virtual".into(),
            mode: "functional".into(),
            ipc: 1.0,
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
            last_stop_by_runtime: HashMap::new(),
            cut_points_default: Vec::new(),
            cut_points_by_runtime: HashMap::new(),
            segment_history_default: Vec::new(),
            segment_history_by_runtime: HashMap::new(),
            checkpoint_history_default: Vec::new(),
            checkpoint_history_by_runtime: HashMap::new(),
        }
    }

    #[test]
    fn current_cycles_defaults_to_zero_when_uninstantiated() {
        let system = HelmSystem {
            timing: "virtual".into(),
            mode: "functional".into(),
            ipc: 1.0,
            num_cpus: 1,
            gic_version: "v3".into(),
            sim: None,
            exited: false,
            exit_code_val: 0,
            plugins: Vec::new(),
            breakpoints: None,
            watchpoints: None,
            native_trigger_state: None,
            last_stop_default: helm_debug::RuntimeStopState::default(),
            last_stop_by_runtime: HashMap::new(),
            cut_points_default: Vec::new(),
            cut_points_by_runtime: HashMap::new(),
            segment_history_default: Vec::new(),
            segment_history_by_runtime: HashMap::new(),
            checkpoint_history_default: Vec::new(),
            checkpoint_history_by_runtime: HashMap::new(),
        };

        assert_eq!(system.current_cycles(), 0);
    }

    #[test]
    fn current_cycles_tracks_virtual_timing_progress() {
        let mut system = system_with_sim(build_simulator(
            Isa::RiscV,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system.load_bytes(
            0x100,
            [
                0x13, 0x00, 0x00, 0x00, // nop
                0x13, 0x00, 0x00, 0x00, // nop
                0x13, 0x00, 0x00, 0x00, // nop
            ]
            .to_vec(),
        );
        system.set_pc(0x100);

        assert_eq!(system.current_cycles(), 0);
        assert_eq!(system.run(3), "quantum");
        assert_eq!(system.current_cycles(), 3);
    }

    #[test]
    fn current_cycles_tracks_virtual_fractional_ipc_progress() {
        let mut system = system_with_sim(build_simulator(
            Isa::RiscV,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 4.0 },
            0,
            0x2000,
        ));

        system.load_bytes(
            0x100,
            [
                0x13, 0x00, 0x00, 0x00, // nop
                0x13, 0x00, 0x00, 0x00, // nop
                0x13, 0x00, 0x00, 0x00, // nop
                0x13, 0x00, 0x00, 0x00, // nop
            ]
            .to_vec(),
        );
        system.set_pc(0x100);

        assert_eq!(system.run(3), "quantum");
        assert_eq!(system.current_cycles(), 0);
        assert_eq!(system.run(1), "quantum");
        assert_eq!(system.current_cycles(), 1);
    }

    #[test]
    fn current_cycles_tracks_interval_timing_progress() {
        let mut system = system_with_sim(build_simulator(
            Isa::RiscV,
            ExecMode::Functional,
            TimingChoice::IntervalTiming {
                ipc: 2.0,
                interval_len: 8,
                mem_model: TimingMemModelConfig::default(),
            },
            0,
            0x2000,
        ));

        system.load_bytes(
            0x100,
            [
                0x23, 0x22, 0x00, 0x00, // sw x0, 4(x0)
                0x83, 0x20, 0x40, 0x00, // lw x1, 4(x0)
                0x63, 0x04, 0x00, 0x00, // beq x0, x0, +8
            ]
            .to_vec(),
        );
        system.set_pc(0x100);

        assert_eq!(system.run(3), "quantum");
        assert_eq!(system.current_cycles(), 12);
    }

    #[test]
    fn parse_timing_interval_accepts_cache_overrides() {
        let parsed = parse_timing(
            "interval:interval_len=256,l1d_size=64KiB,l1d_assoc=4,l1d_line=128,l2_size=1MiB,l2_assoc=16,l2_line=128",
            2.0,
        )
        .unwrap();

        match parsed {
            TimingChoice::IntervalTiming {
                ipc,
                interval_len,
                mem_model,
            } => {
                assert_eq!(ipc, 2.0);
                assert_eq!(interval_len, 256);
                assert_eq!(mem_model.l1d.size_bytes, 64 * 1024);
                assert_eq!(mem_model.l1d.assoc, 4);
                assert_eq!(mem_model.l1d.line_size, 128);
                assert_eq!(mem_model.l2.size_bytes, 1024 * 1024);
                assert_eq!(mem_model.l2.assoc, 16);
                assert_eq!(mem_model.l2.line_size, 128);
            }
            other => panic!("unexpected timing choice: {other:?}"),
        }
    }

    // ── Device introspection tests ──────────────────────────────────────

    #[test]
    fn read_gpr_returns_riscv_registers() {
        let mut system = system_with_sim(build_simulator(
            Isa::RiscV,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        // ADDI x1, x0, 42  =>  0x02A00093
        system.load_bytes(0x100, vec![0x93, 0x00, 0xA0, 0x02]);
        system.set_pc(0x100);

        let sim = system.sim.as_ref().unwrap();
        // Before execution: x1 should be 0
        assert_eq!(sim.read_gpr(1), Some(0));

        system.run(1);

        let sim = system.sim.as_ref().unwrap();
        assert_eq!(sim.read_gpr(1), Some(42));
        // x0 is always 0
        assert_eq!(sim.read_gpr(0), Some(0));
        // PC should have advanced
        assert_eq!(sim.pc(), 0x104);
    }

    #[test]
    fn rv64_state_returns_riscv_core() {
        let mut system = system_with_sim(build_simulator(
            Isa::RiscV,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system.set_pc(0x200);
        let sim = system.sim.as_ref().unwrap();
        let rv = sim.rv64_state();
        assert!(rv.is_some(), "rv64_state should return Some for RISC-V");
        assert_eq!(rv.unwrap().pc, 0x200);
    }

    #[test]
    fn rv64_state_returns_none_for_aarch64() {
        let system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        let sim = system.sim.as_ref().unwrap();
        assert!(
            sim.rv64_state().is_none(),
            "rv64_state should return None for AArch64"
        );
    }

    #[test]
    fn read_gpr_returns_aarch64_registers() {
        let system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        let sim = system.sim.as_ref().unwrap();
        // AArch64: x0-x30, x31=SP
        assert_eq!(sim.read_gpr(0), Some(0));
        assert_eq!(
            sim.read_gpr(31),
            Some(sim.a64_state().unwrap().current_sp())
        );
    }

    #[test]
    fn isa_returns_correct_value() {
        let rv_system = system_with_sim(build_simulator(
            Isa::RiscV,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));
        assert_eq!(rv_system.sim.as_ref().unwrap().isa(), Isa::RiscV);

        let a64_system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));
        assert_eq!(a64_system.sim.as_ref().unwrap().isa(), Isa::AArch64);
    }

    #[test]
    fn gic_queries_return_none_for_se_mode() {
        let system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        let sim = system.sim.as_ref().unwrap();
        // SE mode has no GIC — all queries return None
        assert!(sim.gic_pending_mask(0, 1).is_none());
        assert!(sim.gic_enabled_mask(0, 1).is_none());
        assert!(sim.gic_active_mask(0, 1).is_none());
    }

    #[test]
    fn uart_queries_return_none_for_se_mode() {
        let system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        let sim = system.sim.as_ref().unwrap();
        // SE mode has no UART — all queries return None
        assert!(sim.uart_tx_count().is_none());
        assert!(sim.uart_rx_count().is_none());
        assert!(sim.uart_is_tx_full().is_none());
        assert!(sim.uart_is_rx_empty().is_none());
    }

    #[test]
    fn checkpoint_roundtrip_restores_aarch64_visible_state() {
        let mut system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system
            .sim
            .as_mut()
            .unwrap()
            .with_a64_state_mut(|a64| {
                a64.pc = 0x4000;
                a64.x[0] = 0x1234;
                a64.x[1] = 0x5678;
                a64.write_xsp(31, 0x7FFF_0000);
                a64.nzcv = 0xA000_0000;
            })
            .unwrap();

        Python::with_gil(|py| {
            let checkpoint = system.save_checkpoint(py).expect("save checkpoint");

            system
                .sim
                .as_mut()
                .unwrap()
                .with_a64_state_mut(|a64| {
                    a64.pc = 0x9999;
                    a64.x[0] = 0;
                    a64.x[1] = 0;
                    a64.write_xsp(31, 0);
                    a64.nzcv = 0;
                })
                .unwrap();

            let restored = system
                .restore_checkpoint(checkpoint.as_bytes())
                .expect("restore checkpoint");
            assert!(restored >= 4);
        });

        let sim = system.sim.as_ref().unwrap();
        let a64 = sim.a64_state().unwrap();
        assert_eq!(a64.pc, 0x4000);
        assert_eq!(a64.x[0], 0x1234);
        assert_eq!(a64.x[1], 0x5678);
        assert_eq!(a64.current_sp(), 0x7FFF_0000);
        assert_eq!(a64.nzcv, 0xA000_0000);
    }

    #[cfg(feature = "instrumentation")]
    #[test]
    fn breakpoint_and_watchpoint_use_native_debug_engines() {
        let mut system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system.breakpoint(0x1000, "break").expect("breakpoint");
        system.watchpoint(0x2000, 8, "write").expect("watchpoint");

        let breakpoints = system.breakpoints.as_ref().expect("native breakpoints");
        let watchpoints = system.watchpoints.as_ref().expect("native watchpoints");

        assert_eq!(breakpoints.lock().unwrap().count(), 1);
        assert_eq!(watchpoints.lock().unwrap().count(), 1);
        assert!(
            system.plugins.is_empty(),
            "native path should not install legacy plugins"
        );
    }

    #[cfg(feature = "instrumentation")]
    #[test]
    fn breakpoint_and_watchpoint_management_roundtrip() {
        let mut system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system.breakpoint(0x1000, "log").unwrap();
        system.watchpoint(0x2000, 8, "rw").unwrap();

        let bps = system.breakpoints();
        assert_eq!(bps.len(), 1);
        assert_eq!(bps[0].1, 0x1000);
        assert_eq!(bps[0].2, "log");
        assert!(bps[0].3);

        let wps = system.watchpoints();
        assert_eq!(wps.len(), 1);
        assert_eq!(wps[0].1, 0x2000);
        assert_eq!(wps[0].2, 8);
        assert_eq!(wps[0].3, "rw");
        assert_eq!(wps[0].4, "break");
        assert!(wps[0].5);

        assert!(system.enable_breakpoint(bps[0].0, false).unwrap());
        assert!(system.enable_watchpoint(wps[0].0, false).unwrap());
        assert!(!system.breakpoints()[0].3);
        assert!(!system.watchpoints()[0].5);

        assert!(system.remove_breakpoint(bps[0].0).unwrap());
        assert!(system.remove_watchpoint(wps[0].0).unwrap());
        assert!(system.breakpoints().is_empty());
        assert!(system.watchpoints().is_empty());
    }

    #[cfg(feature = "instrumentation")]
    #[test]
    fn debug_state_reports_native_trigger_views() {
        let mut system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system.breakpoint(0x1000, "log").unwrap();
        system.watchpoint(0x2000, 8, "rw").unwrap();
        let bp_id = system.breakpoints()[0].0;
        system.enable_breakpoint(bp_id, false).unwrap();

        Python::with_gil(|py| {
            let state = system.debug_state(py);
            let breakpoints = state
                .get_item("breakpoints")
                .unwrap()
                .unwrap()
                .extract::<Vec<(u32, u64, String, bool, u64)>>()
                .unwrap();
            let watchpoints = state
                .get_item("watchpoints")
                .unwrap()
                .unwrap()
                .extract::<Vec<(u32, u64, u64, String, String, bool)>>()
                .unwrap();

            assert_eq!(breakpoints.len(), 1);
            assert_eq!(breakpoints[0].1, 0x1000);
            assert_eq!(breakpoints[0].2, "log");
            assert!(!breakpoints[0].3);

            assert_eq!(watchpoints.len(), 1);
            assert_eq!(watchpoints[0].1, 0x2000);
            assert_eq!(watchpoints[0].3, "rw");
            assert_eq!(watchpoints[0].4, "break");
            assert!(watchpoints[0].5);
        });
    }

    #[cfg(feature = "instrumentation")]
    #[test]
    fn inspect_reports_registers_and_debug_state() {
        let mut system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system
            .sim
            .as_mut()
            .unwrap()
            .with_a64_state_mut(|a64| {
                a64.pc = 0x4000;
                a64.x[0] = 0x1234;
                a64.write_xsp(31, 0x7FFF_0000);
                a64.nzcv = 0xA000_0000;
            })
            .unwrap();
        system.breakpoint(0x4000, "log").unwrap();
        system.watchpoint(0x2000, 8, "rw").unwrap();

        Python::with_gil(|py| {
            let inspection = system.inspect(py).expect("inspect");
            assert_eq!(
                inspection
                    .get_item("arch")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "aarch64"
            );
            assert_eq!(
                inspection
                    .get_item("pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4000
            );

            let registers_any = inspection.get_item("registers").unwrap().unwrap();
            let registers = registers_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                registers
                    .get_item("x0")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x1234
            );
            assert_eq!(
                registers
                    .get_item("sp")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x7FFF_0000
            );

            let extras_any = inspection.get_item("extras").unwrap().unwrap();
            let extras = extras_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                extras
                    .get_item("nzcv")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "0xa0000000"
            );

            let debug_state_any = inspection.get_item("debug_state").unwrap().unwrap();
            let debug_state = debug_state_any.downcast::<PyDict>().unwrap();
            let breakpoints = debug_state
                .get_item("breakpoints")
                .unwrap()
                .unwrap()
                .extract::<Vec<(u32, u64, String, bool, u64)>>()
                .unwrap();
            let watchpoints = debug_state
                .get_item("watchpoints")
                .unwrap()
                .unwrap()
                .extract::<Vec<(u32, u64, u64, String, String, bool)>>()
                .unwrap();
            assert_eq!(breakpoints.len(), 1);
            assert_eq!(breakpoints[0].1, 0x4000);
            assert_eq!(watchpoints.len(), 1);
            assert_eq!(watchpoints[0].1, 0x2000);
        });
    }

    #[test]
    fn read_memory_returns_requested_range() {
        let mut system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system.load_bytes(0x100, vec![0xAA, 0xBB, 0xCC, 0xDD]);

        Python::with_gil(|py| {
            let bytes = system.read_memory(py, 0x101, 2).expect("read memory");
            assert_eq!(bytes.as_bytes(), &[0xBB, 0xCC]);
        });
    }

    #[test]
    fn inspect_reports_device_state_for_arm_virt() {
        use helm_platform::BuiltInPlatform;

        let mut system = system_with_sim(helm_engine::build_simulator_from_request(
            helm_engine::SimulatorBuildRequest::new(
                Isa::AArch64,
                ExecMode::System,
                TimingChoice::VirtualTiming { ipc: 1.0 },
                BuiltInPlatform::ArmVirt.default_ram_base(),
                0x20_0000,
            )
            .with_platform(BuiltInPlatform::ArmVirt)
            .with_arm_virt_defaults(1, helm_engine::platform::arm_virt::ArmVirtGicVersion::V2),
        ));

        Python::with_gil(|py| {
            let inspection = system.inspect(py).expect("inspect");
            let devices_any = inspection.get_item("devices").unwrap().unwrap();
            let devices = devices_any.downcast::<PyDict>().unwrap();

            let uart_any = devices.get_item("uart").unwrap().unwrap();
            let uart = uart_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                uart.get_item("tx_count")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "0"
            );
            assert_eq!(
                uart.get_item("rx_empty")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "true"
            );

            let gic_any = devices.get_item("gicv2").unwrap().unwrap();
            let gic = gic_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                gic.get_item("pending_mask_1")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "0x00000000"
            );
        });
    }

    #[cfg(feature = "instrumentation")]
    #[test]
    fn stop_state_reports_native_watchpoint_hit_context() {
        let mut system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system.watchpoint(0x2000, 8, "write").unwrap();
        system.clear_native_trigger_hits();
        system
            .sim
            .as_mut()
            .unwrap()
            .probes_mut()
            .mem
            .notify(&helm_probe::MemAccessEvent {
                addr: 0x2000,
                size: 4,
                is_store: true,
                pc: 0x1000,
            });
        system.record_stop_state(helm_debug::RuntimeStopState {
            stop: helm_debug::RuntimeStopView::Quantum,
            last_native_hit: system.native_trigger_hit_snapshot(None),
        });

        Python::with_gil(|py| {
            let state = system.stop_state(py, None);
            assert_eq!(
                state
                    .get_item("kind")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "quantum"
            );
            let native = state.get_item("last_native_hit").unwrap().unwrap();
            assert_eq!(
                native
                    .get_item("kind")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "native_watchpoint"
            );
            assert_eq!(
                native
                    .get_item("access")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "write"
            );
        });
    }

    #[test]
    fn stop_state_is_tracked_per_debug_connection() {
        let mut system = system_with_sim(build_simulator(
            Isa::RiscV,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system.last_stop_by_runtime.insert(
            7,
            helm_debug::RuntimeStopState {
                stop: helm_debug::RuntimeStopView::Unsupported,
                last_native_hit: None,
            },
        );
        system.last_stop_by_runtime.insert(
            0,
            helm_debug::RuntimeStopState {
                stop: helm_debug::RuntimeStopView::Quantum,
                last_native_hit: None,
            },
        );

        Python::with_gil(|py| {
            let a64_state = system.stop_state(py, Some(7));
            assert_eq!(
                a64_state
                    .get_item("kind")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "unsupported"
            );

            let rv_state = system.stop_state(py, Some(0));
            assert_eq!(
                rv_state
                    .get_item("kind")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "quantum"
            );
        });
    }

    #[cfg(feature = "instrumentation")]
    #[test]
    fn checkpoint_restores_debug_intent() {
        let mut system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system.breakpoint(0x1000, "log").unwrap();
        system.watchpoint(0x2000, 16, "read").unwrap();

        let bp_id = system.breakpoints()[0].0;
        let wp_id = system.watchpoints()[0].0;
        system.enable_breakpoint(bp_id, false).unwrap();
        system.enable_watchpoint(wp_id, false).unwrap();

        Python::with_gil(|py| {
            let checkpoint = system.save_checkpoint(py).expect("save checkpoint");
            system.clear_breakpoints().unwrap();
            system.clear_watchpoints().unwrap();
            assert!(system.breakpoints().is_empty());
            assert!(system.watchpoints().is_empty());

            system
                .restore_checkpoint(checkpoint.as_bytes())
                .expect("restore checkpoint");
        });

        let bps = system.breakpoints();
        let wps = system.watchpoints();
        assert_eq!(bps.len(), 1);
        assert_eq!(bps[0].1, 0x1000);
        assert_eq!(bps[0].2, "log");
        assert!(!bps[0].3);

        assert_eq!(wps.len(), 1);
        assert_eq!(wps[0].1, 0x2000);
        assert_eq!(wps[0].2, 16);
        assert_eq!(wps[0].3, "read");
        assert!(!wps[0].5);
    }

    #[cfg(feature = "instrumentation")]
    #[test]
    fn replay_plan_summarizes_checkpoint_and_stop_state() {
        pyo3::prepare_freethreaded_python();

        let mut system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system
            .sim
            .as_mut()
            .unwrap()
            .with_a64_state_mut(|a64| {
                a64.pc = 0x4000;
                a64.x[0] = 0x1234;
            })
            .unwrap();
        system.load_bytes(0x4000, vec![0x1F, 0x20, 0x03, 0xD5]);
        assert_eq!(system.run(1), "quantum");
        system.breakpoint(0x4004, "log").unwrap();
        system.watchpoint(0x2000, 8, "rw").unwrap();
        system.record_stop_state(helm_debug::RuntimeStopState {
            stop: helm_debug::RuntimeStopView::JitBreakpoint { pc: Some(0x4004) },
            last_native_hit: Some(helm_debug::NativeTriggerHitView::Breakpoint(
                helm_debug::BreakpointHitView {
                    breakpoint_id: system.breakpoints()[0].0,
                    addr: 0x4004,
                    action: "log".to_string(),
                },
            )),
        });

        Python::with_gil(|py| {
            let checkpoint = system.save_checkpoint(py).expect("save checkpoint");
            let plan = system
                .replay_plan(py, checkpoint.as_bytes(), None, None)
                .expect("replay plan");

            assert_eq!(
                plan.get_item("target")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "native_breakpoint"
            );

            let checkpoint_any = plan.get_item("checkpoint").unwrap().unwrap();
            let checkpoint = checkpoint_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                checkpoint
                    .get_item("version")
                    .unwrap()
                    .unwrap()
                    .extract::<u32>()
                    .unwrap(),
                1
            );
            assert_eq!(
                checkpoint
                    .get_item("breakpoint_count")
                    .unwrap()
                    .unwrap()
                    .extract::<usize>()
                    .unwrap(),
                1
            );
            assert_eq!(
                checkpoint
                    .get_item("watchpoint_count")
                    .unwrap()
                    .unwrap()
                    .extract::<usize>()
                    .unwrap(),
                1
            );

            let cut_point_any = plan.get_item("cut_point").unwrap().unwrap();
            let cut_point = cut_point_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                cut_point
                    .get_item("pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4004
            );
            assert_eq!(
                cut_point
                    .get_item("target")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "native_breakpoint"
            );

            let segment_any = plan.get_item("segment").unwrap().unwrap();
            let segment = segment_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                segment
                    .get_item("start_pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4000
            );
            assert_eq!(
                segment
                    .get_item("end_pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4004
            );
            assert_eq!(
                segment
                    .get_item("insn_delta")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                1
            );

            let steps = plan
                .get_item("steps")
                .unwrap()
                .unwrap()
                .extract::<Vec<String>>()
                .unwrap();
            assert!(steps
                .iter()
                .any(|step| step.contains("restore checkpoint version 1")));
            assert!(steps
                .iter()
                .any(|step| step.contains("seek the recorded cut point")));
            assert!(steps
                .iter()
                .any(|step| step.contains("re-run the recorded run window")));
            assert!(steps
                .iter()
                .any(|step| step.contains("re-establish 1 breakpoints and 1 watchpoints")));

            let inspection_any = plan.get_item("inspection").unwrap().unwrap();
            let inspection = inspection_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                inspection
                    .get_item("pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4004
            );

            let cut_points = system.cut_points(py, None);
            assert_eq!(cut_points.len(), 2);
            let segments = system.execution_segments(py, None);
            assert_eq!(segments.len(), 1);
        });
    }

    #[test]
    fn replay_plan_can_select_non_latest_segment() {
        pyo3::prepare_freethreaded_python();

        let mut system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system
            .sim
            .as_mut()
            .unwrap()
            .with_a64_state_mut(|a64| {
                a64.pc = 0x4000;
            })
            .unwrap();
        system.load_bytes(
            0x4000,
            vec![
                0x1F, 0x20, 0x03, 0xD5, // nop
                0x1F, 0x20, 0x03, 0xD5, // nop
            ],
        );

        assert_eq!(system.run(1), "quantum");

        Python::with_gil(|py| {
            let checkpoint = system.save_checkpoint(py).expect("save checkpoint");

            assert_eq!(system.run(1), "quantum");

            let plan = system
                .replay_plan(py, checkpoint.as_bytes(), None, Some(0))
                .expect("replay plan");

            assert_eq!(
                plan.get_item("target")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "quantum"
            );

            let cut_any = plan.get_item("cut_point").unwrap().unwrap();
            let cut = cut_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                cut.get_item("pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4004
            );

            let segment_any = plan.get_item("segment").unwrap().unwrap();
            let segment = segment_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                segment
                    .get_item("start_pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4000
            );
            assert_eq!(
                segment
                    .get_item("end_pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4004
            );
            assert_eq!(
                segment
                    .get_item("insn_delta")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                1
            );

            let segments = system.execution_segments(py, None);
            assert_eq!(segments.len(), 2);
            let first_any = segments.get_item(0).unwrap();
            let first = first_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                first
                    .get_item("history_index")
                    .unwrap()
                    .unwrap()
                    .extract::<usize>()
                    .unwrap(),
                0
            );
            assert_eq!(
                first
                    .get_item("end_pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4004
            );
        });
    }

    #[test]
    fn replay_plan_from_history_can_select_non_latest_checkpoint() {
        pyo3::prepare_freethreaded_python();

        let mut system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system
            .sim
            .as_mut()
            .unwrap()
            .with_a64_state_mut(|a64| {
                a64.pc = 0x4000;
            })
            .unwrap();
        system.load_bytes(0x4000, vec![0x1F, 0x20, 0x03, 0xD5, 0x1F, 0x20, 0x03, 0xD5]);

        assert_eq!(system.run(1), "quantum");

        Python::with_gil(|py| {
            let checkpoint_a = system.save_checkpoint(py).expect("checkpoint a");
            assert_eq!(checkpoint_a.as_bytes().len() > 0, true);
        });

        assert_eq!(system.run(1), "quantum");

        Python::with_gil(|py| {
            let checkpoint_b = system.save_checkpoint(py).expect("checkpoint b");
            assert_eq!(checkpoint_b.as_bytes().len() > 0, true);

            let checkpoints = system.checkpoints(py, None);
            assert_eq!(checkpoints.len(), 2);
            let first_checkpoint_any = checkpoints.get_item(0).unwrap();
            let first_checkpoint = first_checkpoint_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                first_checkpoint
                    .get_item("history_index")
                    .unwrap()
                    .unwrap()
                    .extract::<usize>()
                    .unwrap(),
                0
            );
            assert_eq!(
                first_checkpoint
                    .get_item("pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4004
            );
            assert_eq!(
                first_checkpoint
                    .get_item("insn_count")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                1
            );

            let plan = system
                .replay_plan_from_history(py, 0, None, Some(0))
                .expect("replay plan from history");

            let checkpoint_any = plan.get_item("checkpoint").unwrap().unwrap();
            let checkpoint = checkpoint_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                checkpoint
                    .get_item("pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4004
            );

            let segment_any = plan.get_item("segment").unwrap().unwrap();
            let segment = segment_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                segment
                    .get_item("end_pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4004
            );
        });
    }

    #[test]
    fn replay_plan_for_segment_recommends_checkpoint_anchor() {
        pyo3::prepare_freethreaded_python();

        let mut system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system
            .sim
            .as_mut()
            .unwrap()
            .with_a64_state_mut(|a64| {
                a64.pc = 0x4000;
            })
            .unwrap();
        system.load_bytes(0x4000, vec![0x1F, 0x20, 0x03, 0xD5, 0x1F, 0x20, 0x03, 0xD5]);

        Python::with_gil(|py| {
            let initial = system.save_checkpoint(py).expect("initial checkpoint");
            assert!(!initial.as_bytes().is_empty());
        });

        assert_eq!(system.run(1), "quantum");

        Python::with_gil(|py| {
            let mid = system.save_checkpoint(py).expect("mid checkpoint");
            assert!(!mid.as_bytes().is_empty());
        });

        assert_eq!(system.run(1), "quantum");

        Python::with_gil(|py| {
            let plan = system
                .replay_plan_for_segment(py, 1, None, None)
                .expect("replay plan for segment");

            let checkpoint_any = plan.get_item("checkpoint").unwrap().unwrap();
            let checkpoint = checkpoint_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                checkpoint
                    .get_item("pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4004
            );
            assert_eq!(
                checkpoint
                    .get_item("insn_count")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                1
            );

            let segment_any = plan.get_item("segment").unwrap().unwrap();
            let segment = segment_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                segment
                    .get_item("start_pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4004
            );
            assert_eq!(
                segment
                    .get_item("end_pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4008
            );
        });
    }

    #[test]
    fn replay_anchor_candidates_rank_viable_pairs() {
        pyo3::prepare_freethreaded_python();

        let mut system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system
            .sim
            .as_mut()
            .unwrap()
            .with_a64_state_mut(|a64| {
                a64.pc = 0x4000;
            })
            .unwrap();
        system.load_bytes(0x4000, vec![0x1F, 0x20, 0x03, 0xD5, 0x1F, 0x20, 0x03, 0xD5]);

        Python::with_gil(|py| {
            system.save_checkpoint(py).expect("checkpoint 0");
        });
        assert_eq!(system.run(1), "quantum");
        Python::with_gil(|py| {
            system.save_checkpoint(py).expect("checkpoint 1");
        });
        assert_eq!(system.run(1), "quantum");

        Python::with_gil(|py| {
            let candidates = system
                .replay_anchor_candidates(py, 1, None)
                .expect("anchor candidates");

            assert_eq!(candidates.len(), 2);
            let best_any = candidates.get_item(0).unwrap();
            let best = best_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                best.get_item("checkpoint_index")
                    .unwrap()
                    .unwrap()
                    .extract::<usize>()
                    .unwrap(),
                1
            );
            assert_eq!(
                best.get_item("insn_gap")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0
            );
            assert_eq!(
                best.get_item("exact_pc_match")
                    .unwrap()
                    .unwrap()
                    .extract::<bool>()
                    .unwrap(),
                true
            );

            let second_any = candidates.get_item(1).unwrap();
            let second = second_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                second
                    .get_item("checkpoint_index")
                    .unwrap()
                    .unwrap()
                    .extract::<usize>()
                    .unwrap(),
                0
            );
            assert_eq!(
                second
                    .get_item("insn_gap")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                1
            );
        });
    }

    #[test]
    fn replay_anchor_can_roundtrip_without_live_indices() {
        pyo3::prepare_freethreaded_python();

        let mut system = system_with_sim(build_simulator(
            Isa::AArch64,
            ExecMode::Functional,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            0,
            0x2000,
        ));

        system
            .sim
            .as_mut()
            .unwrap()
            .with_a64_state_mut(|a64| {
                a64.pc = 0x4000;
            })
            .unwrap();
        system.load_bytes(
            0x4000,
            vec![
                0x1F, 0x20, 0x03, 0xD5, 0x1F, 0x20, 0x03, 0xD5, 0x1F, 0x20, 0x03, 0xD5,
            ],
        );

        let anchor_bytes = Python::with_gil(|py| {
            system.save_checkpoint(py).expect("checkpoint 0");
            assert_eq!(system.run(1), "quantum");
            system.save_checkpoint(py).expect("checkpoint 1");
            assert_eq!(system.run(1), "quantum");
            system
                .save_replay_anchor(py, 1, None, None)
                .expect("save anchor")
                .as_bytes()
                .to_vec()
        });

        Python::with_gil(|py| {
            system.save_checkpoint(py).expect("checkpoint 2");
            assert_eq!(system.run(1), "quantum");

            let plan = system
                .replay_plan_from_anchor(py, &anchor_bytes)
                .expect("replay plan from anchor");

            let checkpoint_any = plan.get_item("checkpoint").unwrap().unwrap();
            let checkpoint = checkpoint_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                checkpoint
                    .get_item("pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4004
            );
            assert_eq!(
                checkpoint
                    .get_item("insn_count")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                1
            );

            let segment_any = plan.get_item("segment").unwrap().unwrap();
            let segment = segment_any.downcast::<PyDict>().unwrap();
            assert_eq!(
                segment
                    .get_item("start_pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4004
            );
            assert_eq!(
                segment
                    .get_item("end_pc")
                    .unwrap()
                    .unwrap()
                    .extract::<u64>()
                    .unwrap(),
                0x4008
            );
        });
    }
}

#![allow(missing_docs)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use helm_devices::DeviceParams;
#[cfg(test)]
use helm_engine::{build_simulator, Isa};
use helm_engine::{ExecMode, HelmSim, StopReason, TimingChoice, TimingMemModelConfig};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

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
        let sim = match self.sim.as_mut() {
            Some(s) => s,
            None => return "error:not_instantiated".to_string(),
        };
        match sim.run(max_insns) {
            StopReason::Exit { code } => {
                self.exited = true;
                self.exit_code_val = code;
                format!("exit:{code}")
            }
            StopReason::Quantum => "quantum".to_string(),
            StopReason::Exception(e) => format!("exception:{e}"),
            StopReason::Unsupported => "unsupported".to_string(),
            StopReason::Breakpoint => "breakpoint".to_string(),
        }
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
        let sim = match self.sim.as_mut() {
            Some(s) => s,
            None => return "error:not_instantiated".to_string(),
        };
        match sim.run_jit(max_insns) {
            StopReason::Exit { code } => {
                self.exited = true;
                self.exit_code_val = code;
                format!("exit:{code}")
            }
            StopReason::Quantum => "quantum".to_string(),
            StopReason::Exception(e) => format!("exception:{e}"),
            StopReason::Unsupported => "unsupported".to_string(),
            StopReason::Breakpoint => "breakpoint".to_string(),
        }
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
        self.sim.as_ref().map_or(0, |s| s.pc())
    }

    #[getter]
    fn sp(&self) -> u64 {
        self.xn(31)
    }

    #[getter]
    fn current_sp(&self) -> u64 {
        self.sim
            .as_ref()
            .and_then(|s| s.a64_state())
            .map_or(0, |s| s.current_sp())
    }

    fn xn(&self, n: usize) -> u64 {
        self.sim
            .as_ref()
            .and_then(|s| s.a64_state())
            .map_or(0, |s| if n < 31 { s.x[n] } else { s.current_sp() })
    }

    fn vn(&self, n: usize) -> (u64, u64) {
        self.sim
            .as_ref()
            .and_then(|s| s.a64_state())
            .map_or((0, 0), |s| {
                let val = s.v[n];
                (val as u64, (val >> 64) as u64)
            })
    }

    #[getter]
    fn nzcv(&self) -> u32 {
        self.sim
            .as_ref()
            .and_then(|s| s.a64_state())
            .map_or(0, |s| s.nzcv)
    }

    #[getter]
    fn current_el(&self) -> u8 {
        self.sim
            .as_ref()
            .and_then(|s| s.a64_state())
            .map_or(0, |s| s.current_el)
    }

    #[getter]
    fn daif(&self) -> u32 {
        self.sim
            .as_ref()
            .and_then(|s| s.a64_state())
            .map_or(0, |s| s.daif)
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
        let sim = self.require_sim()?;
        let mgr = helm_debug::CheckpointManager::new();

        #[allow(unused_mut)]
        let mut values: Vec<(String, u64)> = if let Some(a64) = sim.a64_state() {
            let mut vals = Vec::with_capacity(34);
            vals.push(("pc".to_string(), a64.pc));
            vals.push(("sp".to_string(), a64.current_sp()));
            vals.push(("nzcv".to_string(), u64::from(a64.nzcv)));
            for (idx, reg) in a64.x.iter().enumerate() {
                vals.push((format!("x{idx}"), *reg));
            }
            vals
        } else if let Some(rv) = sim.rv64_state() {
            let mut vals = Vec::with_capacity(33);
            vals.push(("pc".to_string(), rv.pc));
            for (idx, reg) in rv.iregs.iter().enumerate() {
                vals.push((format!("x{idx}"), *reg));
            }
            vals
        } else {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "checkpoint save requires an active ISA runtime",
            ));
        };

        #[cfg(feature = "instrumentation")]
        if let Some(engine) = &self.breakpoints {
            let guard = engine.lock().unwrap();
            values.push(("debug.breakpoints.count".to_string(), guard.count() as u64));
            for (idx, bp) in guard.list().iter().enumerate() {
                let prefix = format!("debug.breakpoints.{idx}");
                values.push((format!("{prefix}.addr"), bp.addr));
                values.push((format!("{prefix}.enabled"), u64::from(bp.enabled)));
                values.push((format!("{prefix}.hit_count"), bp.hit_count));
                let (kind, arg) = match bp.action {
                    helm_debug::BreakAction::Break => (0, 0),
                    helm_debug::BreakAction::Log => (1, 0),
                    helm_debug::BreakAction::Callback(id) => (2, id),
                };
                values.push((format!("{prefix}.action_kind"), kind));
                values.push((format!("{prefix}.action_arg"), arg));
            }
        }

        #[cfg(feature = "instrumentation")]
        if let Some(engine) = &self.watchpoints {
            let guard = engine.lock().unwrap();
            values.push(("debug.watchpoints.count".to_string(), guard.count() as u64));
            for (idx, wp) in guard.list().iter().enumerate() {
                let prefix = format!("debug.watchpoints.{idx}");
                values.push((format!("{prefix}.start"), wp.range.start));
                values.push((format!("{prefix}.size"), wp.range.end - wp.range.start));
                values.push((format!("{prefix}.enabled"), u64::from(wp.enabled)));
                let kind = match wp.kind {
                    helm_debug::WatchKind::Read => 0,
                    helm_debug::WatchKind::Write => 1,
                    helm_debug::WatchKind::ReadWrite => 2,
                };
                values.push((format!("{prefix}.kind"), kind));
                let (action_kind, action_arg) = match wp.action {
                    helm_debug::WatchAction::Break => (0, 0),
                    helm_debug::WatchAction::Log => (1, 0),
                    helm_debug::WatchAction::Callback(id) => (2, id),
                };
                values.push((format!("{prefix}.action_kind"), action_kind));
                values.push((format!("{prefix}.action_arg"), action_arg));
            }
        }

        let refs: Vec<(&str, u64)> = values.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        let bytes = mgr.save_values(&refs);
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

        if sim.a64_state().is_some() {
            sim.with_a64_state_mut(|a64| {
                for (name, value) in &restored {
                    if name.starts_with("debug.") {
                        continue;
                    } else if name == "pc" {
                        a64.pc = *value;
                    } else if name == "sp" {
                        a64.write_xsp(31, *value);
                    } else if name == "nzcv" {
                        a64.nzcv = *value as u32;
                    } else if let Some(idx) =
                        name.strip_prefix('x').and_then(|s| s.parse::<usize>().ok())
                    {
                        if idx < 31 {
                            a64.x[idx] = *value;
                        }
                    }
                }
            })
            .ok_or_else(|| {
                pyo3::exceptions::PyRuntimeError::new_err(
                    "checkpoint restore could not access active AArch64 state",
                )
            })?;
        } else if sim.rv64_state().is_some() {
            sim.with_rv64_state_mut(|rv| {
                for (name, value) in &restored {
                    if name.starts_with("debug.") {
                        continue;
                    } else if name == "pc" {
                        rv.pc = *value;
                    } else if let Some(idx) =
                        name.strip_prefix('x').and_then(|s| s.parse::<usize>().ok())
                    {
                        if idx < rv.iregs.len() {
                            rv.iregs[idx] = if idx == 0 { 0 } else { *value };
                        }
                    }
                }
                rv.iregs[0] = 0;
            })
            .ok_or_else(|| {
                pyo3::exceptions::PyRuntimeError::new_err(
                    "checkpoint restore could not access active RISC-V state",
                )
            })?;
        } else {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "checkpoint restore requires an active ISA runtime",
            ));
        }

        #[cfg(feature = "instrumentation")]
        {
            use std::collections::HashMap;

            let map: HashMap<&str, u64> = restored.iter().map(|(k, v)| (k.as_str(), *v)).collect();

            if let Some(count) = map.get("debug.breakpoints.count").copied() {
                let engine = self.ensure_breakpoint_engine()?;
                let mut guard = engine.lock().unwrap();
                guard.clear();
                for idx in 0..count {
                    let prefix = format!("debug.breakpoints.{idx}");
                    let Some(addr) = map.get(format!("{prefix}.addr").as_str()).copied() else {
                        continue;
                    };
                    let enabled = map
                        .get(format!("{prefix}.enabled").as_str())
                        .copied()
                        .unwrap_or(1)
                        != 0;
                    let hit_count = map
                        .get(format!("{prefix}.hit_count").as_str())
                        .copied()
                        .unwrap_or(0);
                    let action_kind = map
                        .get(format!("{prefix}.action_kind").as_str())
                        .copied()
                        .unwrap_or(0);
                    let action_arg = map
                        .get(format!("{prefix}.action_arg").as_str())
                        .copied()
                        .unwrap_or(0);
                    let action = match action_kind {
                        1 => helm_debug::BreakAction::Log,
                        2 => helm_debug::BreakAction::Callback(action_arg),
                        _ => helm_debug::BreakAction::Break,
                    };
                    guard.add_with_state(addr, action, enabled, hit_count);
                }
            }

            if let Some(count) = map.get("debug.watchpoints.count").copied() {
                let engine = self.ensure_watchpoint_engine()?;
                let mut guard = engine.lock().unwrap();
                guard.clear();
                for idx in 0..count {
                    let prefix = format!("debug.watchpoints.{idx}");
                    let Some(start) = map.get(format!("{prefix}.start").as_str()).copied() else {
                        continue;
                    };
                    let size = map
                        .get(format!("{prefix}.size").as_str())
                        .copied()
                        .unwrap_or(0);
                    let enabled = map
                        .get(format!("{prefix}.enabled").as_str())
                        .copied()
                        .unwrap_or(1)
                        != 0;
                    let kind = match map
                        .get(format!("{prefix}.kind").as_str())
                        .copied()
                        .unwrap_or(1)
                    {
                        0 => helm_debug::WatchKind::Read,
                        2 => helm_debug::WatchKind::ReadWrite,
                        _ => helm_debug::WatchKind::Write,
                    };
                    let action_kind = map
                        .get(format!("{prefix}.action_kind").as_str())
                        .copied()
                        .unwrap_or(0);
                    let action_arg = map
                        .get(format!("{prefix}.action_arg").as_str())
                        .copied()
                        .unwrap_or(0);
                    let action = match action_kind {
                        1 => helm_debug::WatchAction::Log,
                        2 => helm_debug::WatchAction::Callback(action_arg),
                        _ => helm_debug::WatchAction::Break,
                    };
                    guard.add_with_state(start, size, kind, action, enabled);
                }
            }
        }

        Ok(restored.len())
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
        if let Some(engine) = &self.breakpoints {
            let guard = engine.lock().unwrap();
            return guard
                .list()
                .iter()
                .map(|bp| {
                    let action = match bp.action {
                        helm_debug::BreakAction::Break => "break",
                        helm_debug::BreakAction::Log => "log",
                        helm_debug::BreakAction::Callback(_) => "callback",
                    };
                    (bp.id, bp.addr, action.to_string(), bp.enabled, bp.hit_count)
                })
                .collect();
        }
        Vec::new()
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
        if let Some(engine) = &self.watchpoints {
            let guard = engine.lock().unwrap();
            return guard
                .list()
                .iter()
                .map(|wp| {
                    let kind = match wp.kind {
                        helm_debug::WatchKind::Read => "read",
                        helm_debug::WatchKind::Write => "write",
                        helm_debug::WatchKind::ReadWrite => "rw",
                    };
                    let action = match wp.action {
                        helm_debug::WatchAction::Break => "break",
                        helm_debug::WatchAction::Log => "log",
                        helm_debug::WatchAction::Callback(_) => "callback",
                    };
                    (
                        wp.id,
                        wp.range.start,
                        wp.range.end - wp.range.start,
                        kind.to_string(),
                        action.to_string(),
                        wp.enabled,
                    )
                })
                .collect();
        }
        Vec::new()
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
    #[cfg(feature = "instrumentation")]
    fn ensure_breakpoint_engine(&mut self) -> PyResult<Arc<Mutex<helm_debug::BreakpointEngine>>> {
        use helm_debug::{BreakAction, BreakResult, BreakpointEngine};

        if let Some(engine) = &self.breakpoints {
            return Ok(Arc::clone(engine));
        }

        let engine = Arc::new(Mutex::new(BreakpointEngine::new()));
        let probe_engine = Arc::clone(&engine);
        let sim = self.require_sim()?;
        sim.probes_mut().pre_step.subscribe(move |event| {
            if let Ok(mut guard) = probe_engine.lock() {
                match guard.check(event.pc) {
                    BreakResult::Hit { addr, action, .. } => match action {
                        BreakAction::Log => eprintln!("[breakpoint] hit at {addr:#x}"),
                        BreakAction::Break => eprintln!("[breakpoint] break at {addr:#x}"),
                        BreakAction::Callback(id) => {
                            eprintln!("[breakpoint] callback id={id} at {addr:#x}")
                        }
                    },
                    BreakResult::None => {}
                }
            }
        });
        self.breakpoints = Some(Arc::clone(&engine));
        Ok(engine)
    }

    #[cfg(feature = "instrumentation")]
    fn ensure_watchpoint_engine(&mut self) -> PyResult<Arc<Mutex<helm_debug::WatchpointEngine>>> {
        use helm_debug::{WatchAction, WatchResult, WatchpointEngine};

        if let Some(engine) = &self.watchpoints {
            return Ok(Arc::clone(engine));
        }

        let engine = Arc::new(Mutex::new(WatchpointEngine::new()));
        let probe_engine = Arc::clone(&engine);
        let sim = self.require_sim()?;
        sim.probes_mut().mem.subscribe(move |event| {
            if let Ok(guard) = probe_engine.lock() {
                match guard.check(event.addr, usize::from(event.size), event.is_store) {
                    WatchResult::Hit {
                        addr,
                        size,
                        is_store,
                        action,
                        ..
                    } => match action {
                        WatchAction::Log => eprintln!(
                            "[watchpoint] {} at {addr:#x} size={size}",
                            if is_store { "write" } else { "read" }
                        ),
                        WatchAction::Break => eprintln!(
                            "[watchpoint] break on {} at {addr:#x} size={size}",
                            if is_store { "write" } else { "read" }
                        ),
                        WatchAction::Callback(id) => eprintln!(
                            "[watchpoint] callback id={id} on {} at {addr:#x} size={size}",
                            if is_store { "write" } else { "read" }
                        ),
                    },
                    WatchResult::None => {}
                }
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
}

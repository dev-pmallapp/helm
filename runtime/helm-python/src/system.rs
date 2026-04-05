#![allow(missing_docs)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use helm_devices::DeviceParams;
#[cfg(test)]
use helm_engine::{build_simulator, Isa};
use helm_engine::{ExecMode, HelmSim, StopReason, TimingChoice, TimingMemModelConfig};
use pyo3::prelude::*;
use pyo3::types::PyDict;

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

    pub(crate) sim: Option<HelmSim>,
    pub(crate) exited: bool,
    pub(crate) exit_code_val: i32,
    pub(crate) plugins: Vec<Box<dyn helm_engine::helm_plugin::api::HelmPlugin>>,
}

#[pymethods]
impl HelmSystem {
    #[new]
    #[pyo3(signature = (name, *, timing = "virtual", mode = "se", ipc = 4.0))]
    fn new(name: &str, timing: &str, mode: &str, ipc: f64) -> (Self, SimObject) {
        (
            HelmSystem {
                timing: timing.into(),
                mode: mode.into(),
                ipc,
                sim: None,
                exited: false,
                exit_code_val: 0,
                plugins: Vec::new(),
            },
            SimObject::new(name),
        )
    }

    /// Freeze config and create all Rust simulation objects.
    fn instantiate(slf: PyRefMut<'_, Self>, py: Python<'_>) -> PyResult<()> {
        crate::instantiate::instantiate_system(slf, py)
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
        }
    }

    // ── ELF / Kernel loading ─────────────────────────────────────────────────

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
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e))
    }

    /// Load an ARM64 Linux kernel Image and configure FS mode.
    #[pyo3(signature = (kernel, dtb=None, dtb_bytes=None, initrd=None, append=None, num_cpus=1, gic_version="v3"))]
    fn load_kernel(
        &mut self,
        kernel: &str,
        dtb: Option<&str>,
        dtb_bytes: Option<Vec<u8>>,
        initrd: Option<&str>,
        append: Option<&str>,
        num_cpus: usize,
        gic_version: &str,
    ) -> PyResult<()> {
        let sim = self.require_sim()?;
        let gic_version = match gic_version {
            "v2" => helm_engine::platform::arm_virt::ArmVirtGicVersion::V2,
            "v3" => helm_engine::platform::arm_virt::ArmVirtGicVersion::V3,
            other => {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "unknown gic_version '{other}' (expected 'v2' or 'v3')"
                )));
            }
        };
        match (dtb, dtb_bytes) {
            (Some(path), None) => sim
                .load_aarch64_kernel(kernel, path, initrd, append, num_cpus, gic_version)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e)),
            (None, Some(bytes)) => sim
                .load_aarch64_kernel_dtb_bytes(
                    kernel,
                    &bytes,
                    initrd,
                    append,
                    num_cpus,
                    gic_version,
                )
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e)),
            (Some(_), Some(_)) => Err(pyo3::exceptions::PyValueError::new_err(
                "pass either dtb or dtb_bytes, not both",
            )),
            (None, None) => Err(pyo3::exceptions::PyValueError::new_err(
                "load_kernel requires either dtb or dtb_bytes",
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

        let warnings = py.import_bound("warnings")?;
        warnings.call_method1(
            "warn",
            (
                "add_plugin() is deprecated, use system.observe() API instead",
                py.get_type_bound::<pyo3::exceptions::PyDeprecationWarning>(),
            ),
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
            "trace-window-fault" => {
                Box::new(helm_engine::helm_plugin::builtins::debug::TraceWindowFault::new())
            }
            "cache" => Box::new(helm_engine::helm_plugin::builtins::memory::CacheSim::new()),
            "mem-trace" => Box::new(helm_engine::helm_plugin::builtins::memory::MemTrace::new()),
            "branch-trace" => {
                Box::new(helm_engine::helm_plugin::builtins::trace::BranchTrace::new())
            }
            "watchpoint" => Box::new(helm_engine::helm_plugin::builtins::debug::Watchpoint::new()),
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
    ))]
    fn spy(
        &mut self,
        cache_l1d_size: Option<usize>,
        cache_l1d_ways: usize,
        cache_l1d_line: usize,
        predictor: Option<&str>,
        predictor_bits: u8,
        predictor_table_bits: Option<u8>,
    ) -> PyResult<HelmSpy> {
        let sim = self.require_sim()?;
        crate::spy::build_spy_session(
            sim,
            cache_l1d_size,
            cache_l1d_ways,
            cache_l1d_line,
            predictor,
            predictor_bits,
            predictor_table_bits,
        )
    }

    // ── Ergonomic tracing API ────────────────────────────────────────────────

    #[pyo3(signature = (*, insn_count=None, pc=None, symbol=None, events=None, max=None, writes_only=false))]
    fn trace_after(
        &mut self,
        insn_count: Option<u64>,
        pc: Option<u64>,
        symbol: Option<&str>,
        events: Option<Vec<String>>,
        max: Option<usize>,
        writes_only: bool,
    ) -> PyResult<()> {
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

    /// Set a memory watchpoint.
    #[pyo3(signature = (addr, size=8, writes_only=true))]
    fn watch(&mut self, addr: u64, size: u64, writes_only: bool) -> PyResult<()> {
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
    ))]
    fn observe(
        &mut self,
        cache_l1d_size: Option<usize>,
        cache_l1d_ways: usize,
        cache_l1d_line: usize,
        predictor: Option<&str>,
        predictor_bits: u8,
        predictor_table_bits: Option<u8>,
    ) -> PyResult<HelmSpy> {
        let sim = self.require_sim()?;
        crate::spy::build_spy_session(
            sim,
            cache_l1d_size,
            cache_l1d_ways,
            cache_l1d_line,
            predictor,
            predictor_bits,
            predictor_table_bits,
        )
    }

    /// Set a PC breakpoint that stops execution.
    #[pyo3(signature = (pc, action="break"))]
    fn breakpoint(&mut self, pc: u64, action: &str) -> PyResult<()> {
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

    /// Set a memory watchpoint.
    #[pyo3(signature = (addr, size=8, kind="write"))]
    fn watchpoint(&mut self, addr: u64, size: u64, kind: &str) -> PyResult<()> {
        let writes_only = kind == "write";
        self.watch(addr, size, writes_only)
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
            sim: Some(sim),
            exited: false,
            exit_code_val: 0,
            plugins: Vec::new(),
        }
    }

    #[test]
    fn current_cycles_defaults_to_zero_when_uninstantiated() {
        let system = HelmSystem {
            timing: "virtual".into(),
            mode: "functional".into(),
            ipc: 1.0,
            sim: None,
            exited: false,
            exit_code_val: 0,
            plugins: Vec::new(),
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
}

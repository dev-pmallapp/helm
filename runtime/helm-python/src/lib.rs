//! `helm-python` — PyO3 bindings for the helm-ng simulator.
//!
//! Exposes the `_helm_ng` module to Python with:
//!
//! - `Simulation` — build a simulator, load ELF, run, inspect registers
//! - `build_simulation()` — constructor with keyword args

#![allow(missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use helm_engine::{build_simulator, ExecMode, Isa, StopReason, TimingChoice};
use pyo3::prelude::*;

// ── Simulation ────────────────────────────────────────────────────────────────

/// Python-facing simulation handle.
#[pyclass(name = "Simulation")]
pub struct PySimulation {
    inner: helm_engine::HelmSim,
    exited: bool,
    exit_code_val: i32,
    plugins: Vec<Box<dyn helm_engine::helm_plugin::api::HelmPlugin>>,
}

#[pymethods]
impl PySimulation {
    /// Load a static AArch64 ELF binary and configure SE mode.
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

    /// Set the ARM CPU core model, configuring ID registers and feature bits.
    ///
    /// Accepted names (case-insensitive): `"generic"`, `"cortex-a55"`, `"cortex-a73"`,
    /// `"neoverse-n1"`, `"cortex-a78"`, `"cortex-x1"`, `"cortex-a510"`, `"cortex-a710"`.
    ///
    /// Raises `ValueError` for unknown names.
    fn set_cpu_model(&mut self, model: &str) -> PyResult<()> {
        self.inner.set_cpu_model(model)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(
                format!("{e}. Valid: generic, cortex-a55, cortex-a73, neoverse-n1, cortex-a78, cortex-x1, cortex-a510, cortex-a710")
            ))
    }

    /// Load an ARM64 Linux kernel Image and configure FS mode.
    ///
    /// `append` overrides the DTB `/chosen/bootargs` property with the highest
    /// precedence (beats DTB bootargs and kernel built-in cmdline).
    #[pyo3(signature = (kernel, dtb, initrd=None, append=None))]
    fn load_kernel(
        &mut self,
        kernel: &str,
        dtb: &str,
        initrd: Option<&str>,
        append: Option<&str>,
    ) -> PyResult<()> {
        self.inner
            .load_aarch64_kernel(kernel, dtb, initrd, append)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e))
    }

    /// Run up to `max_insns` guest instructions.
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
            StopReason::Exception(e) => format!("exception:{e}"),
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

    /// True if the guest executed any instruction sites that are still stubbed.
    #[getter]
    fn has_unimplemented_instructions(&self) -> bool {
        self.inner.has_unimplemented_instructions()
    }

    /// Count of unique stubbed instruction sites encountered.
    #[getter]
    fn unimplemented_instruction_count(&self) -> usize {
        self.inner.unimplemented_instruction_count()
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

    /// Read SIMD/FP register Vn (0-31) as a (lo64, hi64) tuple.
    fn vn(&self, n: usize) -> (u64, u64) {
        let state = match &self.inner {
            helm_engine::HelmSim::Virtual(e)  => e.a64_state.as_ref(),
            helm_engine::HelmSim::Interval(e) => e.a64_state.as_ref(),
            helm_engine::HelmSim::Accurate(e) => e.a64_state.as_ref(),
        };
        state.map_or((0, 0), |s| {
            let val = s.v[n];
            (val as u64, (val >> 64) as u64)
        })
    }

    /// Read NZCV flags.
    #[getter]
    fn nzcv(&self) -> u32 {
        let state = match &self.inner {
            helm_engine::HelmSim::Virtual(e)  => e.a64_state.as_ref(),
            helm_engine::HelmSim::Interval(e) => e.a64_state.as_ref(),
            helm_engine::HelmSim::Accurate(e) => e.a64_state.as_ref(),
        };
        state.map_or(0, |s| s.nzcv)
    }

    /// Install a built-in plugin by name.
    ///
    /// Supported: ``"stub-tracer"``, ``"insn-count"``, ``"syscall-trace"``,
    /// ``"hotblocks"``, ``"howvec"``, ``"execlog"``, ``"fault-detect"``,
    /// ``"cache"``, ``"mem-trace"``, ``"branch-trace"``, ``"watchpoint"``.
    #[pyo3(signature = (name, args=""))]
    fn add_plugin(&mut self, name: &str, args: &str) -> PyResult<()> {
        use helm_engine::helm_plugin::api::{HelmPlugin, PluginArgs};

        let pargs = PluginArgs::parse(args);
        let reg = self.inner.plugins_mut();

        let mut plugin: Box<dyn HelmPlugin> = match name {
            "stub-tracer" => Box::new(helm_engine::helm_plugin::builtins::debug::StubTracer::new()),
            "insn-count"  => Box::new(helm_engine::helm_plugin::builtins::trace::InsnCount::new()),
            "syscall-trace" => Box::new(helm_engine::helm_plugin::builtins::trace::SyscallTrace::new()),
            "hotblocks"   => Box::new(helm_engine::helm_plugin::builtins::trace::HotBlocks::new()),
            "howvec"      => Box::new(helm_engine::helm_plugin::builtins::trace::HowVec::new()),
            "execlog"     => Box::new(helm_engine::helm_plugin::builtins::trace::ExecLog::new()),
            "fault-detect" => Box::new(helm_engine::helm_plugin::builtins::debug::FaultDetect::new()),
            "cache"       => Box::new(helm_engine::helm_plugin::builtins::memory::CacheSim::new()),
            "mem-trace"   => Box::new(helm_engine::helm_plugin::builtins::memory::MemTrace::new()),
            "branch-trace" => Box::new(helm_engine::helm_plugin::builtins::trace::BranchTrace::new()),
            "watchpoint"  => Box::new(helm_engine::helm_plugin::builtins::debug::Watchpoint::new()),
            other => return Err(pyo3::exceptions::PyValueError::new_err(
                format!("unknown plugin '{other}'"),
            )),
        };
        plugin.install(reg, &pargs);
        self.plugins.push(plugin);
        Ok(())
    }

    fn set_pc(&mut self, pc: u64) {
        self.inner.set_pc(pc);
    }

    /// Call atexit on all plugins (prints reports).
    fn finish(&mut self) {
        for p in &mut self.plugins {
            p.atexit();
        }
    }

    fn load_bytes(&mut self, addr: u64, data: Vec<u8>) {
        self.inner.load_bytes(addr, &data);
    }

    /// Read 8 bytes from guest memory (for debugging).
    fn read_mem(&mut self, addr: u64) -> u64 {
        use helm_core::{AccessType, MemInterface};
        match &mut self.inner {
            helm_engine::HelmSim::Virtual(e)  => e.memory.read(addr, 8, AccessType::Load).unwrap_or(0xDEAD),
            helm_engine::HelmSim::Interval(e) => e.memory.read(addr, 8, AccessType::Load).unwrap_or(0xDEAD),
            helm_engine::HelmSim::Accurate(e) => e.memory.read(addr, 8, AccessType::Load).unwrap_or(0xDEAD),
        }
    }

    // ── Symbol table API ─────────────────────────────────────────────────────

    /// Resolve a symbol name to its virtual address.
    ///
    /// Returns ``None`` if the symbol is not found in the ELF symbol table.
    fn resolve_symbol(&self, name: &str) -> Option<u64> {
        self.inner.resolve_symbol(name)
    }

    /// Return all loaded symbols as ``[(name, addr, size), ...]``.
    fn symbols(&self) -> Vec<(String, u64, u64)> {
        self.inner.symbols().iter().map(|s| (s.name.clone(), s.addr, s.size)).collect()
    }

    // ── Ergonomic tracing API ────────────────────────────────────────────────

    /// Start tracing events after a trigger condition is met.
    ///
    /// Trigger types (exactly one required):
    ///   - ``insn_count``: fire after this many instructions have retired
    ///   - ``pc``: fire when execution reaches this address
    ///   - ``symbol``: fire when execution reaches this symbol name
    ///
    /// Events to enable (list of strings):
    ///   - ``"mem"``: memory access tracing
    ///   - ``"branch"``: branch tracing
    ///   - ``"insn"``: instruction tracing (execlog)
    ///   - ``"all"``: all of the above
    ///
    /// Options:
    ///   - ``max``: max events to record per plugin (default: unlimited)
    ///   - ``writes_only``: for mem tracing, only record writes (default: false)
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
        // Resolve trigger to a concrete mechanism
        let trigger_pc = match (insn_count, pc, symbol) {
            (Some(_), None, None) => None, // insn_count-based trigger, handled below
            (None, Some(addr), None) => Some(addr),
            (None, None, Some(sym)) => {
                let addr = self.inner.resolve_symbol(sym).ok_or_else(||
                    pyo3::exceptions::PyValueError::new_err(format!("symbol '{sym}' not found"))
                )?;
                Some(addr)
            }
            _ => return Err(pyo3::exceptions::PyValueError::new_err(
                "exactly one of insn_count, pc, or symbol must be specified"
            )),
        };

        let events = events.unwrap_or_else(|| vec!["all".into()]);
        let want_mem = events.iter().any(|e| e == "mem" || e == "all");
        let want_branch = events.iter().any(|e| e == "branch" || e == "all");
        let want_insn = events.iter().any(|e| e == "insn" || e == "all");

        // Shared activation flag
        let active = Arc::new(AtomicBool::new(false));
        let max_events = max.unwrap_or(usize::MAX);

        let reg = self.inner.plugins_mut();

        // Set up trigger
        if let Some(threshold) = insn_count {
            // Timer-based trigger: activate after N instructions
            let flag = Arc::clone(&active);
            reg.on_timer(1, Box::new(move |_vcpu, count| {
                if count >= threshold && !flag.load(Ordering::Relaxed) {
                    flag.store(true, Ordering::Relaxed);
                    eprintln!("[trace_after] activated at insn_count={count}");
                }
            }));
        } else if let Some(addr) = trigger_pc {
            // PC-based trigger: activate when hitting the address
            let flag = Arc::clone(&active);
            reg.on_insn_exec(Box::new(move |_vcpu, insn| {
                if insn.pc == addr && !flag.load(Ordering::Relaxed) {
                    flag.store(true, Ordering::Relaxed);
                    eprintln!("[trace_after] activated at pc={:#x}", insn.pc);
                }
            }));
        }

        // Register conditional event loggers
        if want_mem {
            let flag = Arc::clone(&active);
            let counter = Arc::new(AtomicU64::new(0));
            let max_e = max_events as u64;
            let filter = if writes_only {
                helm_engine::helm_plugin::runtime::MemFilter::WritesOnly
            } else {
                helm_engine::helm_plugin::runtime::MemFilter::All
            };
            reg.on_mem_access(filter, Box::new(move |_vcpu, info| {
                if !flag.load(Ordering::Relaxed) { return; }
                let n = counter.fetch_add(1, Ordering::Relaxed);
                if n >= max_e { return; }
                let tag = if info.is_store { "W" } else { "R" };
                let atomic = if info.is_atomic { " atomic" } else { "" };
                eprintln!("[trace_after:mem] [{tag}] {:#018x} {}{}", info.vaddr, info.size, atomic);
            }));
        }

        if want_branch {
            let flag = Arc::clone(&active);
            let counter = Arc::new(AtomicU64::new(0));
            let max_e = max_events as u64;
            reg.on_branch(Box::new(move |_vcpu, info| {
                if !flag.load(Ordering::Relaxed) { return; }
                let n = counter.fetch_add(1, Ordering::Relaxed);
                if n >= max_e { return; }
                let dir = if info.taken { "T" } else { "N" };
                eprintln!("[trace_after:branch] {:#018x} -> {:#018x} [{dir}] {:?}",
                    info.pc, info.target, info.kind);
            }));
        }

        if want_insn {
            let flag = Arc::clone(&active);
            let counter = Arc::new(AtomicU64::new(0));
            let max_e = max_events as u64;
            reg.on_insn_exec(Box::new(move |vcpu, insn| {
                if !flag.load(Ordering::Relaxed) { return; }
                let n = counter.fetch_add(1, Ordering::Relaxed);
                if n >= max_e { return; }
                eprintln!("[trace_after:insn] vcpu={vcpu} pc={:#018x} raw={:#010x}",
                    insn.pc, insn.raw);
            }));
        }

        Ok(())
    }

    /// Set a memory watchpoint from Python.
    ///
    /// Fires a log message when the watched address range is accessed.
    #[pyo3(signature = (addr, size=8, writes_only=true))]
    fn watch(&mut self, addr: u64, size: u64, writes_only: bool) -> PyResult<()> {
        use helm_engine::helm_plugin::api::{HelmPlugin, PluginArgs};

        let mut plugin = Box::new(
            helm_engine::helm_plugin::builtins::debug::Watchpoint::with_addr(addr, size, writes_only, None)
        );
        let pargs = PluginArgs::parse("");
        let reg = self.inner.plugins_mut();
        plugin.install(reg, &pargs);
        self.plugins.push(plugin);
        Ok(())
    }
}

// ── build_simulation() ───────────────────────────────────────────────────────

/// Create a new simulation.
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
        plugins: Vec::new(),
    })
}

// ── Module ────────────────────────────────────────────────────────────────────


// ── set_sim_trace() ───────────────────────────────────────────────────────────

/// Install a sim-trace monitor on the current Python thread.
///
/// Call this before `sim.run()` to capture sim-trace events (BRNC, STUB,
/// WARN, etc.) to a file or TCP stream.
///
/// URI formats:
///   ``"stderr:"``       write to stderr (default when no monitor installed)
///   ``"file:/path"``    append to a file
///   ``"tcp:host:port"`` stream to a TCP listener
///   ``"null:"``         discard all events
///
/// Returns the URI that was configured.
///
/// Example::
///
///     import _helm_ng
///     _helm_ng.set_sim_trace("file:/tmp/trace.log")
///     sim = _helm_ng.build_simulation(mode="fs", ...)
///     sim.run(1_000_000)
///
/// The sink is owned by a thread-local and is flushed/drained when the
/// Python process exits or when ``stop_sim_trace()`` is called.
#[pyfunction]
#[pyo3(signature = (uri = "stderr:"))]
fn set_sim_trace(uri: &str) -> PyResult<String> {
    use helm_debug::sim_trace::{MonitorSink, install_monitor};
    let (sink, monitor) = MonitorSink::open(uri)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(
            format!("cannot open sim-trace backend '{uri}': {e}")
        ))?;
    install_monitor(monitor);
    // Leak the sink so it lives for the process lifetime.
    // The background drain thread will flush on process exit.
    std::mem::forget(sink);
    Ok(uri.to_string())
}

#[pymodule]
pub fn _helm_ng(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PySimulation>()?;
    m.add_function(wrap_pyfunction!(build_simulation, m)?)?;
    m.add_function(wrap_pyfunction!(set_sim_trace, m)?)?;
    Ok(())
}

#![allow(missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use helm_engine::{ExecMode, HelmSim, StopReason, TimingChoice};
use pyo3::prelude::*;

use crate::simobject::SimObject;
use crate::spy::PySpySession;

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
    match s {
        "virtual" => Ok(TimingChoice::Virtual { ipc }),
        "interval" => Ok(TimingChoice::Interval {
            ipc,
            interval_len: 10_000,
        }),
        "accurate" => Ok(TimingChoice::Accurate),
        other => Err(pyo3::exceptions::PyValueError::new_err(format!(
            "unknown timing '{other}'"
        ))),
    }
}

/// Top-level simulation container.
///
/// Before `instantiate()`: holds config fields (timing, mode, ipc).
/// After `instantiate()`: wraps a live `HelmSim` with register access and run().
#[pyclass(name = "System", extends = SimObject)]
pub struct System {
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
impl System {
    #[new]
    #[pyo3(signature = (name, *, timing = "virtual", mode = "se", ipc = 4.0))]
    fn new(name: &str, timing: &str, mode: &str, ipc: f64) -> (Self, SimObject) {
        (
            System {
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
    fn instantiate(mut slf: PyRefMut<'_, Self>) -> PyResult<()> {
        use helm_engine::{build_simulator, Isa};

        let base: &SimObject = slf.as_ref();
        base.require_pending()?;

        if slf.sim.is_some() {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "system is already instantiated",
            ));
        }

        let mode_val = parse_mode(&slf.mode)?;
        let timing_val = parse_timing(&slf.timing, slf.ipc)?;

        let sim = build_simulator(Isa::AArch64, mode_val, timing_val, 0x0, 512 * 1024 * 1024);

        slf.sim = Some(sim);

        // Mark system as instantiated
        let base: &mut SimObject = slf.as_mut();
        base.state = crate::simobject::SimObjectState::Instantiated;

        Ok(())
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
    #[pyo3(signature = (kernel, dtb, initrd=None, append=None, num_cpus=1))]
    fn load_kernel(
        &mut self,
        kernel: &str,
        dtb: &str,
        initrd: Option<&str>,
        append: Option<&str>,
        num_cpus: usize,
    ) -> PyResult<()> {
        let sim = self.require_sim()?;
        sim.load_aarch64_kernel(kernel, dtb, initrd, append, num_cpus)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e))
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
            .map_or(0, |s| if n < 31 { s.x[n] } else { s.sp })
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
    fn add_plugin(&mut self, name: &str, args: &str) -> PyResult<()> {
        use helm_engine::helm_plugin::api::{HelmPlugin, PluginArgs};

        let pargs = PluginArgs::parse(args);
        let sim = self.require_sim()?;
        let reg = sim.plugins_mut();

        let mut plugin: Box<dyn HelmPlugin> = match name {
            "stub-tracer" => {
                Box::new(helm_engine::helm_plugin::builtins::debug::StubTracer::new())
            }
            "insn-count" => {
                Box::new(helm_engine::helm_plugin::builtins::trace::InsnCount::new())
            }
            "syscall-trace" => {
                Box::new(helm_engine::helm_plugin::builtins::trace::SyscallTrace::new())
            }
            "hotblocks" => {
                Box::new(helm_engine::helm_plugin::builtins::trace::HotBlocks::new())
            }
            "howvec" => Box::new(helm_engine::helm_plugin::builtins::trace::HowVec::new()),
            "execlog" => Box::new(helm_engine::helm_plugin::builtins::trace::ExecLog::new()),
            "fault-detect" => {
                Box::new(helm_engine::helm_plugin::builtins::debug::FaultDetect::new())
            }
            "trace-window-fault" => {
                Box::new(helm_engine::helm_plugin::builtins::debug::TraceWindowFault::new())
            }
            "cache" => Box::new(helm_engine::helm_plugin::builtins::memory::CacheSim::new()),
            "mem-trace" => {
                Box::new(helm_engine::helm_plugin::builtins::memory::MemTrace::new())
            }
            "branch-trace" => {
                Box::new(helm_engine::helm_plugin::builtins::trace::BranchTrace::new())
            }
            "watchpoint" => {
                Box::new(helm_engine::helm_plugin::builtins::debug::Watchpoint::new())
            }
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

    /// Create an observation session (backward-compat — prefer SpySession standalone).
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
    ) -> PyResult<PySpySession> {
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
        use helm_engine::helm_plugin::api::{HelmPlugin, PluginArgs};

        let sim = self.require_sim()?;
        let mut plugin = Box::new(
            helm_engine::helm_plugin::builtins::debug::Watchpoint::with_addr(
                addr,
                size,
                writes_only,
                None,
            ),
        );
        let pargs = PluginArgs::parse("");
        let reg = sim.plugins_mut();
        plugin.install(reg, &pargs);
        self.plugins.push(plugin);
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
        self.sim
            .as_ref()
            .map_or_else(Vec::new, |s| {
                s.symbols()
                    .iter()
                    .map(|sym| (sym.name.clone(), sym.addr, sym.size))
                    .collect()
            })
    }
}

impl System {
    pub(crate) fn require_sim(&mut self) -> PyResult<&mut HelmSim> {
        self.sim.as_mut().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "operation requires an instantiated system (call instantiate() or build_simulation())",
            )
        })
    }
}

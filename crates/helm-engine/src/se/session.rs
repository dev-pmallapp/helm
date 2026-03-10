//! Suspendable SE-mode session.
//!
//! [`SeSession`] owns all simulation state and exposes incremental
//! execution methods so callers can pause, inspect, hot-load plugins,
//! and resume.
//!
//! ```text
//! let mut s = SeSession::new("./binary", &["binary"], &[])?;
//! s.run_until_insns(1_000_000);        // warm-up without plugins
//! s.add_plugin("fault-detect", "");     // hot-load
//! s.run_until_pc(0x411120);             // run to entry
//! s.run(10_000_000);                    // continue with plugin active
//! println!("{:?}", s.result());
//! ```

use crate::loader;
use crate::loader::TlsInfo;
use crate::monitor::MonitorTarget;
use crate::se::backend::ExecBackend;
use crate::se::linux::{exec_interp, exec_tcg};
use crate::se::thread::Scheduler;
use crate::symbols::SymbolTable;
use helm_core::HelmError;
use helm_isa::arm::aarch64::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;
use helm_plugin::api::ComponentRegistry;
use helm_plugin::api::PluginArgs;
use helm_plugin::runtime::PluginComponentAdapter;
use helm_plugin::PluginRegistry;
use helm_syscall::Aarch64SyscallHandler;
use helm_timing::TimingModel;

/// Why `run*` returned control to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// Reached the instruction budget.
    InsnLimit,
    /// Hit the requested PC breakpoint.
    Breakpoint { pc: u64 },
    /// Guest called exit.
    Exited { code: u64 },
    /// An unrecoverable error occurred.
    Error(String),
}

/// Suspendable SE-mode simulation session.
///
/// Owns CPU, memory, syscall handler, scheduler, timing, backend,
/// and the plugin registry — everything needed to pause and resume.
pub struct SeSession {
    cpu: Aarch64Cpu,
    mem: AddressSpace,
    syscall: Aarch64SyscallHandler,
    sched: Scheduler,
    tls_info: Option<TlsInfo>,
    backend: ExecBackend,
    plugin_reg: PluginRegistry,
    comp_reg: ComponentRegistry,
    adapters: Vec<PluginComponentAdapter>,
    timing: Box<dyn TimingModel>,
    insn_count: u64,
    virtual_cycles: u64,
    exited: bool,
    exit_code: u64,
    symbols: SymbolTable,
}

impl SeSession {
    /// Create a new session by loading a binary.
    pub fn new(binary_path: &str, argv: &[&str], envp: &[&str]) -> Result<Self, HelmError> {
        Self::with_timing(
            binary_path,
            argv,
            envp,
            Box::new(helm_timing::model::FeModel),
        )
    }

    /// Create a session with a specific timing model.
    pub fn with_timing(
        binary_path: &str,
        argv: &[&str],
        envp: &[&str],
        timing: Box<dyn TimingModel>,
    ) -> Result<Self, HelmError> {
        let loaded = loader::load_elf(binary_path, argv, envp)?;
        let tls_info = loaded.tls_info.clone();
        let mut mem = loaded.address_space;
        let mut cpu = Aarch64Cpu::new();
        cpu.set_se_mode(true);
        cpu.regs.pc = loaded.entry_point;
        cpu.regs.sp = loaded.initial_sp;
        mem.map(0, 0x1000, (true, false, false));

        let mut syscall = Aarch64SyscallHandler::new();
        syscall.set_brk(loaded.brk_base);
        syscall.binary_path = std::fs::canonicalize(binary_path)
            .unwrap_or_else(|_| std::path::PathBuf::from(binary_path))
            .to_string_lossy()
            .into_owned();
        mem.map(loaded.brk_base, 0x1000, (true, true, false));

        let sched = Scheduler::new(cpu.regs.clone(), 1000);

        let mut comp_reg = ComponentRegistry::new();
        helm_plugin::runtime::register_builtins(&mut comp_reg);

        // Extract symbols from the ELF binary
        let elf_data = std::fs::read(binary_path).unwrap_or_default();
        let symbols = SymbolTable::from_elf(&elf_data);

        Ok(Self {
            cpu,
            mem,
            syscall,
            sched,
            tls_info,
            backend: ExecBackend::interpretive(),
            plugin_reg: PluginRegistry::new(),
            comp_reg,
            adapters: Vec::new(),
            timing,
            insn_count: 0,
            virtual_cycles: 0,
            exited: false,
            exit_code: 0,
            symbols,
        })
    }
    /// Switch execution backend (interpretive → TCG or vice-versa).
    pub fn set_backend(&mut self, backend: ExecBackend) {
        self.backend = backend;
    }

    /// Hot-load a plugin by name.  Can be called between `run*` calls.
    ///
    /// Returns `true` if the plugin was found and installed.
    pub fn add_plugin(&mut self, name: &str, args: &str) -> bool {
        let fqn = resolve_plugin_name(name);
        let plugin_args = if args.is_empty() {
            PluginArgs::new()
        } else {
            PluginArgs::parse(args)
        };
        match self.comp_reg.create(&fqn) {
            Some(comp) => {
                let raw = Box::into_raw(comp);
                let mut adapter = unsafe { *Box::from_raw(raw as *mut PluginComponentAdapter) };
                adapter.install(&mut self.plugin_reg, &plugin_args);
                self.adapters.push(adapter);
                true
            }
            None => false,
        }
    }

    /// Run up to `n` instructions, then return.
    pub fn run(&mut self, max_insns: u64) -> StopReason {
        self.run_inner(max_insns, None)
    }

    /// Run until `total` instructions have executed since session start.
    pub fn run_until_insns(&mut self, total: u64) -> StopReason {
        if self.insn_count >= total {
            return StopReason::InsnLimit;
        }
        self.run_inner(total - self.insn_count, None)
    }

    /// Run until PC equals `target` (or up to `max_insns` as a safety limit).
    pub fn run_until_pc(&mut self, target: u64, max_insns: u64) -> StopReason {
        self.run_inner(max_insns, Some(target))
    }

    /// Current guest PC.
    pub fn pc(&self) -> u64 {
        self.cpu.regs.pc
    }

    /// Total instructions executed so far.
    pub fn insn_count(&self) -> u64 {
        self.insn_count
    }

    /// Total virtual cycles so far.
    pub fn virtual_cycles(&self) -> u64 {
        self.virtual_cycles
    }

    /// Whether the guest has exited.
    pub fn has_exited(&self) -> bool {
        self.exited
    }

    /// Guest exit code (valid only if `has_exited()`).
    pub fn exit_code(&self) -> u64 {
        self.exit_code
    }

    /// Run until a named symbol is reached.
    pub fn run_until_symbol(&mut self, sym: &str, max_insns: u64) -> StopReason {
        match self.symbols.lookup(sym) {
            Some(addr) => self.run_until_pc(addr, max_insns),
            None => StopReason::Error(format!("symbol not found: {sym}")),
        }
    }

    /// Read a general-purpose register.
    pub fn xn(&self, n: u32) -> u64 {
        self.cpu.xn(n as u16)
    }

    /// Read memory at an address.
    pub fn read_memory(&self, addr: u64, size: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; size];
        let mem_ptr = &self.mem as *const AddressSpace as *mut AddressSpace;
        unsafe {
            match (*mem_ptr).read(addr, &mut buf) {
                Ok(()) => Some(buf),
                Err(_) => None,
            }
        }
    }

    /// Call `atexit` on all loaded plugins.
    pub fn finish(&mut self) {
        for adapter in &mut self.adapters {
            adapter.atexit();
        }
    }

    // ── internal ────────────────────────────────────────────────────

    fn run_inner(&mut self, budget: u64, pc_break: Option<u64>) -> StopReason {
        if self.exited {
            return StopReason::Exited {
                code: self.exit_code,
            };
        }

        let limit = self.insn_count + budget;
        let has_insn_cbs = self.plugin_reg.has_insn_callbacks();
        let plugins: Option<&PluginRegistry> = if self.adapters.is_empty() {
            None
        } else {
            Some(&self.plugin_reg)
        };

        while self.insn_count < limit {
            if let Some(target) = pc_break {
                if self.cpu.regs.pc == target && self.insn_count > 0 {
                    return StopReason::Breakpoint { pc: target };
                }
            }

            self.sched.load_regs(&mut self.cpu.regs);
            self.syscall.set_tid(self.sched.current_tid());

            let step_result = if plugins.is_none()
                && pc_break.is_none()
                && matches!(self.backend, ExecBackend::Interpretive)
            {
                let batch = (limit - self.insn_count).min(4096);
                exec_interp_batch(
                    &mut self.cpu,
                    &mut self.mem,
                    &mut self.syscall,
                    &mut self.sched,
                    self.timing.as_mut(),
                    &mut self.insn_count,
                    &mut self.virtual_cycles,
                    self.tls_info.as_ref(),
                    batch,
                )
            } else {
                match &mut self.backend {
                    ExecBackend::Interpretive => exec_interp(
                        &mut self.cpu,
                        &mut self.mem,
                        &mut self.syscall,
                        &mut self.sched,
                        self.timing.as_mut(),
                        &mut None,
                        plugins,
                        &mut None,
                        has_insn_cbs,
                        &mut self.insn_count,
                        &mut self.virtual_cycles,
                        self.tls_info.as_ref(),
                    ),
                    ExecBackend::Tcg { cache, interp } => exec_tcg(
                        &mut self.cpu,
                        &mut self.mem,
                        &mut self.syscall,
                        &mut self.sched,
                        self.timing.as_mut(),
                        plugins,
                        &mut None,
                        cache,
                        interp,
                        &mut self.insn_count,
                        &mut self.virtual_cycles,
                        self.tls_info.as_ref(),
                    ),
                }
            };

            if let Err(e) = step_result {
                return StopReason::Error(format!("{e}"));
            }

            self.sched.save_regs(&self.cpu.regs);

            if self.syscall.should_exit {
                self.exited = true;
                self.exit_code = self.syscall.exit_code;
                return StopReason::Exited {
                    code: self.exit_code,
                };
            }
        }

        StopReason::InsnLimit
    }
}

impl MonitorTarget for SeSession {
    fn run(&mut self, max_insns: u64) -> StopReason {
        self.run(max_insns)
    }
    fn run_until_pc(&mut self, pc: u64, max_insns: u64) -> StopReason {
        self.run_until_pc(pc, max_insns)
    }
    fn pc(&self) -> u64 {
        self.cpu.regs.pc
    }
    fn xn(&self, n: u32) -> u64 {
        self.cpu.xn(n as u16)
    }
    fn sp(&self) -> u64 {
        self.cpu.current_sp()
    }
    fn read_memory(&self, addr: u64, size: usize) -> Option<Vec<u8>> {
        SeSession::read_memory(self, addr, size)
    }
    fn insn_count(&self) -> u64 {
        self.insn_count
    }
    fn virtual_cycles(&self) -> u64 {
        self.virtual_cycles
    }
    fn current_el(&self) -> u8 {
        self.cpu.regs.current_el
    }
    fn daif(&self) -> u32 {
        self.cpu.regs.daif
    }
    fn sysreg(&self, name: &str) -> Option<u64> {
        match name {
            "nzcv" => Some(self.cpu.regs.nzcv as u64),
            "daif" => Some(self.cpu.regs.daif as u64),
            _ => None,
        }
    }
    fn irq_count(&self) -> u64 {
        0
    }
    fn has_exited(&self) -> bool {
        self.exited
    }
    fn symbols(&self) -> &SymbolTable {
        &self.symbols
    }
}

/// Resolve short plugin name → fully-qualified component type.
fn resolve_plugin_name(short: &str) -> String {
    match short {
        "cache" => "plugin.memory.cache".to_string(),
        "fault-detect" => "plugin.debug.fault-detect".to_string(),
        other => format!("plugin.trace.{other}"),
    }
}
use crate::se::linux::exec_interp_batch;

//! Suspendable FS-mode session.
//!
//! [`FsSession`] owns all simulation state and exposes incremental
//! execution methods so callers can pause, inspect, hot-load plugins,
//! and resume — mirroring [`SeSession`](crate::se::session::SeSession).
//!
//! ```text
//! let mut s = FsSession::new("vmlinuz-rpi", "virt", FsOpts::default())?;
//! s.run(100_000_000);
//! s.run_until_symbol("start_kernel");
//! println!("PC={:#x}", s.pc());
//! ```

use crate::loader::arm64_image;
use crate::monitor::MonitorTarget;
use crate::se::backend::ExecBackend;
use crate::se::session::StopReason;
use crate::symbols::{self, SymbolTable};
use helm_core::{HelmError, IrqSignal};
use helm_isa::arm::aarch64::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;
use helm_tcg::a64_emitter::{A64TcgEmitter, TranslateAction};
use helm_tcg::block::TcgBlock;
use helm_tcg::threaded::{self, CompiledBlock};
use helm_tcg::interp::{InterpExit, TcgInterp, NUM_REGS, REG_NZCV, REG_PC, REG_SP};
use helm_tcg::TcgContext;
use helm_timing::{InsnClass, TimingModel};
use std::collections::HashMap;

/// Options for creating an FsSession.
pub struct FsOpts {
    pub machine: String,
    pub append: String,
    pub memory_size: String,
    pub dtb: Option<String>,
    pub sysmap: Option<String>,
    pub serial: String,
    pub timing: String,
    pub backend: String,
    pub max_insns: u64,
}

impl Default for FsOpts {
    fn default() -> Self {
        Self {
            machine: "virt".to_string(),
            append: String::new(),
            memory_size: "256M".to_string(),
            dtb: None,
            sysmap: None,
            serial: "stdio".to_string(),
            timing: "fe".to_string(),
            backend: "tcg".to_string(),
            max_insns: u64::MAX,
        }
    }
}

/// Suspendable full-system simulation session.
pub struct FsSession {
    cpu: Aarch64Cpu,
    mem: AddressSpace,
    irq_signal: IrqSignal,
    insn_count: u64,
    virtual_cycles: u64,
    irq_count: u64,
    isa_skip_count: u64,
    timing: Box<dyn TimingModel>,
    backend: ExecBackend,
    /// Pre-compiled bytecode cache for threaded dispatch.
    compiled_cache: HashMap<u64, CompiledBlock>,
    /// Cranelift JIT engine and cache.
    jit_engine: Option<helm_tcg::jit::JitEngine>,
    jit_cache: HashMap<u64, helm_tcg::jit::JitBlock>,
    symbols: SymbolTable,
    halted: bool,
}

impl FsSession {
    /// Create a new FS-mode session by loading a kernel image.
    pub fn new(kernel: &str, opts: &FsOpts) -> Result<Self, HelmError> {
        let irq_signal = IrqSignal::new();

        // Build platform
        let serial_backend: Box<dyn helm_device::backend::CharBackend> = match opts.serial.as_str()
        {
            "null" => Box::new(helm_device::backend::NullCharBackend),
            _ => Box::new(helm_device::backend::StdioCharBackend),
        };
        let serial2: Box<dyn helm_device::backend::CharBackend> =
            Box::new(helm_device::backend::NullCharBackend);

        let mut platform = match opts.machine.as_str() {
            "realview-pb" | "realview" => helm_device::realview_pb_platform(serial_backend),
            "rpi3" | "raspi3" => helm_device::rpi3_platform(serial_backend, serial2),
            _ => helm_device::arm_virt_platform(serial_backend, serial2, Some(irq_signal.clone())),
        };

        // DTB
        let ram_size = helm_device::parse_ram_size(&opts.memory_size).unwrap_or(256 * 1024 * 1024);
        let dtb_config = helm_device::DtbConfig {
            ram_base: 0x4000_0000,
            ram_size,
            num_cpus: 1,
            bootargs: opts.append.clone(),
            ..Default::default()
        };

        let base_blob: Option<Vec<u8>> = opts.dtb.as_ref().and_then(|p| std::fs::read(p).ok());
        let infer_ctx = helm_device::InferCtx::from_platform(
            &platform,
            true,
            false,
            false,
            opts.dtb.is_some(),
            false,
        );
        let resolved =
            helm_device::resolve_dtb(&platform, &dtb_config, base_blob.as_deref(), &infer_ctx);

        let effective_dtb: Option<String> = match &resolved {
            helm_device::ResolvedDtb::Blob(blob) => {
                let dtb_tmp = std::env::temp_dir().join("helm-session.dtb");
                let _ = std::fs::write(&dtb_tmp, blob);
                Some(dtb_tmp.to_string_lossy().into_owned())
            }
            helm_device::ResolvedDtb::None => None,
        };

        // Load kernel
        let loaded = arm64_image::load_arm64_image(kernel, effective_dtb.as_deref(), None, None)?;

        // CPU setup
        let mut cpu = Aarch64Cpu::new();
        cpu.set_irq_signal(irq_signal.clone());
        cpu.regs.pc = loaded.entry_point;
        cpu.regs.current_el = 1;
        cpu.regs.sp_sel = 1;
        cpu.regs.sp_el1 = loaded.initial_sp;
        cpu.regs.sp = loaded.initial_sp;
        cpu.set_xn(0, loaded.dtb_addr);
        cpu.set_xn(1, 0);
        cpu.set_xn(2, 0);
        cpu.set_xn(3, 0);

        let mut mem = loaded.address_space;

        // Wire device bus
        let io_handler = DeviceBusIo {
            bus: std::mem::take(&mut platform.system_bus),
        };
        mem.set_io_handler(Box::new(io_handler));

        // Timing model
        let timing: Box<dyn TimingModel> = match opts.timing.as_str() {
            "ape" => Box::new(helm_timing::model::ApeModelDetailed::default()),
            "cae" => Box::new(helm_timing::model::ApeModelDetailed {
                branch_penalty: 14,
                ..Default::default()
            }),
            _ => Box::new(helm_timing::model::FeModel),
        };

        // Symbol table
        let sysmap_path = opts
            .sysmap
            .clone()
            .or_else(|| symbols::find_system_map(kernel));
        let syms = match sysmap_path {
            Some(ref path) => match SymbolTable::from_system_map(path) {
                Ok(t) => {
                    eprintln!("HELM: loaded {} symbols from {path}", t.len());
                    t
                }
                Err(_) => SymbolTable::new(),
            },
            None => SymbolTable::new(),
        };

        let backend = match opts.backend.as_str() {
            "interp" | "interpretive" => ExecBackend::interpretive(),
            _ => ExecBackend::tcg(),
        };

        Ok(Self {
            cpu,
            mem,
            irq_signal,
            insn_count: 0,
            virtual_cycles: 0,
            irq_count: 0,
            isa_skip_count: 0,
            timing,
            backend,
            compiled_cache: HashMap::new(),
            jit_engine: if opts.backend == "jit" {
                Some(helm_tcg::jit::JitEngine::new())
            } else {
                None
            },
            jit_cache: HashMap::new(),
            symbols: syms,
            halted: false,
        })
    }

    /// Run up to `max_insns` instructions, then return.
    pub fn run(&mut self, max_insns: u64) -> StopReason {
        self.run_inner(max_insns, None)
    }

    /// Run until PC equals `target` (with safety limit).
    pub fn run_until_pc(&mut self, target: u64, max_insns: u64) -> StopReason {
        self.run_inner(max_insns, Some(target))
    }

    /// Run until a named symbol is reached.
    pub fn run_until_symbol(&mut self, sym: &str, max_insns: u64) -> StopReason {
        match self.symbols.lookup(sym) {
            Some(addr) => self.run_until_pc(addr, max_insns),
            None => StopReason::Error(format!("symbol not found: {sym}")),
        }
    }

    // ── inner execution loop (extracted from helm_system_arm.rs) ──

    fn run_inner(&mut self, budget: u64, pc_break: Option<u64>) -> StopReason {
        if self.halted {
            return StopReason::Exited { code: 0 };
        }

        let limit = self.insn_count + budget;

        // Timer IRQ injection constants
        const TIMER_CHECK_INTERVAL: u64 = 1024;
        const VTIMER_IRQ_BIT: u32 = 1 << 27;
        const PTIMER_IRQ_BIT: u32 = 1 << 30;

        while self.insn_count < limit {
            // WFI handling
            if self.cpu.wfi_pending {
                let skipped = self.cpu.wfi_advance();
                if skipped > 0 {
                    self.insn_count += skipped;
                    self.virtual_cycles += skipped;
                }
                let (v_fire, p_fire) = self.cpu.check_timers();
                if v_fire {
                    let _ = self.mem.write(0x0800_0200, &VTIMER_IRQ_BIT.to_le_bytes());
                }
                if p_fire {
                    let _ = self.mem.write(0x0800_0200, &PTIMER_IRQ_BIT.to_le_bytes());
                }
                if !self.irq_signal.is_raised() && !v_fire && !p_fire {
                    self.cpu.insn_count += 4096;
                    self.insn_count += 4096;
                    self.virtual_cycles += 4096;
                    continue;
                }
                self.cpu.wfi_pending = false;
            }

            // Timer check (between blocks, not per-instruction)
            if self.insn_count % TIMER_CHECK_INTERVAL == 0 {
                let (v_fire, p_fire) = self.cpu.check_timers();
                if v_fire {
                    let _ = self.mem.write(0x0800_0200, &VTIMER_IRQ_BIT.to_le_bytes());
                }
                if p_fire {
                    let _ = self.mem.write(0x0800_0200, &PTIMER_IRQ_BIT.to_le_bytes());
                }
            }

            // PC breakpoint check
            if let Some(target) = pc_break {
                if self.cpu.regs.pc == target && self.insn_count > 0 {
                    return StopReason::Breakpoint { pc: target };
                }
            }

            if self.cpu.halted {
                self.halted = true;
                return StopReason::Exited { code: 0 };
            }

            // Execute: JIT → threaded → interpretive fallback
            match &mut self.backend {
                ExecBackend::Tcg { cache, interp } => {
                    let pc = self.cpu.regs.pc;

                    // === JIT path ===
                    if self.jit_engine.is_some() {
                        // Try JIT cache first
                        if self.jit_cache.contains_key(&pc) {
                            let mut regs = regs_to_array(&self.cpu);
                            let result = unsafe {
                                helm_tcg::jit::exec_jit(&self.jit_cache[&pc], &mut regs)
                            };
                            array_to_regs(&mut self.cpu, &regs);
                            let n = result.insns_executed as u64;
                            self.insn_count += n;
                            self.cpu.insn_count += n;
                            self.virtual_cycles += n;
                            match result.exit {
                                InterpExit::Chain { target_pc } => self.cpu.regs.pc = target_pc,
                                InterpExit::EndOfBlock { next_pc } => self.cpu.regs.pc = next_pc,
                                InterpExit::Wfi => self.cpu.wfi_pending = true,
                                InterpExit::ExceptionReturn => {}
                                _ => {}
                            }
                            continue;
                        }

                        // Try to JIT compile this block
                        if !cache.contains_key(&pc) {
                            let block = translate_block_fs(pc, &mut self.mem, 64);
                            if block.insn_count > 0 {
                                cache.insert(pc, block);
                            }
                        }
                        if let Some(block) = cache.get(&pc) {
                            if let Some(jit_engine) = &mut self.jit_engine {
                                if let Some(jit_block) = jit_engine.compile(block) {
                                    self.jit_cache.insert(pc, jit_block);
                                    continue; // re-enter loop — will hit JIT cache
                                }
                            }
                        }
                    }

                    // === Threaded dispatch fallback ===
                    if !self.compiled_cache.contains_key(&pc) {
                        if !cache.contains_key(&pc) {
                            let block = translate_block_fs(pc, &mut self.mem, 64);
                            if block.insn_count > 0 {
                                cache.insert(pc, block);
                            }
                        }
                        if let Some(block) = cache.get(&pc) {
                            let compiled = threaded::compile_block(block);
                            self.compiled_cache.insert(pc, compiled);
                        }
                    }

                    if let Some(compiled) = self.compiled_cache.get(&pc) {
                        let mut regs = regs_to_array(&self.cpu);
                        sync_sysregs_to_interp(&self.cpu, interp);

                        match threaded::exec_threaded(
                            compiled, &mut regs, &mut self.mem, &mut interp.sysregs,
                        ) {
                            Ok(result) => {
                                array_to_regs(&mut self.cpu, &regs);
                                sync_sysregs_from_interp(&mut self.cpu, interp);
                                let n = result.insns_executed as u64;
                                self.insn_count += n;
                                self.cpu.insn_count += n;
                                self.virtual_cycles += n;
                                match result.exit {
                                    InterpExit::Chain { target_pc } => self.cpu.regs.pc = target_pc,
                                    InterpExit::EndOfBlock { next_pc } => self.cpu.regs.pc = next_pc,
                                    InterpExit::Syscall { .. } => self.cpu.regs.pc += 4,
                                    InterpExit::Exception { .. } => {}
                                    InterpExit::ExceptionReturn => {}
                                    InterpExit::Wfi => self.cpu.wfi_pending = true,
                                    InterpExit::Exit => {}
                                }
                                continue;
                            }
                            Err(_) => {
                                array_to_regs(&mut self.cpu, &regs);
                            }
                        }
                    }
                    self.step_interp();
                }
                ExecBackend::Interpretive => {
                    self.step_interp();
                }
            }
        }

        StopReason::InsnLimit
    }

    /// Single interpretive step — fast path without trace allocation.
    fn step_interp(&mut self) {
        match self.cpu.step_fast(&mut self.mem) {
            Ok(()) => {
                self.insn_count += 1;
                self.virtual_cycles += 1; // FE timing: 1 cycle/insn
            }
            Err(HelmError::Syscall { .. }) => {
                self.cpu.regs.pc += 4;
                self.insn_count += 1;
                self.virtual_cycles += 1;
            }
            Err(HelmError::Memory { .. }) => {
                // Memory fault — exception taken inside step_fast
            }
            Err(HelmError::Isa(_)) | Err(HelmError::Decode { .. }) => {
                self.cpu.regs.pc += 4;
                self.insn_count += 1;
                self.isa_skip_count += 1;
                self.virtual_cycles += 1;
            }
            Err(_) => {}
        }
    }
}

// ── MonitorTarget implementation ──────────────────────────────────────

impl MonitorTarget for FsSession {
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
        let mut buf = vec![0u8; size];
        // Try reading from the address space (cast away mutability for read)
        let mem_ptr = &self.mem as *const AddressSpace as *mut AddressSpace;
        unsafe {
            match (*mem_ptr).read(addr, &mut buf) {
                Ok(()) => Some(buf),
                Err(_) => None,
            }
        }
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
            "sctlr_el1" => Some(self.cpu.regs.sctlr_el1),
            "tcr_el1" => Some(self.cpu.regs.tcr_el1),
            "ttbr0_el1" => Some(self.cpu.regs.ttbr0_el1),
            "ttbr1_el1" => Some(self.cpu.regs.ttbr1_el1),
            "vbar_el1" => Some(self.cpu.regs.vbar_el1),
            "elr_el1" => Some(self.cpu.regs.elr_el1),
            "spsr_el1" => Some(self.cpu.regs.spsr_el1 as u64),
            "esr_el1" => Some(self.cpu.regs.esr_el1 as u64),
            "far_el1" => Some(self.cpu.regs.far_el1),
            "mair_el1" => Some(self.cpu.regs.mair_el1),
            "nzcv" => Some(self.cpu.regs.nzcv as u64),
            "daif" => Some(self.cpu.regs.daif as u64),
            "cntv_ctl_el0" => Some(self.cpu.regs.cntv_ctl_el0),
            "cntv_cval_el0" => Some(self.cpu.regs.cntv_cval_el0),
            "cntp_ctl_el0" => Some(self.cpu.regs.cntp_ctl_el0),
            "cntp_cval_el0" => Some(self.cpu.regs.cntp_cval_el0),
            "hcr_el2" => Some(self.cpu.regs.hcr_el2),
            "scr_el3" => Some(self.cpu.regs.scr_el3),
            _ => None,
        }
    }

    fn irq_count(&self) -> u64 {
        self.irq_count
    }

    fn has_exited(&self) -> bool {
        self.halted
    }

    fn symbols(&self) -> &SymbolTable {
        &self.symbols
    }
}

// ── DeviceBusIo ────────────────────────────────────────────────────────

// ── TCG helpers ────────────────────────────────────────────────────────

fn translate_block_fs(pc: u64, mem: &mut AddressSpace, max_insns: usize) -> TcgBlock {
    let mut ctx = TcgContext::new();
    let mut cur = pc;
    let mut n = 0;
    for _ in 0..max_insns {
        let mut buf = [0u8; 4];
        if mem.read(cur, &mut buf).is_err() {
            break;
        }
        let mut e = A64TcgEmitter::new(&mut ctx, cur);
        match e.translate_insn(u32::from_le_bytes(buf)) {
            TranslateAction::Continue => {
                n += 1;
                cur += 4;
            }
            TranslateAction::EndBlock => {
                n += 1;
                break;
            }
            TranslateAction::Unhandled => break,
        }
    }
    TcgBlock {
        guest_pc: pc,
        guest_size: (cur - pc) as usize,
        insn_count: n,
        ops: ctx.finish(),
    }
}

fn regs_to_array(cpu: &Aarch64Cpu) -> [u64; NUM_REGS] {
    let mut r = [0u64; NUM_REGS];
    for i in 0..31 {
        r[i] = cpu.xn(i as u16);
    }
    r[REG_SP as usize] = cpu.regs.sp;
    r[REG_PC as usize] = cpu.regs.pc;
    r[REG_NZCV as usize] = cpu.regs.nzcv as u64;
    r[REG_DAIF as usize] = cpu.regs.daif as u64;
    r[REG_ELR_EL1 as usize] = cpu.regs.elr_el1;
    r[REG_SPSR_EL1 as usize] = cpu.regs.spsr_el1 as u64;
    r[REG_ESR_EL1 as usize] = cpu.regs.esr_el1 as u64;
    r[REG_VBAR_EL1 as usize] = cpu.regs.vbar_el1;
    r[REG_CURRENT_EL as usize] = (cpu.regs.current_el as u64) << 2;
    r[REG_SPSEL as usize] = cpu.regs.sp_sel as u64;
    r[REG_SP_EL1 as usize] = cpu.regs.sp_el1;
    r
}

fn array_to_regs(cpu: &mut Aarch64Cpu, r: &[u64; NUM_REGS]) {
    for i in 0..31 {
        cpu.set_xn(i as u16, r[i]);
    }
    cpu.regs.sp = r[REG_SP as usize];
    cpu.regs.pc = r[REG_PC as usize];
    cpu.regs.nzcv = r[REG_NZCV as usize] as u32;
    cpu.regs.daif = r[REG_DAIF as usize] as u32;
    cpu.regs.elr_el1 = r[REG_ELR_EL1 as usize];
    cpu.regs.spsr_el1 = r[REG_SPSR_EL1 as usize] as u32;
    cpu.regs.esr_el1 = r[REG_ESR_EL1 as usize] as u32;
    cpu.regs.vbar_el1 = r[REG_VBAR_EL1 as usize];
    cpu.regs.current_el = ((r[REG_CURRENT_EL as usize] >> 2) & 3) as u8;
    cpu.regs.sp_sel = (r[REG_SPSEL as usize] & 1) as u8;
    cpu.regs.sp_el1 = r[REG_SP_EL1 as usize];
}

/// Copy frequently-accessed system registers from the CPU into the
/// interpreter's sysreg map before executing a TCG block.
fn sync_sysregs_to_interp(cpu: &Aarch64Cpu, interp: &mut TcgInterp) {
    use helm_isa::arm::aarch64::sysreg;
    interp.set_sysreg(sysreg::SCTLR_EL1, cpu.regs.sctlr_el1);
    interp.set_sysreg(sysreg::TCR_EL1, cpu.regs.tcr_el1);
    interp.set_sysreg(sysreg::TTBR0_EL1, cpu.regs.ttbr0_el1);
    interp.set_sysreg(sysreg::TTBR1_EL1, cpu.regs.ttbr1_el1);
    interp.set_sysreg(sysreg::MAIR_EL1, cpu.regs.mair_el1);
    interp.set_sysreg(sysreg::VBAR_EL1, cpu.regs.vbar_el1);
    interp.set_sysreg(sysreg::TPIDR_EL0, cpu.regs.tpidr_el0);
    interp.set_sysreg(sysreg::TPIDR_EL1, cpu.regs.tpidr_el1);
    interp.set_sysreg(sysreg::ELR_EL1, cpu.regs.elr_el1);
    interp.set_sysreg(sysreg::SPSR_EL1, cpu.regs.spsr_el1 as u64);
    interp.set_sysreg(sysreg::ESR_EL1, cpu.regs.esr_el1 as u64);
    interp.set_sysreg(sysreg::FAR_EL1, cpu.regs.far_el1);
    interp.set_sysreg(sysreg::NZCV, cpu.regs.nzcv as u64);
    interp.set_sysreg(sysreg::DAIF, cpu.regs.daif as u64);
    interp.set_sysreg(sysreg::CURRENT_EL, (cpu.regs.current_el as u64) << 2);
    interp.set_sysreg(sysreg::SPSEL, cpu.regs.sp_sel as u64);
    interp.set_sysreg(sysreg::CNTFRQ_EL0, cpu.regs.cntfrq_el0);
    interp.set_sysreg(sysreg::CNTVCT_EL0, cpu.insn_count);
    interp.set_sysreg(sysreg::CNTV_CTL_EL0, cpu.regs.cntv_ctl_el0);
    interp.set_sysreg(sysreg::CNTV_CVAL_EL0, cpu.regs.cntv_cval_el0);
    interp.set_sysreg(sysreg::CNTP_CTL_EL0, cpu.regs.cntp_ctl_el0);
    interp.set_sysreg(sysreg::CNTP_CVAL_EL0, cpu.regs.cntp_cval_el0);
    interp.set_sysreg(sysreg::MIDR_EL1, cpu.regs.midr_el1);
    interp.set_sysreg(sysreg::MPIDR_EL1, cpu.regs.mpidr_el1);
    interp.set_sysreg(sysreg::HCR_EL2, cpu.regs.hcr_el2);
    interp.set_sysreg(sysreg::SCR_EL3, cpu.regs.scr_el3);
    interp.set_sysreg(sysreg::FPCR, cpu.regs.fpcr as u64);
    interp.set_sysreg(sysreg::FPSR, cpu.regs.fpsr as u64);
}

/// Copy system registers that the TCG block may have modified back
/// into the CPU state.
fn sync_sysregs_from_interp(cpu: &mut Aarch64Cpu, interp: &TcgInterp) {
    use helm_isa::arm::aarch64::sysreg;
    cpu.regs.sctlr_el1 = interp.get_sysreg(sysreg::SCTLR_EL1);
    cpu.regs.tcr_el1 = interp.get_sysreg(sysreg::TCR_EL1);
    cpu.regs.ttbr0_el1 = interp.get_sysreg(sysreg::TTBR0_EL1);
    cpu.regs.ttbr1_el1 = interp.get_sysreg(sysreg::TTBR1_EL1);
    cpu.regs.mair_el1 = interp.get_sysreg(sysreg::MAIR_EL1);
    cpu.regs.vbar_el1 = interp.get_sysreg(sysreg::VBAR_EL1);
    cpu.regs.tpidr_el0 = interp.get_sysreg(sysreg::TPIDR_EL0);
    cpu.regs.tpidr_el1 = interp.get_sysreg(sysreg::TPIDR_EL1);
    cpu.regs.elr_el1 = interp.get_sysreg(sysreg::ELR_EL1);
    cpu.regs.spsr_el1 = interp.get_sysreg(sysreg::SPSR_EL1) as u32;
    cpu.regs.esr_el1 = interp.get_sysreg(sysreg::ESR_EL1) as u32;
    cpu.regs.far_el1 = interp.get_sysreg(sysreg::FAR_EL1);
    cpu.regs.cntv_ctl_el0 = interp.get_sysreg(sysreg::CNTV_CTL_EL0);
    cpu.regs.cntv_cval_el0 = interp.get_sysreg(sysreg::CNTV_CVAL_EL0);
    cpu.regs.cntp_ctl_el0 = interp.get_sysreg(sysreg::CNTP_CTL_EL0);
    cpu.regs.cntp_cval_el0 = interp.get_sysreg(sysreg::CNTP_CVAL_EL0);
    cpu.regs.fpcr = interp.get_sysreg(sysreg::FPCR) as u32;
    cpu.regs.fpsr = interp.get_sysreg(sysreg::FPSR) as u32;
}

// ── DeviceBusIo ────────────────────────────────────────────────────────

struct DeviceBusIo {
    bus: helm_device::bus::DeviceBus,
}

impl helm_memory::address_space::IoHandler for DeviceBusIo {
    fn io_read(&mut self, addr: u64, size: usize) -> Option<u64> {
        match self.bus.read_fast(addr, size) {
            Ok(val) => Some(val),
            Err(_) => Some(0),
        }
    }

    fn io_write(&mut self, addr: u64, size: usize, value: u64) -> bool {
        let _ = self.bus.write_fast(addr, size, value);
        true
    }
}
use helm_tcg::interp::{
    REG_CURRENT_EL, REG_DAIF, REG_ELR_EL1, REG_ESR_EL1, REG_SPSEL, REG_SPSR_EL1, REG_SP_EL1,
    REG_VBAR_EL1,
};

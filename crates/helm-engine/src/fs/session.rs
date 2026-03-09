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
use helm_tcg::interp::{sysreg_idx as tcg_sysreg_idx, MMU_DIRTY_IDX};
use helm_tcg::ir::TcgOp;
use helm_tcg::TcgContext;
use helm_timing::{InsnClass, TimingModel};
use std::collections::HashMap;

// ── RunMarker trait ──────────────────────────────────────────────────

/// Controls what checks happen at JIT block boundaries.
///
/// The default markers cover instruction limits and PC breakpoints.
/// Python scripts can select different markers via `run()`, `run_until_pc()`,
/// or `run_forever()`.
pub trait RunMarker: Send {
    /// Called after each block. Return `Some(reason)` to stop execution.
    fn check(&mut self, insn_count: u64, pc: u64) -> Option<StopReason>;
}

/// No stopping condition — run until halt/panic.
pub struct UnlimitedMarker;

impl RunMarker for UnlimitedMarker {
    #[inline(always)]
    fn check(&mut self, _insn_count: u64, _pc: u64) -> Option<StopReason> {
        None
    }
}

/// Stop when `insn_count >= limit`.
pub struct InsnLimitMarker {
    pub limit: u64,
}

impl RunMarker for InsnLimitMarker {
    #[inline(always)]
    fn check(&mut self, insn_count: u64, _pc: u64) -> Option<StopReason> {
        if insn_count >= self.limit {
            Some(StopReason::InsnLimit)
        } else {
            None
        }
    }
}

/// Stop when `pc == target` or `insn_count >= limit`.
pub struct PcBreakMarker {
    pub target: u64,
    pub limit: u64,
}

impl PcBreakMarker {
    pub fn new(target: u64, limit: u64) -> Self {
        Self { target, limit }
    }
}

impl RunMarker for PcBreakMarker {
    #[inline(always)]
    fn check(&mut self, insn_count: u64, pc: u64) -> Option<StopReason> {
        if pc == self.target && insn_count > 0 {
            return Some(StopReason::Breakpoint { pc: self.target });
        }
        if insn_count >= self.limit {
            Some(StopReason::InsnLimit)
        } else {
            None
        }
    }
}

// ── Direct-mapped JIT block cache ─────────────────────────────────────
const JIT_CACHE_BITS: usize = 16;
const JIT_CACHE_SIZE: usize = 1 << JIT_CACHE_BITS;
const JIT_CACHE_MASK: usize = JIT_CACHE_SIZE - 1;

struct JitCacheEntry {
    pc: u64,
    block: helm_tcg::jit::JitBlock,
}


/// Options for creating an FsSession.
pub struct FsOpts {
    pub machine: String,
    pub append: String,
    pub memory_size: String,
    pub dtb: Option<String>,
    pub initrd: Option<String>,
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
            initrd: None,
            sysmap: None,
            serial: "stdio".to_string(),
            timing: "fe".to_string(),
            backend: "jit".to_string(),
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
    /// Direct-mapped compiled-block cache for threaded dispatch.
    compiled_cache: Vec<Option<BlockCacheEntry>>,
    /// Cranelift JIT engine.
    jit_engine: Option<helm_tcg::jit::JitEngine>,
    /// Direct-mapped JIT block cache (indexed by (pc >> 2) & mask).
    jit_cache: Vec<Option<JitCacheEntry>>,
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

        // Pre-compute initrd placement so the DTB chosen node includes
        // linux,initrd-start / linux,initrd-end.  The loader places the
        // initrd at ram_base + 0x0400_0000 (DEFAULT_INITRD_ADDR).
        let initrd_info: Option<(u64, u64)> = opts.initrd.as_ref().and_then(|p| {
            let meta = std::fs::metadata(p).ok()?;
            let start = 0x4000_0000u64 + 0x0400_0000;
            Some((start, start + meta.len()))
        });

        let dtb_config = helm_device::DtbConfig {
            ram_base: 0x4000_0000,
            ram_size,
            num_cpus: 1,
            bootargs: opts.append.clone(),
            initrd: initrd_info,
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
        let loaded = arm64_image::load_arm64_image(
            kernel,
            effective_dtb.as_deref(),
            opts.initrd.as_deref(),
            None,
        )?;

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
            compiled_cache: {
                let mut v = Vec::with_capacity(BLOCK_CACHE_SIZE);
                v.resize_with(BLOCK_CACHE_SIZE, || None);
                v
            },
            jit_engine: if opts.backend == "jit" {
                unsafe { helm_tcg::jit::set_translate_va(jit_translate_va); }
                unsafe { helm_tcg::jit::set_tlbi_cb(jit_tlbi); }
                Some(helm_tcg::jit::JitEngine::new())
            } else {
                None
            },
            jit_cache: {
                let mut v = Vec::with_capacity(JIT_CACHE_SIZE);
                v.resize_with(JIT_CACHE_SIZE, || None);
                v
            },
            symbols: syms,
            halted: false,
        })
    }

    /// Run up to `max_insns` instructions, then return.
    pub fn run(&mut self, max_insns: u64) -> StopReason {
        let mut marker = InsnLimitMarker { limit: self.insn_count + max_insns };
        self.run_inner(&mut marker)
    }

    /// Run until PC equals `target` (with safety limit).
    pub fn run_until_pc(&mut self, target: u64, max_insns: u64) -> StopReason {
        let mut marker = PcBreakMarker::new(target, self.insn_count + max_insns);
        self.run_inner(&mut marker)
    }

    /// Run until halt/panic — no instruction limit.
    pub fn run_forever(&mut self) -> StopReason {
        let mut marker = UnlimitedMarker;
        self.run_inner(&mut marker)
    }

    /// Run until a named symbol is reached.
    pub fn run_until_symbol(&mut self, sym: &str, max_insns: u64) -> StopReason {
        match self.symbols.lookup(sym) {
            Some(addr) => self.run_until_pc(addr, max_insns),
            None => StopReason::Error(format!("symbol not found: {sym}")),
        }
    }

    // ── inner execution loop ─────────────────────────────────────────

    #[inline(never)]  // one copy per M, but don't inline the large body into callers
    /// Inject timer IRQs into the GIC.
    ///
    /// Models the level-sensitive timer→GIC connection: as long as the
    /// timer condition is met (ENABLE && !IMASK && CNTVCT >= CVAL) the
    /// pending bit is kept asserted.  `GICD_ISPENDR` writes are
    /// idempotent so repeated injection is harmless.
    fn inject_timers(&mut self) {
        const VTIMER_IRQ_BIT: u32 = 1 << 27;
        const PTIMER_IRQ_BIT: u32 = 1 << 30;
        // Sync timer sysregs from the interp sysreg array before checking.
        // The kernel re-arms timers via MSR (CNTV_CVAL/CTL) which writes to
        // sysregs, but check_timers reads from cpu.regs.  Without this sync
        // the timer appears permanently fired after the first expiry.
        if let ExecBackend::Tcg { interp, .. } = &self.backend {
            use helm_isa::arm::aarch64::sysreg;
            self.cpu.regs.cntv_ctl_el0 = interp.get_sysreg(sysreg::CNTV_CTL_EL0);
            self.cpu.regs.cntv_cval_el0 = interp.get_sysreg(sysreg::CNTV_CVAL_EL0);
            self.cpu.regs.cntp_ctl_el0 = interp.get_sysreg(sysreg::CNTP_CTL_EL0);
            self.cpu.regs.cntp_cval_el0 = interp.get_sysreg(sysreg::CNTP_CVAL_EL0);
        }
        let (v_fire, p_fire) = self.cpu.check_timers();
        if v_fire {
            let _ = self.mem.write(0x0800_0200, &VTIMER_IRQ_BIT.to_le_bytes());
        }
        if p_fire {
            let _ = self.mem.write(0x0800_0200, &PTIMER_IRQ_BIT.to_le_bytes());
        }
    }

    /// Poll device interrupt lines and update the GIC.
    ///
    /// The PL011 UART sets its internal `irq_level` on MMIO writes but
    /// has no direct path to the GIC.  This method bridges that gap by
    /// reading the masked interrupt status (UARTMIS) and setting/clearing
    /// the corresponding SPI pending bit in the GIC distributor.
    ///
    /// Called from the periodic timer check — the ~1024-block polling
    /// interval is fine for console and RX interrupt latency.
    fn poll_device_irqs(&mut self) {
        // UART0 at 0x0900_0000: SPI 1 → INTID 33 → bit 1 in ISPENDR[1]
        let mut buf = [0u8; 4];
        if self.mem.read(0x0900_0044, &mut buf).is_ok() {
            let mis = u32::from_le_bytes(buf);
            const UART0_BIT: u32 = 1 << 1;
            if mis != 0 {
                let _ = self.mem.write(0x0800_0204, &UART0_BIT.to_le_bytes());
            } else {
                let _ = self.mem.write(0x0800_0284, &UART0_BIT.to_le_bytes());
            }
        }
    }

    fn run_inner<M: RunMarker>(&mut self, marker: &mut M) -> StopReason {
        if self.halted {
            return StopReason::Exited { code: 0 };
        }

        const TIMER_CHECK_INTERVAL: i64 = 1024;

        let mut timer_countdown: i64 = TIMER_CHECK_INTERVAL;

        // Interpretive-only fast path (no register array overhead)
        if matches!(self.backend, ExecBackend::Interpretive) {
            loop {
                if self.cpu.wfi_pending {
                    let skipped = self.cpu.wfi_advance();
                    if skipped > 0 {
                        self.insn_count += skipped;
                        self.virtual_cycles += skipped;
                    }
                    self.inject_timers();
                    self.poll_device_irqs();
                    if !self.irq_signal.is_raised() {
                        self.cpu.insn_count += 4096;
                        self.insn_count += 4096;
                        self.virtual_cycles += 4096;
                        continue;
                    }
                    self.cpu.wfi_pending = false;
                }
                timer_countdown -= 1;
                if timer_countdown <= 0 {
                    timer_countdown = TIMER_CHECK_INTERVAL;
                    self.inject_timers();
                    self.poll_device_irqs();
                }
                // Track IRQ delivery (check_irq runs inside step_fast)
                if self.irq_signal.is_raised() && (self.cpu.regs.daif & 0x80) == 0 {
                    self.irq_count += 1;
                }
                if self.cpu.halted { self.halted = true; return StopReason::Exited { code: 0 }; }
                if let Some(reason) = marker.check(self.insn_count, self.cpu.regs.pc) {
                    return reason;
                }
                self.step_interp();
            }
        }

        // ── JIT/TCG path: persistent register array ──────────────────
        // Load registers ONCE from CPU into a flat array. The JIT operates
        // directly on this array. We only sync back to the CPU struct when
        // we need CPU methods (timer/IRQ checks) or fall back to interp.
        let has_jit = self.jit_engine.is_some();
        let mut regs = regs_to_array(&self.cpu);

        // Capture a raw pointer to the sysreg array once before the loop.
        // This eliminates `match &self.backend` inside the chain loop hot path
        // while remaining safe: the Vec is never resized after session creation.
        let sysregs_raw: *mut u64 = if has_jit {
            match &mut self.backend {
                ExecBackend::Tcg { interp, .. } => {
                    sync_sysregs_to_interp(&self.cpu, interp);
                    interp.sysregs.as_mut_ptr()
                }
                _ => std::ptr::null_mut(),
            }
        } else {
            std::ptr::null_mut()
        };

        loop {
            // WFI handling — needs CPU state synced from JIT
            if self.cpu.wfi_pending {
                // Sync timer + DAIF state from JIT sysreg array so
                // wfi_advance/check_timers see the kernel's latest config.
                self.cpu.regs.daif = regs[REG_DAIF as usize] as u32;
                if has_jit {
                    use helm_isa::arm::aarch64::sysreg;
                    let interp = match &self.backend {
                        ExecBackend::Tcg { interp, .. } => interp,
                        _ => unreachable!(),
                    };
                    self.cpu.regs.cntv_ctl_el0 = interp.get_sysreg(sysreg::CNTV_CTL_EL0);
                    self.cpu.regs.cntv_cval_el0 = interp.get_sysreg(sysreg::CNTV_CVAL_EL0);
                    self.cpu.regs.cntp_ctl_el0 = interp.get_sysreg(sysreg::CNTP_CTL_EL0);
                    self.cpu.regs.cntp_cval_el0 = interp.get_sysreg(sysreg::CNTP_CVAL_EL0);
                }
                let skipped = self.cpu.wfi_advance();
                if skipped > 0 {
                    self.insn_count += skipped;
                    self.virtual_cycles += skipped;
                }
                self.inject_timers();
                self.poll_device_irqs();
                if !self.irq_signal.is_raised() {
                    self.cpu.insn_count += 4096;
                    self.insn_count += 4096;
                    self.virtual_cycles += 4096;
                    continue;
                }
                self.cpu.wfi_pending = false;
                regs = regs_to_array(&self.cpu);
            }

            // Periodic timer poll — countdown fires every N blocks
            timer_countdown -= 1;
            if timer_countdown <= 0 {
                timer_countdown = TIMER_CHECK_INTERVAL;
                self.cpu.regs.daif = regs[REG_DAIF as usize] as u32;
                self.cpu.regs.current_el = ((regs[REG_CURRENT_EL as usize] >> 2) & 3) as u8;
                self.inject_timers();
                self.poll_device_irqs();
            }

            // IRQ delivery — check every block boundary, but fast-path
            // skip when DAIF.I is set (IRQs masked).  The signal stays
            // raised throughout the IRQ handler until GICC_IAR clears
            // the pending bit, so without this guard the expensive
            // array_to_regs/sync would run on every single block.
            if self.irq_signal.is_raised() && (regs[REG_DAIF as usize] & 0x80) == 0 {
                self.cpu.regs.daif = regs[REG_DAIF as usize] as u32;
                self.cpu.regs.current_el = ((regs[REG_CURRENT_EL as usize] >> 2) & 3) as u8;
                array_to_regs(&mut self.cpu, &regs);
                if has_jit {
                    use helm_isa::arm::aarch64::sysreg;
                    let interp = match &self.backend {
                        ExecBackend::Tcg { interp, .. } => interp,
                        _ => unreachable!(),
                    };
                    self.cpu.regs.vbar_el1 = interp.get_sysreg(sysreg::VBAR_EL1);
                    self.cpu.regs.hcr_el2 = interp.get_sysreg(sysreg::HCR_EL2);
                    self.cpu.regs.elr_el1 = interp.get_sysreg(sysreg::ELR_EL1);
                    self.cpu.regs.spsr_el1 = interp.get_sysreg(sysreg::SPSR_EL1) as u32;
                }
                if self.cpu.check_irq() {
                    self.irq_count += 1;
                    if has_jit {
                        let interp = match &mut self.backend {
                            ExecBackend::Tcg { interp, .. } => interp,
                            _ => unreachable!(),
                        };
                        sync_sysregs_to_interp(&self.cpu, interp);
                    }
                }
                regs = regs_to_array(&self.cpu);
            }

            let pc = regs[REG_PC as usize];

            if self.cpu.halted {
                array_to_regs(&mut self.cpu, &regs);
                self.halted = true;
                return StopReason::Exited { code: 0 };
            }

            if let Some(reason) = marker.check(self.insn_count, pc) {
                array_to_regs(&mut self.cpu, &regs);
                return reason;
            }

            // === JIT path: direct-mapped cache, no per-block reg copy ===
            if has_jit {
                let jidx = ((pc >> 2) as usize) & JIT_CACHE_MASK;
                if self.jit_cache[jidx].as_ref().map_or(false, |e| e.pc == pc) {
                    // ── Inner chain loop ───────────────────────────────────
                    // Stays in JIT without returning to the outer loop for
                    // consecutive Chain exits.  Breaks on: timer expiry,
                    // pending unmasked IRQ, JIT cache miss, or any non-trivial
                    // block exit (exception, ERET, WFI).
                    // ── Inner chain loop ──────────────────────────────────
                    // Uses raw pointers derived once before the loop to avoid
                    // per-block `match &self.backend` overhead.  Safe because:
                    //   • sysregs_raw → interp.sysregs.data (Vec never resized)
                    //   • mem_raw     → self.mem (stable address)
                    // jit_cache, cpu, and the Vec internals are separate fields.
                    let cpu_ptr = &mut self.cpu as *mut _ as *mut u8;
                    let mem_raw = &mut self.mem as *mut AddressSpace;
                    // sysregs_raw was captured before the loop; it's stable.
                    let sysregs_slice = unsafe {
                        std::slice::from_raw_parts_mut(sysregs_raw, helm_tcg::interp::SYSREG_FILE_SIZE)
                    };

                    // Pre-compute CNTVCT index once (used every block).
                    let cntvct_idx = tcg_sysreg_idx(helm_isa::arm::aarch64::sysreg::CNTVCT_EL0);

                    // cidx/jidx is already verified as a hit by the outer check.
                    let mut cidx = jidx;

                    'chain: loop {
                        // ── Conditional MMU sync (L1 hit: MMU_DIRTY_IDX shares ──
                        // ── cache line with CNTVCT written every block)         ──
                        if unsafe { *sysregs_raw.add(MMU_DIRTY_IDX) } != 0 {
                            let sysregs_ro = unsafe {
                                std::slice::from_raw_parts(sysregs_raw, helm_tcg::interp::SYSREG_FILE_SIZE)
                            };
                            sync_mmu_to_cpu(&mut self.cpu, &regs, sysregs_ro);
                            // Sync regs[VBAR_EL1] for JIT SvcExc (reads from flat regs)
                            regs[REG_VBAR_EL1 as usize] = sysregs_ro[tcg_sysreg_idx(
                                helm_isa::arm::aarch64::sysreg::VBAR_EL1,
                            )];
                            unsafe { *sysregs_raw.add(MMU_DIRTY_IDX) = 0; }
                        }

                        // ── Execute the JIT block ──────────────────────────────
                        // Update virtual counter and dispatch.
                        unsafe { *sysregs_raw.add(cntvct_idx) = self.cpu.insn_count; }

                        // Block ptr is valid for the duration of exec_jit: the
                        // JIT cache is never written inside exec_jit, only read.
                        let block_ptr: *const helm_tcg::jit::JitBlock =
                            &self.jit_cache[cidx].as_ref().unwrap().block;

                        let result = unsafe {
                            helm_tcg::jit::exec_jit(
                                &*block_ptr,
                                &mut regs,
                                cpu_ptr,
                                &mut *mem_raw,
                                sysregs_slice,
                            )
                        };

                        let n = result.insns_executed as u64;
                        self.insn_count += n;
                        self.cpu.insn_count += n;
                        self.virtual_cycles += n;
                        timer_countdown -= 1;

                        match result.exit {
                            InterpExit::Chain { target_pc } => {
                                regs[REG_PC as usize] = target_pc;
                                // Break on timer expiry or pending unmasked IRQ.
                                if timer_countdown <= 0 {
                                    break 'chain;
                                }
                                if self.irq_signal.is_raised()
                                    && (regs[REG_DAIF as usize] & 0x80) == 0
                                {
                                    break 'chain;
                                }
                                // Look up the next target in the JIT cache.
                                // If it's a miss break to the outer loop for
                                // translation; otherwise update cidx and continue.
                                let next_idx = ((target_pc >> 2) as usize) & JIT_CACHE_MASK;
                                if self.jit_cache[next_idx].as_ref().map_or(true, |e| e.pc != target_pc) {
                                    break 'chain; // miss → outer loop translates
                                }
                                cidx = next_idx;
                                // Stay in the chain — loop back to execute.
                            }
                            InterpExit::EndOfBlock { next_pc } => {
                                regs[REG_PC as usize] = next_pc;
                                break 'chain;
                            }
                            InterpExit::Exit => {
                                // PC already written to regs by JIT; continue
                                // to outer loop for breakpoint / limit checks.
                                break 'chain;
                            }
                            InterpExit::Wfi => {
                                array_to_regs(&mut self.cpu, &regs);
                                self.cpu.wfi_pending = true;
                                break 'chain;
                            }
                            InterpExit::ExceptionReturn => {
                                // Sync ELR/SPSR/ESR/VBAR from sysreg array
                                // before ERET register restoration.
                                // Use sysregs_slice (raw ptr, no match needed).
                                {
                                    use helm_isa::arm::aarch64::sysreg as sr;
                                    regs[REG_ELR_EL1 as usize]  = sysregs_slice[tcg_sysreg_idx(sr::ELR_EL1)];
                                    regs[REG_SPSR_EL1 as usize] = sysregs_slice[tcg_sysreg_idx(sr::SPSR_EL1)];
                                    regs[REG_ESR_EL1 as usize]  = sysregs_slice[tcg_sysreg_idx(sr::ESR_EL1)];
                                    regs[REG_VBAR_EL1 as usize] = sysregs_slice[tcg_sysreg_idx(sr::VBAR_EL1)];
                                }
                                array_to_regs(&mut self.cpu, &regs);
                                {
                                    let interp = match &self.backend {
                                        ExecBackend::Tcg { interp, .. } => interp,
                                        _ => unreachable!(),
                                    };
                                    sync_sysregs_from_interp(&mut self.cpu, interp);
                                }
                                self.cpu.regs.daif = regs[REG_DAIF as usize] as u32;
                                self.cpu.regs.nzcv = regs[REG_NZCV as usize] as u32;
                                self.cpu.regs.current_el =
                                    ((regs[REG_CURRENT_EL as usize] >> 2) & 3) as u8;
                                self.cpu.regs.sp_sel =
                                    (regs[REG_SPSEL as usize] & 1) as u8;
                                {
                                    let interp = match &mut self.backend {
                                        ExecBackend::Tcg { interp, .. } => interp,
                                        _ => unreachable!(),
                                    };
                                    sync_sysregs_to_interp(&self.cpu, interp);
                                }
                                regs = regs_to_array(&self.cpu);
                                break 'chain;
                            }
                            InterpExit::Exception { .. } => {
                                // Sync exception regs, fall back to interpreter
                                // for BRK/SVC/HVC/SMC handling.
                                {
                                    use helm_isa::arm::aarch64::sysreg as sr;
                                    regs[REG_ELR_EL1 as usize]  = sysregs_slice[tcg_sysreg_idx(sr::ELR_EL1)];
                                    regs[REG_SPSR_EL1 as usize] = sysregs_slice[tcg_sysreg_idx(sr::SPSR_EL1)];
                                    regs[REG_ESR_EL1 as usize]  = sysregs_slice[tcg_sysreg_idx(sr::ESR_EL1)];
                                    regs[REG_VBAR_EL1 as usize] = sysregs_slice[tcg_sysreg_idx(sr::VBAR_EL1)];
                                }
                                array_to_regs(&mut self.cpu, &regs);
                                {
                                    let interp = match &self.backend {
                                        ExecBackend::Tcg { interp, .. } => interp,
                                        _ => unreachable!(),
                                    };
                                    sync_sysregs_from_interp(&mut self.cpu, interp);
                                }
                                self.step_interp();
                                {
                                    let interp = match &mut self.backend {
                                        ExecBackend::Tcg { interp, .. } => interp,
                                        _ => unreachable!(),
                                    };
                                    sync_sysregs_to_interp(&self.cpu, interp);
                                }
                                regs = regs_to_array(&self.cpu);
                                break 'chain;
                            }
                            InterpExit::Syscall { .. } => {
                                break 'chain;
                            }
                        }
                    } // 'chain
                    continue; // outer loop — handles timer/IRQ/WFI
                }

                // JIT miss — translate and compile
                {
                    // Sync MMU for instruction fetch translation
                    if !sysregs_raw.is_null() {
                        let sysregs_ro = unsafe {
                            std::slice::from_raw_parts(sysregs_raw, helm_tcg::interp::SYSREG_FILE_SIZE)
                        };
                        sync_mmu_to_cpu(&mut self.cpu, &regs, sysregs_ro);
                    }
                }
                {
                    let cache = match &mut self.backend {
                        ExecBackend::Tcg { cache, .. } => cache,
                        _ => unreachable!(),
                    };
                    if !cache.contains_key(&pc) {
                        let block = translate_block_fs(pc, &mut self.cpu, &mut self.mem, 64);
                        if block.insn_count > 0 {
                            cache.insert(pc, block);
                        }
                    }
                    if let Some(block) = cache.get(&pc) {
                        if let Some(jit_engine) = &mut self.jit_engine {
                            if let Some(jit_block) = jit_engine.compile(block) {
                                self.jit_cache[jidx] = Some(JitCacheEntry { pc, block: jit_block });
                                continue;
                            }
                        }
                    }
                }
            }

            // === Interpretive fallback — sync regs to/from CPU ===
            array_to_regs(&mut self.cpu, &regs);
            if has_jit {
                // Sync sysregs interp→CPU before step so CPU has current
                // VBAR/SCTLR/etc. for exception handling and MMU translation
                let interp = match &self.backend {
                    ExecBackend::Tcg { interp, .. } => interp,
                    _ => unreachable!(),
                };
                sync_sysregs_from_interp(&mut self.cpu, interp);
            }
            self.step_interp();
            if has_jit {
                // Sync sysregs CPU→interp so JIT blocks see updated state
                let interp = match &mut self.backend {
                    ExecBackend::Tcg { interp, .. } => interp,
                    _ => unreachable!(),
                };
                sync_sysregs_to_interp(&self.cpu, interp);
            }
            regs = regs_to_array(&self.cpu);
        }
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

    /// Read `size` bytes from a guest virtual address, translating via the
    /// CPU's MMU.  Returns None on translation fault.
    pub fn read_virtual(&mut self, va: u64, size: usize) -> Option<Vec<u8>> {
        let pa = self.cpu.translate_va_jit(va, false, false, &mut self.mem)?;
        let mut buf = vec![0u8; size];
        self.mem.read(pa, &mut buf).ok()?;
        Some(buf)
    }

    /// Return session statistics.
    pub fn stats(&self) -> FsStats {
        FsStats {
            insn_count: self.insn_count,
            virtual_cycles: self.virtual_cycles,
            irq_count: self.irq_count,
            isa_skip_count: self.isa_skip_count,
        }
    }
}

/// Snapshot of FS session counters.
#[derive(Debug, Clone)]
pub struct FsStats {
    pub insn_count: u64,
    pub virtual_cycles: u64,
    pub irq_count: u64,
    pub isa_skip_count: u64,
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

fn translate_block_fs(
    pc: u64,
    cpu: &mut Aarch64Cpu,
    mem: &mut AddressSpace,
    max_insns: usize,
) -> TcgBlock {
    let mut ctx = TcgContext::new();
    let mut cur = pc;
    let mut n = 0;
    for _ in 0..max_insns {
        // VA→PA for instruction fetch (respects MMU)
        let fetch_pa = if cpu.regs.sctlr_el1 & 1 != 0 {
            match cpu.translate_va_jit(cur, false, true, mem) {
                Some(pa) => pa,
                None => break,
            }
        } else {
            cur
        };
        let mut buf = [0u8; 4];
        if mem.read(fetch_pa, &mut buf).is_err() {
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
                cur += 4;
                break;
            }
            TranslateAction::Unhandled => break,
        }
    }
    // Write fallthrough PC only if the block doesn't already write PC.
    // Branch instructions (B/BL/BR/RET/ERET/CBZ/B.cond) emit WriteReg(PC).
    // Non-branch EndBlock (ISB/WFI/SVC) and Unhandled need the fallthrough.
    if n > 0 {
        let ops = ctx.ops();
        let has_pc_write = ops.iter().any(|op| match op {
            TcgOp::WriteReg { reg_id, .. } if *reg_id == REG_PC => true,
            TcgOp::GotoTb { .. } | TcgOp::Eret | TcgOp::Syscall { .. }
            | TcgOp::SvcExc { .. } | TcgOp::HvcExc { .. } | TcgOp::SmcExc { .. } => true,
            _ => false,
        });
        if !has_pc_write {
            let next_pc = ctx.movi(cur);
            ctx.write_reg(REG_PC, next_pc);
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
    r[REG_SP as usize] = cpu.current_sp();
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
    // Restore both physical SPs before setting EL/SPSel so that the
    // currently-active SP is consistent with the selector.
    //
    // The regs array has:
    //   r[SP]      = the stack pointer that was "current" at the time
    //                 regs_to_array was called (or as modified by the JIT).
    //   r[SP_EL1]  = the kernel stack pointer (sp_el1).
    //
    // We write sp_el1 first, then set EL/SPSel, then write the
    // "current" SP so that set_current_sp targets the right slot.
    cpu.regs.sp_el1 = r[REG_SP_EL1 as usize];
    cpu.regs.current_el = ((r[REG_CURRENT_EL as usize] >> 2) & 3) as u8;
    cpu.regs.sp_sel = (r[REG_SPSEL as usize] & 1) as u8;
    cpu.set_current_sp(r[REG_SP as usize]);
    cpu.regs.pc = r[REG_PC as usize];
    cpu.regs.nzcv = r[REG_NZCV as usize] as u32;
    cpu.regs.daif = r[REG_DAIF as usize] as u32;
    cpu.regs.elr_el1 = r[REG_ELR_EL1 as usize];
    cpu.regs.spsr_el1 = r[REG_SPSR_EL1 as usize] as u32;
    cpu.regs.esr_el1 = r[REG_ESR_EL1 as usize] as u32;
    cpu.regs.vbar_el1 = r[REG_VBAR_EL1 as usize];
}

/// Lightweight sync: copy only MMU-critical sysregs from the interp sysreg
/// array back to cpu.regs so translate_va works from JIT memory helpers.
/// Also syncs current_el and sp_sel from the persistent regs array.
fn sync_mmu_to_cpu(cpu: &mut Aarch64Cpu, regs: &[u64; NUM_REGS], sysregs: &[u64]) {
    use helm_isa::arm::aarch64::sysreg;
    use helm_tcg::interp::sysreg_idx;
    cpu.regs.current_el = ((regs[REG_CURRENT_EL as usize] >> 2) & 3) as u8;
    cpu.regs.sp_sel = (regs[REG_SPSEL as usize] & 1) as u8;
    let new_sctlr = sysregs[sysreg_idx(sysreg::SCTLR_EL1)];
    let new_tcr   = sysregs[sysreg_idx(sysreg::TCR_EL1)];
    let new_ttbr0 = sysregs[sysreg_idx(sysreg::TTBR0_EL1)];
    let new_ttbr1 = sysregs[sysreg_idx(sysreg::TTBR1_EL1)];
    let need_flush = new_sctlr != cpu.regs.sctlr_el1
        || new_tcr != cpu.regs.tcr_el1
        || new_ttbr0 != cpu.regs.ttbr0_el1
        || new_ttbr1 != cpu.regs.ttbr1_el1;
    cpu.regs.sctlr_el1 = new_sctlr;
    cpu.regs.tcr_el1 = new_tcr;
    cpu.regs.ttbr0_el1 = new_ttbr0;
    cpu.regs.ttbr1_el1 = new_ttbr1;
    cpu.regs.mair_el1 = sysregs[sysreg_idx(sysreg::MAIR_EL1)];
    cpu.regs.vbar_el1 = sysregs[sysreg_idx(sysreg::VBAR_EL1)];
    cpu.regs.hcr_el2  = sysregs[sysreg_idx(sysreg::HCR_EL2)];
    if need_flush {
        cpu.flush_tlb_all();
    }
}

/// Sync exception-class sysregs (ELR_EL1, SPSR_EL1, ESR_EL1, VBAR_EL1) from
/// the interp sysreg array into the flat `regs` array.
///
/// Called only on `ExceptionReturn` and `Exception` chain-loop exits, not on
/// every block boundary.  The JIT's `WriteSysReg` keeps these in `sysregs`;
/// the flat `regs` array is the authoritative source for ERET/exception logic
/// in `step_interp` and `array_to_regs`.
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

/// Direct-mapped compiled-block cache.  Indexed by `(pc >> 2) & MASK`.
const BLOCK_CACHE_BITS: usize = 12;
const BLOCK_CACHE_SIZE: usize = 1 << BLOCK_CACHE_BITS;
const BLOCK_CACHE_MASK: usize = BLOCK_CACHE_SIZE - 1;

struct BlockCacheEntry {
    pc: u64,
    block: CompiledBlock,
}

/// JIT VA→PA translation callback. Called from JIT memory helpers.
unsafe extern "C" fn jit_translate_va(
    cpu_ctx: *mut u8,
    mem_ctx: *mut u8,
    va: u64,
    is_write: u64,
) -> u64 {
    let cpu = &mut *(cpu_ctx as *mut Aarch64Cpu);
    let mem = &mut *(mem_ctx as *mut AddressSpace);
    match cpu.translate_va_jit(va, is_write != 0, false, mem) {
        Some(pa) => pa,
        None => u64::MAX, // TRANSLATE_FAIL sentinel
    }
}

/// JIT TLBI callback.  Called when JIT code executes a TLBI instruction.
/// `op` encodes `(op1 << 8) | (crm << 4) | op2`.
/// `addr_value` is the Xt register value for VA-based TLBI variants.
unsafe extern "C" fn jit_tlbi(cpu_ctx: *mut u8, op: u64, addr_value: u64) {
    let cpu = &mut *(cpu_ctx as *mut Aarch64Cpu);
    let op1 = ((op >> 8) & 0x7) as u32;
    let crm = ((op >> 4) & 0xF) as u32;
    let op2 = (op & 0x7) as u32;

    match (op1, crm, op2) {
        // VA-based invalidations: extract VA from Xt[43:0] << 12, sign-extended
        (0, 3, 1) | (0, 7, 1)
        | (0, 3, 5) | (0, 7, 5)
        | (0, 3, 3) | (0, 7, 3)
        | (0, 3, 7) | (0, 7, 7)
        | (4, 3, 1) | (4, 7, 1)
        | (4, 3, 5) | (4, 7, 5)
        | (6, 3, 1) | (6, 7, 1)
        | (6, 3, 5) | (6, 7, 5)
        => {
            let raw = addr_value << 12;
            let va = if raw & (1u64 << 55) != 0 {
                raw | 0xFF00_0000_0000_0000
            } else {
                raw
            };
            cpu.flush_tlb_va(va);
        }
        _ => cpu.flush_tlb_all(),
    }
}

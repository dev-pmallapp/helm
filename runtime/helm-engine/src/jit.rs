use helm_arch::aarch64::mmu::{self, MmuAccess, MmuConfig, TlbStats};
use helm_arch::aarch64_decode;
#[cfg(feature = "jit-stencil")]
use helm_arch::{riscv_decode, riscv_expand_c};
use helm_core::{AccessType, MemInterface};
use helm_jit::block::{EXIT_EL_CHANGE, EXIT_EXIT, EXIT_PSCI, EXIT_SYSCALL, EXIT_WFI};
use helm_jit::runtime::{
    dispatch_trace, ensure_aarch64_jit_runtime_state, execute_cache_hit,
    plan_aarch64_trace_recording, prepare_aarch64_jit_dispatch_context, probe_block_cache,
    record_aarch64_trace_candidate, resolve_aarch64_compile_miss, Aarch64JitBackendMode,
    Aarch64JitBackendPolicy, Aarch64JitMemoryMode, BlockCacheProbe, CompileMissResolution,
    JitRuntimeHost, TraceDispatch, TraceRecordPlan,
};
use helm_jit::{block::EXIT_END_OF_BLOCK, regs};
use helm_probe::probe;
use helm_timing::TimingModel;

use crate::{
    session::Aarch64Core, ExecMode, FlatMem, HelmEngine, HelmGic, HelmSim, Isa, StopReason,
};
use helm_core::HartException;

impl<T: TimingModel> JitRuntimeHost for HelmEngine<T> {
    type StopReason = StopReason;

    fn jit_stats_mut(&mut self) -> &mut helm_stats::JitPerfStats {
        &mut self.jit_stats
    }

    fn insns_retired(&self) -> u64 {
        self.insns_retired
    }

    fn run_interpreter_batch(&mut self, max_insns: u64) -> Self::StopReason {
        self.run(max_insns)
    }

    fn is_resumable_stop(stop: &Self::StopReason) -> bool {
        matches!(stop, StopReason::Quantum)
    }

    fn prepare_interpreter_fallback(&mut self, flat_regs: &mut [u64], retired_insns: u64) {
        self.commit_aarch64_jit_state(flat_regs, retired_insns);
    }

    fn restore_jit_state_after_interpreter(
        &mut self,
        flat_regs: &mut [u64],
    ) -> Result<(), Self::StopReason> {
        let restored = self
            .rebuild_aarch64_jit_flat_state()
            .ok_or(StopReason::Unsupported)?;
        flat_regs.copy_from_slice(&restored);

        Ok(())
    }

    fn record_interpreter_fallback(&mut self, consumed: u64, reason: Option<&'static str>) {
        self.emit_jit_fallback_event(consumed, reason);
    }
}

impl<T: TimingModel> HelmEngine<T> {
    fn current_jit_pc(&self) -> u64 {
        self.aarch64_state_for_current_context()
            .map_or_else(|| self.session.riscv().map_or(0, |r| r.pc), |s| s.pc)
    }

    fn emit_jit_fallback_event(&mut self, consumed: u64, reason: Option<&'static str>) {
        let pc = self.current_jit_pc();

        #[cfg(feature = "jit")]
        if self.jit_probes.any_active() && self.jit_debug.is_window_active() {
            probe!(
                self.jit_probes.fallback,
                helm_probe::JitFallbackEvent {
                    pc,
                    insns: consumed,
                    reason,
                }
            );
        }

        if self.plugins.has_jit_fallback_callbacks() {
            self.plugins.fire_jit_fallback(pc, reason);
        }

        if let Some(reason) = reason {
            *self
                .jit_stats
                .reject_reasons
                .entry(reason.to_string())
                .or_insert(0) += 1;
        }
    }

    fn emit_jit_cache_event(&mut self, pc: u64, op: helm_probe::JitCacheOp, exec_count: u32) {
        #[cfg(feature = "jit")]
        if self.jit_probes.any_active() && self.jit_debug.is_window_active() {
            probe!(
                self.jit_probes.cache,
                helm_probe::JitCacheEvent { pc, op, exec_count }
            );
        }
    }

    fn emit_jit_guard_exit_event(
        &mut self,
        trace_pc: u64,
        guard_id: u32,
        resume_pc: u64,
        miss_count: u32,
        retiring: bool,
    ) {
        #[cfg(feature = "jit")]
        if self.jit_probes.any_active() && self.jit_debug.is_window_active() {
            probe!(
                self.jit_probes.guard_exit,
                helm_probe::JitGuardExitEvent {
                    trace_pc,
                    guard_id,
                    resume_pc,
                    miss_count,
                    retiring,
                }
            );
        }
    }

    fn emit_mmu_translate_event(
        &mut self,
        va: u64,
        access: MmuAccess,
        before: TlbStats,
        after: TlbStats,
    ) {
        if !self.probes.mmu.has_listeners() {
            return;
        }
        let tlb_hit = after.hits > before.hits;
        let tlb_miss = after.misses > before.misses;
        let stage1_walk = after.stage1_walks > before.stage1_walks;
        let stage2_walk = after.stage2_walks > before.stage2_walks;
        if !tlb_hit && !tlb_miss && !stage1_walk && !stage2_walk {
            return;
        }
        probe!(
            self.probes.mmu,
            helm_probe::MmuTranslateEvent {
                va,
                access: match access {
                    MmuAccess::Read => helm_probe::MmuAccessKind::Read,
                    MmuAccess::Write => helm_probe::MmuAccessKind::Write,
                    MmuAccess::Execute => helm_probe::MmuAccessKind::Execute,
                },
                tlb_hit,
                tlb_miss,
                stage1_walk,
                stage2_walk,
            }
        );
    }

    pub(crate) fn effective_jit_runtime_config(&self) -> helm_jit::runtime::JitRuntimeConfig {
        let mut config = self.jit_runtime_config;
        if self.active_mode() == ExecMode::System {
            config.trace_dispatch_enabled = false;
        }
        config
    }

    fn jit_block_context_from_flat(
        flat_regs: &[u64; regs::REG_COUNT],
    ) -> helm_probe::JitBlockContext {
        let mut x = [0u64; 31];
        x.copy_from_slice(&flat_regs[regs::REG_X0..regs::REG_X0 + 31]);
        helm_probe::JitBlockContext {
            x,
            sp: flat_regs[regs::REG_SP],
            pc: flat_regs[regs::REG_PC],
            nzcv: flat_regs[regs::REG_NZCV] as u32,
            current_el: flat_regs[regs::REG_CURRENT_EL] as u8,
        }
    }

    fn is_aarch64_jit_block_terminator(insn: &helm_arch::Aarch64Insn) -> bool {
        matches!(
            insn.opcode,
            helm_arch::aarch64::insn::Opcode::B
                | helm_arch::aarch64::insn::Opcode::Bl
                | helm_arch::aarch64::insn::Opcode::Br
                | helm_arch::aarch64::insn::Opcode::Blr
                | helm_arch::aarch64::insn::Opcode::Ret
                | helm_arch::aarch64::insn::Opcode::Svc
                | helm_arch::aarch64::insn::Opcode::Hvc
                | helm_arch::aarch64::insn::Opcode::Smc
                | helm_arch::aarch64::insn::Opcode::Eret
                | helm_arch::aarch64::insn::Opcode::RetAut
                | helm_arch::aarch64::insn::Opcode::BrAut
                | helm_arch::aarch64::insn::Opcode::BlrAut
                | helm_arch::aarch64::insn::Opcode::BrAutZ
                | helm_arch::aarch64::insn::Opcode::BlrAutZ
                | helm_arch::aarch64::insn::Opcode::EretAut
        )
    }

    /// In FS mode, ISB/DSB/MSR-to-control-regs must terminate blocks so the
    /// dispatch loop refreshes the MmuConfig snapshot. Without this, stores
    /// after an MMU-enabling MSR+ISB sequence use a stale MmuConfig and the
    /// VA->PA translation produces wrong results.
    fn is_aarch64_jit_fs_block_terminator(insn: &helm_arch::Aarch64Insn) -> bool {
        Self::is_aarch64_jit_block_terminator(insn)
            || matches!(
                insn.opcode,
                helm_arch::aarch64::insn::Opcode::Isb | helm_arch::aarch64::insn::Opcode::Dsb
            )
    }

    fn build_aarch64_jit_dispatch_context(
        &mut self,
        flat_regs: &mut [u64; regs::REG_COUNT],
    ) -> Option<helm_jit::runtime::Aarch64JitDispatchContext> {
        if self.active_mode() == ExecMode::System {
            let a64_ref = self.aarch64_state_for_current_context()?;
            let mmu_cfg = helm_arch::aarch64::mmu::MmuConfig::from_arch(a64_ref);
            // Store arch state pointer for MRS/MSR helper access.
            flat_regs[regs::REG_JIT_ARCH_STATE] =
                a64_ref as *const _ as *mut helm_arch::aarch64::Aarch64ArchState as u64;
            let active_fs_vcpu = self.active_fs_vcpu;
            let board = self
                .session
                .aarch64_mut()
                .and_then(Aarch64Core::machine_mut)?;
            let vcpu_idx = active_fs_vcpu.min(board.vcpus.len().saturating_sub(1));
            Some(prepare_aarch64_jit_dispatch_context(
                flat_regs,
                Aarch64JitMemoryMode::Fs {
                    sys_mem: board.sys_mem.as_mut() as *mut _,
                    tlb: &mut board.vcpus[vcpu_idx].fs.tlb as *mut _,
                    mmu_cfg,
                },
            ))
        } else {
            // Store arch state pointer for MRS/MSR helper access (SE mode too).
            if let Some(a64_ref) = self.aarch64_state_for_current_context() {
                flat_regs[regs::REG_JIT_ARCH_STATE] =
                    a64_ref as *const _ as *mut helm_arch::aarch64::Aarch64ArchState as u64;
            }
            let se_tlb = self
                .jit_se_tlb
                .get_or_insert_with(|| Box::new(helm_jit::helpers::JitSeTlb::new()));
            Some(prepare_aarch64_jit_dispatch_context(
                flat_regs,
                Aarch64JitMemoryMode::Se {
                    mem_ptr: &mut self.memory as *mut FlatMem as *mut u8,
                    se_tlb: se_tlb.as_mut(),
                },
            ))
        }
    }

    fn commit_aarch64_jit_state(&mut self, flat_regs: &mut [u64], retired_insns: u64) {
        let flat_regs = <&mut [u64; regs::REG_COUNT]>::try_from(flat_regs)
            .expect("aarch64 flat register image");
        let a64_mut = self
            .aarch64_state_mut_for_current_context()
            .expect("aarch64 state");
        regs::flat_to_arch(flat_regs, a64_mut);
        self.insns_retired += retired_insns;
    }

    /// Commit only GPRs and NZCV from the flat array to arch state after an
    /// EL-transition exit. The exception helper already updated PC, CurrentEL,
    /// DAIF, SPSel, SPSR, and ELR directly in arch state, so we must not
    /// overwrite those fields.
    fn commit_aarch64_jit_gprs_after_el_change(
        &mut self,
        flat_regs: &mut [u64],
        retired_insns: u64,
    ) {
        let flat_regs = <&mut [u64; regs::REG_COUNT]>::try_from(flat_regs)
            .expect("aarch64 flat register image");
        let a64_mut = self
            .aarch64_state_mut_for_current_context()
            .expect("aarch64 state");
        // Write back GPRs (X0-X30).
        for i in 0..31 {
            a64_mut.x[i] = flat_regs[regs::REG_X0 + i];
        }
        // Write back NZCV (not modified by exception helpers).
        a64_mut.nzcv = flat_regs[regs::REG_NZCV] as u32;
        // Write back SIMD registers (V0-V31).
        for i in 0..32 {
            a64_mut.v[i] = flat_regs[regs::REG_V_BASE + i * 2] as u128
                | ((flat_regs[regs::REG_V_BASE + i * 2 + 1] as u128) << 64);
        }
        // Write back SP using the PRE-exception banking. The helper changed
        // current_el/spsel, but the SP in flat_regs corresponds to the
        // banked SP at the time the block was entered (before the exception).
        // The exception entry code does not modify SP itself; it only switches
        // the bank via current_el/spsel. We need to write the flat SP back
        // to the bank that was active BEFORE the exception.
        //
        // However, we don't know the pre-exception EL here. The simplest
        // correct approach: the flat_to_arch function uses current_el/spsel
        // to decide which SP bank to write. Since the helper has already
        // changed current_el, flat_to_arch would write to the wrong bank.
        //
        // Instead, we write SP to the bank indicated by the flat DAIF/CurrentEL
        // slots (which are stale, reflecting the pre-exception state).
        let pre_exc_el = flat_regs[regs::REG_CURRENT_EL] as u8;
        let pre_exc_spsel = flat_regs[regs::REG_SPSEL] != 0;
        if pre_exc_el >= 1 && pre_exc_spsel {
            match pre_exc_el {
                1 => a64_mut.sp_el1 = flat_regs[regs::REG_SP],
                2 => a64_mut.sp_el2 = flat_regs[regs::REG_SP],
                3 => a64_mut.sp_el3 = flat_regs[regs::REG_SP],
                _ => a64_mut.sp = flat_regs[regs::REG_SP],
            }
        } else {
            a64_mut.sp = flat_regs[regs::REG_SP];
        }
        // Re-zero XZR sentinel.
        flat_regs[regs::REG_XZR] = 0;
        self.insns_retired += retired_insns;
    }

    /// Perform FS-mode bookkeeping between JIT blocks: TLB flush, tick
    /// advancement, IRQ injection. Returns `Some(StopReason)` if the JIT
    /// loop should exit (e.g. WFI with no pending IRQ).
    fn jit_fs_bookkeeping(&mut self, block_retired: u64) -> Option<StopReason> {
        let active_fs_vcpu = self.active_fs_vcpu;
        let board = self
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::machine_mut)?;
        let vcpu_idx = active_fs_vcpu.min(board.vcpus.len().saturating_sub(1));
        let vcpu = &mut board.vcpus[vcpu_idx];
        let a64 = &mut vcpu.arch;
        let fs = &mut vcpu.fs;

        // TLB flush: honour TLBI instructions executed during the block.
        if a64.tlb_flush_pending {
            if let Some(va) = a64.tlb_flush_va.take() {
                fs.tlb.invalidate_va(va);
            } else if let Some(asid) = a64.tlb_flush_asid.take() {
                fs.tlb.flush_asid(asid);
            } else if let Some(vmid) = a64.tlb_flush_vmid.take() {
                fs.tlb.flush_vmid(vmid);
            } else if let Some((vmid, ipa_page)) = a64.tlb_flush_ipa.take() {
                fs.tlb.flush_ipa(vmid, ipa_page);
            } else {
                fs.tlb.flush();
            }
            a64.tlb_flush_pending = false;
            a64.tlb_flush_broadcast = false;
        }

        // Advance tick counter by retired instructions.
        let tick_scale = {
            let phys_live = (a64.cntp_ctl_el0 & 1 != 0) && (a64.cntp_ctl_el0 & 2 == 0);
            let virt_live = (a64.cntv_ctl_el0 & 1 != 0) && (a64.cntv_ctl_el0 & 2 == 0);
            let hyp_live = (a64.cnthp_ctl_el2 & 1 != 0) && (a64.cnthp_ctl_el2 & 2 == 0);
            if phys_live || virt_live || hyp_live {
                1u64
            } else {
                fs.tick_scale
            }
        };
        fs.tick += block_retired.saturating_mul(tick_scale);
        a64.cntvct_el0 = fs.tick;

        // Timer check: inject timer IRQs periodically.
        self.timer_countdown = self.timer_countdown.saturating_sub(block_retired as u32);
        if self.timer_countdown == 0 {
            self.timer_countdown = crate::next_timer_countdown(a64, fs);
            match board.gic.as_ref() {
                Some(HelmGic::V3(shared)) => {
                    crate::platform::arm_virt::inject_timers_gicv3(a64, fs, shared, vcpu_idx);
                }
                _ => {
                    crate::platform::arm_virt::inject_timers_gicv2(a64, fs, &mut board.sys_mem);
                }
            }
        }

        // IRQ poll: check the GIC IRQ line.
        self.irq_poll_countdown = self.irq_poll_countdown.saturating_sub(1);
        if self.irq_poll_countdown == 0 {
            self.irq_poll_countdown = crate::IRQ_POLL_INTERVAL;
            fs.irq_pending = board
                .irq_lines
                .get(vcpu_idx)
                .map_or(false, |l| l.load(std::sync::atomic::Ordering::Relaxed));
        }

        // IRQ delivery: if unmasked, deliver the IRQ exception.
        if fs.irq_pending && (a64.daif & 0x2) == 0 {
            use helm_arch::aarch64::exception;
            let target_el = exception::route_physical_irq(a64);
            let vector_offset = exception::irq_vector_offset(a64, target_el);
            exception::irq_entry_with_offset(a64, target_el, vector_offset);
            fs.irq_pending = false;
        }

        None
    }

    #[cfg(feature = "jit-stencil")]
    fn commit_rv64_jit_state(&mut self, flat_regs: &mut [u64], retired_insns: u64) {
        let flat_regs = <&mut [u64; regs::REG_COUNT_RV64]>::try_from(flat_regs)
            .expect("rv64 flat register image");
        let rv_mut = self.session.riscv_mut().expect("riscv state");
        regs::flat_to_arch_rv64(flat_regs, &mut rv_mut.iregs, &mut rv_mut.pc);
        self.insns_retired += retired_insns;
    }

    #[cfg(feature = "jit-stencil")]
    fn handoff_rv64_jit_to_interpreter(
        &mut self,
        flat_regs: &mut [u64],
        retired: &mut u64,
        budget_remaining: u64,
        reason: Option<&'static str>,
    ) -> StopReason {
        let before = self.insns_retired;
        self.commit_rv64_jit_state(flat_regs, *retired);
        *retired = 0;
        let stop = self.run(budget_remaining);
        let consumed = self.insns_retired.saturating_sub(before);
        self.emit_jit_fallback_event(consumed, reason);
        stop
    }

    fn rebuild_aarch64_jit_flat_state(&mut self) -> Option<[u64; regs::REG_COUNT]> {
        let a64 = self.aarch64_state_for_current_context()?;
        Some(regs::arch_to_flat(a64))
    }

    fn fetch_aarch64_jit_word(&mut self, pc: u64) -> Option<u32> {
        if self.active_mode() != ExecMode::System {
            return self.memory.fetch32(pc).ok();
        }

        let active_fs_vcpu = self.active_fs_vcpu;
        let (raw, mmu_stats) = {
            let board = self
                .session
                .aarch64_mut()
                .and_then(Aarch64Core::machine_mut)?;
            let vcpu_idx = active_fs_vcpu.min(board.vcpus.len().saturating_sub(1));
            let mmu_cfg = MmuConfig::from_arch(&board.vcpus[vcpu_idx].arch);
            let (pa, mmu_stats) = if mmu_cfg.mmu_enabled() {
                let before = if self.probes.mmu.has_listeners() {
                    Some(board.vcpus[vcpu_idx].fs.tlb.stats())
                } else {
                    None
                };
                let result = mmu::translate_cfg(
                    &mmu_cfg,
                    pc,
                    MmuAccess::Execute,
                    board.sys_mem.as_mut(),
                    Some(&mut board.vcpus[vcpu_idx].fs.tlb),
                )
                .ok();
                let after = before.map(|_| board.vcpus[vcpu_idx].fs.tlb.stats());
                (result?, before.zip(after))
            } else {
                (pc, None)
            };
            let raw = board
                .sys_mem
                .read(pa, 4, AccessType::Load)
                .ok()
                .map(|word| word as u32)?;
            Some((raw, mmu_stats))
        }?;
        if let Some((before, after)) = mmu_stats {
            self.emit_mmu_translate_event(pc, MmuAccess::Execute, before, after);
        }
        Some(raw)
    }

    fn decode_aarch64_jit_block(&mut self, pc: u64) -> Vec<helm_arch::Aarch64Insn> {
        // Reuse the decode buffer to avoid per-miss or per-promotion allocation.
        let mut insns = std::mem::take(&mut self.jit_decode_buf);
        insns.clear();
        let mut decode_pc = pc;
        for _ in 0..64 {
            let Some(raw) = self.fetch_aarch64_jit_word(decode_pc) else {
                break;
            };
            match aarch64_decode(raw, decode_pc) {
                Ok(insn) => {
                    let terminates_block = if self.active_mode() == ExecMode::System {
                        Self::is_aarch64_jit_fs_block_terminator(&insn)
                    } else {
                        Self::is_aarch64_jit_block_terminator(&insn)
                    };
                    insns.push(insn);
                    decode_pc += 4;
                    if terminates_block {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        insns
    }

    #[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
    fn maybe_note_aarch64_trace_candidate(&mut self, start_pc: u64, next_pc: u64) {
        let should_decode = {
            let recorder = self
                .jit_trace_recorder
                .get_or_insert_with(helm_jit::trace::recorder::TraceRecorder::default);
            plan_aarch64_trace_recording(recorder, start_pc, next_pc)
                == TraceRecordPlan::DecodeCurrentBlock
        };
        if !should_decode {
            return;
        }

        let insns = self.decode_aarch64_jit_block(start_pc);
        let recorder = self
            .jit_trace_recorder
            .as_mut()
            .expect("trace recorder should remain available");
        let trace_result = record_aarch64_trace_candidate(
            recorder,
            self.jit_trace_cache.as_mut(),
            &mut self.jit_stats,
            &insns,
        );
        if let helm_jit::runtime::TraceRecordResult::Compiled {
            start_pc,
            insn_count,
        } = trace_result
        {
            helm_probe::update_probe_insn_count(self.insns_retired);
            probe!(
                self.jit_probes.trace_compile,
                helm_probe::JitTraceCompileEvent {
                    start_pc,
                    insn_count,
                    guard_count: 0, // not available at this level
                }
            );
        }

        self.jit_decode_buf = insns;
    }

    /// Enable or disable HAJ (Helm Adaptive JIT).
    pub fn set_jit(&mut self, enabled: bool) {
        self.jit_enabled = enabled;
        if enabled {
            if self.isa == Isa::RiscV {
                #[cfg(feature = "jit-stencil")]
                if self.jit_rv64_backend.is_none()
                    && helm_jit::runtime::ensure_rv64_jit_runtime_state(
                        &mut self.jit_cache,
                        &mut self.jit_rv64_backend,
                    )
                {
                    log::info!("jit: RISC-V64 stencil backend enabled");
                }
            } else {
                let backend_was_none = self.jit_backend.is_none();
                let policy = {
                    #[cfg(feature = "jit-tiered")]
                    {
                        Aarch64JitBackendPolicy::Tiered
                    }
                    #[cfg(all(feature = "jit-stencil", not(feature = "jit-tiered")))]
                    {
                        Aarch64JitBackendPolicy::StencilOnly
                    }
                    #[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
                    {
                        Aarch64JitBackendPolicy::DynasmOnly
                    }
                    #[cfg(not(any(
                        feature = "jit-tiered",
                        feature = "jit-stencil",
                        feature = "jit-dynasm"
                    )))]
                    {
                        Aarch64JitBackendPolicy::DynasmOnly
                    }
                };

                let mode = ensure_aarch64_jit_runtime_state(
                    &mut self.jit_cache,
                    &mut self.jit_backend,
                    &mut self.jit_hot_backend,
                    &mut self.jit_trace_cache,
                    &mut self.jit_trace_recorder,
                    policy,
                );

                if backend_was_none {
                    match mode {
                        Aarch64JitBackendMode::Tiered => {
                            log::info!("jit: HAJ enabled (adaptive baseline + hot-tier promotion)");
                        }
                        Aarch64JitBackendMode::DynasmOnly => {
                            log::info!("jit: dynasm backend enabled");
                        }
                        Aarch64JitBackendMode::StencilOnly => {
                            log::info!("jit: stencil backend enabled");
                        }
                        Aarch64JitBackendMode::Unavailable => {}
                    }
                }
            }
        }
    }

    /// Override shared JIT runtime policy knobs for this engine instance.
    pub fn set_jit_runtime_config(&mut self, config: helm_jit::runtime::JitRuntimeConfig) {
        self.jit_runtime_config = config;
    }

    /// Run up to `max_insns` instructions using the JIT backend.
    ///
    /// Falls back to the interpreter for unsupported opcodes. Works for
    /// AArch64 SE and FS modes.
    ///
    /// This path stays explicitly opt-in for now. `HelmEngine::run()` keeps
    /// the interpreter as the source of truth for per-instruction timing,
    /// event-queue advancement, and probe/plugin delivery until the JIT path
    /// is fully integrated with the same bookkeeping.
    #[allow(unsafe_code)]
    pub fn run_jit(&mut self, max_insns: u64) -> StopReason {
        if self.isa == Isa::RiscV {
            #[cfg(feature = "jit-stencil")]
            return self.run_jit_rv64(max_insns);
            #[cfg(not(feature = "jit-stencil"))]
            return self.run(max_insns);
        }

        let cache = match self.jit_cache.as_mut() {
            Some(c) => c as *mut helm_jit::cache::JitCache,
            None => return self.run(max_insns),
        };

        let mut flat_regs = match self.rebuild_aarch64_jit_flat_state() {
            Some(regs) => regs,
            None => return StopReason::Unsupported,
        };

        let mut retired: u64 = 0;
        let mut budget_remaining = max_insns;
        let runtime_config = self.effective_jit_runtime_config();

        while budget_remaining > 0 {
            // FS-mode helpers carry a snapshotted MMU/EL view, so rebuild the
            // helper context each dispatch iteration before executing compiled
            // code or re-entering after interpreter fallback.
            let mut dispatch_ctx = match self.build_aarch64_jit_dispatch_context(&mut flat_regs) {
                Some(ctx) => ctx,
                None => return StopReason::Unsupported,
            };
            let mem_ptr = dispatch_ctx.mem_ptr();

            // Propagate runtime-selected helper addresses to backends so
            // stencil-compiled blocks call the correct helpers (SE vs FS).
            let (mr, mw) = (
                flat_regs[regs::REG_JIT_MEM_READ],
                flat_regs[regs::REG_JIT_MEM_WRITE],
            );
            if let Some(b) = self.jit_backend.as_mut() {
                b.set_mem_helpers(mr, mw);
            }
            if let Some(b) = self.jit_hot_backend.as_mut() {
                b.set_mem_helpers(mr, mw);
            }

            let pc = flat_regs[regs::REG_PC];

            // ── JIT interpreter fallback range ───────────────────────────
            if let Some((start, end)) = self.jit_interp_range {
                let in_range = pc >= start && pc < end;
                let use_interp = if self.jit_interp_invert { !in_range } else { in_range };
                if use_interp {
                    self.commit_aarch64_jit_state(&mut flat_regs, retired);
                    let batch = budget_remaining.min(1_000_000);
                    let insns_before = self.insns_retired;
                    let stop = self.run(batch);
                    let consumed = self.insns_retired.saturating_sub(insns_before);
                    retired = 0;
                    flat_regs = match self.rebuild_aarch64_jit_flat_state() {
                        Some(r) => r,
                        None => return StopReason::Unsupported,
                    };
                    budget_remaining = budget_remaining.saturating_sub(consumed);
                    if stop != StopReason::Quantum {
                        return stop;
                    }
                    continue;
                }
            }

            // ── Debug controller gate ────────────────────────────────────
            if self.jit_debug.is_active() {
                use helm_jit::debug::DispatchDecision;
                match self.jit_debug.on_block_entry(pc) {
                    DispatchDecision::Breakpoint => {
                        self.commit_aarch64_jit_state(&mut flat_regs, retired);
                        return StopReason::Breakpoint;
                    }
                    DispatchDecision::FallbackToInterpreter => {
                        self.commit_aarch64_jit_state(&mut flat_regs, retired);
                        return self.run(budget_remaining);
                    }
                    DispatchDecision::Execute => {}
                }
            }
            #[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
            {
                let trace_retired_before = retired;
                match unsafe {
                    dispatch_trace(
                        self.jit_trace_cache.as_mut(),
                        &mut self.jit_stats,
                        pc,
                        &mut flat_regs,
                        mem_ptr,
                        &mut retired,
                        &mut budget_remaining,
                        runtime_config,
                    )
                } {
                    TraceDispatch::NotAvailable
                    | TraceDispatch::Miss
                    | TraceDispatch::SkippedDisabled => {}
                    TraceDispatch::Executed { exit_code, guard } => {
                        let trace_retired = retired.saturating_sub(trace_retired_before) as u32;
                        if self.jit_probes.any_active() && self.jit_debug.is_window_active() {
                            helm_probe::update_probe_insn_count(
                                self.insns_retired.saturating_add(retired),
                            );
                            probe!(
                                self.jit_probes.trace_execute,
                                helm_probe::JitTraceExecuteEvent {
                                    start_pc: pc,
                                    exit_code,
                                    resume_pc: flat_regs[regs::REG_PC],
                                    insns_retired: trace_retired,
                                }
                            );
                        }

                        let blk = retired.saturating_sub(trace_retired_before);
                        match exit_code {
                            EXIT_END_OF_BLOCK => {
                                if self.active_mode() == ExecMode::System {
                                    self.commit_aarch64_jit_state(&mut flat_regs, retired);
                                    retired = 0;
                                    self.jit_fs_bookkeeping(blk);
                                    self.drain_pending_aarch64_exception_event();
                                    flat_regs = match self.rebuild_aarch64_jit_flat_state() {
                                        Some(r) => r,
                                        None => return StopReason::Unsupported,
                                    };
                                }
                                continue;
                            }
                            EXIT_EL_CHANGE => {
                                self.commit_aarch64_jit_gprs_after_el_change(
                                    &mut flat_regs,
                                    retired,
                                );
                                retired = 0;
                                if self.active_mode() == ExecMode::System {
                                    self.jit_fs_bookkeeping(blk);
                                }
                                self.drain_pending_aarch64_exception_event();
                                flat_regs = match self.rebuild_aarch64_jit_flat_state() {
                                    Some(r) => r,
                                    None => return StopReason::Unsupported,
                                };
                                continue;
                            }
                            code if code >= helm_jit::trace::compiler::EXIT_GUARD_BASE => {
                                if let Some(guard) = guard {
                                    self.emit_jit_guard_exit_event(
                                        pc,
                                        guard.guard_id,
                                        guard.resume_pc,
                                        guard.miss_count,
                                        guard.retiring,
                                    );
                                }
                                continue;
                            }
                            _ => break,
                        }
                    }
                }
            }

            let cache_ref = unsafe { &mut *cache };
            match probe_block_cache(cache_ref, &mut self.jit_stats, pc) {
                BlockCacheProbe::Hit(hit) => {
                    helm_probe::update_probe_insn_count(self.insns_retired.saturating_add(retired));
                    self.emit_jit_cache_event(pc, helm_probe::JitCacheOp::Hit, hit.exec_count);
                    let retired_before = retired;
                    let pre_block_snapshot = if self.jit_debug.verify { Some(flat_regs) } else { None };
                    let cached_tier = hit.tier;
                    let exit_code = if hit.exec_count == helm_jit::cache::PROMOTE_THRESHOLD
                        && hit.tier == helm_jit::cache::JitTier::Stencil
                    {
                        self.emit_jit_cache_event(
                            pc,
                            helm_probe::JitCacheOp::Promote,
                            hit.exec_count,
                        );
                        // Decode the block again for dynasm recompilation.
                        // In FS mode, dynasm uses the SE-TLB path which is not
                        // wired in FS; disable hot-tier promotion to keep stencil.
                        let insns = self.decode_aarch64_jit_block(pc);
                        let hot_backend = if self.active_mode() == ExecMode::System {
                            None
                        } else {
                            self.jit_hot_backend
                                .as_mut()
                                .map(|b| b.as_mut() as *mut dyn helm_jit::backend::JitBackend)
                        };
                        let exit_code = unsafe {
                            execute_cache_hit(
                                cache_ref,
                                &mut self.jit_stats,
                                hit,
                                hot_backend.map(|ptr| &mut *ptr),
                                pc,
                                Some(&insns),
                                &mut flat_regs,
                                mem_ptr,
                                &mut retired,
                                &mut budget_remaining,
                            )
                        };
                        self.jit_decode_buf = insns;
                        exit_code
                    } else {
                        unsafe {
                            execute_cache_hit::<dyn helm_jit::backend::JitBackend>(
                                cache_ref,
                                &mut self.jit_stats,
                                hit,
                                None,
                                pc,
                                None,
                                &mut flat_regs,
                                mem_ptr,
                                &mut retired,
                                &mut budget_remaining,
                            )
                        }
                    };

                    #[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
                    if exit_code == EXIT_END_OF_BLOCK {
                        let next_pc = flat_regs[regs::REG_PC];
                        self.maybe_note_aarch64_trace_candidate(pc, next_pc);
                    }

                    // ── JIT block verification ────────────────────────────
                    // Snapshot before block, run interpreter, compare, fire probe on mismatch.
                    if self.jit_debug.verify && exit_code == EXIT_END_OF_BLOCK {
                        if let Some(snapshot) = pre_block_snapshot {
                            static REG_NAMES: [&str; 31] = [
                                "x0",  "x1",  "x2",  "x3",  "x4",  "x5",  "x6",  "x7",
                                "x8",  "x9",  "x10", "x11", "x12", "x13", "x14", "x15",
                                "x16", "x17", "x18", "x19", "x20", "x21", "x22", "x23",
                                "x24", "x25", "x26", "x27", "x28", "x29", "x30",
                            ];

                            let jit_regs: [u64; 31] = {
                                let mut x = [0u64; 31];
                                x.copy_from_slice(&flat_regs[regs::REG_X0..regs::REG_X0 + 31]);
                                x
                            };
                            let jit_sp = flat_regs[regs::REG_SP];
                            let jit_pc = flat_regs[regs::REG_PC];
                            let jit_nzcv = flat_regs[regs::REG_NZCV];

                            // Restore pre-block state and re-run via interpreter
                            let mut check_regs = snapshot;
                            self.commit_aarch64_jit_state(&mut check_regs, 0);
                            let blk_insns = retired.saturating_sub(retired_before);
                            let _stop = self.run(blk_insns);
                            let interp_state = self.rebuild_aarch64_jit_flat_state();
                            if let Some(ref interp) = interp_state {
                                let mut mismatches = Vec::new();
                                for i in 0..31 {
                                    if jit_regs[i] != interp[regs::REG_X0 + i] {
                                        mismatches.push(helm_probe::JitVerifyMismatch {
                                            name: REG_NAMES[i],
                                            jit_val: jit_regs[i],
                                            interp_val: interp[regs::REG_X0 + i],
                                        });
                                    }
                                }
                                if jit_sp != interp[regs::REG_SP] {
                                    mismatches.push(helm_probe::JitVerifyMismatch {
                                        name: "sp",
                                        jit_val: jit_sp,
                                        interp_val: interp[regs::REG_SP],
                                    });
                                }
                                if jit_pc != interp[regs::REG_PC] {
                                    mismatches.push(helm_probe::JitVerifyMismatch {
                                        name: "pc",
                                        jit_val: jit_pc,
                                        interp_val: interp[regs::REG_PC],
                                    });
                                }
                                if (jit_nzcv & 0xF000_0000) != (interp[regs::REG_NZCV] & 0xF000_0000) {
                                    mismatches.push(helm_probe::JitVerifyMismatch {
                                        name: "nzcv",
                                        jit_val: jit_nzcv,
                                        interp_val: interp[regs::REG_NZCV],
                                    });
                                }
                                if !mismatches.is_empty() {
                                    let backend_id = match cached_tier {
                                        helm_jit::cache::JitTier::Stencil => helm_probe::JitBackendId::Stencil,
                                        helm_jit::cache::JitTier::Dynasm => helm_probe::JitBackendId::Dynasm,
                                    };
                                    log::warn!(
                                        "jit-verify: {} mismatches at pc={:#x} ({} insns, {:?})",
                                        mismatches.len(), pc, blk_insns, backend_id
                                    );
                                    probe!(self.jit_probes.verify, helm_probe::JitVerifyEvent {
                                        pc,
                                        insn_count: blk_insns as u32,
                                        backend: backend_id,
                                        mismatches,
                                    });
                                }
                            }
                            // Restore JIT state
                            self.commit_aarch64_jit_state(&mut flat_regs, retired_before);
                            self.insns_retired -= blk_insns; // undo interpreter's count
                        }
                    }

                    // Notify debug controller and emit block-execute probe.
                    {
                        let next_pc = flat_regs[regs::REG_PC];
                        let blk_retired = retired.saturating_sub(retired_before) as u32;
                        if self.jit_probes.any_active() && self.jit_debug.is_window_active() {
                            helm_probe::update_probe_insn_count(
                                self.insns_retired.saturating_add(retired),
                            );
                            let context = Self::jit_block_context_from_flat(&flat_regs);
                            probe!(
                                self.jit_probes.block_execute,
                                helm_probe::JitBlockExecuteEvent {
                                    pc,
                                    next_pc,
                                    insns_retired: blk_retired,
                                    exit_code,
                                    context: Some(context),
                                }
                            );
                        }
                        if self.plugins.has_jit_block_callbacks() {
                            let ctx = Self::jit_block_context_from_flat(&flat_regs);
                            self.plugins.fire_jit_block(
                                &helm_plugin::JitBlockInfo {
                                    pc,
                                    next_pc,
                                    insns_retired: blk_retired,
                                    exit_code,
                                    context: helm_plugin::ArchContext::Aarch64 {
                                        x: ctx.x,
                                        sp: ctx.sp,
                                        pc: ctx.pc,
                                        nzcv: ctx.nzcv,
                                        current_el: ctx.current_el,
                                        tpidrro_el0: 0,
                                    },
                                },
                            );
                        }
                    }
                    match exit_code {
                        EXIT_END_OF_BLOCK => {
                            // FS bookkeeping between blocks.
                            if self.active_mode() == ExecMode::System {
                                let blk = retired.saturating_sub(retired_before);
                                self.commit_aarch64_jit_state(&mut flat_regs, retired);
                                retired = 0;
                                self.jit_fs_bookkeeping(blk);
                                self.drain_pending_aarch64_exception_event();
                                flat_regs = match self.rebuild_aarch64_jit_flat_state() {
                                    Some(r) => r,
                                    None => return StopReason::Unsupported,
                                };
                            }
                            continue;
                        }
                        EXIT_EL_CHANGE => {
                            // EL transition: commit GPRs without overwriting
                            // the PC/EL/DAIF that the helper set.
                            self.commit_aarch64_jit_gprs_after_el_change(&mut flat_regs, retired);
                            retired = 0;
                            // FS bookkeeping (TLB flush, timer, IRQ).
                            if self.active_mode() == ExecMode::System {
                                let block_ret =
                                    flat_regs.get(regs::REG_JIT_RETIRED).copied().unwrap_or(0);
                                self.jit_fs_bookkeeping(block_ret);
                            }
                            self.drain_pending_aarch64_exception_event();
                            // Rebuild flat state from the now-updated arch state.
                            flat_regs = match self.rebuild_aarch64_jit_flat_state() {
                                Some(r) => r,
                                None => return StopReason::Unsupported,
                            };
                            continue;
                        }
                        EXIT_SYSCALL => {
                            self.commit_aarch64_jit_state(&mut flat_regs, retired);
                            let nr = flat_regs[regs::REG_X0 + 8]; // X8 = syscall nr
                            return StopReason::Exception(HartException::EnvironmentCall {
                                pc: 0,
                                nr,
                            });
                        }
                        EXIT_PSCI => {
                            self.commit_aarch64_jit_state(&mut flat_regs, retired);
                            return StopReason::Exception(HartException::PsciCall {
                                conduit: "hvc",
                                function: flat_regs[regs::REG_X0] as u32,
                                arg1: flat_regs[regs::REG_X0 + 1],
                                arg2: flat_regs[regs::REG_X0 + 2],
                                arg3: flat_regs[regs::REG_X0 + 3],
                            });
                        }
                        EXIT_WFI => {
                            self.commit_aarch64_jit_state(&mut flat_regs, retired);
                            if self.active_mode() == ExecMode::System {
                                // Mark vCPU as WFI-idle for the scheduler.
                                if let Some(board) = self
                                    .session
                                    .aarch64_mut()
                                    .and_then(Aarch64Core::machine_mut)
                                {
                                    let vi = self
                                        .active_fs_vcpu
                                        .min(board.vcpus.len().saturating_sub(1));
                                    board.vcpus[vi].fs.wfi_idle = true;
                                }
                            }
                            return StopReason::Quantum;
                        }
                        EXIT_EXIT => {
                            self.commit_aarch64_jit_state(&mut flat_regs, retired);
                            return StopReason::Exit { code: 0 };
                        }
                        _ => break,
                    }
                }
                BlockCacheProbe::Miss => {
                    helm_probe::update_probe_insn_count(self.insns_retired.saturating_add(retired));
                    self.emit_jit_cache_event(pc, helm_probe::JitCacheOp::Miss, 0);
                }
            }

            // Cache miss - decode instructions and try to compile a block.
            log::trace!("jit: cache miss pc={pc:#x}, decoding...");
            let insns = self.decode_aarch64_jit_block(pc);

            // Try to compile the block.
            log::trace!("jit: decoded {} insns starting at pc={pc:#x}", insns.len());
            let cache_ref = unsafe { &mut *cache };
            let backend = self
                .jit_backend
                .as_mut()
                .map(|b| b.as_mut() as *mut dyn helm_jit::backend::JitBackend);
            let fallback_backend = if self.active_mode() == ExecMode::System {
                None
            } else {
                self.jit_hot_backend
                    .as_mut()
                    .map(|b| b.as_mut() as *mut dyn helm_jit::backend::JitBackend)
            };
            self.jit_decode_buf = insns; // restore reusable buffer
            let decoded_insns_ptr = self.jit_decode_buf.as_ptr();
            let decoded_insns_len = self.jit_decode_buf.len();
            let unsupported_opcode = self
                .jit_decode_buf
                .first()
                .map(|insn| format!("{:?}", insn.opcode));
            match resolve_aarch64_compile_miss(
                self,
                cache_ref,
                backend.map(|ptr| unsafe { &mut *ptr }),
                fallback_backend.map(|ptr| unsafe { &mut *ptr }),
                pc,
                unsafe { std::slice::from_raw_parts(decoded_insns_ptr, decoded_insns_len) },
                &mut flat_regs,
                &mut retired,
                budget_remaining,
                unsupported_opcode,
                runtime_config,
            ) {
                CompileMissResolution::Cached { insn_count } => {
                    log::trace!("jit: compiled block pc={pc:#x} insns={insn_count}");
                    helm_probe::update_probe_insn_count(self.insns_retired.saturating_add(retired));
                    probe!(
                        self.jit_probes.block_compile,
                        helm_probe::JitBlockCompileEvent {
                            pc,
                            insn_count,
                            backend: helm_probe::JitBackendId::Other,
                        }
                    );
                    // Loop back to execute the newly cached block.
                }
                CompileMissResolution::Resume {
                    budget_remaining: remaining,
                } => {
                    budget_remaining = remaining;
                    continue;
                }
                CompileMissResolution::Stop { stop } => return stop,
            }
        }

        // Sync flat regs -> arch state.
        self.commit_aarch64_jit_state(&mut flat_regs, retired);

        StopReason::Quantum
    }

    /// Run up to `max_insns` RISC-V64 instructions using the stencil JIT backend.
    ///
    /// Supports both SE and FS modes. In FS mode, memory accesses go through
    /// the Sv39/Sv48 page table walker via `JitFsContextRv64`.
    #[cfg(feature = "jit-stencil")]
    #[allow(unsafe_code)]
    fn run_jit_rv64(&mut self, max_insns: u64) -> StopReason {
        let cache = match self.jit_cache.as_mut() {
            Some(c) => c as *mut helm_jit::cache::JitCache,
            None => return self.run(max_insns),
        };

        let rv = match self.session.riscv() {
            Some(r) => r,
            None => return StopReason::Unsupported,
        };

        // Sync arch state -> flat register array.
        let mut flat_regs = regs::arch_to_flat_rv64(&rv.iregs, rv.pc);

        let is_fs = self.active_mode() == ExecMode::System;

        // Set up memory pointer and helper function pointers based on mode.
        let mut _rv64_fs_ctx: Option<helm_jit::helpers::JitFsContextRv64> = None;
        let mem_ptr: *mut u8;

        let (jit_mr, jit_mw) = if is_fs {
            // FS mode: create JitFsContextRv64 with SATP-derived MMU config.
            // Note: RV64 FS mode requires the board to provide sys_mem + TLB.
            // For now, fall back to interpreter if FS infrastructure isn't ready.
            return self.run(max_insns);
        } else {
            // SE mode: direct FlatMem access.
            mem_ptr = &mut self.memory as *mut FlatMem as *mut u8;
            (
                helm_jit::helpers::jit_mem_read as *const () as u64,
                helm_jit::helpers::jit_mem_write as *const () as u64,
            )
        };

        // Store helper function pointers in the reserved slots.
        const RV64_MEM_READ_SLOT: usize = 38;
        const RV64_MEM_WRITE_SLOT: usize = 39;
        flat_regs[RV64_MEM_READ_SLOT] = jit_mr;
        flat_regs[RV64_MEM_WRITE_SLOT] = jit_mw;

        let mut retired: u64 = 0;

        while retired < max_insns {
            let pc = flat_regs[regs::REG_PC_RV64];

            let cache_ref = unsafe { &mut *cache };
            if let BlockCacheProbe::Hit(hit) = probe_block_cache(cache_ref, &mut self.jit_stats, pc)
            {
                helm_probe::update_probe_insn_count(self.insns_retired.saturating_add(retired));
                self.emit_jit_cache_event(pc, helm_probe::JitCacheOp::Hit, hit.exec_count);
                let mut budget_remaining = max_insns.saturating_sub(retired);
                let exit_code = unsafe {
                    execute_cache_hit::<dyn helm_jit::backend::JitBackend>(
                        cache_ref,
                        &mut self.jit_stats,
                        hit,
                        None,
                        pc,
                        None,
                        &mut flat_regs,
                        mem_ptr,
                        &mut retired,
                        &mut budget_remaining,
                    )
                };
                match exit_code {
                    EXIT_END_OF_BLOCK => continue,
                    _ => break,
                }
            } else {
                helm_probe::update_probe_insn_count(self.insns_retired.saturating_add(retired));
                self.emit_jit_cache_event(pc, helm_probe::JitCacheOp::Miss, 0);
            }

            // Cache miss — decode a block of RISC-V instructions.
            log::trace!("jit-rv64: cache miss pc={pc:#x}, decoding...");
            let mut insns = Vec::new();
            let mut decode_pc = pc;
            for _ in 0..64 {
                let raw32 = match self.memory.fetch32(decode_pc) {
                    Ok(r) => r,
                    Err(_) => break,
                };

                // Handle compressed (RVC) instructions.
                let (insn, insn_size) = if (raw32 & 0b11) != 0b11 {
                    let c = raw32 as u16;
                    match riscv_expand_c(c, decode_pc) {
                        Ok(i) => (i, 2u64),
                        Err(_) => break,
                    }
                } else {
                    match riscv_decode(raw32, decode_pc) {
                        Ok(i) => (i, 4u64),
                        Err(_) => break,
                    }
                };

                let is_branch = insn.is_control_flow();
                insns.push(insn);
                decode_pc += insn_size;
                if is_branch {
                    break;
                }
            }

            if insns.is_empty() {
                // Can't decode — fall back to interpreter.
                let remaining = max_insns.saturating_sub(retired);
                return self.handoff_rv64_jit_to_interpreter(
                    &mut flat_regs,
                    &mut retired,
                    remaining,
                    Some("decode-empty"),
                );
            }

            // Try to compile the block with the RV64 stencil backend.
            log::trace!(
                "jit-rv64: decoded {} insns starting at pc={pc:#x}",
                insns.len()
            );
            let cache_ref = unsafe { &mut *cache };
            let backend = match self.jit_rv64_backend.as_mut() {
                Some(b) => b,
                None => {
                    let remaining = max_insns.saturating_sub(retired);
                    return self.handoff_rv64_jit_to_interpreter(
                        &mut flat_regs,
                        &mut retired,
                        remaining,
                        Some("backend-unavailable"),
                    );
                }
            };
            match backend.compile_block_rv64(pc, &insns) {
                Some(block) => {
                    log::trace!(
                        "jit-rv64: compiled block pc={pc:#x} insns={}",
                        block.insn_count
                    );
                    self.jit_stats.blocks_compiled =
                        self.jit_stats.blocks_compiled.saturating_add(1);
                    self.jit_stats.compiled_guest_insns = self
                        .jit_stats
                        .compiled_guest_insns
                        .saturating_add(u64::from(block.insn_count));
                    cache_ref.insert(block);
                    // Loop back to execute the newly cached block.
                }
                None => {
                    // Unsupported instruction — interpreter fallback.
                    let remaining = max_insns.saturating_sub(retired);
                    return self.handoff_rv64_jit_to_interpreter(
                        &mut flat_regs,
                        &mut retired,
                        remaining,
                        Some("unsupported-start"),
                    );
                }
            }
        }

        // Sync flat regs -> arch state.
        self.commit_rv64_jit_state(&mut flat_regs, retired);

        StopReason::Quantum
    }
}

impl HelmSim {
    /// Enable or disable the JIT backend.
    pub fn set_jit(&mut self, enabled: bool) {
        match self {
            Self::VirtualTiming(e) => e.set_jit(enabled),
            Self::IntervalTiming(e) => e.set_jit(enabled),
            Self::AccurateTiming(e) => e.set_jit(enabled),
        }
    }

    /// Override shared JIT runtime policy knobs for this simulator instance.
    pub fn set_jit_runtime_config(&mut self, config: helm_jit::runtime::JitRuntimeConfig) {
        match self {
            Self::VirtualTiming(e) => e.set_jit_runtime_config(config),
            Self::IntervalTiming(e) => e.set_jit_runtime_config(config),
            Self::AccurateTiming(e) => e.set_jit_runtime_config(config),
        }
    }

    /// Run with JIT if enabled, otherwise fall back to interpreter.
    pub fn run_jit(&mut self, max_insns: u64) -> StopReason {
        match self {
            Self::VirtualTiming(e) => e.run_jit(max_insns),
            Self::IntervalTiming(e) => e.run_jit(max_insns),
            Self::AccurateTiming(e) => e.run_jit(max_insns),
        }
    }
}

// ── JIT debug forwarding on HelmSim ─────────────────────────────────────────

impl HelmSim {
    /// Add a JIT breakpoint at `pc`. Returns `true` if newly inserted.
    pub fn add_jit_breakpoint(&mut self, pc: u64) -> bool {
        match self {
            Self::VirtualTiming(e) => e.jit_debug.add_breakpoint(pc),
            Self::IntervalTiming(e) => e.jit_debug.add_breakpoint(pc),
            Self::AccurateTiming(e) => e.jit_debug.add_breakpoint(pc),
        }
    }

    /// Remove a JIT breakpoint at `pc`. Returns `true` if it existed.
    pub fn remove_jit_breakpoint(&mut self, pc: u64) -> bool {
        match self {
            Self::VirtualTiming(e) => e.jit_debug.remove_breakpoint(pc),
            Self::IntervalTiming(e) => e.jit_debug.remove_breakpoint(pc),
            Self::AccurateTiming(e) => e.jit_debug.remove_breakpoint(pc),
        }
    }

    /// Remove all JIT breakpoints.
    pub fn clear_jit_breakpoints(&mut self) {
        match self {
            Self::VirtualTiming(e) => e.jit_debug.clear_breakpoints(),
            Self::IntervalTiming(e) => e.jit_debug.clear_breakpoints(),
            Self::AccurateTiming(e) => e.jit_debug.clear_breakpoints(),
        }
    }

    /// Configure the JIT trace window (PC range and/or instruction count bounds).
    pub fn set_jit_trace_window(&mut self, window: helm_jit::debug::JitTraceWindow) {
        match self {
            Self::VirtualTiming(e) => e.jit_debug.set_trace_window(window),
            Self::IntervalTiming(e) => e.jit_debug.set_trace_window(window),
            Self::AccurateTiming(e) => e.jit_debug.set_trace_window(window),
        }
    }

    /// Remove the JIT trace window.
    pub fn clear_jit_trace_window(&mut self) {
        match self {
            Self::VirtualTiming(e) => e.jit_debug.clear_trace_window(),
            Self::IntervalTiming(e) => e.jit_debug.clear_trace_window(),
            Self::AccurateTiming(e) => e.jit_debug.clear_trace_window(),
        }
    }

    /// Set a PC range where JIT falls back to interpreter (for debugging).
    pub fn set_jit_interp_range(&mut self, start: u64, end: u64) {
        match self {
            Self::VirtualTiming(e) => e.jit_interp_range = Some((start, end)),
            Self::IntervalTiming(e) => e.jit_interp_range = Some((start, end)),
            Self::AccurateTiming(e) => e.jit_interp_range = Some((start, end)),
        }
    }

    /// Set a PC range where ONLY JIT is used; everything else uses interpreter.
    pub fn set_jit_only_range(&mut self, start: u64, end: u64) {
        match self {
            Self::VirtualTiming(e) => {
                e.jit_interp_range = Some((start, end));
                e.jit_interp_invert = true;
            }
            Self::IntervalTiming(e) => {
                e.jit_interp_range = Some((start, end));
                e.jit_interp_invert = true;
            }
            Self::AccurateTiming(e) => {
                e.jit_interp_range = Some((start, end));
                e.jit_interp_invert = true;
            }
        }
    }

    /// Force the JIT to use interpreter fallback for every block
    /// (enables per-instruction plugin/probe delivery).
    pub fn set_jit_force_interpreter(&mut self, force: bool) {
        match self {
            Self::VirtualTiming(e) => e.jit_debug.force_interpreter = force,
            Self::IntervalTiming(e) => e.jit_debug.force_interpreter = force,
            Self::AccurateTiming(e) => e.jit_debug.force_interpreter = force,
        }
    }

    pub fn set_jit_verify(&mut self, enable: bool) {
        match self {
            Self::VirtualTiming(e) => e.jit_debug.verify = enable,
            Self::IntervalTiming(e) => e.jit_debug.verify = enable,
            Self::AccurateTiming(e) => e.jit_debug.verify = enable,
        }
    }

    /// Total instructions retired through the JIT debug controller.
    pub fn jit_debug_insns_retired(&self) -> u64 {
        match self {
            Self::VirtualTiming(e) => e.jit_debug.insns_retired(),
            Self::IntervalTiming(e) => e.jit_debug.insns_retired(),
            Self::AccurateTiming(e) => e.jit_debug.insns_retired(),
        }
    }
}

use helm_arch::aarch64::mmu::{self, MmuAccess, MmuConfig};
use helm_arch::aarch64_decode;
#[cfg(feature = "jit-stencil")]
use helm_arch::{riscv_decode, riscv_expand_c};
use helm_core::{AccessType, MemInterface};
use helm_jit::runtime::{
    prepare_aarch64_jit_dispatch_context, dispatch_trace, ensure_aarch64_jit_runtime_state,
    execute_cache_hit, plan_aarch64_trace_recording, probe_block_cache,
    record_aarch64_trace_candidate, resolve_aarch64_compile_miss, Aarch64JitBackendMode,
    Aarch64JitBackendPolicy, Aarch64JitMemoryMode, BlockCacheProbe, CompileMissResolution,
    JitRuntimeHost, TraceDispatch, TraceRecordPlan,
};
use helm_jit::{block::EXIT_END_OF_BLOCK, regs};
use helm_timing::TimingModel;

use crate::{session::Aarch64Core, ExecMode, FlatMem, HelmEngine, HelmSim, Isa, StopReason};

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
}

impl<T: TimingModel> HelmEngine<T> {
    pub(crate) fn effective_jit_runtime_config(&self) -> helm_jit::runtime::JitRuntimeConfig {
        let mut config = self.jit_runtime_config;
        if self.active_mode() == ExecMode::System {
            config.trace_dispatch_enabled = false;
        }
        config
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

    fn build_aarch64_jit_dispatch_context(
        &mut self,
        flat_regs: &mut [u64; regs::REG_COUNT],
    ) -> Option<helm_jit::runtime::Aarch64JitDispatchContext> {
        if self.active_mode() == ExecMode::System {
            let a64_ref = self.aarch64_state_for_current_context()?;
            let mmu_cfg = helm_arch::aarch64::mmu::MmuConfig::from_arch(a64_ref);
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
    ) -> StopReason {
        self.commit_rv64_jit_state(flat_regs, *retired);
        *retired = 0;
        self.run(budget_remaining)
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
        let board = self.session.aarch64_mut().and_then(Aarch64Core::machine_mut)?;
        let vcpu_idx = active_fs_vcpu.min(board.vcpus.len().saturating_sub(1));
        let mmu_cfg = MmuConfig::from_arch(&board.vcpus[vcpu_idx].arch);
        let pa = if mmu_cfg.mmu_enabled() {
            mmu::translate_cfg(
                &mmu_cfg,
                pc,
                MmuAccess::Execute,
                board.sys_mem.as_mut(),
                Some(&mut board.vcpus[vcpu_idx].fs.tlb),
            )
            .ok()?
        } else {
            pc
        };

        board
            .sys_mem
            .read(pa, 4, AccessType::Load)
            .ok()
            .map(|raw| raw as u32)
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
                    let terminates_block = Self::is_aarch64_jit_block_terminator(&insn);
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
        let _ = record_aarch64_trace_candidate(
            recorder,
            self.jit_trace_cache.as_mut(),
            &mut self.jit_stats,
            &insns,
        );

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
                    #[cfg(not(any(feature = "jit-tiered", feature = "jit-stencil", feature = "jit-dynasm")))]
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
            let (mr, mw) = (flat_regs[regs::REG_JIT_MEM_READ], flat_regs[regs::REG_JIT_MEM_WRITE]);
            if let Some(b) = self.jit_backend.as_mut() {
                b.set_mem_helpers(mr, mw);
            }
            if let Some(b) = self.jit_hot_backend.as_mut() {
                b.set_mem_helpers(mr, mw);
            }

            let pc = flat_regs[regs::REG_PC];

            #[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
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
                TraceDispatch::Executed { exit_code } => match exit_code {
                    EXIT_END_OF_BLOCK => continue,
                    code if code >= helm_jit::trace::compiler::EXIT_GUARD_BASE => continue,
                    _ => break,
                },
            }

            let cache_ref = unsafe { &mut *cache };
            match probe_block_cache(cache_ref, &mut self.jit_stats, pc) {
                BlockCacheProbe::Hit(hit) => {
                    let exit_code = if hit.exec_count == helm_jit::cache::PROMOTE_THRESHOLD
                        && hit.tier == helm_jit::cache::JitTier::Stencil
                    {
                        // Decode the block again for dynasm recompilation.
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

                    match exit_code {
                        EXIT_END_OF_BLOCK => continue,
                        _ => break,
                    }
                }
                BlockCacheProbe::Miss => {}
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
            let fallback_backend = self
                .jit_hot_backend
                .as_mut()
                .map(|b| b.as_mut() as *mut dyn helm_jit::backend::JitBackend);
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

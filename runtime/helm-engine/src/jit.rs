use helm_arch::aarch64_decode;
#[cfg(feature = "jit-stencil")]
use helm_arch::{riscv_decode, riscv_expand_c};
use helm_core::MemInterface;
#[cfg(feature = "jit-stencil")]
use helm_jit::runtime::execute_compiled_block;
use helm_jit::runtime::{
    execute_cache_hit, probe_block_cache, resolve_aarch64_compile_miss, BlockCacheProbe,
    CompileMissResolution, JitRuntimeHost, DEFAULT_RUNTIME_CONFIG,
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
    fn commit_aarch64_jit_state(&mut self, flat_regs: &mut [u64], retired_insns: u64) {
        let flat_regs = <&mut [u64; regs::REG_COUNT]>::try_from(flat_regs)
            .expect("aarch64 flat register image");
        let a64_mut = self
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::state_mut)
            .expect("aarch64 state");
        regs::flat_to_arch(flat_regs, a64_mut);
        self.insns_retired += retired_insns;
    }

    fn arm_aarch64_jit_flat_context(&mut self, flat_regs: &mut [u64; regs::REG_COUNT]) {
        let is_fs = self.active_mode() == ExecMode::System;
        let (jit_mr, jit_mw) = if is_fs {
            (
                helm_jit::helpers::jit_fs_mem_read as *const () as u64,
                helm_jit::helpers::jit_fs_mem_write as *const () as u64,
            )
        } else {
            (
                helm_jit::helpers::jit_mem_read as *const () as u64,
                helm_jit::helpers::jit_mem_write as *const () as u64,
            )
        };

        flat_regs[regs::REG_JIT_MEM_READ] = jit_mr;
        flat_regs[regs::REG_JIT_MEM_WRITE] = jit_mw;
        if !is_fs {
            let tlb = self
                .jit_se_tlb
                .get_or_insert_with(|| Box::new(helm_jit::helpers::JitSeTlb::new()));
            flat_regs[regs::REG_JIT_SE_TLB] = tlb.entries.as_ptr() as u64;
        }
    }

    fn rebuild_aarch64_jit_flat_state(&mut self) -> Option<[u64; regs::REG_COUNT]> {
        let a64 = self.session.aarch64().and_then(Aarch64Core::state)?;
        let mut flat_regs = regs::arch_to_flat(a64);
        self.arm_aarch64_jit_flat_context(&mut flat_regs);
        Some(flat_regs)
    }

    fn decode_aarch64_jit_block(&mut self, pc: u64) -> Vec<helm_arch::Aarch64Insn> {
        // Reuse the decode buffer to avoid per-miss or per-promotion allocation.
        let mut insns = std::mem::take(&mut self.jit_decode_buf);
        insns.clear();
        let mut decode_pc = pc;
        for _ in 0..64 {
            let raw = match self.memory.fetch32(decode_pc) {
                Ok(r) => r,
                Err(_) => break,
            };
            match aarch64_decode(raw, decode_pc) {
                Ok(insn) => {
                    let is_branch = insn.is_branch();
                    insns.push(insn);
                    decode_pc += 4;
                    if is_branch {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        insns
    }

    /// Enable or disable the JIT backend.
    pub fn set_jit(&mut self, enabled: bool) {
        self.jit_enabled = enabled;
        if enabled {
            if self.jit_cache.is_none() {
                self.jit_cache = Some(helm_jit::cache::JitCache::new());
            }
            if self.jit_backend.is_none() {
                if self.isa == Isa::RiscV {
                    // RISC-V64: stencil backend only (no dynasm for RV64 yet).
                    #[cfg(feature = "jit-stencil")]
                    {
                        // RV64 uses a separate backend type; store it in jit_rv64_backend.
                        self.jit_rv64_backend =
                            Some(Box::new(helm_jit::stencil::StencilBackendRv64::new()));
                        log::info!("jit: RISC-V64 stencil backend enabled");
                    }
                } else {
                    // AArch64: tiered or single backend.
                    #[cfg(feature = "jit-tiered")]
                    {
                        self.jit_backend =
                            Some(Box::new(helm_jit::stencil::StencilBackend::new_aarch64()));
                        self.jit_hot_backend =
                            Some(Box::new(helm_jit::dynasm::DynasmBackend::new()));
                        log::info!("jit: tiered mode (stencil baseline + dynasm hot-tier)");
                    }
                    #[cfg(all(feature = "jit-stencil", not(feature = "jit-tiered")))]
                    {
                        self.jit_backend =
                            Some(Box::new(helm_jit::stencil::StencilBackend::new_aarch64()));
                    }
                    #[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
                    {
                        self.jit_backend = Some(Box::new(helm_jit::dynasm::DynasmBackend::new()));
                    }
                }
            }
        }
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

        let is_fs = self.active_mode() == ExecMode::System;

        // Sync arch state -> flat register array.
        // `run_jit()` may temporarily fall back to the interpreter and then
        // resume JIT in the same call, so the flat array needs to be a full
        // state image that can be rebuilt from architectural state.
        // Set up memory pointer and helper function pointers based on mode.
        //
        // SE mode: mem_ptr = &mut FlatMem, helpers = jit_mem_read/write
        // FS mode: mem_ptr = &mut JitFsContext, helpers = jit_fs_mem_read/write
        //
        // The JitFsContext contains pointers to the address space + TLB +
        // snapshotted MMU config for VA->PA translation.
        #[allow(unused_assignments)]
        let mut fs_ctx: Option<helm_jit::helpers::JitFsContext> = None;
        let mem_ptr: *mut u8;

        if is_fs {
            // FS mode: create JitFsContext with MMU config snapshot.
            let a64_ref = self
                .session
                .aarch64()
                .and_then(Aarch64Core::state)
                .expect("aarch64 state");
            let mmu_cfg = helm_arch::aarch64::mmu::MmuConfig::from_arch(a64_ref);
            let board = self
                .session
                .aarch64_mut()
                .and_then(Aarch64Core::machine_mut)
                .expect("board");
            fs_ctx = Some(helm_jit::helpers::JitFsContext {
                sys_mem: &mut board.sys_mem as *mut _,
                tlb: &mut board.vcpus[board.next_vcpu].fs.tlb as *mut _,
                mmu_cfg,
            });
            mem_ptr = fs_ctx.as_mut().expect("fs jit ctx") as *mut helm_jit::helpers::JitFsContext
                as *mut u8;
        } else {
            // SE mode: direct FlatMem access.
            mem_ptr = &mut self.memory as *mut FlatMem as *mut u8;
        }

        let mut flat_regs = match self.rebuild_aarch64_jit_flat_state() {
            Some(regs) => regs,
            None => return StopReason::Unsupported,
        };

        let mut retired: u64 = 0;
        let mut budget_remaining = max_insns;

        while budget_remaining > 0 {
            let pc = flat_regs[regs::REG_PC];

            let cache_ref = unsafe { &mut *cache };
            match probe_block_cache(cache_ref, &mut self.jit_stats, pc) {
                BlockCacheProbe::Hit(hit) => {
                    let exit_code = if hit.exec_count == helm_jit::cache::PROMOTE_THRESHOLD
                        && hit.tier == helm_jit::cache::JitTier::Stencil
                    {
                        // Decode the block again for dynasm recompilation.
                        let insns = self.decode_aarch64_jit_block(pc);
                        let hot_backend = self
                            .jit_hot_backend
                            .as_mut()
                            .map(|b| b.as_mut() as *mut dyn helm_jit::backend::JitBackend);
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
            self.jit_decode_buf = insns; // restore reusable buffer
            let decoded_insns_ptr = self.jit_decode_buf.as_ptr();
            let decoded_insns_len = self.jit_decode_buf.len();
            let unsupported_opcode = self
                .jit_decode_buf
                .first()
                .map(|insn| crate::classify_aarch64_opcode(insn.opcode).1);
            match resolve_aarch64_compile_miss(
                self,
                cache_ref,
                backend.map(|ptr| unsafe { &mut *ptr }),
                pc,
                unsafe { std::slice::from_raw_parts(decoded_insns_ptr, decoded_insns_len) },
                &mut flat_regs,
                &mut retired,
                budget_remaining,
                unsupported_opcode,
                DEFAULT_RUNTIME_CONFIG,
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

            // Try cache lookup.
            let cache_ref = unsafe { &mut *cache };
            if let Some(hit) = cache_ref.lookup_hot(pc) {
                let mut budget_remaining = max_insns.saturating_sub(retired);
                let exit_code = unsafe {
                    execute_compiled_block(
                        &mut self.jit_stats,
                        &hit.block,
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
                let rv_mut = self.session.riscv_mut().expect("riscv state");
                regs::flat_to_arch_rv64(&mut flat_regs, &mut rv_mut.iregs, &mut rv_mut.pc);
                self.insns_retired += retired;
                return self.run(max_insns.saturating_sub(retired));
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
                    let rv_mut = self.session.riscv_mut().expect("riscv state");
                    regs::flat_to_arch_rv64(&mut flat_regs, &mut rv_mut.iregs, &mut rv_mut.pc);
                    self.insns_retired += retired;
                    return self.run(max_insns.saturating_sub(retired));
                }
            };
            match backend.compile_block_rv64(pc, &insns) {
                Some(block) => {
                    log::trace!(
                        "jit-rv64: compiled block pc={pc:#x} insns={}",
                        block.insn_count
                    );
                    cache_ref.insert(block);
                    // Loop back to execute the newly cached block.
                }
                None => {
                    // Unsupported instruction — interpreter fallback.
                    let rv_mut = self.session.riscv_mut().expect("riscv state");
                    regs::flat_to_arch_rv64(&mut flat_regs, &mut rv_mut.iregs, &mut rv_mut.pc);
                    self.insns_retired += retired;
                    return self.run(max_insns.saturating_sub(retired));
                }
            }
        }

        // Sync flat regs -> arch state.
        let rv_mut = self.session.riscv_mut().expect("riscv state");
        regs::flat_to_arch_rv64(&mut flat_regs, &mut rv_mut.iregs, &mut rv_mut.pc);
        self.insns_retired += retired;

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

    /// Run with JIT if enabled, otherwise fall back to interpreter.
    pub fn run_jit(&mut self, max_insns: u64) -> StopReason {
        match self {
            Self::VirtualTiming(e) => e.run_jit(max_insns),
            Self::IntervalTiming(e) => e.run_jit(max_insns),
            Self::AccurateTiming(e) => e.run_jit(max_insns),
        }
    }
}

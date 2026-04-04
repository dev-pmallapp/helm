use helm_arch::aarch64_decode;
#[cfg(feature = "jit-stencil")]
use helm_arch::{riscv_decode, riscv_expand_c};
use helm_core::MemInterface;
use helm_jit::{block::EXIT_END_OF_BLOCK, regs};
use helm_timing::TimingModel;

use crate::{session::Aarch64Core, ExecMode, FlatMem, HelmEngine, HelmSim, Isa, StopReason};

impl<T: TimingModel> HelmEngine<T> {
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
        // arch_to_flat_nonpinned() skips slots that are pinned to host registers
        // (X0–X4, X19, X20, X30, SP, NZCV). The block prologue loads those from
        // the flat array on entry; the epilogue writes them back on exit.
        // Full arch_to_flat() would be redundant and would initialize stale values
        // that get overwritten by the prologue load anyway.
        let a64 = match self.session.aarch64().and_then(Aarch64Core::state) {
            Some(s) => s,
            None => return StopReason::Unsupported,
        };
        let mut flat_regs = [0u64; regs::REG_COUNT];
        regs::arch_to_flat_nonpinned(a64, &mut flat_regs);

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

        let (jit_mr, jit_mw) = if is_fs {
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
            (
                helm_jit::helpers::jit_fs_mem_read as *const () as u64,
                helm_jit::helpers::jit_fs_mem_write as *const () as u64,
            )
        } else {
            // SE mode: direct FlatMem access.
            mem_ptr = &mut self.memory as *mut FlatMem as *mut u8;
            (
                helm_jit::helpers::jit_mem_read as *const () as u64,
                helm_jit::helpers::jit_mem_write as *const () as u64,
            )
        };

        flat_regs[regs::REG_JIT_MEM_READ] = jit_mr;
        flat_regs[regs::REG_JIT_MEM_WRITE] = jit_mw;

        // Set up SE-mode inline TLB pointer in slot 44.
        // Only valid in SE mode; FS mode uses the full MMU translation path.
        if !is_fs {
            let tlb = self
                .jit_se_tlb
                .get_or_insert_with(|| Box::new(helm_jit::helpers::JitSeTlb::new()));
            flat_regs[regs::REG_JIT_SE_TLB] = tlb.entries.as_ptr() as u64;
        }

        let mut retired: u64 = 0;

        while retired < max_insns {
            let pc = flat_regs[regs::REG_PC];

            // Try cache lookup with heat tracking.
            let cache_ref = unsafe { &mut *cache };
            if let Some(hit) = cache_ref.lookup_hot(pc) {
                // Check for tiered promotion: if this is a stencil block that
                // has been executed enough times, recompile with dynasm.
                if hit.exec_count == helm_jit::cache::PROMOTE_THRESHOLD
                    && hit.tier == helm_jit::cache::JitTier::Stencil
                {
                    if let Some(hot) = self.jit_hot_backend.as_mut() {
                        // Decode the block again for dynasm recompilation.
                        // Reuse pre-allocated buffer to avoid per-miss heap allocation.
                        let mut insns = std::mem::take(&mut self.jit_decode_buf);
                        insns.clear();
                        let mut dpc = pc;
                        for _ in 0..64 {
                            let raw = match self.memory.fetch32(dpc) {
                                Ok(r) => r,
                                Err(_) => break,
                            };
                            match aarch64_decode(raw, dpc) {
                                Ok(insn) => {
                                    let is_branch = insn.is_branch();
                                    insns.push(insn);
                                    dpc += 4;
                                    if is_branch {
                                        break;
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        let has_insns = !insns.is_empty();
                        if has_insns {
                            if let Some(promoted) = hot.compile_block(pc, &insns) {
                                log::trace!(
                                    "jit: promoting pc={pc:#x} to {} ({} insns)",
                                    hot.name(),
                                    promoted.insn_count
                                );
                                cache_ref.promote(pc, promoted, helm_jit::cache::JitTier::Dynasm);
                                // Re-lookup to get the promoted block.
                                if let Some(new_hit) = cache_ref.lookup_hot(pc) {
                                    let exit_code = unsafe {
                                        (new_hit.block.entry)(flat_regs.as_mut_ptr(), mem_ptr)
                                    };
                                    retired += u64::from(new_hit.block.insn_count);
                                    match exit_code {
                                        EXIT_END_OF_BLOCK => continue,
                                        _ => break,
                                    }
                                }
                            }
                        }
                        self.jit_decode_buf = insns;
                    }
                }

                // Execute the cached block (stencil or dynasm).
                let exit_code = unsafe { (hit.block.entry)(flat_regs.as_mut_ptr(), mem_ptr) };
                retired += u64::from(hit.block.insn_count);

                match exit_code {
                    EXIT_END_OF_BLOCK => continue,
                    _ => break,
                }
            }

            // Cache miss - decode instructions and try to compile a block.
            log::trace!("jit: cache miss pc={pc:#x}, decoding...");
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

            if insns.is_empty() {
                // Can't decode anything - fall back to interpreter for one step.
                self.jit_decode_buf = insns; // restore reusable buffer
                let a64_mut = self
                    .session
                    .aarch64_mut()
                    .and_then(Aarch64Core::state_mut)
                    .expect("aarch64 state");
                regs::flat_to_arch(&mut flat_regs, a64_mut);
                self.insns_retired += retired;
                return self.run(max_insns.saturating_sub(retired));
            }

            // Try to compile the block.
            log::trace!("jit: decoded {} insns starting at pc={pc:#x}", insns.len());
            let cache_ref = unsafe { &mut *cache };
            let backend = match self.jit_backend.as_mut() {
                Some(b) => b,
                None => {
                    // No backend available - fall back to interpreter.
                    self.jit_decode_buf = insns; // restore reusable buffer
                    let a64_mut = self
                        .session
                        .aarch64_mut()
                        .and_then(Aarch64Core::state_mut)
                        .expect("aarch64 state");
                    regs::flat_to_arch(&mut flat_regs, a64_mut);
                    self.insns_retired += retired;
                    return self.run(max_insns.saturating_sub(retired));
                }
            };
            let compiled = backend.compile_block(pc, &insns);
            self.jit_decode_buf = insns; // restore reusable buffer
            match compiled {
                Some(block) => {
                    log::trace!("jit: compiled block pc={pc:#x} insns={}", block.insn_count);
                    cache_ref.insert(block);
                    // Phase 2-B: link any cached blocks that were waiting to chain
                    // to this newly compiled block's guest PC.
                    cache_ref.link_waiters(pc);
                    // Phase 2-C: record compiled-instruction heat for adaptive binding.
                    self.jit_heat_map.add_insns(1);
                    if self.jit_heat_map.should_rebind() {
                        let new_binding = self.jit_heat_map.compute_optimal_binding();
                        if helm_jit::regs::RegHeatMap::binding_changed(&new_binding) {
                            log::debug!(
                                "jit: adaptive binding changed — flushing cache (generation {})",
                                self.jit_heat_map.generation
                            );
                            cache_ref.flush();
                        }
                        self.jit_heat_map.reset();
                    }
                    // Loop back to execute the newly cached block.
                }
                None => {
                    // First instruction unsupported - interpreter fallback.
                    // Commit JIT-retired insns, run interpreter for a batch,
                    // then return. The Python loop re-enters run_jit() where
                    // the JIT cache may hit for subsequent blocks.
                    let a64_mut = self
                        .session
                        .aarch64_mut()
                        .and_then(Aarch64Core::state_mut)
                        .expect("aarch64 state");
                    regs::flat_to_arch(&mut flat_regs, a64_mut);
                    self.insns_retired += retired;
                    // FS: small batch to re-enter JIT sooner for hot blocks.
                    // SE: full remaining quantum (Python loop re-enters).
                    let batch = if is_fs {
                        256u64.min(max_insns.saturating_sub(retired))
                    } else {
                        max_insns.saturating_sub(retired)
                    };
                    return self.run(batch);
                }
            }
        }

        // Sync flat regs -> arch state.
        let a64_mut = self
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::state_mut)
            .expect("aarch64 state");
        regs::flat_to_arch(&mut flat_regs, a64_mut);
        self.insns_retired += retired;

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
                let exit_code = unsafe { (hit.block.entry)(flat_regs.as_mut_ptr(), mem_ptr) };
                retired += u64::from(hit.block.insn_count);
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

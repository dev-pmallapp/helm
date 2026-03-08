//! AArch64 Syscall-Emulation mode runner.
//!
//! The execution backend (interpretive vs TCG) is selected via
//! [`ExecBackend`] — orthogonal to the simulation mode.

use crate::loader;
use crate::loader::TlsInfo;
use crate::se::backend::ExecBackend;
use crate::se::thread::{CloneRequest, Scheduler, ThreadState};
use helm_core::HelmError;
use helm_device::DeviceBus;
use helm_isa::arm::aarch64::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;
use helm_plugin::runtime::{
    ArchContext, FaultInfo, FaultKind, InsnInfo, SyscallInfo, SyscallRetInfo,
};
use helm_plugin::PluginRegistry;
use helm_syscall::{Aarch64SyscallHandler, SyscallAction};
use helm_tcg::a64_emitter::{A64TcgEmitter, TranslateAction};
use helm_tcg::block::TcgBlock;
use helm_tcg::interp::{InterpExit, TcgInterp, NUM_REGS, REG_NZCV, REG_PC, REG_SP};
use helm_tcg::TcgContext;
use helm_timing::{InsnClass, SamplingController, TimingModel};

/// Result of an SE-mode simulation run.
pub struct SeResult {
    pub exit_code: u64,
    pub instructions_executed: u64,
    pub hit_limit: bool,
}

/// Result of a timing-annotated SE-mode simulation run.
pub struct SeTimedResult {
    pub exit_code: u64,
    pub instructions_executed: u64,
    pub virtual_cycles: u64,
    pub hit_limit: bool,
}

/// Run a static AArch64 binary in SE mode (convenience wrapper).
pub fn run_aarch64_se(
    binary_path: &str,
    argv: &[&str],
    envp: &[&str],
    max_insns: u64,
) -> Result<SeResult, HelmError> {
    run_aarch64_se_with_plugins(binary_path, argv, envp, max_insns, None)
}

/// Run with optional plugin callbacks (interpretive, FE timing).
pub fn run_aarch64_se_with_plugins(
    binary_path: &str,
    argv: &[&str],
    envp: &[&str],
    max_insns: u64,
    plugins: Option<&PluginRegistry>,
) -> Result<SeResult, HelmError> {
    let mut timing = helm_timing::model::FeModel;
    let mut backend = ExecBackend::interpretive();
    let r = run_se_inner(
        binary_path,
        argv,
        envp,
        max_insns,
        &mut timing,
        &mut backend,
        None,
        plugins,
        None,
    )?;
    Ok(SeResult {
        exit_code: r.exit_code,
        instructions_executed: r.instructions_executed,
        hit_limit: r.hit_limit,
    })
}

/// Run with timing model, execution backend, plugins, and devices.
pub fn run_aarch64_se_timed(
    binary_path: &str,
    argv: &[&str],
    envp: &[&str],
    max_insns: u64,
    timing: &mut dyn TimingModel,
    backend: &mut ExecBackend,
    sampling: Option<&mut SamplingController>,
    plugins: Option<&PluginRegistry>,
    devices: Option<&mut DeviceBus>,
) -> Result<SeTimedResult, HelmError> {
    run_se_inner(
        binary_path,
        argv,
        envp,
        max_insns,
        timing,
        backend,
        sampling,
        plugins,
        devices,
    )
}

// ── unified inner runner ────────────────────────────────────────────────────

fn run_se_inner(
    binary_path: &str,
    argv: &[&str],
    envp: &[&str],
    max_insns: u64,
    timing: &mut dyn TimingModel,
    backend: &mut ExecBackend,
    mut sampling: Option<&mut SamplingController>,
    plugins: Option<&PluginRegistry>,
    mut devices: Option<&mut DeviceBus>,
) -> Result<SeTimedResult, HelmError> {
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
    let has_insn_cbs = plugins.is_some_and(|p| p.has_insn_callbacks());
    if let Some(p) = plugins {
        p.fire_vcpu_init(0);
    }

    let mut insn_count: u64 = 0;
    let mut virtual_cycles: u64 = 0;

    let main_tid: u64 = 1000;
    let mut sched = Scheduler::new(cpu.regs.clone(), main_tid);

    loop {
        if insn_count >= max_insns {
            return Ok(SeTimedResult {
                exit_code: 0,
                instructions_executed: insn_count,
                virtual_cycles,
                hit_limit: true,
            });
        }

        // Load current thread's registers into CPU
        sched.load_regs(&mut cpu.regs);
        syscall.set_tid(sched.current_tid());

        match backend {
            ExecBackend::Interpretive => exec_interp(
                &mut cpu,
                &mut mem,
                &mut syscall,
                &mut sched,
                timing,
                &mut sampling,
                plugins,
                &mut devices,
                has_insn_cbs,
                &mut insn_count,
                &mut virtual_cycles,
                tls_info.as_ref(),
            )?,
            ExecBackend::Tcg { cache, interp } => exec_tcg(
                &mut cpu,
                &mut mem,
                &mut syscall,
                &mut sched,
                timing,
                plugins,
                &mut devices,
                cache,
                interp,
                &mut insn_count,
                &mut virtual_cycles,
                tls_info.as_ref(),
            )?,
        }

        // Save registers back after step
        sched.save_regs(&cpu.regs);

        if syscall.should_exit {
            return Ok(SeTimedResult {
                exit_code: syscall.exit_code,
                instructions_executed: insn_count,
                virtual_cycles,
                hit_limit: false,
            });
        }
    }
}

// ── interpretive backend ────────────────────────────────────────────────────

pub(crate) fn exec_interp(
    cpu: &mut Aarch64Cpu,
    mem: &mut AddressSpace,
    syscall: &mut Aarch64SyscallHandler,
    sched: &mut Scheduler,
    timing: &mut dyn TimingModel,
    sampling: &mut Option<&mut SamplingController>,
    plugins: Option<&PluginRegistry>,
    devices: &mut Option<&mut DeviceBus>,
    has_insn_cbs: bool,
    insn_count: &mut u64,
    virtual_cycles: &mut u64,
    tls_info: Option<&TlsInfo>,
) -> Result<(), HelmError> {
    let pc_before = cpu.regs.pc;
    match cpu.step(mem) {
        Ok(trace) => {
            *insn_count += 1;
            let mut stall = timing.instruction_latency_for_class(trace.class);
            for a in &trace.mem_accesses {
                stall += timing.memory_latency(a.addr, a.size, a.is_write);
                if let Some(bus) = devices.as_deref_mut() {
                    if bus.contains(a.addr) {
                        stall += if a.is_write {
                            bus.bus_write(a.addr, a.size, 0).unwrap_or(0)
                        } else {
                            bus.bus_read(a.addr, a.size)
                                .map(|r| r.stall_cycles)
                                .unwrap_or(0)
                        };
                    }
                }
            }
            if let Some(taken) = trace.branch_taken {
                if trace.class == InsnClass::CondBranch && taken && (pc_before >> 2) % 5 == 0 {
                    stall += timing.branch_misprediction_penalty();
                }
            }
            *virtual_cycles += stall;
            if let Some(sc) = sampling.as_deref_mut() {
                sc.advance(1);
            }
            if has_insn_cbs {
                if let Some(p) = plugins {
                    p.fire_insn_exec(
                        0,
                        &InsnInfo {
                            vaddr: pc_before,
                            bytes: trace.insn_word.to_le_bytes().to_vec(),
                            size: 4,
                            mnemonic: String::new(),
                            symbol: None,
                        },
                    );
                }
            }
        }
        Err(HelmError::Syscall { number, .. }) => handle_sc(
            cpu,
            mem,
            syscall,
            sched,
            timing,
            plugins,
            has_insn_cbs,
            pc_before,
            number,
            insn_count,
            virtual_cycles,
            tls_info,
        )?,
        Err(HelmError::Memory { addr, reason }) => {
            return Err(fire_and_err(
                cpu,
                plugins,
                *insn_count,
                FaultKind::MemFault,
                HelmError::Memory { addr, reason },
            ));
        }
        Err(e) => {
            let kind = if cpu.regs.pc == 0 {
                FaultKind::NullJump
            } else {
                FaultKind::Undef
            };
            return Err(fire_and_err(cpu, plugins, *insn_count, kind, e));
        }
    }
    Ok(())
}

// ── TCG backend ─────────────────────────────────────────────────────────────

pub(crate) fn exec_tcg(
    cpu: &mut Aarch64Cpu,
    mem: &mut AddressSpace,
    syscall: &mut Aarch64SyscallHandler,
    sched: &mut Scheduler,
    timing: &mut dyn TimingModel,
    plugins: Option<&PluginRegistry>,
    devices: &mut Option<&mut DeviceBus>,
    cache: &mut std::collections::HashMap<u64, TcgBlock>,
    interp: &mut TcgInterp,
    insn_count: &mut u64,
    virtual_cycles: &mut u64,
    tls_info: Option<&TlsInfo>,
) -> Result<(), HelmError> {
    let pc = cpu.regs.pc;
    if !cache.contains_key(&pc) {
        if devices.is_some() {
            log::trace!("TCG: translating block at {pc:#x} (device bus attached)");
        }
        let block = translate_block(pc, mem, 64);
        if block.insn_count > 0 {
            cache.insert(pc, block);
        }
    }
    if let Some(block) = cache.get(&pc) {
        let mut regs = regs_to_array(cpu);
        let result = interp.exec_block(block, &mut regs, mem)?;
        array_to_regs(cpu, &regs);
        let n = result.insns_executed as u64;
        *insn_count += n;
        for _ in 0..n {
            *virtual_cycles += timing.instruction_latency_for_class(InsnClass::IntAlu);
        }
        for a in &result.mem_accesses {
            *virtual_cycles += timing.memory_latency(a.addr, a.size, a.is_write);
        }
        match result.exit {
            InterpExit::Chain { target_pc } => cpu.regs.pc = target_pc,
            InterpExit::EndOfBlock { next_pc } => cpu.regs.pc = next_pc,
            InterpExit::Syscall { nr } => {
                let pc_before = cpu.regs.pc;
                handle_sc(
                    cpu,
                    mem,
                    syscall,
                    sched,
                    timing,
                    plugins,
                    false,
                    pc_before,
                    nr,
                    insn_count,
                    virtual_cycles,
                    tls_info,
                )?;
            }
            InterpExit::Exit => {}
            InterpExit::Wfi | InterpExit::Exception { .. } | InterpExit::ExceptionReturn => {
                // These are FS-mode events; in SE mode, treat as no-op.
            }
        }
    } else {
        // Fallback: interpretive step
        let pc_before = cpu.regs.pc;
        match cpu.step(mem) {
            Ok(trace) => {
                *insn_count += 1;
                *virtual_cycles += timing.instruction_latency_for_class(trace.class);
                for a in &trace.mem_accesses {
                    *virtual_cycles += timing.memory_latency(a.addr, a.size, a.is_write);
                }
            }
            Err(HelmError::Syscall { number, .. }) => handle_sc(
                cpu,
                mem,
                syscall,
                sched,
                timing,
                plugins,
                false,
                pc_before,
                number,
                insn_count,
                virtual_cycles,
                tls_info,
            )?,
            Err(e) => {
                let kind = if cpu.regs.pc == 0 {
                    FaultKind::NullJump
                } else {
                    FaultKind::Undef
                };
                return Err(fire_and_err(cpu, plugins, *insn_count, kind, e));
            }
        }
    }
    Ok(())
}

// ── shared syscall handler ──────────────────────────────────────────────────

pub(crate) fn handle_sc(
    cpu: &mut Aarch64Cpu,
    mem: &mut AddressSpace,
    syscall: &mut Aarch64SyscallHandler,
    sched: &mut Scheduler,
    timing: &mut dyn TimingModel,
    plugins: Option<&PluginRegistry>,
    has_insn_cbs: bool,
    pc_before: u64,
    number: u64,
    insn_count: &mut u64,
    virtual_cycles: &mut u64,
    tls_info: Option<&TlsInfo>,
) -> Result<(), HelmError> {
    let args = [
        cpu.xn(0),
        cpu.xn(1),
        cpu.xn(2),
        cpu.xn(3),
        cpu.xn(4),
        cpu.xn(5),
    ];
    if let Some(p) = plugins {
        p.fire_syscall(&SyscallInfo {
            number,
            args,
            vcpu_idx: 0,
        });
    }

    // Check if this syscall needs scheduler involvement
    if let Some(action) = syscall.try_sched_action(number, &args, mem) {
        match action {
            SyscallAction::Handled(ret) => {
                cpu.set_xn(0, ret);
                if let Some(p) = plugins {
                    p.fire_syscall_ret(&SyscallRetInfo {
                        number,
                        ret_value: ret,
                        vcpu_idx: 0,
                    });
                }
            }
            SyscallAction::Clone {
                flags,
                child_stack,
                parent_tid_ptr,
                child_tid_ptr,
                tls,
            } => {
                // Save current CPU state so spawn() clones up-to-date regs
                sched.save_regs(&cpu.regs);
                let req = CloneRequest {
                    flags,
                    child_stack,
                    parent_tid_ptr,
                    child_tid_ptr,
                    tls,
                };
                let child_tid = sched.spawn(req);

                // When CLONE_SETTLS was NOT set, spawn() left the child
                // with the parent's TPIDR_EL0.  Allocate an isolated
                // TLS block so the two threads never share a TLS region.
                const CLONE_SETTLS: u64 = 0x0008_0000;
                if flags & CLONE_SETTLS == 0 {
                    let parent_tp = cpu.regs.tpidr_el0;
                    if parent_tp != 0 {
                        let tls_size = tls_info.map_or(256, |t| t.mem_size.max(256));
                        let new_tp =
                            alloc_and_copy_tls(parent_tp, tls_size, tls_info, syscall, mem);
                        sched.set_last_spawned_tpidr(new_tp);
                    }
                }

                // Write parent_tid if CLONE_PARENT_SETTID
                if flags & 0x100000 != 0 {
                    let _ = mem.write(parent_tid_ptr, &(child_tid as u32).to_le_bytes());
                }
                // Write child_tid if CLONE_CHILD_SETTID
                if flags & 0x01000000 != 0 {
                    let _ = mem.write(child_tid_ptr, &(child_tid as u32).to_le_bytes());
                }
                cpu.set_xn(0, child_tid); // parent gets child TID
                if let Some(p) = plugins {
                    p.fire_syscall_ret(&SyscallRetInfo {
                        number,
                        ret_value: child_tid,
                        vcpu_idx: 0,
                    });
                }
            }
            SyscallAction::FutexWait { uaddr, val } => {
                // Save regs, block, context-switch
                cpu.regs.pc += 4;
                sched.save_regs(&cpu.regs);
                sched.block_current(ThreadState::FutexWait { uaddr, val });
                if sched.is_deadlocked() {
                    sched.break_deadlock();
                }
                sched.load_regs(&mut cpu.regs);
                *insn_count += 1;
                *virtual_cycles += timing.instruction_latency_for_class(InsnClass::Syscall);
                if let Some(p) = plugins {
                    p.fire_syscall_ret(&SyscallRetInfo {
                        number,
                        ret_value: 0,
                        vcpu_idx: 0,
                    });
                }
                return Ok(());
            }
            SyscallAction::FutexWake { uaddr, count } => {
                let woken = sched.futex_wake(uaddr, count);
                cpu.set_xn(0, woken as u64);
                if let Some(p) = plugins {
                    p.fire_syscall_ret(&SyscallRetInfo {
                        number,
                        ret_value: woken as u64,
                        vcpu_idx: 0,
                    });
                }
            }
            SyscallAction::ThreadExit { code } => {
                if sched.live_count() <= 1 {
                    syscall.should_exit = true;
                    syscall.exit_code = code;
                    if let Some(p) = plugins {
                        p.fire_syscall_ret(&SyscallRetInfo {
                            number,
                            ret_value: code,
                            vcpu_idx: 0,
                        });
                    }
                    return Ok(());
                }
                // Thread exit: clear TID, wake futex, switch
                if let Some(ctid_addr) = sched.exit_current() {
                    let _ = mem.write(ctid_addr, &0u32.to_le_bytes());
                    sched.futex_wake(ctid_addr, i32::MAX as u32);
                }
                if !sched.try_switch() {
                    syscall.should_exit = true;
                    syscall.exit_code = code;
                    return Ok(());
                }
                sched.load_regs(&mut cpu.regs);
                *insn_count += 1;
                if let Some(p) = plugins {
                    p.fire_syscall_ret(&SyscallRetInfo {
                        number,
                        ret_value: code,
                        vcpu_idx: 0,
                    });
                }
                return Ok(());
            }
            SyscallAction::Block(reason) => {
                let ts = match reason {
                    ThreadBlockReason::Read => ThreadState::BlockedRead,
                    ThreadBlockReason::Poll => ThreadState::BlockedPoll,
                };
                cpu.regs.pc += 4;
                sched.save_regs(&cpu.regs);
                if sched.live_count() > 1 {
                    sched.block_current(ts);
                    if sched.is_deadlocked() {
                        sched.break_deadlock();
                    }
                    sched.load_regs(&mut cpu.regs);
                } else {
                    // Single thread — return EAGAIN / 0 immediately
                    cpu.set_xn(
                        0,
                        match reason {
                            ThreadBlockReason::Read => (-11i64) as u64, // -EAGAIN
                            ThreadBlockReason::Poll => 0,
                        },
                    );
                }
                *insn_count += 1;
                *virtual_cycles += timing.instruction_latency_for_class(InsnClass::Syscall);
                if let Some(p) = plugins {
                    let ret_value = cpu.xn(0);
                    p.fire_syscall_ret(&SyscallRetInfo {
                        number,
                        ret_value,
                        vcpu_idx: 0,
                    });
                }
                return Ok(());
            }
        }
    } else {
        // Normal syscall — not scheduler-related
        let result = syscall.handle(number, &args, mem)?;
        cpu.set_xn(0, result);
        if let Some(p) = plugins {
            p.fire_syscall_ret(&SyscallRetInfo {
                number,
                ret_value: result,
                vcpu_idx: 0,
            });
        }
    }

    if !syscall.should_exit {
        cpu.regs.pc += 4;
        *insn_count += 1;
        *virtual_cycles += timing.instruction_latency_for_class(InsnClass::Syscall);
        if has_insn_cbs {
            if let Some(p) = plugins {
                p.fire_insn_exec(
                    0,
                    &InsnInfo {
                        vaddr: pc_before,
                        bytes: vec![0; 4],
                        size: 4,
                        mnemonic: "SVC".to_string(),
                        symbol: None,
                    },
                );
            }
        }
    }
    Ok(())
}

// ── per-thread TLS allocation ───────────────────────────────────────────────

/// Allocate an isolated TLS + pthread-struct block for a child thread.
///
/// On AArch64 (variant I TLS), TPIDR_EL0 points to the end of the
/// static TLS block.  The pthread struct (musl: ~200 bytes) lives at
/// **negative** offsets from TPIDR_EL0.  We allocate room for both
/// regions and return a thread-pointer in the middle so negative-offset
/// accesses hit a fresh, zero-initialised pthread struct.
fn alloc_and_copy_tls(
    parent_tp: u64,
    tls_mem_size: u64,
    tls_info: Option<&TlsInfo>,
    syscall: &mut Aarch64SyscallHandler,
    mem: &mut AddressSpace,
) -> u64 {
    // Size of the pthread struct area below the thread pointer.
    // musl uses offsets up to -0xb8 from TPIDR; 0x100 gives margin.
    const PTHREAD_SIZE: u64 = 0x100;

    let tls_above = tls_mem_size.max(256);
    let total = (PTHREAD_SIZE + tls_above + 0xF) & !0xF;
    let alloc = syscall.alloc_anon(total, mem);

    // The thread-pointer sits PTHREAD_SIZE bytes into the allocation.
    let new_tp = alloc + PTHREAD_SIZE;

    // The area below new_tp (the pthread struct) is already zero from
    // map, so destructor lists, TID, robust-list, etc. are all NULL.

    // Copy the parent's TLS data (above the TP) into the child.
    let copy_len = tls_above as usize;
    let mut buf = vec![0u8; copy_len];
    let _ = mem.read(parent_tp, &mut buf);
    let _ = mem.write(new_tp, &buf);

    // Re-apply the TLS initialisation image so __tdata starts clean.
    if let Some(info) = tls_info {
        if info.file_size > 0 {
            let mut tdata = vec![0u8; info.file_size as usize];
            if mem.read(info.template_vaddr, &mut tdata).is_ok() {
                let _ = mem.write(new_tp, &tdata);
            }
        }
    }

    // Write the self-pointer that musl stores at [tp - 0xb8].
    // It must point to the base of the pthread struct (tp - 0xc8).
    let _ = mem.write(
        new_tp.wrapping_sub(0xb8),
        &new_tp.wrapping_sub(0xc8).to_le_bytes(),
    );

    new_tp
}

// ── TCG translation ─────────────────────────────────────────────────────────

/// Build an AArch64 arch context from the current CPU state.
fn aarch64_context(cpu: &Aarch64Cpu) -> ArchContext {
    let mut x = [0u64; 31];
    for i in 0..31 {
        x[i] = cpu.xn(i as u16);
    }
    ArchContext::Aarch64 {
        x,
        sp: cpu.regs.sp,
        pc: cpu.regs.pc,
        nzcv: cpu.regs.nzcv,
        tpidr_el0: cpu.regs.tpidr_el0,
        current_el: cpu.regs.current_el,
    }
}

/// Fire fault callback and return the error.
fn fire_and_err(
    cpu: &Aarch64Cpu,
    plugins: Option<&PluginRegistry>,
    insn_count: u64,
    kind: FaultKind,
    err: HelmError,
) -> HelmError {
    if let Some(p) = plugins {
        p.fire_fault(&FaultInfo {
            vcpu_idx: 0,
            pc: cpu.regs.pc,
            insn_word: 0,
            fault_kind: kind,
            message: format!("{err}"),
            insn_count,
            arch_context: aarch64_context(cpu),
        });
    }
    err
}

fn translate_block(pc: u64, mem: &mut AddressSpace, max_insns: usize) -> TcgBlock {
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
    r
}

fn array_to_regs(cpu: &mut Aarch64Cpu, r: &[u64; NUM_REGS]) {
    for i in 0..31 {
        cpu.set_xn(i as u16, r[i]);
    }
    cpu.regs.sp = r[REG_SP as usize];
    cpu.regs.pc = r[REG_PC as usize];
    cpu.regs.nzcv = r[REG_NZCV as usize] as u32;
}
use helm_syscall::ThreadBlockReason;

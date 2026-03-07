//! AArch64 Syscall-Emulation mode runner.
//!
//! The execution backend (interpretive vs TCG) is selected via
//! [`ExecBackend`] — orthogonal to the simulation mode.

use crate::loader;
use crate::se::backend::ExecBackend;
use helm_core::HelmError;
use helm_device::DeviceBus;
use helm_isa::arm::aarch64::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;
use helm_plugin::runtime::{InsnInfo, SyscallInfo, SyscallRetInfo};
use helm_plugin::PluginRegistry;
use helm_syscall::Aarch64SyscallHandler;
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
    binary_path: &str, argv: &[&str], envp: &[&str], max_insns: u64,
) -> Result<SeResult, HelmError> {
    run_aarch64_se_with_plugins(binary_path, argv, envp, max_insns, None)
}

/// Run with optional plugin callbacks (interpretive, FE timing).
pub fn run_aarch64_se_with_plugins(
    binary_path: &str, argv: &[&str], envp: &[&str], max_insns: u64,
    plugins: Option<&PluginRegistry>,
) -> Result<SeResult, HelmError> {
    let mut timing = helm_timing::model::FeModel;
    let mut backend = ExecBackend::interpretive();
    let r = run_se_inner(binary_path, argv, envp, max_insns,
        &mut timing, &mut backend, None, plugins, None)?;
    Ok(SeResult { exit_code: r.exit_code, instructions_executed: r.instructions_executed, hit_limit: r.hit_limit })
}

/// Run with timing model, execution backend, plugins, and devices.
pub fn run_aarch64_se_timed(
    binary_path: &str, argv: &[&str], envp: &[&str], max_insns: u64,
    timing: &mut dyn TimingModel,
    backend: &mut ExecBackend,
    sampling: Option<&mut SamplingController>,
    plugins: Option<&PluginRegistry>,
    devices: Option<&mut DeviceBus>,
) -> Result<SeTimedResult, HelmError> {
    run_se_inner(binary_path, argv, envp, max_insns, timing, backend, sampling, plugins, devices)
}

// ── unified inner runner ────────────────────────────────────────────────────

fn run_se_inner(
    binary_path: &str, argv: &[&str], envp: &[&str], max_insns: u64,
    timing: &mut dyn TimingModel, backend: &mut ExecBackend,
    mut sampling: Option<&mut SamplingController>,
    plugins: Option<&PluginRegistry>, mut devices: Option<&mut DeviceBus>,
) -> Result<SeTimedResult, HelmError> {
    let loaded = loader::load_elf(binary_path, argv, envp)?;
    let mut mem = loaded.address_space;
    let mut cpu = Aarch64Cpu::new();
    cpu.regs.pc = loaded.entry_point;
    cpu.regs.sp = loaded.initial_sp;
    mem.map(0, 0x1000, (true, false, false));
    let mut syscall = Aarch64SyscallHandler::new();
    syscall.set_brk(loaded.brk_base);
    syscall.binary_path = std::fs::canonicalize(binary_path)
        .unwrap_or_else(|_| std::path::PathBuf::from(binary_path))
        .to_string_lossy().into_owned();
    mem.map(loaded.brk_base, 0x1000, (true, true, false));
    let has_insn_cbs = plugins.is_some_and(|p| p.has_insn_callbacks());
    if let Some(p) = plugins { p.fire_vcpu_init(0); }

    let mut insn_count: u64 = 0;
    let mut virtual_cycles: u64 = 0;

    loop {
        if insn_count >= max_insns {
            return Ok(SeTimedResult { exit_code: 0, instructions_executed: insn_count, virtual_cycles, hit_limit: true });
        }
        match backend {
            ExecBackend::Interpretive => exec_interp(
                &mut cpu, &mut mem, &mut syscall, timing, &mut sampling,
                plugins, &mut devices, has_insn_cbs, &mut insn_count, &mut virtual_cycles,
            )?,
            ExecBackend::Tcg { cache, interp } => exec_tcg(
                &mut cpu, &mut mem, &mut syscall, timing,
                plugins, &mut devices, cache, interp, &mut insn_count, &mut virtual_cycles,
            )?,
        }
        if syscall.should_exit {
            return Ok(SeTimedResult {
                exit_code: syscall.exit_code, instructions_executed: insn_count,
                virtual_cycles, hit_limit: false,
            });
        }
    }
}

// ── interpretive backend ────────────────────────────────────────────────────

fn exec_interp(
    cpu: &mut Aarch64Cpu, mem: &mut AddressSpace, syscall: &mut Aarch64SyscallHandler,
    timing: &mut dyn TimingModel, sampling: &mut Option<&mut SamplingController>,
    plugins: Option<&PluginRegistry>, devices: &mut Option<&mut DeviceBus>,
    has_insn_cbs: bool, insn_count: &mut u64, virtual_cycles: &mut u64,
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
                        stall += if a.is_write { bus.bus_write(a.addr, a.size, 0).unwrap_or(0) }
                                 else { bus.bus_read(a.addr, a.size).map(|r| r.stall_cycles).unwrap_or(0) };
                    }
                }
            }
            if let Some(taken) = trace.branch_taken {
                if trace.class == InsnClass::CondBranch && taken && (pc_before >> 2) % 5 == 0 {
                    stall += timing.branch_misprediction_penalty();
                }
            }
            *virtual_cycles += stall;
            if let Some(sc) = sampling.as_deref_mut() { sc.advance(1); }
            if has_insn_cbs {
                if let Some(p) = plugins {
                    p.fire_insn_exec(0, &InsnInfo {
                        vaddr: pc_before, bytes: trace.insn_word.to_le_bytes().to_vec(),
                        size: 4, mnemonic: String::new(), symbol: None,
                    });
                }
            }
        }
        Err(HelmError::Syscall { number, .. }) =>
            handle_sc(cpu, mem, syscall, timing, plugins, has_insn_cbs, pc_before, number, insn_count, virtual_cycles)?,
        Err(HelmError::Memory { addr, reason }) => return Err(HelmError::Memory { addr, reason }),
        Err(e) => return Err(e),
    }
    Ok(())
}

// ── TCG backend ─────────────────────────────────────────────────────────────

fn exec_tcg(
    cpu: &mut Aarch64Cpu, mem: &mut AddressSpace, syscall: &mut Aarch64SyscallHandler,
    timing: &mut dyn TimingModel, plugins: Option<&PluginRegistry>,
    devices: &mut Option<&mut DeviceBus>,
    cache: &mut std::collections::HashMap<u64, TcgBlock>, interp: &mut TcgInterp,
    insn_count: &mut u64, virtual_cycles: &mut u64,
) -> Result<(), HelmError> {
    let pc = cpu.regs.pc;
    if !cache.contains_key(&pc) {
        let block = translate_block(pc, mem, 64);
        if block.insn_count > 0 { cache.insert(pc, block); }
    }
    if let Some(block) = cache.get(&pc) {
        let mut regs = regs_to_array(cpu);
        let result = interp.exec_block(block, &mut regs, mem)?;
        array_to_regs(cpu, &regs);
        let n = result.insns_executed as u64;
        *insn_count += n;
        for _ in 0..n { *virtual_cycles += timing.instruction_latency_for_class(InsnClass::IntAlu); }
        for a in &result.mem_accesses { *virtual_cycles += timing.memory_latency(a.addr, a.size, a.is_write); }
        match result.exit {
            InterpExit::Chain { target_pc } => cpu.regs.pc = target_pc,
            InterpExit::EndOfBlock { next_pc } => cpu.regs.pc = next_pc,
            InterpExit::Syscall { nr } => {
                let args = [cpu.xn(0), cpu.xn(1), cpu.xn(2), cpu.xn(3), cpu.xn(4), cpu.xn(5)];
                if let Some(p) = plugins { p.fire_syscall(&SyscallInfo { number: nr, args, vcpu_idx: 0 }); }
                let ret = syscall.handle(nr, &args, mem)?;
                cpu.set_xn(0, ret);
                if let Some(p) = plugins { p.fire_syscall_ret(&SyscallRetInfo { number: nr, ret_value: ret, vcpu_idx: 0 }); }
                if !syscall.should_exit { cpu.regs.pc += 4; }
                *virtual_cycles += timing.instruction_latency_for_class(InsnClass::Syscall);
            }
            InterpExit::Exit => {}
        }
    } else {
        // Fallback: interpretive step
        let pc_before = cpu.regs.pc;
        match cpu.step(mem) {
            Ok(trace) => {
                *insn_count += 1;
                *virtual_cycles += timing.instruction_latency_for_class(trace.class);
                for a in &trace.mem_accesses { *virtual_cycles += timing.memory_latency(a.addr, a.size, a.is_write); }
            }
            Err(HelmError::Syscall { number, .. }) =>
                handle_sc(cpu, mem, syscall, timing, plugins, false, pc_before, number, insn_count, virtual_cycles)?,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

// ── shared syscall handler ──────────────────────────────────────────────────

fn handle_sc(
    cpu: &mut Aarch64Cpu, mem: &mut AddressSpace, syscall: &mut Aarch64SyscallHandler,
    timing: &mut dyn TimingModel, plugins: Option<&PluginRegistry>,
    has_insn_cbs: bool, pc_before: u64, number: u64,
    insn_count: &mut u64, virtual_cycles: &mut u64,
) -> Result<(), HelmError> {
    let args = [cpu.xn(0), cpu.xn(1), cpu.xn(2), cpu.xn(3), cpu.xn(4), cpu.xn(5)];
    if let Some(p) = plugins { p.fire_syscall(&SyscallInfo { number, args, vcpu_idx: 0 }); }
    let result = syscall.handle(number, &args, mem)?;
    cpu.set_xn(0, result);
    if let Some(p) = plugins { p.fire_syscall_ret(&SyscallRetInfo { number, ret_value: result, vcpu_idx: 0 }); }
    if !syscall.should_exit {
        cpu.regs.pc += 4;
        *insn_count += 1;
        *virtual_cycles += timing.instruction_latency_for_class(InsnClass::Syscall);
        if has_insn_cbs {
            if let Some(p) = plugins {
                p.fire_insn_exec(0, &InsnInfo { vaddr: pc_before, bytes: vec![0; 4], size: 4, mnemonic: "SVC".to_string(), symbol: None });
            }
        }
    }
    Ok(())
}

// ── TCG translation ─────────────────────────────────────────────────────────

fn translate_block(pc: u64, mem: &mut AddressSpace, max_insns: usize) -> TcgBlock {
    let mut ctx = TcgContext::new();
    let mut cur = pc;
    let mut n = 0;
    for _ in 0..max_insns {
        let mut buf = [0u8; 4];
        if mem.read(cur, &mut buf).is_err() { break; }
        let mut e = A64TcgEmitter::new(&mut ctx, cur);
        match e.translate_insn(u32::from_le_bytes(buf)) {
            TranslateAction::Continue => { n += 1; cur += 4; }
            TranslateAction::EndBlock => { n += 1; break; }
            TranslateAction::Unhandled => break,
        }
    }
    TcgBlock { guest_pc: pc, guest_size: (cur - pc) as usize, insn_count: n, ops: ctx.finish() }
}

fn regs_to_array(cpu: &Aarch64Cpu) -> [u64; NUM_REGS] {
    let mut r = [0u64; NUM_REGS];
    for i in 0..31 { r[i] = cpu.xn(i as u16); }
    r[REG_SP as usize] = cpu.regs.sp;
    r[REG_PC as usize] = cpu.regs.pc;
    r[REG_NZCV as usize] = cpu.regs.nzcv as u64;
    r
}

fn array_to_regs(cpu: &mut Aarch64Cpu, r: &[u64; NUM_REGS]) {
    for i in 0..31 { cpu.set_xn(i as u16, r[i]); }
    cpu.regs.sp = r[REG_SP as usize];
    cpu.regs.pc = r[REG_PC as usize];
    cpu.regs.nzcv = r[REG_NZCV as usize] as u32;
}

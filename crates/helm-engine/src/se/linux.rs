//! AArch64 Syscall-Emulation mode runner.
//!
//! Loads a static ELF binary, sets up the CPU and syscall handler,
//! and runs the fetch-decode-execute loop until exit.
//!
//! Plugin callbacks (if a [`PluginRegistry`] is provided) fire on every
//! instruction execution, syscall entry, and syscall return.

use crate::loader;
use helm_core::HelmError;
use helm_isa::arm::aarch64::Aarch64Cpu;
use helm_plugin::runtime::{InsnInfo, SyscallInfo, SyscallRetInfo};
use helm_plugin::PluginRegistry;
use helm_syscall::Aarch64SyscallHandler;

/// Result of an SE-mode simulation run.
pub struct SeResult {
    pub exit_code: u64,
    pub instructions_executed: u64,
    pub hit_limit: bool,
}

/// Run a static AArch64 binary in SE mode.
///
/// `argv` is the argument list (argv[0] should be the binary path).
/// `envp` is the environment variables.
pub fn run_aarch64_se(
    binary_path: &str,
    argv: &[&str],
    envp: &[&str],
    max_insns: u64,
) -> Result<SeResult, HelmError> {
    run_aarch64_se_with_plugins(binary_path, argv, envp, max_insns, None)
}

/// Run a static AArch64 binary in SE mode with optional plugin callbacks.
pub fn run_aarch64_se_with_plugins(
    binary_path: &str,
    argv: &[&str],
    envp: &[&str],
    max_insns: u64,
    plugins: Option<&PluginRegistry>,
) -> Result<SeResult, HelmError> {
    // Load ELF
    let loaded = loader::load_elf(binary_path, argv, envp)?;
    let mut mem = loaded.address_space;

    // Set up CPU
    let mut cpu = Aarch64Cpu::new();
    cpu.regs.pc = loaded.entry_point;
    cpu.regs.sp = loaded.initial_sp;

    // Map low page as readable (null-terminated string scans may probe address 0)
    mem.map(0, 0x1000, (true, false, false));

    // Set up syscall handler with brk starting after loaded segments
    let mut syscall = Aarch64SyscallHandler::new();
    let brk_start = (loaded.entry_point & !0xFFF) + 0x800000; // entry + 8MB gap
    syscall.set_brk(brk_start);

    let has_insn_cbs = plugins.is_some_and(|p| p.has_insn_callbacks());

    // Notify plugin of vCPU init
    if let Some(p) = plugins {
        p.fire_vcpu_init(0);
    }

    let mut insn_count: u64 = 0;

    loop {
        if insn_count >= max_insns {
            log::warn!(
                "instruction limit reached: PC={:#x} SP={:#x} X0={:#x} X8={:#x}",
                cpu.regs.pc,
                cpu.regs.sp,
                cpu.xn(0),
                cpu.xn(8)
            );
            for r in [0u16, 1, 19, 20, 22] {
                log::warn!("  X{}={:#x}", r, cpu.xn(r));
            }
            return Ok(SeResult {
                exit_code: 0,
                instructions_executed: insn_count,
                hit_limit: true,
            });
        }

        let pc_before = cpu.regs.pc;

        match cpu.step(&mut mem) {
            Ok(()) => {
                insn_count += 1;

                if has_insn_cbs {
                    if let Some(p) = plugins {
                        // Read the 4-byte instruction at pc_before for the callback
                        let mut ibuf = [0u8; 4];
                        let _ = mem.read(pc_before, &mut ibuf);
                        p.fire_insn_exec(
                            0,
                            &InsnInfo {
                                vaddr: pc_before,
                                bytes: ibuf.to_vec(),
                                size: 4,
                                mnemonic: String::new(), // decoding mnemonic is expensive; skip for hot path
                                symbol: None,
                            },
                        );
                    }
                }
            }
            Err(HelmError::Syscall { number, .. }) => {
                let args = [
                    cpu.xn(0),
                    cpu.xn(1),
                    cpu.xn(2),
                    cpu.xn(3),
                    cpu.xn(4),
                    cpu.xn(5),
                ];

                // Fire syscall-entry callback
                if let Some(p) = plugins {
                    p.fire_syscall(&SyscallInfo {
                        number,
                        args,
                        vcpu_idx: 0,
                    });
                }

                let result = syscall.handle(number, &args, &mut mem)?;
                cpu.set_xn(0, result);

                // Fire syscall-return callback
                if let Some(p) = plugins {
                    p.fire_syscall_ret(&SyscallRetInfo {
                        number,
                        ret_value: result,
                        vcpu_idx: 0,
                    });
                }

                if syscall.should_exit {
                    log::info!(
                        "exit({}) after {insn_count} instructions",
                        syscall.exit_code
                    );
                    return Ok(SeResult {
                        exit_code: syscall.exit_code,
                        instructions_executed: insn_count,
                        hit_limit: false,
                    });
                }

                cpu.regs.pc += 4;
                insn_count += 1;

                // Fire insn callback for the SVC instruction itself
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
            Err(HelmError::Memory { addr, reason }) => {
                log::error!(
                    "memory fault at PC={:#x}: addr={addr:#x} {reason}",
                    cpu.regs.pc
                );
                return Err(HelmError::Memory { addr, reason });
            }
            Err(e) => return Err(e),
        }
    }
}

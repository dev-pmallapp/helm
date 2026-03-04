//! AArch64 Syscall-Emulation mode runner.
//!
//! Loads a static ELF binary, sets up the CPU and syscall handler,
//! and runs the fetch-decode-execute loop until exit.

use crate::loader;
use helm_core::HelmError;
use helm_isa::arm::aarch64::Aarch64Cpu;
use helm_syscall::Aarch64SyscallHandler;

/// Result of an SE-mode simulation run.
pub struct SeResult {
    pub exit_code: u64,
    pub instructions_executed: u64,
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
    // Load ELF
    let loaded = loader::load_elf(binary_path, argv, envp)?;
    let mut mem = loaded.address_space;

    // Set up CPU
    let mut cpu = Aarch64Cpu::new();
    cpu.regs.pc = loaded.entry_point;
    cpu.regs.sp = loaded.initial_sp;

    // Map a guard page at address 0 (readable, for null-terminated string scans)
    mem.map(0, 0x1000, (true, false, false));

    // Set up syscall handler with brk starting after loaded segments
    let mut syscall = Aarch64SyscallHandler::new();
    // Set brk past the highest loaded address
    let brk_start = (loaded.entry_point & !0xFFF) + 0x800000; // entry + 8MB gap
    syscall.set_brk(brk_start);

    // Execution loop
    let mut insn_count: u64 = 0;

    loop {
        if insn_count >= max_insns {
            log::info!("hit max instruction limit ({max_insns})");
            break;
        }

        match cpu.step(&mut mem) {
            Ok(()) => {
                insn_count += 1;
            }
            Err(HelmError::Syscall { number, .. }) => {
                // Extract args from registers
                let args = [
                    cpu.xn(0),
                    cpu.xn(1),
                    cpu.xn(2),
                    cpu.xn(3),
                    cpu.xn(4),
                    cpu.xn(5),
                ];
                let result = syscall.handle(number, &args, &mut mem)?;
                cpu.set_xn(0, result);

                if syscall.should_exit {
                    log::info!(
                        "exit({}) after {insn_count} instructions",
                        syscall.exit_code
                    );
                    return Ok(SeResult {
                        exit_code: syscall.exit_code,
                        instructions_executed: insn_count,
                    });
                }

                // Advance PC past the SVC instruction
                cpu.regs.pc += 4;
                insn_count += 1;
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

    Ok(SeResult {
        exit_code: 0,
        instructions_executed: insn_count,
    })
}

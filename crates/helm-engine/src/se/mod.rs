//! Syscall emulation (SE) mode — Linux ABI for statically-linked RISC-V binaries.
//!
//! # Phase 0 target
//! Implement ~50 essential syscalls to run `riscv-tests` and simple hello-world binaries.
//!
//! # Design
//! `SyscallHandler` is a `dyn` trait (cold path only — never on the hot fetch/decode/execute path).
//! `HelmEngine` calls it only on `ecall` / `EnvironmentCall` exception.

use helm_core::HartException;

// ── SyscallHandler trait ──────────────────────────────────────────────────────

/// Cold-path syscall dispatch interface.
///
/// Receives control after the engine intercepts an `ecall`.
/// Returns `Ok(retval)` to place in `a0`, or `Err(HartException)` to propagate.
pub trait SyscallHandler: Send {
    /// Handle one syscall. `nr` = syscall number (from `a7`).
    /// The handler reads arguments from `ctx` (a0–a5) and writes the return value to `a0`.
    fn handle(&mut self, nr: u64, args: SyscallArgs) -> Result<i64, HartException>;
}

/// Syscall arguments (RISC-V Linux calling convention: a0–a5 = x10–x15, nr = x17).
#[derive(Debug, Clone, Copy)]
pub struct SyscallArgs {
    pub a0: u64,
    pub a1: u64,
    pub a2: u64,
    pub a3: u64,
    pub a4: u64,
    pub a5: u64,
}

// ── LinuxSyscallHandler ───────────────────────────────────────────────────────

/// Linux ABI syscall handler for RISC-V SE mode.
///
/// Implements the ~50 syscalls needed for `riscv-tests` and simple userspace binaries.
///
/// Phase 0 syscall list (implement in order of need):
/// - exit / exit_group
/// - write (to support hello-world via fd=1)
/// - read
/// - brk / mmap / munmap (heap support)
/// - openat / close / fstat / lseek
/// - uname / getpid / gettid / getuid / getgid
/// - clock_gettime / gettimeofday
/// - ioctl / fcntl
pub struct LinuxSyscallHandler {
    // TODO(phase-0): add host file descriptor table, brk pointer, etc.
    brk: u64,
}

impl LinuxSyscallHandler {
    pub fn new(initial_brk: u64) -> Self { Self { brk: initial_brk } }
}

impl SyscallHandler for LinuxSyscallHandler {
    fn handle(&mut self, nr: u64, args: SyscallArgs) -> Result<i64, HartException> {
        // RISC-V Linux syscall numbers (from <asm/unistd.h> for riscv)
        match nr {
            // exit_group
            94 => Err(HartException::Exit { code: args.a0 as i32 }),
            // exit
            93 => Err(HartException::Exit { code: args.a0 as i32 }),
            // write(fd, buf_addr, count) — Phase 0: only fd=1 (stdout) and fd=2 (stderr)
            64 => {
                // TODO(phase-0): access guest memory via ctx to read buf_addr..count
                // For now, stub returns count to keep programs happy
                Ok(args.a2 as i64)
            }
            // brk
            214 => {
                if args.a0 == 0 {
                    Ok(self.brk as i64)
                } else {
                    self.brk = args.a0;
                    Ok(self.brk as i64)
                }
            }
            // Unimplemented — return -ENOSYS
            _ => {
                log::debug!("unimplemented syscall {nr} (a0={:#x})", args.a0);
                Ok(-38) // -ENOSYS
            }
        }
    }
}

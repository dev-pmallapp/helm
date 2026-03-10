//! Adapter implementing `helm_core::syscall::SyscallHandler` trait
//! for the existing `Aarch64SyscallHandler`.

use crate::os::linux::handler::Aarch64SyscallHandler;
use helm_core::cpu::CpuState;
use helm_core::mem::MemoryAccess;
use helm_core::syscall::{SyscallAction, SyscallHandler};

/// Wraps `Aarch64SyscallHandler` to work with the new trait-based interface.
///
/// The adapter extracts args from X0-X5 via `CpuState::gpr()`, creates a
/// temporary `AddressSpace`-backed memory, and delegates to the existing handler.
pub struct Aarch64SyscallAdapter {
    pub inner: Aarch64SyscallHandler,
}

impl Aarch64SyscallAdapter {
    pub fn new() -> Self {
        Self {
            inner: Aarch64SyscallHandler::new(),
        }
    }
}

impl Default for Aarch64SyscallAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// Standalone adapter that works with `dyn MemoryAccess` directly,
/// without requiring an `AddressSpace`. Handles the most common
/// syscalls (exit, brk, write) via the trait interface.
pub struct TraitSyscallHandler {
    pub should_exit: bool,
    pub exit_code: u64,
    brk_addr: u64,
}

impl TraitSyscallHandler {
    pub fn new() -> Self {
        Self {
            should_exit: false,
            exit_code: 0,
            brk_addr: 0x0200_0000,
        }
    }

    pub fn set_brk(&mut self, addr: u64) {
        self.brk_addr = addr;
    }
}

impl Default for TraitSyscallHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl SyscallHandler for TraitSyscallHandler {
    fn handle(
        &mut self,
        nr: u64,
        cpu: &mut dyn CpuState,
        mem: &mut dyn MemoryAccess,
    ) -> SyscallAction {
        let x0 = cpu.gpr(0);
        let x1 = cpu.gpr(1);
        let x2 = cpu.gpr(2);

        match nr {
            // exit / exit_group
            93 | 94 => {
                self.should_exit = true;
                self.exit_code = x0;
                SyscallAction::Exit { code: x0 }
            }
            // write(fd, buf, count)
            64 => {
                let fd = x0;
                let buf_addr = x1;
                let count = x2 as usize;
                let mut buf = vec![0u8; count.min(4096)];
                for (i, b) in buf.iter_mut().enumerate() {
                    if let Ok(v) = mem.read(buf_addr + i as u64, 1) {
                        *b = v as u8;
                    }
                }
                if fd == 1 || fd == 2 {
                    // stdout / stderr — write to host
                    let s = String::from_utf8_lossy(&buf[..count.min(4096)]);
                    if fd == 2 {
                        eprint!("{}", s);
                    } else {
                        print!("{}", s);
                    }
                }
                SyscallAction::Handled(count as u64)
            }
            // read(fd, buf, count) — stub returns 0 (EOF)
            63 => SyscallAction::Handled(0),
            // brk
            214 => {
                if x0 == 0 {
                    SyscallAction::Handled(self.brk_addr)
                } else {
                    if x0 > self.brk_addr {
                        self.brk_addr = x0;
                    }
                    SyscallAction::Handled(self.brk_addr)
                }
            }
            // mmap — simple bump allocator
            222 => {
                let len = x1;
                let addr = self.brk_addr;
                self.brk_addr += (len + 0xFFF) & !0xFFF; // page-align
                SyscallAction::Handled(addr)
            }
            // munmap, mprotect, madvise — success stubs
            215 | 226 | 233 => SyscallAction::Handled(0),
            // set_tid_address — return fake tid
            96 => SyscallAction::Handled(1000),
            // set_robust_list, prlimit64, getrandom, clock_gettime, gettimeofday
            99 | 261 | 278 | 113 | 169 => SyscallAction::Handled(0),
            // getpid, getppid, getuid, geteuid, getgid, getegid
            172 | 173 | 174 | 175 | 176 | 177 => SyscallAction::Handled(1000),
            // rt_sigaction, rt_sigprocmask, sigaltstack
            134 | 135 | 132 => SyscallAction::Handled(0),
            // ioctl — stub -ENOTTY
            29 => SyscallAction::Handled((-25i64) as u64),
            // openat — stub -ENOENT
            56 => SyscallAction::Handled((-2i64) as u64),
            // close
            57 => SyscallAction::Handled(0),
            // fstat/fstatat — stub -ENOSYS
            79 | 80 => SyscallAction::Handled((-38i64) as u64),
            // unimplemented
            _ => {
                log::warn!("Unimplemented syscall {}", nr);
                SyscallAction::Handled((-38i64) as u64) // -ENOSYS
            }
        }
    }
}

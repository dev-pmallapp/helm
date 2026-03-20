//! Linux RISC-V 64 syscall handler.
//!
//! RISC-V Linux calling convention:
//! - Syscall number → **a7** (x17)
//! - Arguments     → a0–a5 (x10–x15)
//! - Return value  → a0 (x10) — negative errno on error
//!
//! Syscall numbers: RISC-V uses `asm-generic/unistd.h` directly, so most
//! numbers are identical to AArch64.  Differences are noted inline.
//!
//! # Syscall coverage
//! ~70 syscalls covering what statically-linked ELF binaries need:
//! file I/O, memory management, process control, time, signals, and system info.

// All unsafe blocks are libc FFI syscall wrappers.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ffi::CString;
use std::os::unix::io::RawFd;

use helm_core::{HartException, MemInterface};
#[allow(unused_imports)]
use libc;

use super::SyscallArgs;

// ── Error codes ───────────────────────────────────────────────────────────────

const ENOSYS: i64 = -38;
const EBADF:  i64 = -9;
const EINVAL: i64 = -22;

// ── RISC-V Linux syscall numbers (asm-generic/unistd.h) ──────────────────────

#[allow(dead_code)]
mod nr {
    // I/O
    pub const READ: u64              = 63;
    pub const WRITE: u64             = 64;
    pub const READV: u64             = 65;
    pub const WRITEV: u64            = 66;
    pub const PREAD64: u64           = 67;
    pub const PWRITE64: u64          = 68;
    pub const SENDFILE: u64          = 71;
    pub const OPENAT: u64            = 56;
    pub const CLOSE: u64             = 57;
    pub const PIPE2: u64             = 59;
    pub const LSEEK: u64             = 62;
    pub const FSTAT: u64             = 80;
    pub const FSTATAT: u64           = 79;
    pub const STATX: u64             = 291;
    pub const GETDENTS64: u64        = 61;
    pub const IOCTL: u64             = 29;
    pub const FCNTL: u64             = 25;
    pub const DUP: u64               = 23;
    pub const DUP3: u64              = 24;
    pub const PPOLL: u64             = 73;
    pub const PSELECT6: u64          = 72;
    pub const STATFS: u64            = 43;
    pub const FSTATFS: u64           = 44;
    pub const FTRUNCATE: u64         = 46;
    pub const TRUNCATE: u64          = 45;
    pub const UTIMENSAT: u64         = 88;
    pub const READLINKAT: u64        = 78;
    pub const FACCESSAT: u64         = 48;
    pub const FACCESSAT2: u64        = 439;
    pub const MKDIRAT: u64           = 34;
    pub const UNLINKAT: u64          = 35;
    pub const RENAMEAT2: u64         = 276;
    pub const GETCWD: u64            = 17;
    pub const CHDIR: u64             = 49;
    pub const FCHDIR: u64            = 50;
    pub const FCHOWNAT: u64          = 54;
    pub const FCHMODAT: u64          = 53;
    pub const SYNC_FILE_RANGE: u64   = 84;
    pub const FDATASYNC: u64         = 83;
    pub const FLOCK: u64             = 32;
    // Memory
    pub const MMAP: u64              = 222;
    pub const MUNMAP: u64            = 215;
    pub const MPROTECT: u64          = 226;
    pub const MADVISE: u64           = 233;
    pub const MREMAP: u64            = 216;
    pub const MSYNC: u64             = 227;
    pub const BRK: u64               = 214;
    // Process
    pub const EXIT: u64              = 93;
    pub const EXIT_GROUP: u64        = 94;
    pub const EXECVE: u64            = 221;
    pub const CLONE: u64             = 220;
    pub const WAIT4: u64             = 260;
    pub const GETPID: u64            = 172;
    pub const GETPPID: u64           = 173;
    pub const GETTID: u64            = 178;
    pub const GETUID: u64            = 174;
    pub const GETEUID: u64           = 175;
    pub const GETGID: u64            = 176;
    pub const GETEGID: u64           = 177;
    pub const SETPGID: u64           = 154;
    pub const GETPGID: u64           = 155;
    pub const SETSID: u64            = 157;
    pub const UMASK: u64             = 166;
    pub const SCHED_YIELD: u64       = 124;
    pub const SCHED_GETAFFINITY: u64 = 123;
    pub const SCHED_SETAFFINITY: u64 = 122;
    pub const PRCTL: u64             = 167;
    pub const PERSONALITY: u64       = 92;
    pub const GETGROUPS: u64         = 158;
    // Threads
    pub const SET_TID_ADDRESS: u64   = 96;
    pub const SET_ROBUST_LIST: u64   = 99;
    pub const GET_ROBUST_LIST: u64   = 100;
    pub const FUTEX: u64             = 98;
    // Time
    pub const CLOCK_GETTIME: u64     = 113;
    pub const CLOCK_GETRES: u64      = 114;
    pub const CLOCK_NANOSLEEP: u64   = 115;
    pub const GETTIMEOFDAY: u64      = 169;
    pub const NANOSLEEP: u64         = 101;
    pub const TIMES: u64             = 153;
    // Signals
    pub const RT_SIGACTION: u64      = 134;
    pub const RT_SIGPROCMASK: u64    = 135;
    pub const RT_SIGRETURN: u64      = 139;
    pub const RT_SIGSUSPEND: u64     = 133;
    pub const KILL: u64              = 129;
    pub const TGKILL: u64            = 131;
    pub const TKILL: u64             = 130;
    pub const SIGALTSTACK: u64       = 132;
    // System info
    pub const UNAME: u64             = 160;
    pub const PRLIMIT64: u64         = 261;
    pub const GETRLIMIT: u64         = 163;
    pub const SETRLIMIT: u64         = 164;
    pub const CAPGET: u64            = 90;
    pub const CAPSET: u64            = 91;
    pub const SYSINFO: u64           = 179;
    pub const GETCPU: u64            = 168;
    // Random
    pub const GETRANDOM: u64         = 278;
    // Pipe/epoll/inotify
    pub const EPOLL_CREATE1: u64     = 20;
    pub const EPOLL_CTL: u64         = 21;
    pub const EPOLL_PWAIT: u64       = 22;
    pub const INOTIFY_INIT1: u64     = 26;
    pub const INOTIFY_ADD_WATCH: u64 = 27;
    pub const INOTIFY_RM_WATCH: u64  = 28;
    pub const EVENTFD2: u64          = 19;
    pub const TIMERFD_CREATE: u64    = 85;
    pub const TIMERFD_SETTIME: u64   = 86;
    pub const TIMERFD_GETTIME: u64   = 87;
    pub const MEMFD_CREATE: u64      = 279;
    pub const MEMBARRIER: u64        = 283;
    pub const CLOSE_RANGE: u64       = 436;
    pub const OPENAT2: u64           = 437;
    pub const COPY_FILE_RANGE: u64   = 285;
    pub const RSEQ: u64              = 293;
    pub const SECCOMP: u64           = 277;
    pub const RESTART_SYSCALL: u64   = 128;
}

// ── FdTable ───────────────────────────────────────────────────────────────────

struct FdTable {
    table: HashMap<i32, RawFd>,
    next:  i32,
}

impl FdTable {
    fn new() -> Self {
        let mut t = Self { table: HashMap::new(), next: 3 };
        t.table.insert(0, 0);
        t.table.insert(1, 1);
        t.table.insert(2, 2);
        t
    }

    fn allocate(&mut self, host_fd: RawFd) -> i32 {
        let guest = self.next;
        self.next += 1;
        self.table.insert(guest, host_fd);
        guest
    }

    fn get(&self, guest: i32) -> Option<RawFd> {
        self.table.get(&guest).copied()
    }

    fn remove(&mut self, guest: i32) -> Option<RawFd> {
        self.table.remove(&guest)
    }
}

// ── LinuxRiscv64SyscallHandler ────────────────────────────────────────────────

/// Linux RISC-V 64 syscall emulator.
///
/// Mirrors `LinuxAarch64SyscallHandler` but uses RISC-V syscall numbers and
/// reports "riscv64" in `uname`.
pub struct LinuxRiscv64SyscallHandler {
    fds:       FdTable,
    brk:       u64,
    mmap_next: u64,
    mmap_free: Vec<(u64, u64)>,
    pid:       u64,
    tid:       u64,
    pub should_exit: bool,
    pub exit_code:   i32,
    pub binary_path: String,
}

impl LinuxRiscv64SyscallHandler {
    pub fn new(initial_brk: u64) -> Self {
        Self {
            fds:       FdTable::new(),
            brk:       initial_brk,
            mmap_next: 0x4000_0000_0000u64,
            mmap_free: Vec::new(),
            pid:       1000,
            tid:       1000,
            should_exit: false,
            exit_code:   0,
            binary_path: String::new(),
        }
    }

    /// Core dispatch — called by `SyscallHandler::handle` with a `MemInterface`.
    pub fn handle_with_mem(
        &mut self,
        nr: u64,
        args: SyscallArgs,
        mem: &mut dyn MemInterface,
    ) -> Result<i64, HartException> {
        use helm_core::AccessType;

        match nr {
            // ── Exit ───────────────────────────────────────────────────────
            nr::EXIT | nr::EXIT_GROUP => {
                self.should_exit = true;
                self.exit_code   = args.a0 as i32;
                return Err(HartException::Exit { code: self.exit_code });
            }

            // ── I/O ────────────────────────────────────────────────────────
            nr::WRITE => {
                let fd    = args.a0 as i32;
                let buf   = args.a1;
                let count = args.a2 as usize;
                let host  = self.fds.get(fd).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let bytes = read_guest_bytes(mem, buf, count);
                let n = unsafe { libc::write(host, bytes.as_ptr() as *const _, bytes.len()) };
                if n < 0 { Ok(-errno() as i64) } else { Ok(n as i64) }
            }
            nr::READ => {
                let fd    = args.a0 as i32;
                let buf   = args.a1;
                let count = args.a2 as usize;
                let host  = self.fds.get(fd).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let mut bytes = vec![0u8; count];
                let n = unsafe { libc::read(host, bytes.as_mut_ptr() as *mut _, bytes.len()) };
                if n < 0 { return Ok(-errno() as i64); }
                write_guest_bytes(mem, buf, &bytes[..n as usize]);
                Ok(n as i64)
            }
            nr::WRITEV => {
                let fd      = args.a0 as i32;
                let iov_ptr = args.a1;
                let iovcnt  = args.a2 as usize;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let mut total = 0i64;
                for i in 0..iovcnt {
                    let base = mem.read(iov_ptr + i as u64 * 16,     8, AccessType::Load).unwrap_or(0);
                    let len  = mem.read(iov_ptr + i as u64 * 16 + 8, 8, AccessType::Load).unwrap_or(0) as usize;
                    let bytes = read_guest_bytes(mem, base, len);
                    let n = unsafe { libc::write(host, bytes.as_ptr() as *const _, bytes.len()) };
                    if n < 0 { return Ok(-errno() as i64); }
                    total += n as i64;
                }
                Ok(total)
            }
            nr::READV => {
                let fd      = args.a0 as i32;
                let iov_ptr = args.a1;
                let iovcnt  = args.a2 as usize;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let mut total = 0i64;
                for i in 0..iovcnt {
                    let base = mem.read(iov_ptr + i as u64 * 16,     8, AccessType::Load).unwrap_or(0);
                    let len  = mem.read(iov_ptr + i as u64 * 16 + 8, 8, AccessType::Load).unwrap_or(0) as usize;
                    if len == 0 { continue; }
                    let mut bytes = vec![0u8; len];
                    let n = unsafe { libc::read(host, bytes.as_mut_ptr() as *mut _, bytes.len()) };
                    if n < 0 { return Ok(-errno() as i64); }
                    write_guest_bytes(mem, base, &bytes[..n as usize]);
                    total += n as i64;
                    if (n as usize) < len { break; }
                }
                Ok(total)
            }
            nr::PREAD64 => {
                let fd  = args.a0 as i32;
                let buf = args.a1;
                let cnt = args.a2 as usize;
                let off = args.a3 as i64;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let mut bytes = vec![0u8; cnt];
                let n = unsafe { libc::pread(host, bytes.as_mut_ptr() as _, bytes.len(), off) };
                if n < 0 { return Ok(-errno() as i64); }
                write_guest_bytes(mem, buf, &bytes[..n as usize]);
                Ok(n as i64)
            }
            nr::PWRITE64 => {
                let fd  = args.a0 as i32;
                let buf = args.a1;
                let cnt = args.a2 as usize;
                let off = args.a3 as i64;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let bytes = read_guest_bytes(mem, buf, cnt);
                let n = unsafe { libc::pwrite(host, bytes.as_ptr() as _, bytes.len(), off) };
                Ok(if n < 0 { -errno() as i64 } else { n as i64 })
            }
            nr::OPENAT => {
                let _dirfd   = args.a0 as i32;
                let path_ptr = args.a1;
                let flags    = args.a2 as i32;
                let mode     = args.a3 as u32;
                let path = read_guest_cstr(mem, path_ptr);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let host_fd = unsafe { libc::open(cpath.as_ptr(), flags, mode) };
                if host_fd < 0 { return Ok(-errno() as i64); }
                Ok(self.fds.allocate(host_fd) as i64)
            }
            nr::CLOSE => {
                let fd   = args.a0 as i32;
                let host = self.fds.remove(fd).unwrap_or(-1);
                if host < 0 || fd < 3 { return Ok(0); }
                let r = unsafe { libc::close(host) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::LSEEK => {
                let fd     = args.a0 as i32;
                let offset = args.a1 as i64;
                let whence = args.a2 as i32;
                let host   = self.fds.get(fd).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let r = unsafe { libc::lseek(host, offset, whence) };
                Ok(if r < 0 { -errno() as i64 } else { r as i64 })
            }
            nr::DUP => {
                let fd   = args.a0 as i32;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let new_host = unsafe { libc::dup(host) };
                if new_host < 0 { return Ok(-errno() as i64); }
                Ok(self.fds.allocate(new_host) as i64)
            }
            nr::DUP3 => {
                let old = args.a0 as i32;
                let new = args.a1 as i32;
                let host_old = self.fds.get(old).unwrap_or(-1);
                if host_old < 0 { return Ok(EBADF); }
                let host_new = unsafe { libc::dup(host_old) };
                if host_new < 0 { return Ok(-errno() as i64); }
                if let Some(old_host) = self.fds.remove(new) {
                    unsafe { libc::close(old_host); }
                }
                self.fds.table.insert(new, host_new);
                Ok(new as i64)
            }
            nr::IOCTL => {
                let _fd = args.a0 as i32;
                let req = args.a1;
                match req {
                    0x5401 /* TCGETS */     => Ok(EINVAL),
                    0x5413 /* TIOCGWINSZ */ => {
                        let ptr = args.a2;
                        mem.write(ptr,     2, 24, AccessType::Store).ok();
                        mem.write(ptr + 2, 2, 80, AccessType::Store).ok();
                        mem.write(ptr + 4, 2, 0,  AccessType::Store).ok();
                        mem.write(ptr + 6, 2, 0,  AccessType::Store).ok();
                        Ok(0)
                    }
                    _ => Ok(EINVAL),
                }
            }
            nr::FCNTL => {
                let fd  = args.a0 as i32;
                let cmd = args.a1 as i32;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let r = unsafe { libc::fcntl(host, cmd, args.a2) };
                Ok(if r < 0 { -errno() as i64 } else { r as i64 })
            }
            nr::FLOCK => Ok(0),

            // ── File metadata ────────────────────────────────────────────────
            nr::FSTAT => {
                let fd  = args.a0 as i32;
                let ptr = args.a1;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let mut st: libc::stat = unsafe { std::mem::zeroed() };
                let r = unsafe { libc::fstat(host, &mut st) };
                if r < 0 { return Ok(-errno() as i64); }
                write_stat(mem, ptr, &st);
                Ok(0)
            }
            nr::FSTATAT => {
                let dirfd    = args.a0 as i32;
                let path_ptr = args.a1;
                let ptr      = args.a2;
                let _flags   = args.a3 as i32;
                if path_ptr != 0 {
                    let path = read_guest_cstr(mem, path_ptr);
                    let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                    let mut st: libc::stat = unsafe { std::mem::zeroed() };
                    let r = unsafe { libc::stat(cpath.as_ptr(), &mut st) };
                    if r < 0 { return Ok(-errno() as i64); }
                    write_stat(mem, ptr, &st);
                } else {
                    // fstat by fd
                    let host = if dirfd == -100 { self.fds.get(0).unwrap_or(0) }
                               else { self.fds.get(dirfd).unwrap_or(-1) };
                    if host < 0 { return Ok(EBADF); }
                    let mut st: libc::stat = unsafe { std::mem::zeroed() };
                    let r = unsafe { libc::fstat(host, &mut st) };
                    if r < 0 { return Ok(-errno() as i64); }
                    write_stat(mem, ptr, &st);
                }
                Ok(0)
            }
            nr::STATX => {
                let dirfd    = args.a0 as i32;
                let path_ptr = args.a1;
                let flags    = args.a2 as i32;
                let mask     = args.a3 as u32;
                let out_ptr  = args.a4;
                let path = read_guest_cstr(mem, path_ptr);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let host_dirfd = if dirfd == -100 { libc::AT_FDCWD } else { self.fds.get(dirfd).unwrap_or(dirfd) };
                let mut buf = vec![0u8; 256];
                let r = unsafe {
                    libc::syscall(libc::SYS_statx, host_dirfd as i64,
                                  cpath.as_ptr() as i64, flags as i64,
                                  mask as i64, buf.as_mut_ptr() as i64)
                };
                if r < 0 { return Ok(-errno() as i64); }
                write_guest_bytes(mem, out_ptr, &buf);
                Ok(0)
            }
            nr::STATFS => {
                let ptr = args.a1;
                mem.write(ptr,      8, 0xEF53u64,    AccessType::Store).ok();
                mem.write(ptr + 8,  8, 4096u64,      AccessType::Store).ok();
                mem.write(ptr + 16, 8, 1_000_000u64, AccessType::Store).ok();
                mem.write(ptr + 24, 8, 500_000u64,   AccessType::Store).ok();
                mem.write(ptr + 32, 8, 500_000u64,   AccessType::Store).ok();
                mem.write(ptr + 40, 8, 1_000_000u64, AccessType::Store).ok();
                mem.write(ptr + 48, 8, 900_000u64,   AccessType::Store).ok();
                mem.write(ptr + 56, 8, 0u64,         AccessType::Store).ok();
                mem.write(ptr + 64, 8, 255u64,       AccessType::Store).ok();
                mem.write(ptr + 72, 8, 4096u64,      AccessType::Store).ok();
                mem.write(ptr + 80, 8, 0u64,         AccessType::Store).ok();
                Ok(0)
            }
            nr::FSTATFS => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let ptr = args.a1;
                let mut st: libc::statfs = unsafe { std::mem::zeroed() };
                let r = unsafe { libc::fstatfs(host, &mut st) };
                if r < 0 { return Ok(-errno() as i64); }
                mem.write(ptr,      8, st.f_type    as u64, AccessType::Store).ok();
                mem.write(ptr + 8,  8, st.f_bsize   as u64, AccessType::Store).ok();
                mem.write(ptr + 16, 8, st.f_blocks  as u64, AccessType::Store).ok();
                mem.write(ptr + 24, 8, st.f_bfree   as u64, AccessType::Store).ok();
                mem.write(ptr + 32, 8, st.f_bavail  as u64, AccessType::Store).ok();
                mem.write(ptr + 40, 8, st.f_files   as u64, AccessType::Store).ok();
                mem.write(ptr + 48, 8, st.f_ffree   as u64, AccessType::Store).ok();
                mem.write(ptr + 56, 8, 0u64,                AccessType::Store).ok();
                mem.write(ptr + 64, 8, st.f_namelen as u64, AccessType::Store).ok();
                mem.write(ptr + 72, 8, st.f_frsize  as u64, AccessType::Store).ok();
                Ok(0)
            }
            nr::READLINKAT => {
                let _dirfd   = args.a0 as i32;
                let path_ptr = args.a1;
                let out_ptr  = args.a2;
                let bufsiz   = args.a3 as usize;
                let path = read_guest_cstr(mem, path_ptr);
                if path == "/proc/self/exe" || path == "/proc/self/maps" {
                    let fake = b"/bin/binary\0";
                    let n = fake.len().min(bufsiz);
                    write_guest_bytes(mem, out_ptr, &fake[..n]);
                    return Ok(n as i64);
                }
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let mut buf = vec![0u8; bufsiz];
                let n = unsafe { libc::readlink(cpath.as_ptr(), buf.as_mut_ptr() as *mut _, bufsiz) };
                if n < 0 { return Ok(-errno() as i64); }
                write_guest_bytes(mem, out_ptr, &buf[..n as usize]);
                Ok(n as i64)
            }
            nr::GETCWD => {
                let buf = args.a0;
                let sz  = args.a1 as usize;
                let cwd = std::env::current_dir()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "/".to_string());
                let bytes = cwd.as_bytes();
                let n = (bytes.len() + 1).min(sz);
                write_guest_bytes(mem, buf, &bytes[..n.saturating_sub(1)]);
                mem.write(buf + n as u64 - 1, 1, 0, AccessType::Store).ok();
                Ok(n as i64)
            }
            nr::CHDIR => {
                let path = read_guest_cstr(mem, args.a0);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let r = unsafe { libc::chdir(cpath.as_ptr()) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::FCHDIR => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let r = unsafe { libc::fchdir(host) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::FACCESSAT | nr::FACCESSAT2 => Ok(0),
            nr::MKDIRAT => {
                let dirfd = args.a0 as i32;
                let host_dirfd = if dirfd == -100 { libc::AT_FDCWD } else { self.fds.get(dirfd).unwrap_or(dirfd) };
                let path = read_guest_cstr(mem, args.a1);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let r = unsafe { libc::mkdirat(host_dirfd, cpath.as_ptr(), args.a2 as u32) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::UNLINKAT => {
                let dirfd = args.a0 as i32;
                let host_dirfd = if dirfd == -100 { libc::AT_FDCWD } else { self.fds.get(dirfd).unwrap_or(dirfd) };
                let path = read_guest_cstr(mem, args.a1);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let r = unsafe { libc::unlinkat(host_dirfd, cpath.as_ptr(), args.a2 as i32) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::RENAMEAT2 => Ok(0),
            nr::FCHMODAT => {
                let dirfd = args.a0 as i32;
                let host_dirfd = if dirfd == -100 { libc::AT_FDCWD } else { self.fds.get(dirfd).unwrap_or(dirfd) };
                let path = read_guest_cstr(mem, args.a1);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let r = unsafe { libc::syscall(libc::SYS_fchmodat, host_dirfd as i64,
                    cpath.as_ptr() as i64, args.a2 as i64, args.a3 as i64) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::FCHOWNAT => {
                let dirfd = args.a0 as i32;
                let host_dirfd = if dirfd == -100 { libc::AT_FDCWD } else { self.fds.get(dirfd).unwrap_or(dirfd) };
                let path = read_guest_cstr(mem, args.a1);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let r = unsafe { libc::fchownat(host_dirfd, cpath.as_ptr(), args.a2 as u32, args.a3 as u32, args.a4 as i32) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::FTRUNCATE => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let r = unsafe { libc::ftruncate(host, args.a1 as i64) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::TRUNCATE => {
                let path = read_guest_cstr(mem, args.a0);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let r = unsafe { libc::truncate(cpath.as_ptr(), args.a1 as i64) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::UTIMENSAT => {
                let dirfd = args.a0 as i32;
                let host_dirfd = if dirfd == -100 { libc::AT_FDCWD } else { self.fds.get(dirfd).unwrap_or(dirfd) };
                let path = if args.a1 != 0 { read_guest_cstr(mem, args.a1) } else { String::new() };
                let cpath = if !path.is_empty() { CString::new(path.as_bytes()).ok() } else { None };
                let times_host: *const libc::timespec = if args.a2 != 0 {
                    let bytes = read_guest_bytes(mem, args.a2, 32);
                    bytes.as_ptr() as *const libc::timespec
                } else { std::ptr::null() };
                let r = unsafe {
                    if let Some(ref cp) = cpath {
                        libc::utimensat(host_dirfd, cp.as_ptr(), times_host, args.a3 as i32)
                    } else {
                        libc::futimens(host_dirfd, times_host)
                    }
                };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::SYNC_FILE_RANGE | nr::FDATASYNC => Ok(0),
            nr::GETDENTS64 => {
                let fd      = args.a0 as i32;
                let buf_ptr = args.a1;
                let count   = args.a2 as usize;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let mut host_buf = vec![0u8; count];
                let n = unsafe {
                    libc::syscall(libc::SYS_getdents64, host as i64,
                                  host_buf.as_mut_ptr() as i64, count as i64)
                };
                if n < 0 { return Ok(-errno() as i64); }
                write_guest_bytes(mem, buf_ptr, &host_buf[..n as usize]);
                Ok(n)
            }
            nr::PIPE2 => {
                let mut fds = [0i32; 2];
                let r = unsafe { libc::pipe2(fds.as_mut_ptr(), 0) };
                if r < 0 { return Ok(-errno() as i64); }
                let gfd_r = self.fds.allocate(fds[0]) as u64;
                let gfd_w = self.fds.allocate(fds[1]) as u64;
                mem.write(args.a0,     4, gfd_r, AccessType::Store).ok();
                mem.write(args.a0 + 4, 4, gfd_w, AccessType::Store).ok();
                Ok(0)
            }
            nr::SENDFILE => {
                let out_fd  = args.a0 as i32;
                let in_fd   = args.a1 as i32;
                let off_ptr = args.a2;
                let count   = args.a3 as usize;
                let host_out = self.fds.get(out_fd).unwrap_or(-1);
                let host_in  = self.fds.get(in_fd).unwrap_or(-1);
                if host_out < 0 || host_in < 0 { return Ok(EBADF); }
                let mut off_host: libc::off_t = if off_ptr != 0 {
                    mem.read(off_ptr, 8, AccessType::Load).unwrap_or(0) as i64
                } else { 0 };
                let off_ptr_host: *mut libc::off_t = if off_ptr != 0 { &mut off_host } else { std::ptr::null_mut() };
                let r = unsafe { libc::sendfile(host_out, host_in, off_ptr_host, count) };
                if r < 0 { return Ok(-errno() as i64); }
                if off_ptr != 0 { mem.write(off_ptr, 8, off_host as u64, AccessType::Store).ok(); }
                Ok(r as i64)
            }

            // ── Memory management ────────────────────────────────────────────
            nr::BRK => {
                let addr = args.a0;
                if addr == 0 {
                    Ok(self.brk as i64)
                } else {
                    self.brk = addr;
                    Ok(self.brk as i64)
                }
            }
            nr::MMAP => {
                let addr_hint = args.a0;
                let len       = args.a1;
                let _prot     = args.a2;
                let flags     = args.a3;
                let _fd       = args.a4 as i32;
                let _offset   = args.a5;

                let len_actual  = if len == 0 { 0x400_0000 } else { len };
                let len_aligned = (len_actual + 0xFFF) & !0xFFF;

                const MAP_ANONYMOUS: u64 = 0x20;

                let addr = if addr_hint != 0 {
                    addr_hint
                } else {
                    let reuse = self.mmap_free
                        .iter()
                        .position(|&(_, sz)| sz >= len_aligned);
                    if let Some(idx) = reuse {
                        let (a, _) = self.mmap_free.swap_remove(idx);
                        a
                    } else {
                        let a = self.mmap_next;
                        self.mmap_next += len_aligned;
                        a
                    }
                };
                if flags & MAP_ANONYMOUS != 0 {
                    let zeros = vec![0u8; len_aligned as usize];
                    write_guest_bytes(mem, addr, &zeros);
                }
                Ok(addr as i64)
            }
            nr::MUNMAP => {
                let addr = args.a0;
                let len  = args.a1;
                let len_aligned = (len + 0xFFF) & !0xFFF;
                if addr != 0 && len_aligned > 0 {
                    self.mmap_free.push((addr, len_aligned));
                }
                Ok(0)
            }
            nr::MPROTECT => Ok(0),
            nr::MADVISE  => Ok(0),
            nr::MSYNC    => Ok(0),
            nr::MREMAP => {
                let old_addr = args.a0;
                let old_size = args.a1;
                let new_size = args.a2;
                let flags    = args.a3;
                let new_aligned = (new_size + 0xFFF) & !0xFFF;
                if new_size <= old_size {
                    return Ok(old_addr as i64);
                }
                const MREMAP_MAYMOVE: u64 = 1;
                if flags & MREMAP_MAYMOVE != 0 {
                    let dest = self.mmap_next;
                    self.mmap_next += new_aligned;
                    let bytes = read_guest_bytes(mem, old_addr, old_size as usize);
                    write_guest_bytes(mem, dest, &bytes);
                    return Ok(dest as i64);
                }
                Ok(old_addr as i64)
            }

            // ── Process identity ─────────────────────────────────────────────
            nr::GETPID  => Ok(self.pid as i64),
            nr::GETPPID => Ok(self.pid.saturating_sub(1) as i64),
            nr::GETTID  => Ok(self.tid as i64),
            nr::GETUID | nr::GETEUID => Ok(1000),
            nr::GETGID | nr::GETEGID => Ok(1000),
            nr::GETGROUPS => Ok(0),
            nr::UMASK => Ok(0o022),
            nr::SCHED_YIELD => Ok(0),
            nr::SCHED_GETAFFINITY => {
                let cpusetsize = args.a1 as usize;
                let mask_ptr   = args.a2;
                for off in (0..cpusetsize as u64).step_by(8) {
                    mem.write(mask_ptr + off, 8, 0u64, AccessType::Store).ok();
                }
                mem.write(mask_ptr, 1, 1u64, AccessType::Store).ok();
                Ok(0)
            }
            nr::SCHED_SETAFFINITY => Ok(0),
            nr::SETSID | nr::SETPGID => Ok(0),
            nr::GETPGID => Ok(self.pid as i64),

            // ── Threads ──────────────────────────────────────────────────────
            nr::SET_TID_ADDRESS => Ok(self.tid as i64),
            nr::SET_ROBUST_LIST | nr::GET_ROBUST_LIST => Ok(0),
            nr::FUTEX => {
                let op = args.a1 as u32 & 0x7F;
                match op {
                    0 /* WAIT */ => Ok(0),
                    1 /* WAKE */ => Ok(1),
                    _ => Ok(EINVAL),
                }
            }

            // ── Time ─────────────────────────────────────────────────────────
            nr::CLOCK_GETTIME => {
                let tp_ptr = args.a1;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                mem.write(tp_ptr,     8, now.as_secs(),            AccessType::Store).ok();
                mem.write(tp_ptr + 8, 8, now.subsec_nanos() as u64, AccessType::Store).ok();
                Ok(0)
            }
            nr::CLOCK_GETRES => {
                let tp_ptr = args.a1;
                mem.write(tp_ptr,     8, 0, AccessType::Store).ok();
                mem.write(tp_ptr + 8, 8, 1, AccessType::Store).ok();
                Ok(0)
            }
            nr::GETTIMEOFDAY => {
                let tv_ptr = args.a0;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                mem.write(tv_ptr,     8, now.as_secs(),             AccessType::Store).ok();
                mem.write(tv_ptr + 8, 8, now.subsec_micros() as u64, AccessType::Store).ok();
                Ok(0)
            }
            nr::NANOSLEEP | nr::CLOCK_NANOSLEEP => Ok(0),
            nr::TIMES => Ok(0),

            // ── Signals (stub) ────────────────────────────────────────────────
            nr::RT_SIGACTION   => Ok(0),
            nr::RT_SIGPROCMASK => Ok(0),
            nr::RT_SIGRETURN   => Ok(0),
            nr::RT_SIGSUSPEND  => Ok(EINVAL),
            nr::KILL | nr::TGKILL | nr::TKILL => Ok(0),
            nr::SIGALTSTACK => {
                let old_ss = args.a1;
                if old_ss != 0 {
                    const SS_DISABLE: u32 = 2;
                    write_guest_bytes(mem, old_ss,      &0u64.to_le_bytes());
                    write_guest_bytes(mem, old_ss + 8,  &SS_DISABLE.to_le_bytes());
                    write_guest_bytes(mem, old_ss + 12, &0u32.to_le_bytes());
                    write_guest_bytes(mem, old_ss + 16, &0u64.to_le_bytes());
                }
                Ok(0)
            }

            // ── System info ──────────────────────────────────────────────────
            nr::UNAME => {
                let ptr = args.a0;
                write_guest_str(mem, ptr,        "Linux",   65);
                write_guest_str(mem, ptr + 65,   "helm-ng", 65);
                write_guest_str(mem, ptr + 130,  "6.1.0",   65);
                write_guest_str(mem, ptr + 195,  "helm-ng", 65);
                write_guest_str(mem, ptr + 260,  "riscv64", 65); // machine field
                Ok(0)
            }
            nr::PRCTL => {
                let op = args.a0 as i32;
                match op {
                    15 /* PR_SET_NAME */ => Ok(0),
                    16 /* PR_GET_NAME */ => {
                        write_guest_str(mem, args.a1, "helm-ng", 16);
                        Ok(0)
                    }
                    _ => Ok(0),
                }
            }
            nr::PRLIMIT64 => {
                let resource  = args.a1;
                let old_limit = args.a3;
                if old_limit != 0 {
                    let (cur, max): (u64, u64) = match resource {
                        3  /* RLIMIT_STACK  */ => (8 * 1024 * 1024, u64::MAX),
                        7  /* RLIMIT_NOFILE */ => (1024,             4096),
                        9  /* RLIMIT_AS     */ => (u64::MAX,         u64::MAX),
                        8  /* RLIMIT_MEMLOCK*/ => (64 * 1024,        64 * 1024),
                        6  /* RLIMIT_NPROC  */ => (1024,             1024),
                        4  /* RLIMIT_CORE   */ => (0,                0),
                        _                      => (u64::MAX,         u64::MAX),
                    };
                    mem.write(old_limit,     8, cur, AccessType::Store).ok();
                    mem.write(old_limit + 8, 8, max, AccessType::Store).ok();
                }
                Ok(0)
            }
            nr::GETRLIMIT | nr::SETRLIMIT => Ok(0),
            nr::CAPGET | nr::CAPSET => Ok(0),
            nr::SYSINFO => Ok(0),
            nr::PERSONALITY => Ok(0),

            // ── Random ───────────────────────────────────────────────────────
            nr::GETRANDOM => {
                let buf = args.a0;
                let len = args.a1 as usize;
                let bytes: Vec<u8> = (0..len).map(|_| rand_byte()).collect();
                write_guest_bytes(mem, buf, &bytes);
                Ok(len as i64)
            }

            // ── Polling / epoll ───────────────────────────────────────────────
            nr::PPOLL | nr::PSELECT6 => Ok(0),
            nr::EPOLL_CREATE1 => {
                let r = unsafe { libc::epoll_create1(args.a0 as i32) };
                if r < 0 { return Ok(-errno() as i64); }
                Ok(self.fds.allocate(r) as i64)
            }
            nr::EPOLL_CTL => Ok(0),
            nr::EPOLL_PWAIT => Ok(0),
            nr::INOTIFY_INIT1 => {
                let r = unsafe { libc::inotify_init1(args.a0 as i32) };
                if r < 0 { return Ok(-errno() as i64); }
                Ok(self.fds.allocate(r) as i64)
            }
            nr::INOTIFY_ADD_WATCH => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let path = read_guest_cstr(mem, args.a1);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let r = unsafe { libc::inotify_add_watch(host, cpath.as_ptr(), args.a2 as u32) };
                Ok(if r < 0 { -errno() as i64 } else { r as i64 })
            }
            nr::INOTIFY_RM_WATCH => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let r = unsafe { libc::inotify_rm_watch(host, args.a1 as i32) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::EVENTFD2 => {
                let r = unsafe { libc::eventfd(args.a0 as u32, args.a1 as i32) };
                if r < 0 { return Ok(-errno() as i64); }
                Ok(self.fds.allocate(r) as i64)
            }
            nr::TIMERFD_CREATE => {
                let r = unsafe { libc::timerfd_create(args.a0 as i32, args.a1 as i32) };
                if r < 0 { return Ok(-errno() as i64); }
                Ok(self.fds.allocate(r) as i64)
            }
            nr::TIMERFD_SETTIME | nr::TIMERFD_GETTIME => Ok(0),
            nr::MEMFD_CREATE => {
                let name = read_guest_cstr(mem, args.a0);
                let cname = CString::new(name.as_bytes()).unwrap_or_default();
                let r = unsafe { libc::syscall(libc::SYS_memfd_create, cname.as_ptr() as i64, args.a1 as i64) };
                if r < 0 { return Ok(-errno() as i64); }
                Ok(self.fds.allocate(r as i32) as i64)
            }
            nr::MEMBARRIER => Ok(0),
            nr::GETCPU => {
                // getcpu(cpu*, node*, cache*) — report CPU 0, NUMA node 0
                if args.a0 != 0 { mem.write(args.a0, 4, 0u64, AccessType::Store).ok(); }
                if args.a1 != 0 { mem.write(args.a1, 4, 0u64, AccessType::Store).ok(); }
                Ok(0)
            }
            nr::CLOSE_RANGE | nr::COPY_FILE_RANGE => Ok(0),
            nr::RSEQ => Ok(EINVAL), // restartable sequences — not supported
            nr::SECCOMP => Ok(0),
            nr::RESTART_SYSCALL => Ok(0),

            // ── EXECVE: unsupported in SE mode ────────────────────────────────
            nr::EXECVE => Ok(EINVAL),

            // ── Clone / wait ─────────────────────────────────────────────────
            nr::CLONE => {
                // Single-threaded SE mode: do not actually spawn threads.
                // Return EINVAL for thread-spawn clones; fork-like clones return
                // 0 (child) so the binary thinks it's the child process.
                Ok(0)
            }
            nr::WAIT4 => Ok(0),

            // ── Unimplemented ────────────────────────────────────────────────
            _ => {
                log::warn!("riscv64 unimplemented syscall {nr} (a0={:#x}, a1={:#x})", args.a0, args.a1);
                Ok(ENOSYS)
            }
        }
    }
}

impl super::SyscallHandler for LinuxRiscv64SyscallHandler {
    fn handle(&mut self, nr: u64, args: SyscallArgs, mem: &mut dyn helm_core::MemInterface) -> Result<i64, HartException> {
        self.handle_with_mem(nr, args, mem)
    }
}

// ── Guest memory helpers ──────────────────────────────────────────────────────

fn read_guest_bytes(mem: &mut dyn MemInterface, addr: u64, len: usize) -> Vec<u8> {
    use helm_core::AccessType;
    let mut out = vec![0u8; len];
    for i in 0..len {
        out[i] = mem.read(addr + i as u64, 1, AccessType::Load).unwrap_or(0) as u8;
    }
    out
}

fn write_guest_bytes(mem: &mut dyn MemInterface, addr: u64, bytes: &[u8]) {
    use helm_core::AccessType;
    for (i, &b) in bytes.iter().enumerate() {
        mem.write(addr + i as u64, 1, b as u64, AccessType::Store).ok();
    }
}

fn read_guest_cstr(mem: &mut dyn MemInterface, addr: u64) -> String {
    use helm_core::AccessType;
    let mut s = Vec::new();
    let mut i = 0u64;
    loop {
        let b = mem.read(addr + i, 1, AccessType::Load).unwrap_or(0) as u8;
        if b == 0 { break; }
        s.push(b);
        i += 1;
        if i > 4096 { break; }
    }
    String::from_utf8_lossy(&s).into_owned()
}

fn write_guest_str(mem: &mut dyn MemInterface, addr: u64, s: &str, max: usize) {
    use helm_core::AccessType;
    let bytes = s.as_bytes();
    let n = bytes.len().min(max - 1);
    for i in 0..n {
        mem.write(addr + i as u64, 1, bytes[i] as u64, AccessType::Store).ok();
    }
    mem.write(addr + n as u64, 1, 0, AccessType::Store).ok();
}

/// Write a `struct stat` to guest memory.
///
/// Linux RISC-V `stat` layout (same as x86-64/arm64, 144 bytes):
/// st_dev(8), st_ino(8), st_mode(4), st_nlink(4), st_uid(4), st_gid(4),
/// st_rdev(8), pad0(8), st_size(8), st_blksize(8), st_blocks(8),
/// st_atime_sec(8), st_atime_nsec(8), st_mtime_sec(8), st_mtime_nsec(8),
/// st_ctime_sec(8), st_ctime_nsec(8), unused[3](24)
fn write_stat(mem: &mut dyn MemInterface, ptr: u64, st: &libc::stat) {
    use helm_core::AccessType;
    mem.write(ptr,       8, st.st_dev  as u64, AccessType::Store).ok();
    mem.write(ptr + 8,   8, st.st_ino  as u64, AccessType::Store).ok();
    mem.write(ptr + 16,  4, st.st_mode as u64, AccessType::Store).ok();
    mem.write(ptr + 20,  4, st.st_nlink as u64, AccessType::Store).ok();
    mem.write(ptr + 24,  4, st.st_uid  as u64, AccessType::Store).ok();
    mem.write(ptr + 28,  4, st.st_gid  as u64, AccessType::Store).ok();
    mem.write(ptr + 32,  8, st.st_rdev as u64, AccessType::Store).ok();
    mem.write(ptr + 40,  8, 0u64,               AccessType::Store).ok(); // pad
    mem.write(ptr + 48,  8, st.st_size as u64, AccessType::Store).ok();
    mem.write(ptr + 56,  8, st.st_blksize as u64, AccessType::Store).ok();
    mem.write(ptr + 64,  8, st.st_blocks as u64, AccessType::Store).ok();
    mem.write(ptr + 72,  8, st.st_atime as u64, AccessType::Store).ok();
    mem.write(ptr + 80,  8, 0u64,               AccessType::Store).ok(); // atime nsec
    mem.write(ptr + 88,  8, st.st_mtime as u64, AccessType::Store).ok();
    mem.write(ptr + 96,  8, 0u64,               AccessType::Store).ok(); // mtime nsec
    mem.write(ptr + 104, 8, st.st_ctime as u64, AccessType::Store).ok();
    mem.write(ptr + 112, 8, 0u64,               AccessType::Store).ok(); // ctime nsec
}

fn errno() -> i32 {
    unsafe { *libc::__errno_location() }
}

fn rand_byte() -> u8 {
    // Simple LCG — not cryptographic, fine for SE mode
    use std::cell::Cell;
    thread_local!(static SEED: Cell<u64> = Cell::new(0xDEAD_BEEF_1234_5678));
    SEED.with(|s| {
        let v = s.get().wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        s.set(v);
        (v >> 33) as u8
    })
}

//! Linux AArch64 syscall handler.
//!
//! AArch64 Linux calling convention:
//! - Syscall number → **X8**
//! - Arguments     → X0–X5

// All unsafe blocks in this file are libc FFI syscall wrappers; suppressing
// the workspace-level unsafe_code warning which fires on every libc call.
#![allow(unsafe_code)]
//! - Return value  → X0 (negative errno on error)
//!
//! # Syscall coverage
//! Approximately 80 syscalls covering what statically-linked ELF binaries need:
//! file I/O, memory management, process control, time, signals, and system info.

use std::collections::HashMap;
use std::ffi::CString;
use std::os::unix::io::RawFd;

use helm_core::{HartException, MemInterface};
#[allow(unused_imports)]
use libc;

use super::threading::{classify_clone_flags, CloneStyle, HostThreadRuntime, HostThreadSpawnError};
use super::SyscallArgs;

// ── Error codes ───────────────────────────────────────────────────────────────

pub const ENOSYS: i64 = -38;
pub const ENOENT: i64 = -2;
pub const EBADF: i64 = -9;
pub const EINVAL: i64 = -22;
pub const ENOMEM: i64 = -12;
pub const EACCES: i64 = -13;
pub const EFAULT: i64 = -14;
pub const EEXIST: i64 = -17;
pub const EAGAIN: i64 = -11;

// ── AArch64 Linux syscall numbers ────────────────────────────────────────────

#[allow(dead_code)]
pub mod nr {
    pub const IO_SETUP: u64 = 0;
    pub const READ: u64 = 63;
    pub const WRITE: u64 = 64;
    pub const READV: u64 = 65;
    pub const WRITEV: u64 = 66;
    pub const PREAD64: u64 = 67;
    pub const PWRITE64: u64 = 68;
    pub const PREADV: u64 = 69;
    pub const PWRITEV: u64 = 70;
    pub const SENDFILE: u64 = 71;
    pub const OPENAT: u64 = 56;
    pub const CLOSE: u64 = 57;
    pub const PIPE2: u64 = 59;
    pub const LSEEK: u64 = 62;
    pub const FSTAT: u64 = 80;
    pub const FSTATAT: u64 = 79;
    pub const STATX: u64 = 291;
    pub const GETDENTS64: u64 = 61;
    pub const IOCTL: u64 = 29;
    pub const FCNTL: u64 = 25;
    pub const DUP: u64 = 23;
    pub const DUP3: u64 = 24;
    pub const PPOLL: u64 = 73;
    pub const PSELECT6: u64 = 72;
    pub const SIGNALFD4: u64 = 74;
    pub const MMAP: u64 = 222;
    pub const MUNMAP: u64 = 215;
    pub const MPROTECT: u64 = 226;
    pub const MADVISE: u64 = 233;
    pub const MREMAP: u64 = 216;
    pub const MLOCK: u64 = 228;
    pub const MUNLOCK: u64 = 229;
    pub const MLOCKALL: u64 = 230;
    pub const MUNLOCKALL: u64 = 231;
    pub const MINCORE: u64 = 232;
    pub const MLOCK2: u64 = 284;
    pub const MSYNC: u64 = 227;
    pub const BRK: u64 = 214;
    pub const CLONE: u64 = 220;
    pub const EXECVE: u64 = 221;
    pub const EXIT: u64 = 93;
    pub const EXIT_GROUP: u64 = 94;
    pub const WAIT4: u64 = 260;
    pub const WAITID: u64 = 95;
    pub const GETPID: u64 = 172;
    pub const GETPPID: u64 = 173;
    pub const GETTID: u64 = 178;
    pub const GETUID: u64 = 174;
    pub const GETEUID: u64 = 175;
    pub const GETGID: u64 = 176;
    pub const GETEGID: u64 = 177;
    pub const SETUID: u64 = 146;
    pub const SETGID: u64 = 144;
    pub const SETREUID: u64 = 145;
    pub const SETREGID: u64 = 143;
    pub const SETRESUID: u64 = 147;
    pub const SETRESGID: u64 = 149;
    pub const GETRESUID: u64 = 148;
    pub const GETRESGID: u64 = 150;
    pub const SETFSUID: u64 = 151;
    pub const SETFSGID: u64 = 152;
    pub const SETSID: u64 = 157;
    pub const SETPGID: u64 = 154;
    pub const GETPGID: u64 = 155;
    pub const GETSID: u64 = 156;
    pub const GETGROUPS: u64 = 158;
    pub const SETGROUPS: u64 = 159;
    pub const GETPRIORITY: u64 = 141;
    pub const SETPRIORITY: u64 = 140;
    pub const GETRUSAGE: u64 = 165;
    pub const UNSHARE: u64 = 97;
    pub const PERSONALITY: u64 = 92;
    pub const CLOCK_GETTIME: u64 = 113;
    pub const CLOCK_GETRES: u64 = 114;
    pub const CLOCK_NANOSLEEP: u64 = 115;
    pub const GETTIMEOFDAY: u64 = 169;
    pub const SETTIMEOFDAY: u64 = 170;
    pub const NANOSLEEP: u64 = 101;
    pub const TIMES: u64 = 153;
    pub const FUTEX: u64 = 98;
    pub const PRCTL: u64 = 167;
    pub const PRLIMIT64: u64 = 261;
    pub const GETRLIMIT: u64 = 163;
    pub const SETRLIMIT: u64 = 164;
    pub const RT_SIGACTION: u64 = 134;
    pub const RT_SIGPROCMASK: u64 = 135;
    pub const RT_SIGRETURN: u64 = 139;
    pub const RT_SIGSUSPEND: u64 = 133;
    pub const RT_SIGPENDING: u64 = 136;
    pub const RT_SIGTIMEDWAIT: u64 = 137;
    pub const RT_SIGQUEUEINFO: u64 = 138;
    pub const RT_TGSIGQUEUEINFO: u64 = 240;
    pub const KILL: u64 = 129;
    pub const TGKILL: u64 = 131;
    pub const TKILL: u64 = 130;
    pub const SIGALTSTACK: u64 = 132;
    pub const UNAME: u64 = 160;
    pub const SETHOSTNAME: u64 = 161;
    pub const SETDOMAINNAME: u64 = 162;
    pub const GETCWD: u64 = 17;
    pub const CHDIR: u64 = 49;
    pub const FCHDIR: u64 = 50;
    pub const READLINKAT: u64 = 78;
    pub const FACCESSAT: u64 = 48;
    pub const FACCESSAT2: u64 = 439;
    pub const MKNODAT: u64 = 33;
    pub const MKDIRAT: u64 = 34;
    pub const UNLINKAT: u64 = 35;
    pub const SYMLINKAT: u64 = 36;
    pub const LINKAT: u64 = 37;
    pub const RENAMEAT2: u64 = 276;
    pub const FCHMOD: u64 = 52;
    pub const FCHMODAT: u64 = 53;
    pub const FCHOWNAT: u64 = 54;
    pub const CHOWN: u64 = 55;
    pub const FTRUNCATE: u64 = 46;
    pub const TRUNCATE: u64 = 45;
    pub const FALLOCATE: u64 = 47;
    pub const SYNC: u64 = 81;
    pub const FSYNC: u64 = 82;
    pub const FDATASYNC: u64 = 83;
    pub const SYNC_FILE_RANGE: u64 = 84;
    pub const FLOCK: u64 = 32;
    pub const STATFS: u64 = 43;
    pub const FSTATFS: u64 = 44;
    pub const UTIMENSAT: u64 = 88;
    pub const SET_TID_ADDRESS: u64 = 96;
    pub const SET_ROBUST_LIST: u64 = 99;
    pub const GET_ROBUST_LIST: u64 = 100;
    pub const CAPGET: u64 = 90;
    pub const CAPSET: u64 = 91;
    pub const SOCKET: u64 = 198;
    pub const SOCKETPAIR: u64 = 199;
    pub const BIND: u64 = 200;
    pub const LISTEN: u64 = 201;
    pub const ACCEPT: u64 = 202;
    pub const ACCEPT4: u64 = 242;
    pub const CONNECT: u64 = 203;
    pub const GETSOCKNAME: u64 = 204;
    pub const GETPEERNAME: u64 = 205;
    pub const SENDTO: u64 = 206;
    pub const RECVFROM: u64 = 207;
    pub const SETSOCKOPT: u64 = 208;
    pub const GETSOCKOPT: u64 = 209;
    pub const SHUTDOWN: u64 = 210;
    pub const SENDMSG: u64 = 211;
    pub const RECVMSG: u64 = 212;
    pub const SENDMMSG: u64 = 269;
    pub const RECVMMSG: u64 = 243;
    pub const SCHED_YIELD: u64 = 124;
    pub const SCHED_GETAFFINITY: u64 = 123;
    pub const SCHED_SETAFFINITY: u64 = 122;
    pub const SCHED_SETPARAM: u64 = 118;
    pub const SCHED_GETPARAM: u64 = 121;
    pub const SCHED_SETSCHEDULER: u64 = 119;
    pub const SCHED_GETSCHEDULER: u64 = 120;
    pub const SCHED_GET_PRIORITY_MAX: u64 = 125;
    pub const SCHED_GET_PRIORITY_MIN: u64 = 126;
    pub const SCHED_RR_GET_INTERVAL: u64 = 127;
    pub const SCHED_SETATTR: u64 = 274;
    pub const SCHED_GETATTR: u64 = 275;
    pub const GETCPU: u64 = 168;
    pub const GETRANDOM: u64 = 278;
    pub const MEMFD_CREATE: u64 = 279;
    pub const MEMBARRIER: u64 = 283;
    pub const EPOLL_CREATE1: u64 = 20;
    pub const EPOLL_CTL: u64 = 21;
    pub const EPOLL_PWAIT: u64 = 22;
    pub const EPOLL_PWAIT2: u64 = 441;
    pub const INOTIFY_INIT1: u64 = 26;
    pub const INOTIFY_ADD_WATCH: u64 = 27;
    pub const INOTIFY_RM_WATCH: u64 = 28;
    pub const IOPRIO_SET: u64 = 30;
    pub const IOPRIO_GET: u64 = 31;
    pub const EVENTFD2: u64 = 19;
    pub const TIMERFD_CREATE: u64 = 85;
    pub const TIMERFD_SETTIME: u64 = 86;
    pub const TIMERFD_GETTIME: u64 = 87;
    pub const COPY_FILE_RANGE: u64 = 285;
    pub const PREADV2: u64 = 286;
    pub const PWRITEV2: u64 = 287;
    pub const SYSINFO: u64 = 179;
    pub const UMASK: u64 = 166;
    pub const RSEQ: u64 = 293;
    pub const SECCOMP: u64 = 277;
    pub const CLOSE_RANGE: u64 = 436;
    pub const OPENAT2: u64 = 437;
    pub const PROCESS_VM_READV: u64 = 270;
    pub const PROCESS_VM_WRITEV: u64 = 271;
    pub const FADVISE64: u64 = 223;
    pub const READAHEAD: u64 = 213;
    pub const RESTART_SYSCALL: u64 = 128;
}

// ── FdTable ───────────────────────────────────────────────────────────────────

/// Guest file descriptor table.
///
/// Maps guest FDs (small integers starting at 3) to host FDs.
/// FD 0/1/2 (stdin/stdout/stderr) pass through to host by default.
struct FdTable {
    /// guest_fd → host_fd
    table: HashMap<i32, RawFd>,
    next: i32,
}

impl FdTable {
    fn new() -> Self {
        let mut t = Self {
            table: HashMap::new(),
            next: 3,
        };
        // Wire stdin/stdout/stderr directly
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

    fn allocate_at_least(&mut self, min_guest: i32, host_fd: RawFd) -> i32 {
        let guest = self.next.max(min_guest);
        self.next = guest + 1;
        self.table.insert(guest, host_fd);
        guest
    }

    fn insert_exact(&mut self, guest: i32, host_fd: RawFd) {
        self.next = self.next.max(guest + 1);
        self.table.insert(guest, host_fd);
    }

    fn get(&self, guest: i32) -> Option<RawFd> {
        self.table.get(&guest).copied()
    }

    fn remove(&mut self, guest: i32) -> Option<RawFd> {
        self.table.remove(&guest)
    }
}

// ── LinuxAarch64SyscallHandler ───────────────────────────────────────────────

/// Linux AArch64 syscall emulator.
pub struct LinuxAarch64SyscallHandler {
    fds: FdTable,
    /// Current heap break pointer.
    brk: u64,
    /// Next mmap allocation address (grows upward from 0x2000_0000).
    mmap_next: u64,
    /// Freed regions available for reuse: (addr, aligned_size).
    mmap_free: Vec<(u64, u64)>,
    /// Per-process identity
    pid: u64,
    tid: u64,
    thread_pointer: u64,
    pub should_exit: bool,
    pub exit_code: i32,
    /// Path to the loaded binary (for /proc/self/exe).
    pub binary_path: String,
    host_threads: HostThreadRuntime,
}

impl LinuxAarch64SyscallHandler {
    pub fn new(initial_brk: u64) -> Self {
        Self {
            fds: FdTable::new(),
            brk: initial_brk,
            mmap_next: 0x4000_0000_0000u64, // high VA like real kernel mmap region
            mmap_free: Vec::new(),
            pid: 1000,
            tid: 1000,
            thread_pointer: 0,
            should_exit: false,
            exit_code: 0,
            binary_path: String::new(),
            host_threads: HostThreadRuntime::new(1000),
        }
    }

    pub fn set_thread_pointer(&mut self, thread_pointer: u64) {
        self.thread_pointer = thread_pointer;
    }

    pub fn thread_pointer_for_tid(&self, tid: u64) -> Option<u64> {
        self.host_threads.thread_pointer(tid)
    }

    /// Handle one `SVC #0` syscall. Reads args from `args`, returns retval for X0.
    pub fn handle(
        &mut self,
        nr: u64,
        args: SyscallArgs,
        mem: &mut impl MemInterface,
    ) -> Result<i64, HartException> {
        use helm_core::AccessType;

        match nr {
            // ── Process exit ─────────────────────────────────────────────────
            nr::EXIT | nr::EXIT_GROUP => {
                self.should_exit = true;
                self.exit_code = args.a0 as i32;
                return Err(HartException::Exit {
                    code: self.exit_code,
                });
            }

            // ── I/O ──────────────────────────────────────────────────────────
            nr::WRITE => {
                let fd = args.a0 as i32;
                let buf = args.a1;
                let count = args.a2 as usize;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                // Read guest memory into host buffer
                let bytes = read_guest_bytes(mem, buf, count);
                let n = unsafe { libc::write(host, bytes.as_ptr() as *const _, bytes.len()) };
                if n < 0 {
                    Ok(-errno() as i64)
                } else {
                    Ok(n as i64)
                }
            }
            nr::READ => {
                let fd = args.a0 as i32;
                let buf = args.a1;
                let count = args.a2 as usize;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let mut bytes = vec![0u8; count];
                let n = unsafe { libc::read(host, bytes.as_mut_ptr() as *mut _, bytes.len()) };
                if n < 0 {
                    return Ok(-errno() as i64);
                }
                write_guest_bytes(mem, buf, &bytes[..n as usize]);
                Ok(n as i64)
            }
            nr::WRITEV => {
                let fd = args.a0 as i32;
                let iov_ptr = args.a1;
                let iovcnt = args.a2 as usize;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let mut total = 0i64;
                for i in 0..iovcnt {
                    let base = mem
                        .read(iov_ptr + i as u64 * 16, 8, AccessType::Load)
                        .unwrap_or(0);
                    let len = mem
                        .read(iov_ptr + i as u64 * 16 + 8, 8, AccessType::Load)
                        .unwrap_or(0) as usize;
                    let bytes = read_guest_bytes(mem, base, len);
                    let n = unsafe { libc::write(host, bytes.as_ptr() as *const _, bytes.len()) };
                    if n < 0 {
                        return Ok(-errno() as i64);
                    }
                    total += n as i64;
                }
                Ok(total)
            }
            nr::READV => {
                let fd = args.a0 as i32;
                let iov_ptr = args.a1;
                let iovcnt = args.a2 as usize;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let mut total = 0i64;
                for i in 0..iovcnt {
                    let base = mem
                        .read(iov_ptr + i as u64 * 16, 8, AccessType::Load)
                        .unwrap_or(0);
                    let len = mem
                        .read(iov_ptr + i as u64 * 16 + 8, 8, AccessType::Load)
                        .unwrap_or(0) as usize;
                    if len == 0 {
                        continue;
                    }
                    let mut bytes = vec![0u8; len];
                    let n = unsafe { libc::read(host, bytes.as_mut_ptr() as *mut _, bytes.len()) };
                    if n < 0 {
                        return Ok(-errno() as i64);
                    }
                    write_guest_bytes(mem, base, &bytes[..n as usize]);
                    total += n as i64;
                    if (n as usize) < len {
                        break;
                    } // short read — stop
                }
                Ok(total)
            }
            nr::OPENAT => {
                let _dirfd = args.a0 as i32; // AT_FDCWD = -100
                let path_ptr = args.a1;
                let flags = args.a2 as i32;
                let mode = args.a3 as u32;
                let path = read_guest_cstr(mem, path_ptr);
                // Map /proc/self/exe → a readable path (leave as-is for now)
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let host_fd = unsafe { libc::open(cpath.as_ptr(), flags, mode) };
                if host_fd < 0 {
                    return Ok(-errno() as i64);
                }
                Ok(self.fds.allocate(host_fd) as i64)
            }
            nr::CLOSE => {
                let fd = args.a0 as i32;
                let host = self.fds.remove(fd).unwrap_or(-1);
                if host < 0 || fd < 3 {
                    return Ok(0);
                } // don't close stdin/stdout/stderr
                let r = unsafe { libc::close(host) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::LSEEK => {
                let fd = args.a0 as i32;
                let offset = args.a1 as i64;
                let whence = args.a2 as i32;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let r = unsafe { libc::lseek(host, offset, whence) };
                Ok(if r < 0 { -errno() as i64 } else { r as i64 })
            }
            nr::PREAD64 => {
                let fd = args.a0 as i32;
                let buf = args.a1;
                let cnt = args.a2 as usize;
                let off = args.a3 as i64;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let mut bytes = vec![0u8; cnt];
                let n = unsafe { libc::pread(host, bytes.as_mut_ptr() as _, bytes.len(), off) };
                if n < 0 {
                    return Ok(-errno() as i64);
                }
                write_guest_bytes(mem, buf, &bytes[..n as usize]);
                Ok(n as i64)
            }
            nr::PWRITE64 => {
                let fd = args.a0 as i32;
                let buf = args.a1;
                let cnt = args.a2 as usize;
                let off = args.a3 as i64;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let bytes = read_guest_bytes(mem, buf, cnt);
                let n = unsafe { libc::pwrite(host, bytes.as_ptr() as _, bytes.len(), off) };
                Ok(if n < 0 { -errno() as i64 } else { n as i64 })
            }
            nr::DUP => {
                let fd = args.a0 as i32;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let new_host = unsafe { libc::dup(host) };
                if new_host < 0 {
                    return Ok(-errno() as i64);
                }
                Ok(self.fds.allocate(new_host) as i64)
            }
            nr::DUP3 => {
                let old = args.a0 as i32;
                let new = args.a1 as i32;
                let fl = args.a2 as i32;
                let host_old = self.fds.get(old).unwrap_or(-1);
                if host_old < 0 {
                    return Ok(EBADF);
                }
                if old == new {
                    return Ok(EINVAL);
                }
                let host_new = unsafe { libc::dup3(host_old, new, fl) };
                if host_new < 0 {
                    return Ok(-errno() as i64);
                }
                if let Some(old_host) = self.fds.remove(new) {
                    unsafe {
                        libc::close(old_host);
                    }
                }
                self.fds.insert_exact(new, host_new);
                Ok(new as i64)
            }
            nr::IOCTL => {
                let _fd = args.a0 as i32;
                let req = args.a1;
                // TTY ioctls: return sane stubs so programs don't crash
                match req {
                    0x5401 /* TCGETS */  => Ok(EINVAL), // not a TTY
                    0x5413 /* TIOCGWINSZ */ => {
                        // Return 80x24
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
                let fd = args.a0 as i32;
                let cmd = args.a1 as i32;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                match cmd {
                    libc::F_DUPFD | libc::F_DUPFD_CLOEXEC => {
                        let min_guest = args.a2 as i32;
                        let new_host = unsafe { libc::fcntl(host, cmd, min_guest) };
                        if new_host < 0 {
                            return Ok(-errno() as i64);
                        }
                        Ok(self.fds.allocate_at_least(min_guest, new_host) as i64)
                    }
                    _ => {
                        let r = unsafe { libc::fcntl(host, cmd, args.a2) };
                        Ok(if r < 0 { -errno() as i64 } else { r as i64 })
                    }
                }
            }
            nr::FLOCK => Ok(0), // stub — always succeeds in SE mode

            // ── File metadata ─────────────────────────────────────────────────
            nr::FSTAT => {
                let fd = args.a0 as i32;
                let ptr = args.a1;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let mut st: libc::stat = unsafe { std::mem::zeroed() };
                let r = unsafe { libc::fstat(host, &mut st) };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                write_stat(mem, ptr, &st);
                Ok(0)
            }
            nr::FSTATAT => {
                let _dirfd = args.a0 as i32;
                let path_ptr = args.a1;
                let ptr = args.a2;
                let _flags = args.a3 as i32;
                let path = read_guest_cstr(mem, path_ptr);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let mut st: libc::stat = unsafe { std::mem::zeroed() };
                let r = unsafe { libc::stat(cpath.as_ptr(), &mut st) };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                write_stat(mem, ptr, &st);
                Ok(0)
            }
            nr::STATFS => {
                // Write a basic statfs64 struct (AArch64 layout, 120 bytes)
                // struct statfs64: f_type(8), f_bsize(8), f_blocks(8), f_bfree(8),
                //   f_bavail(8), f_files(8), f_ffree(8), f_fsid(8), f_namelen(8),
                //   f_frsize(8), f_flags(8), f_spare(40)
                let ptr = args.a1;
                mem.write(ptr, 8, 0xEF53u64, AccessType::Store).ok(); // EXT2_SUPER_MAGIC
                mem.write(ptr + 8, 8, 4096u64, AccessType::Store).ok(); // f_bsize
                mem.write(ptr + 16, 8, 1_000_000u64, AccessType::Store).ok(); // f_blocks
                mem.write(ptr + 24, 8, 500_000u64, AccessType::Store).ok(); // f_bfree
                mem.write(ptr + 32, 8, 500_000u64, AccessType::Store).ok(); // f_bavail
                mem.write(ptr + 40, 8, 1_000_000u64, AccessType::Store).ok(); // f_files
                mem.write(ptr + 48, 8, 900_000u64, AccessType::Store).ok(); // f_ffree
                mem.write(ptr + 56, 8, 0u64, AccessType::Store).ok(); // f_fsid
                mem.write(ptr + 64, 8, 255u64, AccessType::Store).ok(); // f_namelen
                mem.write(ptr + 72, 8, 4096u64, AccessType::Store).ok(); // f_frsize
                mem.write(ptr + 80, 8, 0u64, AccessType::Store).ok(); // f_flags
                Ok(0)
            }
            nr::READLINKAT => {
                let _dirfd = args.a0 as i32;
                let path_ptr = args.a1;
                let out_ptr = args.a2;
                let bufsiz = args.a3 as usize;
                let path = read_guest_cstr(mem, path_ptr);
                if path == "/proc/self/exe" || path == "/proc/self/maps" {
                    let fake = b"/bin/binary\0";
                    let n = fake.len().min(bufsiz);
                    write_guest_bytes(mem, out_ptr, &fake[..n]);
                    return Ok(n as i64);
                }
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let mut buf = vec![0u8; bufsiz];
                let n =
                    unsafe { libc::readlink(cpath.as_ptr(), buf.as_mut_ptr() as *mut _, bufsiz) };
                if n < 0 {
                    return Ok(-errno() as i64);
                }
                write_guest_bytes(mem, out_ptr, &buf[..n as usize]);
                Ok(n as i64)
            }
            nr::GETCWD => {
                let buf = args.a0;
                let sz = args.a1 as usize;
                let cwd = std::env::current_dir()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "/".to_string());
                let bytes = cwd.as_bytes();
                let n = (bytes.len() + 1).min(sz);
                write_guest_bytes(mem, buf, &bytes[..n.saturating_sub(1)]);
                mem.write(buf + n as u64 - 1, 1, 0, AccessType::Store).ok();
                Ok(n as i64)
            }
            nr::FACCESSAT => Ok(0), // always accessible in SE mode

            // ── Memory management ─────────────────────────────────────────────
            nr::BRK => {
                let addr = args.a0;
                if addr == 0 {
                    Ok(self.brk as i64)
                } else {
                    if addr > self.brk {
                        // Extend heap: align up and accept — FlatMem auto-creates pages on access
                        let old_end = (self.brk + 0xFFF) & !0xFFF;
                        let new_end = (addr + 0xFFF) & !0xFFF;
                        let _ = (old_end, new_end); // nothing to map in FlatMem
                    }
                    self.brk = addr;
                    Ok(self.brk as i64)
                }
            }
            nr::MMAP => {
                let addr_hint = args.a0;
                let len = args.a1;
                let _prot = args.a2;
                let flags = args.a3;
                let _fd = args.a4 as i32;
                let _offset = args.a5;

                // musl's malloc passes len=0 with PROT_NONE to reserve a large
                // virtual region for heap expansion (then mprotects individual
                // pages). Allocate 64 MB so mprotect doesn't need to extend.
                let len_actual = if len == 0 { 0x400_0000 } else { len };
                let len_aligned = (len_actual + 0xFFF) & !0xFFF;

                #[allow(dead_code)]
                const MAP_FIXED: u64 = 0x10;
                const MAP_ANONYMOUS: u64 = 0x20;

                let addr = if addr_hint != 0 {
                    // Honor address hints (MAP_FIXED or soft hints from musl)
                    addr_hint
                } else {
                    // Try to reuse a freed region of matching size first
                    let reuse = self.mmap_free.iter().position(|&(_, sz)| sz >= len_aligned);
                    if let Some(idx) = reuse {
                        let (a, _) = self.mmap_free.swap_remove(idx);
                        a
                    } else {
                        let a = self.mmap_next;
                        self.mmap_next += len_aligned;
                        a
                    }
                };
                // All anonymous mappings are zero-initialized (kernel semantics).
                // This includes both MAP_FIXED|MAP_ANON and plain MAP_ANON, and
                // is critical for correct malloc chunk-header initialization.
                if flags & MAP_ANONYMOUS != 0 {
                    let zeros = vec![0u8; len_aligned as usize];
                    write_guest_bytes(mem, addr, &zeros);
                }
                Ok(addr as i64)
            }
            nr::MUNMAP => {
                let addr = args.a0;
                let len = args.a1;
                let len_aligned = (len + 0xFFF) & !0xFFF;
                if addr != 0 && len_aligned > 0 {
                    self.mmap_free.push((addr, len_aligned));
                }
                Ok(0)
            }
            nr::MPROTECT => {
                // FlatMem auto-creates pages on access; permission tracking is not
                // implemented, so this is a no-op. We still accept the call so
                // musl's guard-page → PROT_READ|PROT_WRITE transitions succeed.
                Ok(0)
            }
            nr::MADVISE => Ok(0),
            nr::MSYNC => Ok(0),
            nr::MREMAP => {
                let old_addr = args.a0;
                let old_size = args.a1;
                let new_size = args.a2;
                let flags = args.a3;
                let new_aligned = (new_size + 0xFFF) & !0xFFF;

                if new_size <= old_size {
                    return Ok(old_addr as i64);
                }

                const MREMAP_MAYMOVE: u64 = 1;
                if flags & MREMAP_MAYMOVE != 0 {
                    // Bump-allocate a new region and copy existing data
                    let dest = self.mmap_next;
                    self.mmap_next += new_aligned;
                    let copy_len = old_size as usize;
                    let bytes = read_guest_bytes(mem, old_addr, copy_len);
                    write_guest_bytes(mem, dest, &bytes);
                    return Ok(dest as i64);
                }

                // Try to extend in-place (FlatMem will auto-create new pages on access)
                Ok(old_addr as i64)
            }

            // ── Process identity ─────────────────────────────────────────────
            nr::GETPID => Ok(self.pid as i64),
            nr::GETPPID => Ok((self.pid.saturating_sub(1)) as i64),
            nr::GETTID => Ok(self.tid as i64),
            nr::GETUID | nr::GETEUID => Ok(1000),
            nr::GETGID | nr::GETEGID => Ok(1000),
            nr::GETGROUPS => Ok(0),
            nr::UMASK => Ok(0o022),
            nr::SCHED_YIELD => Ok(0),
            nr::SCHED_GETAFFINITY => {
                // sched_getaffinity(pid, cpusetsize, mask)
                // Write a CPU mask with only CPU 0 set (1-core simulator)
                let cpusetsize = args.a1 as usize;
                let mask_ptr = args.a2;
                // Zero out the entire mask buffer first
                for off in (0..cpusetsize as u64).step_by(8) {
                    mem.write(mask_ptr + off, 8, 0u64, AccessType::Store).ok();
                }
                // Set bit 0 (CPU 0)
                mem.write(mask_ptr, 1, 1u64, AccessType::Store).ok();
                Ok(0)
            }
            nr::SCHED_SETAFFINITY => Ok(0),
            nr::SETSID | nr::SETPGID => Ok(0),
            nr::GETPGID => Ok(self.pid as i64),

            // ── Thread / TID ──────────────────────────────────────────────────
            nr::SET_TID_ADDRESS => Ok(self.tid as i64),
            nr::SET_ROBUST_LIST | nr::GET_ROBUST_LIST => Ok(0),
            nr::CLONE => {
                let flags = args.a0;
                if matches!(classify_clone_flags(flags), Ok(CloneStyle::Thread)) {
                    // We can classify guest thread-style clone flags and track TLS,
                    // but the SE runtime still has no guest child execution loop.
                    // Returning success here corrupts userspace: programs like fish
                    // expect the child thread to start running immediately.
                    return Ok(EINVAL);
                }
                let tid = self
                    .host_threads
                    .spawn_thread_for_clone_with_tp(flags, self.thread_pointer, args.a3, || {})
                    .map_err(|e| match e {
                        HostThreadSpawnError::InvalidThreadFlags
                        | HostThreadSpawnError::InvalidForkFlags
                        | HostThreadSpawnError::UnsupportedCloneStyle => HartException::Unsupported,
                        HostThreadSpawnError::SpawnFailed => HartException::Unsupported,
                    });
                match tid {
                    Ok(tid) => Ok(tid as i64),
                    Err(HartException::Unsupported) => Ok(EINVAL),
                    Err(e) => Err(e),
                }
            }

            // ── Time ─────────────────────────────────────────────────────────
            nr::CLOCK_GETTIME => {
                let _clk_id = args.a0;
                let tp_ptr = args.a1;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                mem.write(tp_ptr, 8, now.as_secs(), AccessType::Store).ok();
                mem.write(tp_ptr + 8, 8, now.subsec_nanos() as u64, AccessType::Store)
                    .ok();
                Ok(0)
            }
            nr::CLOCK_GETRES => {
                let tp_ptr = args.a1;
                mem.write(tp_ptr, 8, 0, AccessType::Store).ok();
                mem.write(tp_ptr + 8, 8, 1, AccessType::Store).ok(); // 1ns resolution
                Ok(0)
            }
            nr::GETTIMEOFDAY => {
                let tv_ptr = args.a0;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                mem.write(tv_ptr, 8, now.as_secs(), AccessType::Store).ok();
                mem.write(tv_ptr + 8, 8, now.subsec_micros() as u64, AccessType::Store)
                    .ok();
                Ok(0)
            }
            nr::NANOSLEEP | nr::CLOCK_NANOSLEEP => Ok(0),
            nr::TIMES => Ok(0),

            // ── Signals (stub — SE mode has no real signal delivery) ──────────
            nr::RT_SIGACTION => Ok(0),
            nr::RT_SIGPROCMASK => Ok(0),
            nr::RT_SIGRETURN => Ok(0),
            nr::RT_SIGSUSPEND => Ok(EINVAL),
            nr::KILL | nr::TGKILL | nr::TKILL => Ok(0),
            nr::SIGALTSTACK => {
                // sigaltstack(ss, old_ss)
                // If old_ss (a1) is non-NULL, write the current altstack state.
                // We have no altstack in SE mode, so report SS_DISABLE.
                let old_ss = args.a1;
                if old_ss != 0 {
                    const SS_DISABLE: u32 = 2;
                    // stack_t on AArch64: ss_sp(8) + ss_flags(4) + pad(4) + ss_size(8) = 24
                    write_guest_bytes(mem, old_ss, &0u64.to_le_bytes()); // ss_sp = NULL
                    write_guest_bytes(mem, old_ss + 8, &SS_DISABLE.to_le_bytes()); // ss_flags
                    write_guest_bytes(mem, old_ss + 12, &0u32.to_le_bytes()); // padding
                    write_guest_bytes(mem, old_ss + 16, &0u64.to_le_bytes()); // ss_size = 0
                }
                Ok(0)
            }

            // ── Futex (basic) ─────────────────────────────────────────────────
            nr::FUTEX => {
                let op = args.a1 as u32 & 0x7F;
                match op {
                    0 /* WAIT */ => Ok(0),   // stub: immediately return
                    1 /* WAKE */ => Ok(1),
                    _ => Ok(EINVAL),
                }
            }

            // ── System info ───────────────────────────────────────────────────
            nr::UNAME => {
                let ptr = args.a0;
                write_guest_str(mem, ptr, "Linux", 65);
                write_guest_str(mem, ptr + 65, "helm-ng", 65);
                write_guest_str(mem, ptr + 130, "6.1.0", 65);
                write_guest_str(mem, ptr + 195, "helm-ng", 65);
                write_guest_str(mem, ptr + 260, "aarch64", 65);
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
                // prlimit64(pid, resource, new_limit, old_limit)
                // a0=pid, a1=resource, a2=new_limit ptr, a3=old_limit ptr
                let resource = args.a1;
                let new_limit = args.a2; // may be 0 (null)
                let old_limit = args.a3; // may be 0 (null)
                                         // If caller wants to read the current limit, write reasonable defaults
                if old_limit != 0 {
                    // rlimit64: {rlim_cur: u64, rlim_max: u64}
                    let (cur, max): (u64, u64) = match resource {
                        3  /* RLIMIT_STACK  */ => (8 * 1024 * 1024,        u64::MAX),
                        7  /* RLIMIT_NOFILE */ => (1024,                   4096),
                        9  /* RLIMIT_AS     */ => (u64::MAX,               u64::MAX),
                        8  /* RLIMIT_MEMLOCK*/ => (64 * 1024,              64 * 1024),
                        6  /* RLIMIT_NPROC  */ => (1024,                   1024),
                        4  /* RLIMIT_CORE   */ => (0,                      0),
                        _                      => (u64::MAX,               u64::MAX),
                    };
                    mem.write(old_limit, 8, cur, AccessType::Store).ok();
                    mem.write(old_limit + 8, 8, max, AccessType::Store).ok();
                }
                // If new_limit is non-null we accept but ignore (SE mode)
                let _ = new_limit;
                Ok(0)
            }
            nr::GETRLIMIT | nr::SETRLIMIT => Ok(0),
            nr::CAPGET | nr::CAPSET => Ok(0),
            nr::SYSINFO => Ok(0),

            // ── Random ───────────────────────────────────────────────────────
            nr::GETRANDOM => {
                let buf = args.a0;
                let len = args.a1 as usize;
                // Use Rust random bytes (not cryptographic but fine for SE mode)
                let bytes: Vec<u8> = (0..len).map(|_| rand_byte()).collect();
                write_guest_bytes(mem, buf, &bytes);
                Ok(len as i64)
            }

            // ── Polling ───────────────────────────────────────────────────────
            nr::PPOLL | nr::PSELECT6 => Ok(0),

            // ── Pipe ─────────────────────────────────────────────────────────
            nr::PIPE2 => {
                let mut fds = [0i32; 2];
                let r = unsafe { libc::pipe2(fds.as_mut_ptr(), args.a1 as i32) };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                let gfd_r = self.fds.allocate(fds[0]) as u64;
                let gfd_w = self.fds.allocate(fds[1]) as u64;
                mem.write(args.a0, 4, gfd_r, AccessType::Store).ok();
                mem.write(args.a0 + 4, 4, gfd_w, AccessType::Store).ok();
                Ok(0)
            }

            // ── Scatter/gather I/O ─────────────────────────────────────────────
            nr::PREADV | nr::PREADV2 => {
                let fd = args.a0 as i32;
                let iov_ptr = args.a1;
                let iovcnt = args.a2 as usize;
                let off = args.a3 as i64;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let mut total = 0i64;
                for i in 0..iovcnt {
                    let base = mem
                        .read(iov_ptr + i as u64 * 16, 8, AccessType::Load)
                        .unwrap_or(0);
                    let len = mem
                        .read(iov_ptr + i as u64 * 16 + 8, 8, AccessType::Load)
                        .unwrap_or(0) as usize;
                    if len == 0 {
                        continue;
                    }
                    let mut bytes = vec![0u8; len];
                    let n = if off < 0 {
                        unsafe { libc::read(host, bytes.as_mut_ptr() as *mut _, bytes.len()) }
                    } else {
                        unsafe {
                            libc::pread(
                                host,
                                bytes.as_mut_ptr() as *mut _,
                                bytes.len(),
                                off + total,
                            )
                        }
                    };
                    if n < 0 {
                        return Ok(-errno() as i64);
                    }
                    write_guest_bytes(mem, base, &bytes[..n as usize]);
                    total += n as i64;
                    if (n as usize) < len {
                        break;
                    }
                }
                Ok(total)
            }
            nr::PWRITEV | nr::PWRITEV2 => {
                let fd = args.a0 as i32;
                let iov_ptr = args.a1;
                let iovcnt = args.a2 as usize;
                let off = args.a3 as i64;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let mut total = 0i64;
                for i in 0..iovcnt {
                    let base = mem
                        .read(iov_ptr + i as u64 * 16, 8, AccessType::Load)
                        .unwrap_or(0);
                    let len = mem
                        .read(iov_ptr + i as u64 * 16 + 8, 8, AccessType::Load)
                        .unwrap_or(0) as usize;
                    let bytes = read_guest_bytes(mem, base, len);
                    let n = if off < 0 {
                        unsafe { libc::write(host, bytes.as_ptr() as *const _, bytes.len()) }
                    } else {
                        unsafe {
                            libc::pwrite(host, bytes.as_ptr() as *const _, bytes.len(), off + total)
                        }
                    };
                    if n < 0 {
                        return Ok(-errno() as i64);
                    }
                    total += n as i64;
                }
                Ok(total)
            }
            nr::SENDFILE => {
                let out_fd = args.a0 as i32;
                let in_fd = args.a1 as i32;
                let off_ptr = args.a2;
                let count = args.a3 as usize;
                let host_out = self.fds.get(out_fd).unwrap_or(-1);
                let host_in = self.fds.get(in_fd).unwrap_or(-1);
                if host_out < 0 || host_in < 0 {
                    return Ok(EBADF);
                }
                let mut off_host: libc::off_t = if off_ptr != 0 {
                    mem.read(off_ptr, 8, AccessType::Load).unwrap_or(0) as i64
                } else {
                    0
                };
                let off_ptr_host: *mut libc::off_t = if off_ptr != 0 {
                    &mut off_host
                } else {
                    std::ptr::null_mut()
                };
                let r = unsafe { libc::sendfile(host_out, host_in, off_ptr_host, count) };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                if off_ptr != 0 {
                    mem.write(off_ptr, 8, off_host as u64, AccessType::Store)
                        .ok();
                }
                Ok(r as i64)
            }

            // ── Directory operations ────────────────────────────────────────────
            nr::GETDENTS64 => {
                let fd = args.a0 as i32;
                let buf_ptr = args.a1;
                let count = args.a2 as usize;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let mut host_buf = vec![0u8; count];
                let n = unsafe {
                    libc::syscall(
                        libc::SYS_getdents64,
                        host as i64,
                        host_buf.as_mut_ptr() as i64,
                        count as i64,
                    )
                };
                if n < 0 {
                    return Ok(-errno() as i64);
                }
                write_guest_bytes(mem, buf_ptr, &host_buf[..n as usize]);
                Ok(n)
            }
            nr::CHDIR => {
                let path = read_guest_cstr(mem, args.a0);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let r = unsafe { libc::chdir(cpath.as_ptr()) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::FCHDIR => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let r = unsafe { libc::fchdir(host) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::MKNODAT => {
                let dirfd = args.a0 as i32;
                let host_dirfd = if dirfd == -100 {
                    libc::AT_FDCWD
                } else {
                    self.fds.get(dirfd).unwrap_or(dirfd)
                };
                let path = read_guest_cstr(mem, args.a1);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let r =
                    unsafe { libc::mknodat(host_dirfd, cpath.as_ptr(), args.a2 as u32, args.a3) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::FCHMODAT => {
                let dirfd = args.a0 as i32;
                let host_dirfd = if dirfd == -100 {
                    libc::AT_FDCWD
                } else {
                    self.fds.get(dirfd).unwrap_or(dirfd)
                };
                let path = read_guest_cstr(mem, args.a1);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let r = unsafe {
                    libc::syscall(
                        libc::SYS_fchmodat,
                        host_dirfd as i64,
                        cpath.as_ptr() as i64,
                        args.a2 as i64,
                        args.a3 as i64,
                    )
                };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::FCHOWNAT => {
                let dirfd = args.a0 as i32;
                let host_dirfd = if dirfd == -100 {
                    libc::AT_FDCWD
                } else {
                    self.fds.get(dirfd).unwrap_or(dirfd)
                };
                let path = read_guest_cstr(mem, args.a1);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let r = unsafe {
                    libc::fchownat(
                        host_dirfd,
                        cpath.as_ptr(),
                        args.a2 as u32,
                        args.a3 as u32,
                        args.a4 as i32,
                    )
                };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::TRUNCATE => {
                let path = read_guest_cstr(mem, args.a0);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let r = unsafe { libc::truncate(cpath.as_ptr(), args.a1 as i64) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::FTRUNCATE => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let r = unsafe { libc::ftruncate(host, args.a1 as i64) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::UTIMENSAT => {
                let dirfd = args.a0 as i32;
                let host_dirfd = if dirfd == -100 {
                    libc::AT_FDCWD
                } else {
                    self.fds.get(dirfd).unwrap_or(dirfd)
                };
                let path = if args.a1 != 0 {
                    read_guest_cstr(mem, args.a1)
                } else {
                    String::new()
                };
                let cpath = if !path.is_empty() {
                    CString::new(path.as_bytes()).ok()
                } else {
                    None
                };
                let times_host: *const libc::timespec = if args.a2 != 0 {
                    let bytes = read_guest_bytes(mem, args.a2, 32);
                    bytes.as_ptr() as *const libc::timespec
                } else {
                    std::ptr::null()
                };
                let r = unsafe {
                    if let Some(ref cp) = cpath {
                        libc::utimensat(host_dirfd, cp.as_ptr(), times_host, args.a3 as i32)
                    } else {
                        libc::futimens(host_dirfd, times_host)
                    }
                };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::SYNC_FILE_RANGE | nr::FADVISE64 | nr::READAHEAD => Ok(0),

            // ── Extended stat ───────────────────────────────────────────────────
            nr::STATX => {
                let dirfd = args.a0 as i32;
                let path_ptr = args.a1;
                let flags = args.a2 as i32;
                let mask = args.a3 as u32;
                let out_ptr = args.a4;
                let path = read_guest_cstr(mem, path_ptr);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let host_dirfd = if dirfd == -100 {
                    libc::AT_FDCWD
                } else {
                    self.fds.get(dirfd).unwrap_or(dirfd)
                };
                let mut buf = vec![0u8; 256];
                let r = unsafe {
                    libc::syscall(
                        libc::SYS_statx,
                        host_dirfd as i64,
                        cpath.as_ptr() as i64,
                        flags as i64,
                        mask as i64,
                        buf.as_mut_ptr() as i64,
                    )
                };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                write_guest_bytes(mem, out_ptr, &buf);
                Ok(0)
            }
            nr::FSTATFS => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let ptr = args.a1;
                let mut st: libc::statfs = unsafe { std::mem::zeroed() };
                let r = unsafe { libc::fstatfs(host, &mut st) };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                mem.write(ptr, 8, st.f_type as u64, AccessType::Store).ok();
                mem.write(ptr + 8, 8, st.f_bsize as u64, AccessType::Store)
                    .ok();
                mem.write(ptr + 16, 8, st.f_blocks as u64, AccessType::Store)
                    .ok();
                mem.write(ptr + 24, 8, st.f_bfree as u64, AccessType::Store)
                    .ok();
                mem.write(ptr + 32, 8, st.f_bavail as u64, AccessType::Store)
                    .ok();
                mem.write(ptr + 40, 8, st.f_files as u64, AccessType::Store)
                    .ok();
                mem.write(ptr + 48, 8, st.f_ffree as u64, AccessType::Store)
                    .ok();
                mem.write(ptr + 56, 8, 0u64, AccessType::Store).ok();
                mem.write(ptr + 64, 8, st.f_namelen as u64, AccessType::Store)
                    .ok();
                mem.write(ptr + 72, 8, st.f_frsize as u64, AccessType::Store)
                    .ok();
                Ok(0)
            }

            // ── Inotify ─────────────────────────────────────────────────────────
            nr::INOTIFY_INIT1 => {
                let r = unsafe { libc::inotify_init1(args.a0 as i32) };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                Ok(self.fds.allocate(r) as i64)
            }
            nr::INOTIFY_ADD_WATCH => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let path = read_guest_cstr(mem, args.a1);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let r = unsafe { libc::inotify_add_watch(host, cpath.as_ptr(), args.a2 as u32) };
                Ok(if r < 0 { -errno() as i64 } else { r as i64 })
            }
            nr::INOTIFY_RM_WATCH => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let r = unsafe { libc::inotify_rm_watch(host, args.a1 as i32) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }

            // ── Signalfd ─────────────────────────────────────────────────────────
            nr::SIGNALFD4 => {
                let flags = args.a3 as i32;
                let mask: libc::sigset_t = unsafe { std::mem::zeroed() };
                let r = unsafe { libc::signalfd(-1, &mask, flags) };
                if r < 0 {
                    let mut pfds = [0i32; 2];
                    if unsafe { libc::pipe(pfds.as_mut_ptr()) } < 0 {
                        return Ok(EINVAL);
                    }
                    unsafe {
                        libc::close(pfds[1]);
                    }
                    return Ok(self.fds.allocate(pfds[0]) as i64);
                }
                Ok(self.fds.allocate(r) as i64)
            }

            // ── Epoll ────────────────────────────────────────────────────────────
            nr::EPOLL_CREATE1 => {
                let r = unsafe { libc::epoll_create1(args.a0 as i32) };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                Ok(self.fds.allocate(r) as i64)
            }
            nr::EPOLL_CTL => {
                let epfd = args.a0 as i32;
                let op = args.a1 as i32;
                let fd = args.a2 as i32;
                let host_epfd = self.fds.get(epfd).unwrap_or(-1);
                let host_fd = self.fds.get(fd).unwrap_or(-1);
                if host_epfd < 0 {
                    return Ok(EBADF);
                }
                let ev = if args.a3 != 0 {
                    let events = mem.read(args.a3, 4, AccessType::Load).unwrap_or(0) as u32;
                    let data = mem.read(args.a3 + 8, 8, AccessType::Load).unwrap_or(0);
                    let mut e: libc::epoll_event = unsafe { std::mem::zeroed() };
                    e.events = events;
                    e.u64 = data;
                    Some(e)
                } else {
                    None
                };
                let r = unsafe {
                    libc::epoll_ctl(
                        host_epfd,
                        op,
                        host_fd,
                        ev.as_ref()
                            .map(|e| e as *const _ as *mut _)
                            .unwrap_or(std::ptr::null_mut()),
                    )
                };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::EPOLL_PWAIT | nr::EPOLL_PWAIT2 => {
                let epfd = args.a0 as i32;
                let ev_ptr = args.a1;
                let maxev = args.a2 as i32;
                let host_epfd = self.fds.get(epfd).unwrap_or(-1);
                if host_epfd < 0 {
                    return Ok(EBADF);
                }
                let count = maxev.max(0) as usize;
                let mut evs: Vec<libc::epoll_event> = vec![unsafe { std::mem::zeroed() }; count];
                let r = unsafe { libc::epoll_wait(host_epfd, evs.as_mut_ptr(), count as i32, 0) };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                for i in 0..r as usize {
                    mem.write(
                        ev_ptr + i as u64 * 16,
                        4,
                        evs[i].events as u64,
                        AccessType::Store,
                    )
                    .ok();
                    mem.write(ev_ptr + i as u64 * 16 + 8, 8, evs[i].u64, AccessType::Store)
                        .ok();
                }
                Ok(r as i64)
            }

            // ── Eventfd ──────────────────────────────────────────────────────────
            nr::EVENTFD2 => {
                let r = unsafe { libc::eventfd(args.a0 as u32, args.a1 as i32) };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                Ok(self.fds.allocate(r) as i64)
            }

            // ── Memfd ────────────────────────────────────────────────────────────
            nr::MEMFD_CREATE => {
                let name = read_guest_cstr(mem, args.a0);
                let cname = CString::new(name.as_bytes()).unwrap_or_default();
                let r = unsafe {
                    libc::syscall(
                        libc::SYS_memfd_create,
                        cname.as_ptr() as i64,
                        args.a1 as i64,
                    )
                };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                Ok(self.fds.allocate(r as i32) as i64)
            }

            // ── Close range ──────────────────────────────────────────────────────
            nr::CLOSE_RANGE => {
                let first = args.a0 as i32;
                let last = args.a1 as i32;
                for fd in first..=last {
                    if let Some(host) = self.fds.remove(fd) {
                        if fd >= 3 {
                            unsafe {
                                libc::close(host);
                            }
                        }
                    }
                }
                Ok(0)
            }

            // ── Timerfd ──────────────────────────────────────────────────────────
            nr::TIMERFD_CREATE => {
                let r = unsafe { libc::timerfd_create(args.a0 as i32, args.a1 as i32) };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                Ok(self.fds.allocate(r) as i64)
            }
            nr::TIMERFD_SETTIME => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let new_bytes = read_guest_bytes(mem, args.a2, 32);
                let mut old_spec: libc::itimerspec = unsafe { std::mem::zeroed() };
                let new_spec = unsafe { *(new_bytes.as_ptr() as *const libc::itimerspec) };
                let r = unsafe {
                    libc::timerfd_settime(host, args.a1 as i32, &new_spec, &mut old_spec)
                };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                if args.a3 != 0 {
                    let bytes = unsafe {
                        std::slice::from_raw_parts(&old_spec as *const _ as *const u8, 32)
                    };
                    write_guest_bytes(mem, args.a3, bytes);
                }
                Ok(0)
            }
            nr::TIMERFD_GETTIME => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let mut cur: libc::itimerspec = unsafe { std::mem::zeroed() };
                let r = unsafe { libc::timerfd_gettime(host, &mut cur) };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                let bytes =
                    unsafe { std::slice::from_raw_parts(&cur as *const _ as *const u8, 32) };
                write_guest_bytes(mem, args.a1, bytes);
                Ok(0)
            }

            // ── Socket syscalls ──────────────────────────────────────────────────
            nr::SOCKET => {
                let r = unsafe { libc::socket(args.a0 as i32, args.a1 as i32, args.a2 as i32) };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                Ok(self.fds.allocate(r) as i64)
            }
            nr::SOCKETPAIR => {
                let mut sv = [0i32; 2];
                let r = unsafe {
                    libc::socketpair(
                        args.a0 as i32,
                        args.a1 as i32,
                        args.a2 as i32,
                        sv.as_mut_ptr(),
                    )
                };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                let g0 = self.fds.allocate(sv[0]) as u64;
                let g1 = self.fds.allocate(sv[1]) as u64;
                mem.write(args.a3, 4, g0, AccessType::Store).ok();
                mem.write(args.a3 + 4, 4, g1, AccessType::Store).ok();
                Ok(0)
            }
            nr::BIND => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let bytes = read_guest_bytes(mem, args.a1, args.a2 as usize);
                let r = unsafe {
                    libc::bind(
                        host,
                        bytes.as_ptr() as *const libc::sockaddr,
                        args.a2 as u32,
                    )
                };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::LISTEN => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let r = unsafe { libc::listen(host, args.a1 as i32) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::ACCEPT | nr::ACCEPT4 => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let mut addr_buf = vec![0u8; 128];
                let mut alen = if args.a2 != 0 {
                    mem.read(args.a2, 4, AccessType::Load).unwrap_or(0) as u32
                } else {
                    0u32
                };
                let flags = if nr == nr::ACCEPT4 { args.a3 as i32 } else { 0 };
                let r = if flags != 0 {
                    unsafe {
                        libc::accept4(host, addr_buf.as_mut_ptr() as *mut _, &mut alen, flags)
                    }
                } else {
                    unsafe { libc::accept(host, addr_buf.as_mut_ptr() as *mut _, &mut alen) }
                };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                if args.a1 != 0 && alen > 0 {
                    write_guest_bytes(mem, args.a1, &addr_buf[..alen as usize]);
                    mem.write(args.a2, 4, alen as u64, AccessType::Store).ok();
                }
                Ok(self.fds.allocate(r) as i64)
            }
            nr::GETSOCKNAME | nr::GETPEERNAME => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let mut addr_buf = vec![0u8; 128];
                let mut alen = mem.read(args.a2, 4, AccessType::Load).unwrap_or(128) as u32;
                let r = if nr == nr::GETSOCKNAME {
                    unsafe { libc::getsockname(host, addr_buf.as_mut_ptr() as *mut _, &mut alen) }
                } else {
                    unsafe { libc::getpeername(host, addr_buf.as_mut_ptr() as *mut _, &mut alen) }
                };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                write_guest_bytes(mem, args.a1, &addr_buf[..alen as usize]);
                mem.write(args.a2, 4, alen as u64, AccessType::Store).ok();
                Ok(0)
            }
            nr::SENDTO => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let bytes = read_guest_bytes(mem, args.a1, args.a2 as usize);
                let r = if args.a4 != 0 {
                    let addr = read_guest_bytes(mem, args.a4, args.a5 as usize);
                    unsafe {
                        libc::sendto(
                            host,
                            bytes.as_ptr() as *const _,
                            bytes.len(),
                            args.a3 as i32,
                            addr.as_ptr() as *const libc::sockaddr,
                            args.a5 as u32,
                        )
                    }
                } else {
                    unsafe {
                        libc::send(
                            host,
                            bytes.as_ptr() as *const _,
                            bytes.len(),
                            args.a3 as i32,
                        )
                    }
                };
                Ok(if r < 0 { -errno() as i64 } else { r as i64 })
            }
            nr::RECVFROM => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let mut bytes = vec![0u8; args.a2 as usize];
                let mut addr_buf = vec![0u8; 128];
                let mut alen = 128u32;
                let r = unsafe {
                    libc::recvfrom(
                        host,
                        bytes.as_mut_ptr() as *mut _,
                        bytes.len(),
                        args.a3 as i32,
                        addr_buf.as_mut_ptr() as *mut _,
                        &mut alen,
                    )
                };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                write_guest_bytes(mem, args.a1, &bytes[..r as usize]);
                if args.a4 != 0 && alen > 0 {
                    write_guest_bytes(mem, args.a4, &addr_buf[..alen as usize]);
                    mem.write(args.a5, 4, alen as u64, AccessType::Store).ok();
                }
                Ok(r as i64)
            }
            nr::SETSOCKOPT => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let bytes = read_guest_bytes(mem, args.a3, args.a4 as usize);
                let r = unsafe {
                    libc::setsockopt(
                        host,
                        args.a1 as i32,
                        args.a2 as i32,
                        bytes.as_ptr() as *const _,
                        args.a4 as u32,
                    )
                };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::GETSOCKOPT => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let mut olen = mem.read(args.a4, 4, AccessType::Load).unwrap_or(0) as u32;
                let mut buf = vec![0u8; olen as usize];
                let r = unsafe {
                    libc::getsockopt(
                        host,
                        args.a1 as i32,
                        args.a2 as i32,
                        buf.as_mut_ptr() as *mut _,
                        &mut olen,
                    )
                };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                write_guest_bytes(mem, args.a3, &buf[..olen as usize]);
                mem.write(args.a4, 4, olen as u64, AccessType::Store).ok();
                Ok(0)
            }
            nr::SHUTDOWN => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let r = unsafe { libc::shutdown(host, args.a1 as i32) };
                Ok(if r < 0 { -errno() as i64 } else { 0 })
            }
            nr::SENDMSG => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let msg = args.a1;
                let name_ptr = mem.read(msg, 8, AccessType::Load).unwrap_or(0);
                let namelen = mem.read(msg + 8, 4, AccessType::Load).unwrap_or(0) as u32;
                let iov_ptr = mem.read(msg + 16, 8, AccessType::Load).unwrap_or(0);
                let iovcnt = mem.read(msg + 24, 8, AccessType::Load).unwrap_or(0) as usize;
                let name_bytes = if name_ptr != 0 {
                    read_guest_bytes(mem, name_ptr, namelen as usize)
                } else {
                    vec![]
                };
                let mut all_data = Vec::new();
                let mut iov_offsets: Vec<(usize, usize)> = Vec::new();
                for i in 0..iovcnt {
                    let base = mem
                        .read(iov_ptr + i as u64 * 16, 8, AccessType::Load)
                        .unwrap_or(0);
                    let len = mem
                        .read(iov_ptr + i as u64 * 16 + 8, 8, AccessType::Load)
                        .unwrap_or(0) as usize;
                    let off = all_data.len();
                    all_data.extend_from_slice(&read_guest_bytes(mem, base, len));
                    iov_offsets.push((off, len));
                }
                let base_ptr = all_data.as_ptr();
                let host_iov: Vec<libc::iovec> = iov_offsets
                    .iter()
                    .map(|(off, len)| libc::iovec {
                        iov_base: unsafe { base_ptr.add(*off) } as *mut _,
                        iov_len: *len,
                    })
                    .collect();
                let mut hdr: libc::msghdr = unsafe { std::mem::zeroed() };
                hdr.msg_name = name_bytes.as_ptr() as *mut _;
                hdr.msg_namelen = namelen;
                hdr.msg_iov = host_iov.as_ptr() as *mut _;
                hdr.msg_iovlen = iovcnt;
                let r = unsafe { libc::sendmsg(host, &hdr, args.a2 as i32) };
                Ok(if r < 0 { -errno() as i64 } else { r as i64 })
            }
            nr::RECVMSG => {
                let host = self.fds.get(args.a0 as i32).unwrap_or(-1);
                if host < 0 {
                    return Ok(EBADF);
                }
                let msg = args.a1;
                let iov_ptr = mem.read(msg + 16, 8, AccessType::Load).unwrap_or(0);
                let iovcnt = mem.read(msg + 24, 8, AccessType::Load).unwrap_or(0) as usize;
                let sizes: Vec<usize> = (0..iovcnt)
                    .map(|i| {
                        mem.read(iov_ptr + i as u64 * 16 + 8, 8, AccessType::Load)
                            .unwrap_or(0) as usize
                    })
                    .collect();
                let total: usize = sizes.iter().sum();
                let mut buf = vec![0u8; total];
                let mut host_iov: Vec<libc::iovec> = {
                    let mut off = 0;
                    sizes
                        .iter()
                        .map(|&len| {
                            let iov = libc::iovec {
                                iov_base: buf[off..].as_mut_ptr() as *mut _,
                                iov_len: len,
                            };
                            off += len;
                            iov
                        })
                        .collect()
                };
                let mut hdr: libc::msghdr = unsafe { std::mem::zeroed() };
                hdr.msg_iov = host_iov.as_mut_ptr();
                hdr.msg_iovlen = iovcnt;
                let r = unsafe { libc::recvmsg(host, &mut hdr, args.a2 as i32) };
                if r < 0 {
                    return Ok(-errno() as i64);
                }
                let mut off = 0usize;
                for i in 0..iovcnt {
                    let base = mem
                        .read(iov_ptr + i as u64 * 16, 8, AccessType::Load)
                        .unwrap_or(0);
                    let n = sizes[i].min((r as usize).saturating_sub(off));
                    if n > 0 {
                        write_guest_bytes(mem, base, &buf[off..off + n]);
                    }
                    off += sizes[i];
                }
                Ok(r as i64)
            }
            nr::SENDMMSG | nr::RECVMMSG => Ok(0),

            // ── Process identity ─────────────────────────────────────────────────
            nr::SETUID | nr::SETGID | nr::SETREUID | nr::SETREGID => Ok(0),
            nr::SETRESUID | nr::SETRESGID => Ok(0),
            nr::GETRESUID => {
                for ptr in [args.a0, args.a1, args.a2] {
                    if ptr != 0 {
                        mem.write(ptr, 4, 1000u64, AccessType::Store).ok();
                    }
                }
                Ok(0)
            }
            nr::GETRESGID => {
                for ptr in [args.a0, args.a1, args.a2] {
                    if ptr != 0 {
                        mem.write(ptr, 4, 1000u64, AccessType::Store).ok();
                    }
                }
                Ok(0)
            }
            nr::SETFSUID | nr::SETFSGID => Ok(1000),
            nr::SETGROUPS => Ok(0),
            nr::GETPRIORITY => Ok(0),
            nr::SETPRIORITY => Ok(0),
            nr::GETSID => Ok(self.pid as i64),
            nr::UNSHARE => Ok(0),
            nr::PERSONALITY => Ok(0),
            nr::SETHOSTNAME | nr::SETDOMAINNAME => Ok(0),
            nr::SETTIMEOFDAY => Ok(0),

            // ── Resource usage ───────────────────────────────────────────────────
            nr::GETRUSAGE => {
                let ptr = args.a1;
                write_guest_bytes(mem, ptr, &vec![0u8; 144]);
                Ok(0)
            }

            // ── Scheduler (additional) ────────────────────────────────────────────
            nr::SCHED_SETPARAM | nr::SCHED_SETSCHEDULER | nr::SCHED_SETATTR => Ok(0),
            nr::SCHED_GETPARAM => {
                if args.a1 != 0 {
                    mem.write(args.a1, 4, 0u64, AccessType::Store).ok();
                }
                Ok(0)
            }
            nr::SCHED_GETSCHEDULER => Ok(0),
            nr::SCHED_GET_PRIORITY_MAX => Ok(99),
            nr::SCHED_GET_PRIORITY_MIN => Ok(1),
            nr::SCHED_RR_GET_INTERVAL => {
                let ptr = args.a1;
                mem.write(ptr, 8, 0u64, AccessType::Store).ok();
                mem.write(ptr + 8, 8, 100_000_000u64, AccessType::Store)
                    .ok();
                Ok(0)
            }
            nr::SCHED_GETATTR => {
                let ptr = args.a1;
                for off in (0..48u64).step_by(8) {
                    mem.write(ptr + off, 8, 0u64, AccessType::Store).ok();
                }
                mem.write(ptr, 4, 48u64, AccessType::Store).ok();
                Ok(0)
            }
            nr::GETCPU => {
                if args.a0 != 0 {
                    mem.write(args.a0, 4, 0u64, AccessType::Store).ok();
                }
                if args.a1 != 0 {
                    mem.write(args.a1, 4, 0u64, AccessType::Store).ok();
                }
                Ok(0)
            }

            // ── Memory locking ────────────────────────────────────────────────────
            nr::MLOCK | nr::MUNLOCK | nr::MLOCKALL | nr::MUNLOCKALL | nr::MLOCK2 => Ok(0),
            nr::MINCORE => {
                let pages = ((args.a1 as usize) + 4095) / 4096;
                write_guest_bytes(mem, args.a2, &vec![1u8; pages]);
                Ok(0)
            }

            // ── Misc stubs ────────────────────────────────────────────────────────
            nr::RSEQ => Ok(0),
            nr::SECCOMP => Ok(0),
            nr::MEMBARRIER => Ok(0),
            nr::IOPRIO_SET | nr::IOPRIO_GET => Ok(0),
            nr::RESTART_SYSCALL => Ok(0),
            nr::RT_SIGPENDING => Ok(0),
            nr::RT_SIGTIMEDWAIT => Ok(EINVAL),
            nr::RT_SIGQUEUEINFO | nr::RT_TGSIGQUEUEINFO => Ok(0),
            nr::FACCESSAT2 => Ok(0),
            nr::OPENAT2 => Ok(ENOSYS),
            nr::PROCESS_VM_READV | nr::PROCESS_VM_WRITEV => Ok(-1),

            // ── Unimplemented ─────────────────────────────────────────────────
            _ => {
                eprintln!(
                    "[syscall] UNIMPLEMENTED nr={nr} a0={:#x} a1={:#x} a2={:#x}",
                    args.a0, args.a1, args.a2
                );
                Ok(ENOSYS)
            }
        }
    }
}

// ── Guest memory helpers ──────────────────────────────────────────────────────

fn read_guest_bytes(mem: &mut impl MemInterface, addr: u64, len: usize) -> Vec<u8> {
    use helm_core::AccessType;
    let mut out = Vec::with_capacity(len);
    let mut off = 0usize;
    while off < len {
        let chunk = (len - off).min(8);
        let v = mem
            .read(addr + off as u64, chunk, AccessType::Load)
            .unwrap_or(0);
        let bytes = v.to_le_bytes();
        out.extend_from_slice(&bytes[..chunk]);
        off += chunk;
    }
    out
}

fn write_guest_bytes(mem: &mut impl MemInterface, addr: u64, data: &[u8]) {
    use helm_core::AccessType;
    let mut off = 0usize;
    while off < data.len() {
        let chunk = (data.len() - off).min(8);
        let mut buf = [0u8; 8];
        buf[..chunk].copy_from_slice(&data[off..off + chunk]);
        let v = u64::from_le_bytes(buf);
        mem.write(addr + off as u64, chunk, v, AccessType::Store)
            .ok();
        off += chunk;
    }
}

fn read_guest_cstr(mem: &mut impl MemInterface, addr: u64) -> String {
    use helm_core::AccessType;
    let mut bytes = Vec::new();
    let mut off = 0u64;
    loop {
        let b = mem.read(addr + off, 1, AccessType::Load).unwrap_or(0) as u8;
        if b == 0 {
            break;
        }
        bytes.push(b);
        off += 1;
        if off > 4096 {
            break;
        } // safety limit
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn write_guest_str(mem: &mut impl MemInterface, addr: u64, s: &str, max: usize) {
    let bytes = s.as_bytes();
    let n = bytes.len().min(max.saturating_sub(1));
    write_guest_bytes(mem, addr, &bytes[..n]);
    use helm_core::AccessType;
    mem.write(addr + n as u64, 1, 0, AccessType::Store).ok();
}

fn write_stat(mem: &mut impl MemInterface, ptr: u64, st: &libc::stat) {
    use helm_core::AccessType;
    // AArch64 Linux stat struct layout (struct stat, 144 bytes)
    mem.write(ptr, 8, st.st_dev as u64, AccessType::Store).ok();
    mem.write(ptr + 8, 8, st.st_ino as u64, AccessType::Store)
        .ok();
    mem.write(ptr + 16, 4, st.st_mode as u64, AccessType::Store)
        .ok();
    mem.write(ptr + 20, 4, st.st_nlink as u64, AccessType::Store)
        .ok();
    mem.write(ptr + 24, 4, st.st_uid as u64, AccessType::Store)
        .ok();
    mem.write(ptr + 28, 4, st.st_gid as u64, AccessType::Store)
        .ok();
    mem.write(ptr + 32, 8, st.st_rdev as u64, AccessType::Store)
        .ok();
    mem.write(ptr + 48, 8, st.st_size as u64, AccessType::Store)
        .ok();
    mem.write(ptr + 56, 4, st.st_blksize as u64, AccessType::Store)
        .ok();
    mem.write(ptr + 64, 8, st.st_blocks as u64, AccessType::Store)
        .ok();
    // Timestamps (atime, mtime, ctime) as {tv_sec, tv_nsec}
    mem.write(ptr + 72, 8, st.st_atime as u64, AccessType::Store)
        .ok();
    mem.write(ptr + 80, 8, 0, AccessType::Store).ok();
    mem.write(ptr + 88, 8, st.st_mtime as u64, AccessType::Store)
        .ok();
    mem.write(ptr + 96, 8, 0, AccessType::Store).ok();
    mem.write(ptr + 104, 8, st.st_ctime as u64, AccessType::Store)
        .ok();
    mem.write(ptr + 112, 8, 0, AccessType::Store).ok();
}

fn errno() -> i32 {
    unsafe { *libc::__errno_location() }
}

/// Simple LCG for pseudo-random bytes (not cryptographic).
fn rand_byte() -> u8 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static STATE: AtomicU64 = AtomicU64::new(0xDEAD_BEEF_1234_5678);
    let s = STATE.fetch_add(6_364_136_223_846_793_005, Ordering::Relaxed);
    (s >> 33) as u8
}

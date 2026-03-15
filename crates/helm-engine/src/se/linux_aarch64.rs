//! Linux AArch64 syscall handler.
//!
//! AArch64 Linux calling convention:
//! - Syscall number → **X8**
//! - Arguments     → X0–X5
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

use super::SyscallArgs;

// ── Error codes ───────────────────────────────────────────────────────────────

pub const ENOSYS:  i64 = -38;
pub const ENOENT:  i64 = -2;
pub const EBADF:   i64 = -9;
pub const EINVAL:  i64 = -22;
pub const ENOMEM:  i64 = -12;
pub const EACCES:  i64 = -13;
pub const EFAULT:  i64 = -14;
pub const EEXIST:  i64 = -17;
pub const EAGAIN:  i64 = -11;

// ── AArch64 Linux syscall numbers ────────────────────────────────────────────

mod nr {
    pub const IO_SETUP: u64       = 0;
    pub const READ: u64           = 63;
    pub const WRITE: u64          = 64;
    pub const READV: u64          = 65;
    pub const WRITEV: u64         = 66;
    pub const PREAD64: u64        = 67;
    pub const PWRITE64: u64       = 68;
    pub const OPENAT: u64         = 56;
    pub const CLOSE: u64          = 57;
    pub const LSEEK: u64          = 62;
    pub const FSTAT: u64          = 80;
    pub const FSTATAT: u64        = 79;
    pub const STATX: u64          = 291;
    pub const GETDENTS64: u64     = 61;
    pub const IOCTL: u64          = 29;
    pub const FCNTL: u64          = 25;
    pub const DUP: u64            = 23;
    pub const DUP3: u64           = 24;
    pub const PPOLL: u64          = 73;
    pub const PSELECT6: u64       = 72;
    pub const MMAP: u64           = 222;
    pub const MUNMAP: u64         = 215;
    pub const MPROTECT: u64       = 226;
    pub const MADVISE: u64        = 233;
    pub const MREMAP: u64         = 216;
    pub const BRK: u64            = 214;
    pub const CLONE: u64          = 220;
    pub const EXECVE: u64         = 221;
    pub const EXIT: u64           = 93;
    pub const EXIT_GROUP: u64     = 94;
    pub const WAIT4: u64          = 260;
    pub const WAITID: u64         = 95;
    pub const GETPID: u64         = 172;
    pub const GETPPID: u64        = 173;
    pub const GETTID: u64         = 178;
    pub const GETUID: u64         = 174;
    pub const GETEUID: u64        = 175;
    pub const GETGID: u64         = 176;
    pub const GETEGID: u64        = 177;
    pub const SETSID: u64         = 157;
    pub const SETPGID: u64        = 154;
    pub const GETPGID: u64        = 155;
    pub const GETGROUPS: u64      = 158;
    pub const CLOCK_GETTIME: u64  = 113;
    pub const CLOCK_GETRES: u64   = 114;
    pub const CLOCK_NANOSLEEP: u64 = 115;
    pub const GETTIMEOFDAY: u64   = 169;
    pub const NANOSLEEP: u64      = 101;
    pub const TIMES: u64          = 153;
    pub const FUTEX: u64          = 98;
    pub const PRCTL: u64          = 167;
    pub const PRLIMIT64: u64      = 261;
    pub const GETRLIMIT: u64      = 163;
    pub const SETRLIMIT: u64      = 164;
    pub const RT_SIGACTION: u64   = 134;
    pub const RT_SIGPROCMASK: u64 = 135;
    pub const RT_SIGRETURN: u64   = 139;
    pub const RT_SIGSUSPEND: u64  = 133;
    pub const KILL: u64           = 129;
    pub const TGKILL: u64         = 131;
    pub const TKILL: u64          = 130;
    pub const SIGALTSTACK: u64    = 132;
    pub const UNAME: u64          = 160;
    pub const GETCWD: u64         = 17;
    pub const CHDIR: u64          = 49;
    pub const READLINKAT: u64     = 78;
    pub const FACCESSAT: u64      = 48;
    pub const SET_TID_ADDRESS: u64 = 96;
    pub const SET_ROBUST_LIST: u64 = 99;
    pub const GET_ROBUST_LIST: u64 = 100;
    pub const CAPGET: u64         = 90;
    pub const CAPSET: u64         = 91;
    pub const SOCKET: u64         = 198;
    pub const CONNECT: u64        = 203;
    pub const SCHED_YIELD: u64    = 124;
    pub const SCHED_GETAFFINITY: u64 = 123;
    pub const SCHED_SETAFFINITY: u64 = 122;
    // ARCH_PRCTL reuses PRCTL number (167) on AArch64 — not declared separately
    pub const GETRANDOM: u64      = 278;
    pub const MEMFD_CREATE: u64   = 279;
    pub const EPOLL_CREATE1: u64  = 20;
    pub const EPOLL_CTL: u64      = 21;
    pub const EPOLL_PWAIT: u64    = 22;
    pub const PIPE2: u64          = 59;
    pub const EVENTFD2: u64       = 19;
    pub const TIMERFD_CREATE: u64 = 85;
    pub const COPY_FILE_RANGE: u64 = 285;
    pub const SENDFILE: u64       = 71;
    pub const MSYNC: u64          = 227;
    pub const MINCORE: u64        = 232;
    pub const SYSINFO: u64        = 179;
    pub const UMASK: u64          = 166;
    pub const FCHMOD: u64         = 52;
    pub const CHOWN: u64          = 55;
    pub const FTRUNCATE: u64      = 46;
    pub const FALLOCATE: u64      = 47;
    pub const SYNC: u64           = 81;
    pub const FSYNC: u64          = 82;
    pub const FDATASYNC: u64      = 83;
    pub const UNLINKAT: u64       = 35;
    pub const RENAMEAT2: u64      = 276;
    pub const MKDIRAT: u64        = 34;
    pub const SYMLINKAT: u64      = 36;
    pub const LINKAT: u64         = 37;
    pub const FLOCK: u64          = 32;
    pub const STATFS: u64         = 43;
}

// ── FdTable ───────────────────────────────────────────────────────────────────

/// Guest file descriptor table.
///
/// Maps guest FDs (small integers starting at 3) to host FDs.
/// FD 0/1/2 (stdin/stdout/stderr) pass through to host by default.
struct FdTable {
    /// guest_fd → host_fd
    table: HashMap<i32, RawFd>,
    next:  i32,
}

impl FdTable {
    fn new() -> Self {
        let mut t = Self { table: HashMap::new(), next: 3 };
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
    fds:         FdTable,
    /// Current heap break pointer.
    brk:         u64,
    /// Next mmap allocation address (grows downward from stack).
    mmap_next:   u64,
    /// Per-process identity
    pid:         u64,
    tid:         u64,
    pub should_exit:   bool,
    pub exit_code:     i32,
    /// Path to the loaded binary (for /proc/self/exe).
    pub binary_path:   String,
}

impl LinuxAarch64SyscallHandler {
    pub fn new(initial_brk: u64) -> Self {
        Self {
            fds:         FdTable::new(),
            brk:         initial_brk,
            mmap_next:   0x4000_0000_0000u64, // 64 TiB
            pid:         1000,
            tid:         1000,
            should_exit: false,
            exit_code:   0,
            binary_path: String::new(),
        }
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
                self.exit_code   = args.a0 as i32;
                return Err(HartException::Exit { code: self.exit_code });
            }

            // ── I/O ──────────────────────────────────────────────────────────
            nr::WRITE => {
                let fd    = args.a0 as i32;
                let buf   = args.a1;
                let count = args.a2 as usize;
                let host  = self.fds.get(fd).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                // Read guest memory into host buffer
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
                    if (n as usize) < len { break; } // short read — stop
                }
                Ok(total)
            }
            nr::GETDENTS64 => {
                // Return 0 (EOF) — directory listing not supported in SE mode
                Ok(0)
            }
            nr::OPENAT => {
                let _dirfd   = args.a0 as i32; // AT_FDCWD = -100
                let path_ptr = args.a1;
                let flags    = args.a2 as i32;
                let mode     = args.a3 as u32;
                let path = read_guest_cstr(mem, path_ptr);
                // Map /proc/self/exe → a readable path (leave as-is for now)
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let host_fd = unsafe { libc::open(cpath.as_ptr(), flags, mode) };
                if host_fd < 0 { return Ok(-errno() as i64); }
                Ok(self.fds.allocate(host_fd) as i64)
            }
            nr::CLOSE => {
                let fd   = args.a0 as i32;
                let host = self.fds.remove(fd).unwrap_or(-1);
                if host < 0 || fd < 3 { return Ok(0); } // don't close stdin/stdout/stderr
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
            nr::DUP => {
                let fd   = args.a0 as i32;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let new_host = unsafe { libc::dup(host) };
                if new_host < 0 { return Ok(-errno() as i64); }
                Ok(self.fds.allocate(new_host) as i64)
            }
            nr::DUP3 => {
                let old  = args.a0 as i32;
                let new  = args.a1 as i32;
                let _fl  = args.a2;
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
                let fd  = args.a0 as i32;
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
                let fd  = args.a0 as i32;
                let cmd = args.a1 as i32;
                let host = self.fds.get(fd).unwrap_or(-1);
                if host < 0 { return Ok(EBADF); }
                let r = unsafe { libc::fcntl(host, cmd, args.a2) };
                Ok(if r < 0 { -errno() as i64 } else { r as i64 })
            }
            nr::FLOCK => Ok(0), // stub — always succeeds in SE mode

            // ── File metadata ─────────────────────────────────────────────────
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
                let _dirfd   = args.a0 as i32;
                let path_ptr = args.a1;
                let ptr      = args.a2;
                let _flags   = args.a3 as i32;
                let path = read_guest_cstr(mem, path_ptr);
                let cpath = CString::new(path.as_bytes()).unwrap_or_default();
                let mut st: libc::stat = unsafe { std::mem::zeroed() };
                let r = unsafe { libc::stat(cpath.as_ptr(), &mut st) };
                if r < 0 { return Ok(-errno() as i64); }
                write_stat(mem, ptr, &st);
                Ok(0)
            }
            nr::STATFS => {
                // Write a basic statfs64 struct (AArch64 layout, 120 bytes)
                // struct statfs64: f_type(8), f_bsize(8), f_blocks(8), f_bfree(8),
                //   f_bavail(8), f_files(8), f_ffree(8), f_fsid(8), f_namelen(8),
                //   f_frsize(8), f_flags(8), f_spare(40)
                let ptr = args.a1;
                mem.write(ptr,      8, 0xEF53u64,    AccessType::Store).ok(); // EXT2_SUPER_MAGIC
                mem.write(ptr + 8,  8, 4096u64,      AccessType::Store).ok(); // f_bsize
                mem.write(ptr + 16, 8, 1_000_000u64, AccessType::Store).ok(); // f_blocks
                mem.write(ptr + 24, 8, 500_000u64,   AccessType::Store).ok(); // f_bfree
                mem.write(ptr + 32, 8, 500_000u64,   AccessType::Store).ok(); // f_bavail
                mem.write(ptr + 40, 8, 1_000_000u64, AccessType::Store).ok(); // f_files
                mem.write(ptr + 48, 8, 900_000u64,   AccessType::Store).ok(); // f_ffree
                mem.write(ptr + 56, 8, 0u64,         AccessType::Store).ok(); // f_fsid
                mem.write(ptr + 64, 8, 255u64,       AccessType::Store).ok(); // f_namelen
                mem.write(ptr + 72, 8, 4096u64,      AccessType::Store).ok(); // f_frsize
                mem.write(ptr + 80, 8, 0u64,         AccessType::Store).ok(); // f_flags
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
            nr::FACCESSAT => Ok(0), // always accessible in SE mode

            // ── Memory management ─────────────────────────────────────────────
            nr::BRK => {
                if args.a0 == 0 {
                    Ok(self.brk as i64)
                } else if args.a0 >= self.brk {
                    // Extend heap — in SE mode we just accept it; FlatMem must cover the range
                    self.brk = args.a0;
                    Ok(self.brk as i64)
                } else {
                    self.brk = args.a0;
                    Ok(self.brk as i64)
                }
            }
            nr::MMAP => {
                let len   = ((args.a1 + 0xFFF) & !0xFFF).max(0x1000);
                let flags = args.a3;
                let fd_   = args.a4 as i32;
                // Anonymous mmap — allocate from our virtual pool
                let addr = if args.a0 != 0 { args.a0 } else {
                    self.mmap_next -= len;
                    self.mmap_next
                };
                // Zero the region (best-effort; may fail if outside FlatMem)
                for off in (0..len).step_by(8) {
                    mem.write(addr + off, 8, 0, AccessType::Store).ok();
                }
                Ok(addr as i64)
            }
            nr::MUNMAP  => Ok(0),
            nr::MPROTECT => Ok(0),
            nr::MADVISE  => Ok(0),
            nr::MSYNC    => Ok(0),
            nr::MREMAP => {
                // Stub: return the old address (best-effort)
                Ok(args.a0 as i64)
            }

            // ── Process identity ─────────────────────────────────────────────
            nr::GETPID  => Ok(self.pid as i64),
            nr::GETPPID => Ok((self.pid.saturating_sub(1)) as i64),
            nr::GETTID  => Ok(self.tid as i64),
            nr::GETUID | nr::GETEUID => Ok(1000),
            nr::GETGID | nr::GETEGID => Ok(1000),
            nr::GETGROUPS => Ok(0),
            nr::UMASK => Ok(0o022),
            nr::SCHED_YIELD => Ok(0),
            nr::SCHED_GETAFFINITY => {
                // sched_getaffinity(pid, cpusetsize, mask)
                // Write a CPU mask with only CPU 0 set (1-core simulator)
                let cpusetsize = args.a1 as usize;
                let mask_ptr   = args.a2;
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

            // ── Time ─────────────────────────────────────────────────────────
            nr::CLOCK_GETTIME => {
                let _clk_id = args.a0;
                let tp_ptr  = args.a1;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                mem.write(tp_ptr,     8, now.as_secs(),       AccessType::Store).ok();
                mem.write(tp_ptr + 8, 8, now.subsec_nanos() as u64, AccessType::Store).ok();
                Ok(0)
            }
            nr::CLOCK_GETRES => {
                let tp_ptr = args.a1;
                mem.write(tp_ptr,     8, 0, AccessType::Store).ok();
                mem.write(tp_ptr + 8, 8, 1, AccessType::Store).ok(); // 1ns resolution
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

            // ── Signals (stub — SE mode has no real signal delivery) ──────────
            nr::RT_SIGACTION   => Ok(0),
            nr::RT_SIGPROCMASK => Ok(0),
            nr::RT_SIGRETURN   => Ok(0),
            nr::RT_SIGSUSPEND  => Ok(EINVAL),
            nr::KILL | nr::TGKILL | nr::TKILL => Ok(0),
            nr::SIGALTSTACK => Ok(0),

            // ── Futex (basic) ─────────────────────────────────────────────────
            nr::FUTEX => {
                let op  = args.a1 as u32 & 0x7F;
                match op {
                    0 /* WAIT */ => Ok(0),   // stub: immediately return
                    1 /* WAKE */ => Ok(1),
                    _ => Ok(EINVAL),
                }
            }

            // ── System info ───────────────────────────────────────────────────
            nr::UNAME => {
                let ptr = args.a0;
                write_guest_str(mem, ptr,        "Linux",   65);
                write_guest_str(mem, ptr + 65,   "helm-ng", 65);
                write_guest_str(mem, ptr + 130,  "6.1.0",   65);
                write_guest_str(mem, ptr + 195,  "helm-ng", 65);
                write_guest_str(mem, ptr + 260,  "aarch64", 65);
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
                let resource  = args.a1;
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
                    mem.write(old_limit,     8, cur, AccessType::Store).ok();
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
                let r = unsafe { libc::pipe2(fds.as_mut_ptr(), 0) };
                if r < 0 { return Ok(-errno() as i64); }
                let gfd_r = self.fds.allocate(fds[0]) as u64;
                let gfd_w = self.fds.allocate(fds[1]) as u64;
                mem.write(args.a0,     4, gfd_r, AccessType::Store).ok();
                mem.write(args.a0 + 4, 4, gfd_w, AccessType::Store).ok();
                Ok(0)
            }

            // ── Unimplemented ─────────────────────────────────────────────────
            _ => {
                log::debug!("unimplemented aarch64 syscall {nr} (a0={:#x})", args.a0);
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
        let v = mem.read(addr + off as u64, chunk, AccessType::Load).unwrap_or(0);
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
        mem.write(addr + off as u64, chunk, v, AccessType::Store).ok();
        off += chunk;
    }
}

fn read_guest_cstr(mem: &mut impl MemInterface, addr: u64) -> String {
    use helm_core::AccessType;
    let mut bytes = Vec::new();
    let mut off = 0u64;
    loop {
        let b = mem.read(addr + off, 1, AccessType::Load).unwrap_or(0) as u8;
        if b == 0 { break; }
        bytes.push(b);
        off += 1;
        if off > 4096 { break; } // safety limit
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
    mem.write(ptr,      8, st.st_dev     as u64, AccessType::Store).ok();
    mem.write(ptr + 8,  8, st.st_ino     as u64, AccessType::Store).ok();
    mem.write(ptr + 16, 4, st.st_mode    as u64, AccessType::Store).ok();
    mem.write(ptr + 20, 4, st.st_nlink   as u64, AccessType::Store).ok();
    mem.write(ptr + 24, 4, st.st_uid     as u64, AccessType::Store).ok();
    mem.write(ptr + 28, 4, st.st_gid     as u64, AccessType::Store).ok();
    mem.write(ptr + 32, 8, st.st_rdev    as u64, AccessType::Store).ok();
    mem.write(ptr + 48, 8, st.st_size    as u64, AccessType::Store).ok();
    mem.write(ptr + 56, 4, st.st_blksize as u64, AccessType::Store).ok();
    mem.write(ptr + 64, 8, st.st_blocks  as u64, AccessType::Store).ok();
    // Timestamps (atime, mtime, ctime) as {tv_sec, tv_nsec}
    mem.write(ptr + 72,  8, st.st_atime as u64, AccessType::Store).ok();
    mem.write(ptr + 80,  8, 0,                  AccessType::Store).ok();
    mem.write(ptr + 88,  8, st.st_mtime as u64, AccessType::Store).ok();
    mem.write(ptr + 96,  8, 0,                  AccessType::Store).ok();
    mem.write(ptr + 104, 8, st.st_ctime as u64, AccessType::Store).ok();
    mem.write(ptr + 112, 8, 0,                  AccessType::Store).ok();
}

fn errno() -> i32 { unsafe { *libc::__errno_location() } }

/// Simple LCG for pseudo-random bytes (not cryptographic).
fn rand_byte() -> u8 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static STATE: AtomicU64 = AtomicU64::new(0xDEAD_BEEF_1234_5678);
    let s = STATE.fetch_add(6_364_136_223_846_793_005, Ordering::Relaxed);
    (s >> 33) as u8
}

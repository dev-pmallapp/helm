//! AArch64 Linux syscall handler for SE mode.

use super::aarch64::nr;
use crate::fd_table::FdTable;
use helm_core::types::Addr;
use helm_core::HelmResult;
use helm_memory::address_space::AddressSpace;

/// AArch64 SE-mode syscall handler.
pub struct Aarch64SyscallHandler {
    pub fds: FdTable,
    brk_addr: Addr,
    mmap_next: Addr,
    pub should_exit: bool,
    pub exit_code: u64,
    tid: u64,
}

impl Aarch64SyscallHandler {
    pub fn new() -> Self {
        Self {
            fds: FdTable::new(),
            brk_addr: 0x0200_0000, // will be adjusted after ELF load
            mmap_next: 0x2000_0000,
            should_exit: false,
            exit_code: 0,
            tid: 1000,
        }
    }

    /// Dispatch syscall by number. Args from X0-X5, returns result for X0.
    pub fn handle(
        &mut self,
        nr_val: u64,
        args: &[u64; 6],
        mem: &mut AddressSpace,
    ) -> HelmResult<u64> {
        match nr_val {
            nr::READ => self.sys_read(args, mem),
            nr::WRITE => self.sys_write(args, mem),
            nr::OPENAT => self.sys_openat(args, mem),
            nr::CLOSE => self.sys_close(args),
            nr::LSEEK => Ok(neg(22)), // -EINVAL stub
            nr::FSTAT | nr::FSTATAT => self.sys_fstat(args, mem),
            nr::DUP => self.sys_dup(args),
            nr::DUP3 => self.sys_dup3(args),
            nr::FCNTL => self.sys_fcntl(args),
            nr::IOCTL => self.sys_ioctl(args, mem),
            nr::PIPE2 => self.sys_pipe2(args, mem),
            nr::GETCWD => self.sys_getcwd(args, mem),
            nr::CHDIR => Ok(0),            // stub
            nr::FACCESSAT => Ok(neg(2)),   // -ENOENT
            nr::READLINKAT => Ok(neg(22)), // -EINVAL
            nr::GETDENTS64 => Ok(0),       // EOF
            nr::UNLINKAT | nr::MKDIRAT => Ok(0),
            nr::STATFS => Ok(neg(2)),
            nr::FTRUNCATE => Ok(0),
            nr::BRK => self.sys_brk(args, mem),
            nr::MMAP => self.sys_mmap(args, mem),
            nr::MUNMAP => Ok(0),
            nr::MPROTECT | nr::MADVISE => Ok(0),
            nr::EXIT | nr::EXIT_GROUP => {
                self.should_exit = true;
                self.exit_code = args[0];
                Ok(0)
            }
            nr::SET_TID_ADDRESS => {
                // Store pointer, return tid
                Ok(self.tid)
            }
            nr::SET_ROBUST_LIST => Ok(0),
            nr::GETPID => Ok(self.tid),
            nr::GETPPID => Ok(1),
            nr::GETUID | nr::GETEUID => Ok(1000),
            nr::GETGID | nr::GETEGID => Ok(1000),
            nr::GETTID => Ok(self.tid),
            nr::SCHED_YIELD => Ok(0),
            nr::SCHED_GETAFFINITY => self.sys_sched_getaffinity(args, mem),
            nr::RT_SIGACTION => Ok(0),   // record but don't deliver
            nr::RT_SIGPROCMASK => Ok(0), // track but don't enforce
            nr::RT_SIGRETURN => Ok(0),
            nr::SIGALTSTACK => Ok(0), // record but don't enforce
            nr::UNAME => self.sys_uname(args, mem),
            nr::PRCTL => Ok(0),
            nr::PRLIMIT64 => self.sys_prlimit64(args, mem),
            nr::CLOCK_GETTIME | nr::GETTIMEOFDAY => self.sys_clock_gettime(args, mem),
            nr::PPOLL | nr::PSELECT6 => self.sys_ppoll(args),
            nr::GETRANDOM => self.sys_getrandom(args, mem),
            nr::MEMFD_CREATE => Ok(neg(38)), // -ENOSYS
            _ => {
                log::warn!("unimplemented aarch64 syscall {nr_val}");
                Ok(neg(38)) // -ENOSYS
            }
        }
    }

    fn sys_read(&self, args: &[u64; 6], mem: &mut AddressSpace) -> HelmResult<u64> {
        let fd = args[0] as i32;
        let buf_addr = args[1];
        let count = args[2] as usize;
        let host_fd = match self.fds.get_host_fd(fd) {
            Some(h) => h,
            None => return Ok(neg(9)), // -EBADF
        };
        let mut buf = vec![0u8; count.min(0x10000)];
        let n = unsafe { libc::read(host_fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            return Ok(neg((-n) as u64));
        }
        mem.write(buf_addr, &buf[..n as usize])?;
        Ok(n as u64)
    }

    fn sys_write(&self, args: &[u64; 6], mem: &AddressSpace) -> HelmResult<u64> {
        let fd = args[0] as i32;
        let buf_addr = args[1];
        let count = args[2] as usize;
        let host_fd = match self.fds.get_host_fd(fd) {
            Some(h) => h,
            None => return Ok(neg(9)),
        };
        let mut buf = vec![0u8; count.min(0x10000)];
        mem.read(buf_addr, &mut buf)?;
        let n = unsafe { libc::write(host_fd, buf.as_ptr().cast(), buf.len()) };
        if n < 0 {
            return Ok(neg((-n) as u64));
        }
        Ok(n as u64)
    }

    fn sys_openat(&mut self, args: &[u64; 6], mem: &AddressSpace) -> HelmResult<u64> {
        let path_addr = args[1];
        let flags = args[2] as i32;
        let mode = args[3] as u32;
        let path = read_cstring(mem, path_addr, 4096)?;

        // /dev/null special case
        if path == "/dev/null" {
            let fd = unsafe { libc::open(c"/dev/null".as_ptr().cast(), flags, mode) };
            if fd < 0 {
                return Ok(neg(2));
            }
            return Ok(self.fds.alloc(fd) as u64);
        }

        let c_path = std::ffi::CString::new(path).map_err(|_| helm_core::HelmError::Syscall {
            number: nr::OPENAT,
            reason: "invalid path".into(),
        })?;
        let fd = unsafe { libc::open(c_path.as_ptr(), flags, mode) };
        if fd < 0 {
            return Ok(neg(2)); // -ENOENT
        }
        Ok(self.fds.alloc(fd) as u64)
    }

    fn sys_close(&mut self, args: &[u64; 6]) -> HelmResult<u64> {
        let fd = args[0] as i32;
        if fd <= 2 {
            return Ok(0); // don't close stdio
        }
        if let Some(host_fd) = self.fds.get_host_fd(fd) {
            unsafe { libc::close(host_fd) };
        }
        self.fds.close(fd);
        Ok(0)
    }

    fn sys_fstat(&self, args: &[u64; 6], mem: &mut AddressSpace) -> HelmResult<u64> {
        let buf_addr = args[1];
        // Return a minimal stat struct — just enough to not crash.
        // struct stat is 128 bytes on AArch64.
        let mut stat_buf = [0u8; 128];
        // st_mode at offset 16: S_IFCHR | 0666 for stdio, S_IFREG for files
        let mode: u32 = 0o100644; // S_IFREG | rw-r--r--
        stat_buf[16..20].copy_from_slice(&mode.to_le_bytes());
        // st_blksize at offset 88
        stat_buf[88..92].copy_from_slice(&4096u32.to_le_bytes());
        mem.write(buf_addr, &stat_buf)?;
        Ok(0)
    }

    fn sys_dup(&mut self, args: &[u64; 6]) -> HelmResult<u64> {
        let fd = args[0] as i32;
        match self.fds.dup(fd) {
            Some(new_fd) => Ok(new_fd as u64),
            None => Ok(neg(9)),
        }
    }

    fn sys_dup3(&mut self, args: &[u64; 6]) -> HelmResult<u64> {
        let old_fd = args[0] as i32;
        let new_fd = args[1] as i32;
        match self.fds.dup_to(old_fd, new_fd) {
            Some(fd) => Ok(fd as u64),
            None => Ok(neg(9)),
        }
    }

    fn sys_fcntl(&self, args: &[u64; 6]) -> HelmResult<u64> {
        let cmd = args[1];
        match cmd {
            1 => Ok(0),    // F_GETFD -> 0
            2 => Ok(0),    // F_SETFD -> ok
            3 => Ok(0o02), // F_GETFL -> O_RDWR
            4 => Ok(0),    // F_SETFL -> ok
            _ => Ok(0),
        }
    }

    fn sys_ioctl(&self, args: &[u64; 6], mem: &mut AddressSpace) -> HelmResult<u64> {
        let cmd = args[1];
        let arg_addr = args[2];
        match cmd {
            0x5401 => {
                // TCGETS — return a default termios struct (60 bytes)
                let mut termios = [0u8; 60];
                // c_iflag, c_oflag, c_cflag, c_lflag (4 bytes each)
                // Set sane raw-mode defaults
                termios[8..12].copy_from_slice(&0x00BFu32.to_le_bytes()); // c_cflag
                mem.write(arg_addr, &termios)?;
                Ok(0)
            }
            0x5402..=0x5404 => Ok(0), // TCSETS/W/F
            0x5413 => {
                // TIOCGWINSZ — return 24 rows, 80 cols
                let mut winsize = [0u8; 8];
                winsize[0..2].copy_from_slice(&24u16.to_le_bytes()); // ws_row
                winsize[2..4].copy_from_slice(&80u16.to_le_bytes()); // ws_col
                mem.write(arg_addr, &winsize)?;
                Ok(0)
            }
            0x5414 => Ok(0),        // TIOCSWINSZ
            0x540F => Ok(self.tid), // TIOCGPGRP
            0x5410 => Ok(0),        // TIOCSPGRP
            0x541B => {
                // FIONREAD — 0 bytes available
                mem.write(arg_addr, &0u32.to_le_bytes())?;
                Ok(0)
            }
            _ => Ok(neg(25)), // -ENOTTY
        }
    }

    fn sys_pipe2(&mut self, args: &[u64; 6], mem: &mut AddressSpace) -> HelmResult<u64> {
        let pipefd_addr = args[0];
        let mut fds = [0i32; 2];
        let ret = unsafe { libc::pipe(fds.as_mut_ptr()) };
        if ret < 0 {
            return Ok(neg(24)); // -EMFILE
        }
        let guest_r = self.fds.alloc(fds[0]);
        let guest_w = self.fds.alloc(fds[1]);
        mem.write(pipefd_addr, &guest_r.to_le_bytes())?;
        mem.write(pipefd_addr + 4, &guest_w.to_le_bytes())?;
        Ok(0)
    }

    fn sys_getcwd(&self, args: &[u64; 6], mem: &mut AddressSpace) -> HelmResult<u64> {
        let buf_addr = args[0];
        let size = args[1] as usize;
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "/".to_string());
        let bytes = cwd.as_bytes();
        let len = bytes.len().min(size - 1);
        mem.write(buf_addr, &bytes[..len])?;
        mem.write(buf_addr + len as u64, &[0u8])?;
        Ok(buf_addr)
    }

    fn sys_brk(&mut self, args: &[u64; 6], mem: &mut AddressSpace) -> HelmResult<u64> {
        let addr = args[0];
        if addr == 0 {
            return Ok(self.brk_addr);
        }
        if addr > self.brk_addr {
            // Map the new memory region
            let old = self.brk_addr;
            let new_end = (addr + 0xFFF) & !0xFFF;
            let old_end = (old + 0xFFF) & !0xFFF;
            if new_end > old_end {
                mem.map(old_end, new_end - old_end, (true, true, false));
            }
            self.brk_addr = addr;
        }
        Ok(self.brk_addr)
    }

    fn sys_mmap(&mut self, args: &[u64; 6], mem: &mut AddressSpace) -> HelmResult<u64> {
        let addr_hint = args[0];
        let len = args[1];
        let prot = args[2] as u32;
        let _flags = args[3] as u32;
        let _fd = args[4] as i32;
        let _offset = args[5];

        if len == 0 {
            return Ok(neg(22));
        } // -EINVAL
        let len_aligned = (len + 0xFFF) & !0xFFF;
        let addr = if addr_hint != 0 {
            addr_hint
        } else {
            let a = self.mmap_next;
            self.mmap_next += len_aligned;
            a
        };

        let r = prot & 1 != 0;
        let w = prot & 2 != 0;
        let x = prot & 4 != 0;
        mem.map(addr, len_aligned, (r, w, x));
        Ok(addr)
    }

    fn sys_uname(&self, args: &[u64; 6], mem: &mut AddressSpace) -> HelmResult<u64> {
        let buf_addr = args[0];
        // struct utsname: 5 fields of 65 bytes each = 325 bytes
        let mut buf = [0u8; 325];
        let fields = [
            (0, "Linux"),
            (65, "helm"),
            (130, "6.1.0-helm"),
            (195, "#1 SMP"),
            (260, "aarch64"),
        ];
        for (off, val) in fields {
            let b = val.as_bytes();
            buf[off..off + b.len()].copy_from_slice(b);
        }
        mem.write(buf_addr, &buf)?;
        Ok(0)
    }

    fn sys_prlimit64(&self, args: &[u64; 6], mem: &mut AddressSpace) -> HelmResult<u64> {
        let old_addr = args[3];
        if old_addr != 0 {
            // Return a sane rlimit: cur=8MB, max=unlimited
            let mut rlimit = [0u8; 16];
            rlimit[0..8].copy_from_slice(&(8 * 1024 * 1024u64).to_le_bytes());
            rlimit[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
            mem.write(old_addr, &rlimit)?;
        }
        Ok(0)
    }

    fn sys_clock_gettime(&self, args: &[u64; 6], mem: &mut AddressSpace) -> HelmResult<u64> {
        let tp_addr = args[1];
        // Return current host time
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts) };
        mem.write(tp_addr, &(ts.tv_sec as u64).to_le_bytes())?;
        mem.write(tp_addr + 8, &(ts.tv_nsec as u64).to_le_bytes())?;
        Ok(0)
    }

    fn sys_ppoll(&self, _args: &[u64; 6]) -> HelmResult<u64> {
        // Return 0 (timeout immediately) — simplest behavior for SE
        Ok(0)
    }

    fn sys_getrandom(&self, args: &[u64; 6], mem: &mut AddressSpace) -> HelmResult<u64> {
        let buf_addr = args[0];
        let count = args[1] as usize;
        // Fill with pseudo-random bytes from host
        let mut buf = vec![0u8; count.min(256)];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(0x6D).wrapping_add(0x3B);
        }
        mem.write(buf_addr, &buf)?;
        Ok(buf.len() as u64)
    }

    fn sys_sched_getaffinity(&self, args: &[u64; 6], mem: &mut AddressSpace) -> HelmResult<u64> {
        let size = args[1] as usize;
        let mask_addr = args[2];
        let mut mask = vec![0u8; size.min(128)];
        mask[0] = 1; // CPU 0
        mem.write(mask_addr, &mask)?;
        Ok(mask.len() as u64)
    }
}

impl Aarch64SyscallHandler {
    pub fn set_brk(&mut self, addr: u64) {
        self.brk_addr = addr;
    }
}

impl Default for Aarch64SyscallHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Encode a negative errno as the AArch64 convention: -errno as u64.
fn neg(errno: u64) -> u64 {
    (-(errno as i64)) as u64
}

/// Read a null-terminated string from guest memory.
fn read_cstring(mem: &AddressSpace, addr: Addr, max_len: usize) -> HelmResult<String> {
    let mut buf = vec![0u8; max_len];
    let mut i = 0;
    while i < max_len {
        let mut b = [0u8; 1];
        mem.read(addr + i as u64, &mut b)?;
        if b[0] == 0 {
            break;
        }
        buf[i] = b[0];
        i += 1;
    }
    Ok(String::from_utf8_lossy(&buf[..i]).to_string())
}

# helm-engine/se — LLD: Syscall Handler

> **Module:** `helm_engine::se::handler`
> **Interface:** `SyscallHandler` trait + `LinuxSyscallHandler` implementation

---

## Table of Contents

1. [SyscallHandler Trait](#1-syscallhandler-trait)
2. [SyscallResult and SyscallError](#2-syscallresult-and-syscallerror)
3. [LinuxSyscallHandler](#3-linuxsyscallhandler)
4. [LinuxProcess State](#4-linuxprocess-state)
5. [FdTable — File Descriptor Management](#5-fdtable--file-descriptor-management)
6. [Dispatch Table](#6-dispatch-table)
7. [Syscall Implementations](#7-syscall-implementations)
8. [mmap Integration with MemoryMap](#8-mmap-integration-with-memorymap)
9. [brk Implementation](#9-brk-implementation)
10. [Error Handling](#10-error-handling)

---

## 1. SyscallHandler Trait

`SyscallHandler` is the interface between `HelmEngine<T>` and the syscall emulation layer. It is held as `Box<dyn SyscallHandler>` inside the engine — one per simulated hart, or one shared instance for single-threaded SE mode.

```rust
use crate::{SyscallArgs, SyscallResult};
use helm_core::ThreadContext;

pub trait SyscallHandler: Send {
    /// Dispatch a syscall.
    ///
    /// `nr`   — syscall number, extracted from the ISA-specific register by
    ///          `SyscallAbi::extract_args()` before this call.
    /// `args` — up to 6 arguments, already extracted by `SyscallAbi`.
    /// `ctx`  — mutable access to the hart's register file and memory.
    ///
    /// The implementation must write the return value to `ctx` via
    /// `SyscallAbi::set_return()` after this method returns.
    ///
    /// Called on the simulation thread. Not called concurrently unless the
    /// simulator is running multiple harts on multiple threads.
    fn handle(
        &mut self,
        nr: u64,
        args: SyscallArgs,
        ctx: &mut dyn ThreadContext,
    ) -> SyscallResult;

    /// Return the list of syscall numbers this handler implements.
    /// Used for diagnostics and Phase 0 ENOSYS routing.
    fn supported_syscalls(&self) -> &[u64];
}
```

### Integration in HelmEngine

```rust
impl<T: TimingModel> HelmEngine<T> {
    fn handle_syscall(&mut self) {
        // ISA-specific ABI extracts args from ThreadContext
        let (nr, args) = self.abi.extract_args(self.hart.thread_context());

        let result = self.syscall_handler.handle(nr, args, self.hart.thread_context_mut());

        match result {
            SyscallResult::Ok(ret) => {
                self.abi.set_return(self.hart.thread_context_mut(), ret);
            }
            SyscallResult::Err(e) => {
                self.abi.set_return(self.hart.thread_context_mut(), -(e.errno() as i64));
            }
            SyscallResult::Passthrough => {
                // Unimplemented syscall in Phase 0: return ENOSYS.
                self.abi.set_return(self.hart.thread_context_mut(), -38); // -ENOSYS
                log::warn!("unimplemented syscall nr={nr}");
            }
        }
    }
}
```

---

## 2. SyscallResult and SyscallError

```rust
/// Return value from `SyscallHandler::handle()`.
#[derive(Debug)]
pub enum SyscallResult {
    /// Syscall succeeded. `i64` is the raw return value (e.g. 0, or a count).
    Ok(i64),
    /// Syscall failed. `SyscallError` carries the errno value.
    Err(SyscallError),
    /// Syscall is not implemented by this handler.
    /// In Phase 0: caller returns ENOSYS (-38) to the guest.
    /// In future: may forward to a chained handler.
    Passthrough,
}

/// Syscall error codes. Mirrors Linux errno values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyscallError {
    errno: i32,
}

impl SyscallError {
    pub const fn new(errno: i32) -> Self { Self { errno } }
    pub fn errno(self) -> i32 { self.errno }
    pub fn to_negative(self) -> i64 { -(self.errno as i64) }

    // Common errno constants
    pub const EPERM:   Self = Self::new(1);
    pub const ENOENT:  Self = Self::new(2);
    pub const EBADF:   Self = Self::new(9);
    pub const ENOMEM:  Self = Self::new(12);
    pub const EACCES:  Self = Self::new(13);
    pub const EFAULT:  Self = Self::new(14);
    pub const EINVAL:  Self = Self::new(22);
    pub const ENOSYS:  Self = Self::new(38);
}

/// Up to 6 syscall arguments.
#[derive(Debug, Clone, Copy, Default)]
pub struct SyscallArgs {
    pub a: [u64; 6],
}

impl SyscallArgs {
    pub fn arg0(&self) -> u64 { self.a[0] }
    pub fn arg1(&self) -> u64 { self.a[1] }
    pub fn arg2(&self) -> u64 { self.a[2] }
    pub fn arg3(&self) -> u64 { self.a[3] }
    pub fn arg4(&self) -> u64 { self.a[4] }
    pub fn arg5(&self) -> u64 { self.a[5] }
}
```

---

## 3. LinuxSyscallHandler

The Phase 0 implementation. Owns `LinuxProcess` state and delegates to per-syscall handler functions via a dispatch table.

```rust
pub struct LinuxSyscallHandler {
    process: LinuxProcess,
    memory:  Arc<Mutex<MemoryMap>>,  // shared with HelmEngine for mmap/munmap
    dispatch: SyscallDispatch,
}

impl LinuxSyscallHandler {
    /// Construct with an initialized `LinuxProcess` and access to the guest `MemoryMap`.
    pub fn new(process: LinuxProcess, memory: Arc<Mutex<MemoryMap>>) -> Self {
        Self {
            process,
            memory,
            dispatch: SyscallDispatch::build(),
        }
    }
}

impl SyscallHandler for LinuxSyscallHandler {
    fn handle(
        &mut self,
        nr: u64,
        args: SyscallArgs,
        ctx: &mut dyn ThreadContext,
    ) -> SyscallResult {
        self.dispatch.call(nr, args, ctx, &mut self.process, &self.memory)
    }

    fn supported_syscalls(&self) -> &[u64] {
        self.dispatch.supported()
    }
}
```

---

## 4. LinuxProcess State

All per-process mutable state that syscall handlers need. Owned exclusively by `LinuxSyscallHandler`.

```rust
pub struct LinuxProcess {
    /// File descriptor table: guest fd → host fd.
    pub fds: FdTable,
    /// Current program break (top of the heap region).
    pub brk_addr: u64,
    /// Initial program break (base of the heap region, set at ELF load time).
    pub brk_base: u64,
    /// Maximum heap size (bytes). Enforced by sys_brk.
    pub brk_limit: u64,
    /// Base address for the next anonymous mmap allocation.
    /// Grows upward from `mmap_base_initial`.
    pub mmap_next: u64,
    pub mmap_base_initial: u64,
    /// Synthetic guest PID (fixed for SE mode).
    pub pid: u32,
    /// Guest UID/GID (mirrored from host).
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
}

impl LinuxProcess {
    pub fn new(brk_base: u64, mmap_base: u64) -> Self {
        Self {
            fds: FdTable::with_stdio(),
            brk_addr: brk_base,
            brk_base,
            brk_limit: brk_base + 256 * 1024 * 1024,  // 256 MiB heap limit
            mmap_next: mmap_base,
            mmap_base_initial: mmap_base,
            pid: 1000,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            euid: unsafe { libc::geteuid() },
            egid: unsafe { libc::getegid() },
        }
    }
}
```

---

## 5. FdTable — File Descriptor Management

Maps guest file descriptors (small non-negative integers starting at 0) to host file descriptors. The guest always sees fd 0/1/2 as stdin/stdout/stderr.

```rust
/// Guest fd → host fd mapping.
pub struct FdTable {
    table: HashMap<i32, i32>,
    next_fd: i32,
}

impl FdTable {
    /// Initialize with fd 0/1/2 mapped to host stdin/stdout/stderr.
    pub fn with_stdio() -> Self {
        let mut t = Self { table: HashMap::new(), next_fd: 3 };
        t.table.insert(0, 0);  // stdin
        t.table.insert(1, 1);  // stdout
        t.table.insert(2, 2);  // stderr
        t
    }

    /// Allocate a new guest fd and associate it with the given host fd.
    pub fn insert(&mut self, host_fd: i32) -> i32 {
        let guest_fd = self.next_fd;
        self.next_fd += 1;
        self.table.insert(guest_fd, host_fd);
        guest_fd
    }

    /// Look up the host fd for a guest fd.
    pub fn get(&self, guest_fd: i32) -> Option<i32> {
        self.table.get(&guest_fd).copied()
    }

    /// Remove a guest fd entry (on `close()`).
    pub fn remove(&mut self, guest_fd: i32) -> Option<i32> {
        self.table.remove(&guest_fd)
    }
}
```

---

## 6. Dispatch Table

```rust
type SyscallFn = fn(
    SyscallArgs,
    &mut dyn ThreadContext,
    &mut LinuxProcess,
    &Arc<Mutex<MemoryMap>>,
) -> SyscallResult;

pub struct SyscallDispatch {
    table: HashMap<u64, SyscallFn>,
    supported: Vec<u64>,
}

impl SyscallDispatch {
    pub fn build() -> Self {
        let mut d = Self { table: HashMap::new(), supported: Vec::new() };
        d.register(63,  sys_read);
        d.register(64,  sys_write);
        d.register(56,  sys_openat);
        d.register(57,  sys_close);
        d.register(80,  sys_fstat);
        d.register(62,  sys_lseek);
        d.register(214, sys_brk);
        d.register(222, sys_mmap);
        d.register(215, sys_munmap);
        d.register(29,  sys_ioctl);
        d.register(66,  sys_writev);
        d.register(160, sys_uname);
        d.register(172, sys_getpid);
        d.register(169, sys_gettimeofday);
        d.register(174, sys_getuid);
        d.register(176, sys_getgid);
        d.register(175, sys_geteuid);
        d.register(177, sys_getegid);
        d.register(93,  sys_exit);
        d.register(94,  sys_exit_group);
        d
    }

    fn register(&mut self, nr: u64, f: SyscallFn) {
        self.table.insert(nr, f);
        self.supported.push(nr);
    }

    pub fn call(
        &self,
        nr: u64,
        args: SyscallArgs,
        ctx: &mut dyn ThreadContext,
        process: &mut LinuxProcess,
        memory: &Arc<Mutex<MemoryMap>>,
    ) -> SyscallResult {
        match self.table.get(&nr) {
            Some(f) => f(args, ctx, process, memory),
            None    => SyscallResult::Passthrough,
        }
    }

    pub fn supported(&self) -> &[u64] {
        &self.supported
    }
}
```

---

## 7. Syscall Implementations

### sys_write

```rust
fn sys_write(
    args: SyscallArgs,
    ctx: &mut dyn ThreadContext,
    process: &mut LinuxProcess,
    _memory: &Arc<Mutex<MemoryMap>>,
) -> SyscallResult {
    let guest_fd = args.arg0() as i32;
    let buf_addr  = args.arg1();
    let count     = args.arg2() as usize;

    let host_fd = match process.fds.get(guest_fd) {
        Some(fd) => fd,
        None     => return SyscallResult::Err(SyscallError::EBADF),
    };

    // Read `count` bytes from guest memory.
    let buf = match ctx.read_mem_bytes(buf_addr, count) {
        Ok(b)  => b,
        Err(_) => return SyscallResult::Err(SyscallError::EFAULT),
    };

    let written = unsafe { libc::write(host_fd, buf.as_ptr() as *const _, buf.len()) };
    if written < 0 {
        SyscallResult::Err(SyscallError::new(unsafe { *libc::__errno_location() }))
    } else {
        SyscallResult::Ok(written as i64)
    }
}
```

### sys_read

```rust
fn sys_read(
    args: SyscallArgs,
    ctx: &mut dyn ThreadContext,
    process: &mut LinuxProcess,
    _memory: &Arc<Mutex<MemoryMap>>,
) -> SyscallResult {
    let guest_fd = args.arg0() as i32;
    let buf_addr  = args.arg1();
    let count     = args.arg2() as usize;

    let host_fd = match process.fds.get(guest_fd) {
        Some(fd) => fd,
        None     => return SyscallResult::Err(SyscallError::EBADF),
    };

    let mut buf = vec![0u8; count];
    let n = unsafe { libc::read(host_fd, buf.as_mut_ptr() as *mut _, count) };
    if n < 0 {
        return SyscallResult::Err(SyscallError::new(unsafe { *libc::__errno_location() }));
    }

    if let Err(_) = ctx.write_mem_bytes(buf_addr, &buf[..n as usize]) {
        return SyscallResult::Err(SyscallError::EFAULT);
    }

    SyscallResult::Ok(n as i64)
}
```

### sys_openat

```rust
fn sys_openat(
    args: SyscallArgs,
    ctx: &mut dyn ThreadContext,
    process: &mut LinuxProcess,
    _memory: &Arc<Mutex<MemoryMap>>,
) -> SyscallResult {
    let _dirfd   = args.arg0() as i32;   // AT_FDCWD = -100; ignored for Phase 0
    let path_addr = args.arg1();
    let flags     = args.arg2() as i32;
    let mode      = args.arg3() as u32;

    // Read the null-terminated path from guest memory.
    let path = match ctx.read_cstring(path_addr, 4096) {
        Ok(p)  => p,
        Err(_) => return SyscallResult::Err(SyscallError::EFAULT),
    };

    let cpath = match std::ffi::CString::new(path) {
        Ok(s)  => s,
        Err(_) => return SyscallResult::Err(SyscallError::EINVAL),
    };

    let host_fd = unsafe { libc::open(cpath.as_ptr(), flags, mode) };
    if host_fd < 0 {
        SyscallResult::Err(SyscallError::new(unsafe { *libc::__errno_location() }))
    } else {
        let guest_fd = process.fds.insert(host_fd);
        SyscallResult::Ok(guest_fd as i64)
    }
}
```

### sys_close

```rust
fn sys_close(
    args: SyscallArgs,
    _ctx: &mut dyn ThreadContext,
    process: &mut LinuxProcess,
    _memory: &Arc<Mutex<MemoryMap>>,
) -> SyscallResult {
    let guest_fd = args.arg0() as i32;
    // Never close host fds 0/1/2 to protect stdin/stdout/stderr.
    if guest_fd < 3 {
        return SyscallResult::Ok(0);
    }
    match process.fds.remove(guest_fd) {
        Some(host_fd) => {
            let ret = unsafe { libc::close(host_fd) };
            if ret < 0 {
                SyscallResult::Err(SyscallError::new(unsafe { *libc::__errno_location() }))
            } else {
                SyscallResult::Ok(0)
            }
        }
        None => SyscallResult::Err(SyscallError::EBADF),
    }
}
```

### sys_exit / sys_exit_group

```rust
fn sys_exit(
    args: SyscallArgs,
    _ctx: &mut dyn ThreadContext,
    _process: &mut LinuxProcess,
    _memory: &Arc<Mutex<MemoryMap>>,
) -> SyscallResult {
    let code = args.arg0() as i32;
    std::process::exit(code);  // Terminate the simulator process.
}

fn sys_exit_group(
    args: SyscallArgs,
    ctx: &mut dyn ThreadContext,
    process: &mut LinuxProcess,
    memory: &Arc<Mutex<MemoryMap>>,
) -> SyscallResult {
    sys_exit(args, ctx, process, memory)
}
```

### sys_getpid

```rust
fn sys_getpid(
    _args: SyscallArgs,
    _ctx: &mut dyn ThreadContext,
    process: &mut LinuxProcess,
    _memory: &Arc<Mutex<MemoryMap>>,
) -> SyscallResult {
    SyscallResult::Ok(process.pid as i64)
}
```

### sys_uname

```rust
fn sys_uname(
    args: SyscallArgs,
    ctx: &mut dyn ThreadContext,
    _process: &mut LinuxProcess,
    _memory: &Arc<Mutex<MemoryMap>>,
) -> SyscallResult {
    // struct utsname layout (each field is 65 bytes on Linux/x86_64)
    const FIELD_LEN: usize = 65;
    let buf_addr = args.arg0();

    let write_field = |ctx: &mut dyn ThreadContext, offset: usize, s: &str| {
        let mut buf = [0u8; FIELD_LEN];
        let src = s.as_bytes();
        buf[..src.len().min(FIELD_LEN - 1)].copy_from_slice(&src[..src.len().min(FIELD_LEN - 1)]);
        ctx.write_mem_bytes(buf_addr + offset as u64, &buf)
    };

    write_field(ctx, 0 * FIELD_LEN, "Linux").ok();
    write_field(ctx, 1 * FIELD_LEN, "helm-ng").ok();
    write_field(ctx, 2 * FIELD_LEN, "5.15.0-helm").ok();
    write_field(ctx, 3 * FIELD_LEN, "#1 SMP").ok();
    write_field(ctx, 4 * FIELD_LEN, "riscv64").ok();

    SyscallResult::Ok(0)
}
```

---

## 8. mmap Integration with MemoryMap

`sys_mmap` allocates host memory and adds it to the guest `MemoryMap` as a `MemoryRegion::Ram`. The allocation address is taken from `process.mmap_next` and bumped upward.

```rust
fn sys_mmap(
    args: SyscallArgs,
    _ctx: &mut dyn ThreadContext,
    process: &mut LinuxProcess,
    memory: &Arc<Mutex<MemoryMap>>,
) -> SyscallResult {
    let hint_addr = args.arg0();
    let length    = args.arg1() as usize;
    let prot      = args.arg2() as i32;
    let flags     = args.arg3() as i32;
    let fd        = args.arg4() as i32;
    let offset    = args.arg5() as i64;

    // Phase 0: anonymous mmap only (MAP_ANONYMOUS). File-backed mmap deferred.
    const MAP_ANONYMOUS: i32 = 0x20;
    if flags & MAP_ANONYMOUS == 0 {
        return SyscallResult::Err(SyscallError::EINVAL);
    }

    if length == 0 {
        return SyscallResult::Err(SyscallError::EINVAL);
    }

    // Align to page boundary.
    let page_size = 4096usize;
    let aligned_len = (length + page_size - 1) & !(page_size - 1);

    // Allocate a guest address for this mapping.
    let guest_addr = if hint_addr != 0 {
        hint_addr  // Use the hint if provided (MAP_FIXED not enforced in Phase 0).
    } else {
        let addr = process.mmap_next;
        process.mmap_next += aligned_len as u64;
        addr
    };

    // Create a zeroed host backing buffer.
    let data = vec![0u8; aligned_len];

    // Insert into the guest memory map.
    let mut mem = memory.lock().unwrap();
    mem.add_region(guest_addr, MemoryRegion::Ram { data });

    SyscallResult::Ok(guest_addr as i64)
}
```

### sys_munmap

```rust
fn sys_munmap(
    args: SyscallArgs,
    _ctx: &mut dyn ThreadContext,
    _process: &mut LinuxProcess,
    memory: &Arc<Mutex<MemoryMap>>,
) -> SyscallResult {
    let addr   = args.arg0();
    let length = args.arg1();

    let mut mem = memory.lock().unwrap();
    mem.remove_region(addr, length);
    SyscallResult::Ok(0)
}
```

---

## 9. brk Implementation

```rust
fn sys_brk(
    args: SyscallArgs,
    _ctx: &mut dyn ThreadContext,
    process: &mut LinuxProcess,
    _memory: &Arc<Mutex<MemoryMap>>,
) -> SyscallResult {
    let new_brk = args.arg0();

    if new_brk == 0 {
        // Query: return the current brk.
        return SyscallResult::Ok(process.brk_addr as i64);
    }

    if new_brk < process.brk_base {
        // Cannot move brk below the base.
        return SyscallResult::Ok(process.brk_addr as i64);
    }

    if new_brk > process.brk_limit {
        // Exceeded the heap limit.
        return SyscallResult::Ok(process.brk_addr as i64);
    }

    process.brk_addr = new_brk;
    SyscallResult::Ok(new_brk as i64)
}
```

The heap memory is pre-allocated as a single large `MemoryRegion::Ram` at ELF load time, covering `[brk_base, brk_limit)`. `sys_brk` only adjusts the `brk_addr` pointer; no new `MemoryRegion` is added. The guest can write to any address in `[brk_base, brk_limit)` regardless of the current `brk_addr` — this matches Linux behavior where the region is committed but only the bytes up to `brk_addr` are "officially" in use.

---

## 10. Error Handling

### ThreadContext Memory Access Errors

`ctx.read_mem_bytes()` and `ctx.write_mem_bytes()` can fail if the guest address is unmapped. Syscall handlers convert these to `SyscallResult::Err(SyscallError::EFAULT)`.

### Host Errno Propagation

Host `libc` calls that fail return -1. The errno is read via `*libc::__errno_location()` and wrapped in `SyscallError`. The negative errno is written to the guest return register by `SyscallAbi::set_return()`.

### Unimplemented Syscalls

`SyscallDispatch::call()` returns `SyscallResult::Passthrough` for unknown syscall numbers. The caller in `HelmEngine` converts this to `-ENOSYS` (-38). A `log::warn!` is emitted so the user can see which syscalls are missing.

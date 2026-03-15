# helm-engine/se — High-Level Design

> **Crate:** `helm-engine/se`
> **Mode:** SE (Syscall Emulation)
> **Phase:** Phase 0 MVP
> **Dependencies:** `helm-core`, `helm-memory`

---

## Overview

`helm-engine/se` implements Syscall Emulation (SE) mode. When the simulated CPU executes a syscall instruction, `HelmEngine` calls `SyscallHandler::handle()`. In SE mode, this dispatches to the host OS: the simulator translates the guest's register arguments into a native `libc` call, executes it on the host, and writes the result back into the guest register file via `ThreadContext`.

This eliminates the need to simulate a kernel, an interrupt controller, or a page table walker. The tradeoff is limited syscall coverage and no scheduling or page-fault modeling. SE mode is sufficient for statically-linked userspace binaries.

### Execution Flow

```
HelmEngine detects syscall instruction
        │
        ▼
SyscallHandler::handle(nr, args, ctx)
        │
        ├── SyscallAbi::extract_args(ctx)    ← ISA-specific register extraction
        │
        ├── LinuxSyscallDispatch::dispatch(nr, args, process)
        │       │
        │       ├── nr=64  → sys_write(fd, buf, count)
        │       ├── nr=93  → sys_exit(code)
        │       ├── nr=214 → sys_brk(addr)
        │       ├── nr=222 → sys_mmap(...)
        │       └── ...
        │
        └── SyscallAbi::set_return(ctx, ret)  ← write result to a0/x0/etc.
```

---

## Subsystems

### 1. SyscallHandler (trait + LinuxSyscallHandler)

The trait interface that `HelmEngine<T>` holds as `Box<dyn SyscallHandler>`. In SE mode, the concrete implementation is `LinuxSyscallHandler`, which owns a `LinuxProcess` (file descriptor table, heap state) and a dispatch table mapping syscall numbers to handler functions.

### 2. SyscallAbi (trait + ISA implementations)

Extracts syscall number and arguments from `ThreadContext` and writes return values back. Each ISA defines its own ABI mapping: register names, argument count, and return register.

| ISA | Syscall nr reg | Arg regs | Return reg |
|-----|----------------|----------|------------|
| RISC-V 64 | `a7` (x17) | `a0`–`a5` (x10–x15) | `a0` (x10) |
| AArch64 | `x8` | `x0`–`x5` | `x0` |

### 3. LinuxProcess State

Per-process state that the syscall handlers need access to:

- `FdTable` — maps guest file descriptors to host file descriptors.
- `brk_addr` — current program break (top of heap).
- `mmap_base` — base address for anonymous mmap allocations.
- Optional: `argv`, `envp`, `auxv` (for `execve` and process startup).

### 4. MemoryMap Integration (mmap/brk)

`sys_mmap` allocates host memory (`Vec<u8>`) and inserts a new `MemoryRegion::Ram` into the guest `MemoryMap`. `sys_munmap` removes the region. `sys_brk` adjusts the heap boundary within a pre-allocated heap region.

---

## Phase 0 Syscall Set

~20 syscalls sufficient for: hello world, `ls`, `bash` (statically linked).

| Syscall name | RISC-V nr | AArch64 nr | Notes |
|---|---|---|---|
| `exit` | 93 | 93 | Terminate simulation |
| `exit_group` | 94 | 94 | Same as exit for single-threaded |
| `read` | 63 | 63 | `FdTable` → `libc::read` |
| `write` | 64 | 64 | `FdTable` → `libc::write` |
| `openat` | 56 | 56 | Host filesystem direct |
| `close` | 57 | 57 | `FdTable` release |
| `fstat` | 80 | 80 | `libc::fstat` |
| `lseek` | 62 | 62 | `libc::lseek` |
| `mmap` | 222 | 222 | Allocate + insert into MemoryMap |
| `munmap` | 215 | 215 | Remove from MemoryMap |
| `brk` | 214 | 214 | Adjust heap pointer |
| `ioctl` | 29 | 29 | Pass through to host |
| `writev` | 66 | 66 | Scatter-gather write |
| `uname` | 160 | 160 | Return synthetic `struct utsname` |
| `getpid` | 172 | 172 | Return fixed guest PID (e.g. 1000) |
| `gettimeofday` | 169 | 169 | `libc::gettimeofday` |
| `getuid` | 174 | 174 | Host `getuid()` |
| `getgid` | 176 | 176 | Host `getgid()` |
| `geteuid` | 175 | 175 | Host `geteuid()` |
| `getegid` | 177 | 177 | Host `getegid()` |

Unimplemented syscalls: return `ENOSYS (-38)` in Phase 0. This is the `SyscallResult::Passthrough` path.

---

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Syscall handler dispatch | `Box<dyn SyscallHandler>` in `HelmEngine` | Cold path; one call per syscall; allows SE/FS swap at config time |
| ISA ABI | `SyscallAbi` trait (per ISA) | Decouples argument extraction from dispatch logic |
| Register access | `&mut dyn ThreadContext` | Cold path; dynamic dispatch acceptable; avoids ISA coupling |
| Filesystem | Host direct (no virtual FS) | Sufficient for Phase 0; virtual FS deferred to Phase 3 |
| Signal delivery | Deferred to Phase 1 | Signals require per-tick delivery check — too complex for Phase 0 |
| Guest PID | Fixed synthetic value (1000) | SE mode has no scheduler; PID is meaningless but must be non-zero |
| mmap implementation | Allocate `Vec<u8>`, add `MemoryRegion::Ram` | Integrates correctly with the existing `MemoryMap` flat view |
| brk implementation | Track pointer in `LinuxProcess`; resize pre-allocated heap region | Simple; avoids creating one MemoryRegion per brk call |
| Unimplemented syscalls | Return `ENOSYS` | Most programs handle ENOSYS gracefully; fatal panic is too strict for Phase 0 |
| fd 0/1/2 | Map to host stdin/stdout/stderr | Correct behavior for programs that write to stdout |

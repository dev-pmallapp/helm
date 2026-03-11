# SE Threading: QEMU User-Mode Analysis

**Date**: 2026-03-11
**Status**: Analysis complete, cooperative fixes applied, real-thread impl is future work
**Test case**: `fish -c 'echo hello'` (without `--no-config`)

## Problem Statement

Fish shell without `--no-config` creates 2 I/O worker threads via
`clone(CLONE_VM|CLONE_THREAD)`. Under QEMU user-mode, it completes in
~3600 syscalls and exits 0. Under HELM SE, it deadlocked at 100M+
instructions and never exited.

## QEMU Architecture (linux-user)

Source: `assets/qemu/linux-user/syscall.c`, `aarch64/cpu_loop.c`, `aarch64/target_cpu.h`

### Thread Creation (`do_fork`, line 6875)

When `CLONE_VM` is set, QEMU creates a **real host pthread**:

```c
// syscall.c:6931-6975
new_env = cpu_copy(env);                          // full CPU state clone
cpu_clone_regs_child(new_env, newsp, flags);      // x0=0, sp=child_stack
cpu_set_tls(new_env, newtls);                     // TPIDR_EL0 = tls arg
ret = pthread_create(&info.thread, &attr, clone_func, &info);
```

Each host thread runs its own CPU emulation loop independently:

```c
// clone_func, line 6825
static void *clone_func(void *arg) {
    tcg_register_thread();    // per-thread TCG state
    thread_cpu = cpu;         // assign CPU to this host thread
    cpu_loop(env);            // independent cpu_exec -> do_syscall loop
}
```

### CPU Loop (`aarch64/cpu_loop.c`, line 157)

```c
void cpu_loop(CPUARMState *env) {
    for (;;) {
        trapnr = cpu_exec(cs);          // run guest code until exception
        switch (trapnr) {
        case EXCP_SWI:
            ret = do_syscall(env, ...); // handle syscall
            env->xregs[0] = ret;        // set return value
            break;
        }
    }
}
```

Each thread has independent `CPUState`, independent TCG caches, and runs
simultaneously on different host CPU cores.

### Syscall Passthrough

QEMU passes **all blocking syscalls directly to the host kernel**:

| Syscall | QEMU implementation | Key code |
|---------|-------------------|----------|
| `pipe2` | `pipe2(host_pipe, flags)` → writes host fds to guest memory | `do_pipe`, line 1641 |
| `read` | `safe_read(fd, buf, count)` → host kernel read | line 9668 |
| `write` | `safe_write(fd, buf, count)` → host kernel write | line 9681 |
| `ppoll` | `safe_ppoll(pfd, nfds, tsp, sigmask)` → host kernel ppoll | line 722, 1599 |
| `futex` | `do_safe_futex(g2h(uaddr), op, val, ...)` → host kernel futex | line 8170 |
| `fcntl` | `safe_fcntl(fd, cmd, arg)` → host kernel fcntl | `do_fcntl`, line 7338 |
| `close` | `close(fd)` → host kernel close | line 9553 |

The `safe_syscall` wrapper is an assembly trampoline that allows the host
syscall to be interrupted by signals (for guest signal delivery).

Key: `g2h(cpu, uaddr)` translates guest virtual address to host virtual
address. Since guest memory is mmap'd into the host process, host futex
works directly on guest memory — the host kernel handles blocking/waking.

### Thread Exit (line 9621)

```c
case TARGET_NR_exit:
    if (CPU_NEXT(first_cpu)) {
        // Multi-threaded: clean up this thread
        put_user_u32(0, ts->child_tidptr);           // CLONE_CHILD_CLEARTID
        do_sys_futex(g2h(ts->child_tidptr), WAKE);   // wake waiters
        object_unref(OBJECT(cpu));
        pthread_exit(NULL);                           // host thread exits
    } else {
        _exit(arg1);                                  // last thread: process exit
    }
```

### Clone Flags (line 181)

```c
#define CLONE_THREAD_FLAGS  \
    (CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD | CLONE_SYSVSEM)
```

Fish uses exactly these flags. QEMU requires all of them for the
`pthread_create` path; any missing flags → `-EINVAL`.

### Fd Handling

QEMU does **NOT remap fds** — guest fd numbers equal host fd numbers:

```c
// do_pipe, line 1669
put_user_s32(host_pipe[0], pipedes);       // write host fd directly
put_user_s32(host_pipe[1], pipedes + 4);
```

## HELM's Approach and Gaps

### Current: Cooperative Scheduling

HELM runs all guest threads in a **single host thread** with round-robin
scheduling. Thread switches happen at syscall boundaries.

| Aspect | QEMU | HELM |
|--------|------|------|
| Thread model | 1 host pthread per guest thread | 1 host thread, cooperative scheduler |
| CPU state | Independent `CPUState` per thread | Single `Aarch64Cpu`, regs swapped |
| Memory | Shared host mmap (`g2h` translation) | Shared `AddressSpace` |
| Pipe I/O | Host kernel blocks/wakes real threads | `O_NONBLOCK` + `Yield` action |
| Futex | Host kernel futex on guest memory | Emulated `FutexWait`/`FutexWake` |
| ppoll | Host kernel blocks thread | Host `poll(timeout=0)` + `Yield` |
| fcntl | Host `fcntl` passthrough | Host `fcntl` passthrough (fixed) |

### Bugs Found and Fixed

#### 1. fcntl completely stubbed (most damaging)

`F_GETFL` always returned `O_RDWR`. `F_SETFL` was a no-op.
`F_DUPFD_CLOEXEC` returned 0 (fd 0 = stdin), corrupting fish's fd table.
Fish uses `fcntl(fd, F_DUPFD_CLOEXEC, 10)` to remap pipe fds above 10.

**Fix**: pass all fcntl commands to host `fcntl`, translate guest fds.

#### 2. pipe2 forced O_NONBLOCK

Guest requests `pipe2(O_CLOEXEC)`, HELM created `pipe2(O_NONBLOCK)`.

**Fix**: pass guest flags through, add `O_NONBLOCK` for cooperative compat.

#### 3. ppoll return value stale after block

Blocked threads resumed with the pre-syscall x0 (ppoll fd pointer)
instead of the return value (0 = timeout).

**Fix**: set x0 to return value before `save_regs`.

#### 4. FUTEX_WAIT_BITSET unhandled (op=9)

Fish uses `futex(FUTEX_PRIVATE_FLAG|FUTEX_WAIT_BITSET, ...)`. Op 9 fell
through to the default handler returning 0 without blocking, causing a
tight spin loop consuming 100M+ instructions.

**Fix**: handle op 9 same as op 0 (WAIT), op 10 same as op 1 (WAKE).

#### 5. TLS BSS not zeroed after CLONE_SETTLS

musl copies the parent's TLS template into the child. Rust's
`std::thread::current()` had already written the parent's thread handle
into a `#[thread_local]` static. The child inherited this → "current
thread handle already set during thread spawn" abort.

**Fix**: zero `[tp+file_size .. tp+mem_size]` (BSS portion) after spawn.

#### 6. munmap was a no-op

`mmap_next` advanced forever; QEMU reuses freed addresses. Fish's
malloc/free cycle consumed unbounded address space.

**Fix**: track freed `(addr, size)` pairs; mmap checks free list first.

#### 7. break_deadlock only woke futex waiters

ppoll/read/waitchild threads stayed blocked while futex waiters livelooped.

**Fix**: wake ALL blocked categories simultaneously.

#### 8. No try_switch after block_current

After `block_current(ts)`, `load_regs` loaded the just-blocked thread's
registers. The thread re-ran from PC+4 with the syscall return value
and immediately re-blocked.

**Fix**: call `try_switch()` after every `block_current()`.

#### 9. SyscallAction::Yield

ppoll/read returning immediately (no data) caused 678K syscalls with no
useful work. New `Yield` variant: sets x0, advances PC, switches to next
thread — avoids busy-spinning while letting the writer thread run.

#### 10. wake_io_waiters after write

Threads blocked on read/ppoll were never woken after another thread
wrote to a pipe. Only `exit_current` called `wake_io_waiters`.

**Fix**: call `wake_io_waiters()` after every successful write syscall.

### Remaining Gap: Cooperative Scheduling Deadlock

Fish's I/O thread pattern requires true parallelism:
1. Main thread writes to pipe A → expects I/O thread to read it
2. I/O thread reads pipe A → processes → writes to pipe B
3. Main thread reads from pipe B

In cooperative scheduling, step 1 and 2 can't overlap. The `Yield`
mechanism approximates it but the scheduling granularity (4096-instruction
batches) prevents correct interleaving in all cases.

**Result**: fish without `--no-config` runs 14.9M instructions without
crashing but deadlocks when all threads block on pipe I/O. QEMU finishes
in ~200K instructions because real threads handle the pipe ping-pong
natively.

## Fix Direction: Real Host Threads

For `CLONE_VM|CLONE_THREAD`, spawn a real host thread:

```rust
// Pseudocode for the real-thread approach
fn handle_clone_vm_thread(cpu_regs, child_stack, tls, mem: Arc<AddressSpace>) {
    std::thread::spawn(move || {
        let mut child_cpu = Aarch64Cpu::new();
        child_cpu.regs = cpu_regs.clone();
        child_cpu.regs.sp = child_stack;
        child_cpu.regs.x[0] = 0;
        child_cpu.regs.pc += 4;
        child_cpu.regs.tpidr_el0 = tls;

        // Run independently — pipes/futex/ppoll go through host kernel
        loop {
            match child_cpu.step_fast(&mut *mem.lock()) {
                Err(HelmError::Syscall { number, .. }) => {
                    handle_syscall(&mut child_cpu, &mut *mem.lock(), number);
                }
                _ => {}
            }
        }
    });
}
```

Requirements:
- `AddressSpace` wrapped in `Arc<Mutex<>>` or `Arc<RwLock<>>`
- Per-thread `Aarch64Cpu` (not shared)
- Per-thread `FdTable` for fd remapping (or shared, since `CLONE_FILES`)
- Futex can use real host `futex(2)` on `AddressSpace` memory addresses
- Thread exit: `pthread_exit` equivalent + futex-wake on `child_tidptr`

This matches QEMU's architecture exactly and would eliminate all
cooperative scheduling limitations for multi-threaded SE workloads.

## QEMU Source Reference

| File | Key functions | Purpose |
|------|--------------|---------|
| `linux-user/syscall.c:6875` | `do_fork` | Clone dispatch: CLONE_VM → pthread, !CLONE_VM → fork |
| `linux-user/syscall.c:6825` | `clone_func` | Child thread entry: tcg_register_thread + cpu_loop |
| `linux-user/syscall.c:7338` | `do_fcntl` | fcntl passthrough to host |
| `linux-user/syscall.c:1641` | `do_pipe` | pipe2 passthrough, host fds to guest |
| `linux-user/syscall.c:8107` | `do_futex` | futex passthrough via g2h |
| `linux-user/syscall.c:9621` | `TARGET_NR_exit` | Thread exit: pthread_exit + futex_wake |
| `linux-user/aarch64/cpu_loop.c:157` | `cpu_loop` | Per-thread cpu_exec → do_syscall loop |
| `linux-user/aarch64/target_cpu.h` | `cpu_clone_regs_child` | x0=0, sp=newsp |
| `linux-user/aarch64/target_cpu.h` | `cpu_set_tls` | TPIDR_EL0 = newtls |

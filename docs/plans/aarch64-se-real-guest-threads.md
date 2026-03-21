# Plan: AArch64 SE Real Guest Thread Execution

> **Status:** Design pass started — 2026-03-21
> **Goal:** Replace the current `clone(CLONE_VM|CLONE_THREAD)` rejection path with real guest child execution in SE mode
> **Completion gate:** `fish` without `--no-config` runs to exit 0 under `helm-aarch64`

---

## Current State

The repo now has the right honesty boundary but not the real implementation:

| Component | State |
|-----------|-------|
| `runtime/helm-engine/src/se/threading.rs` | `clone` flag classification and TID allocation exist |
| `runtime/helm-engine/src/se/linux_aarch64.rs` | thread-style `clone` returns `-EINVAL` on purpose |
| `runtime/helm-engine/tests/se_host_threads.rs` | asserts the explicit `-EINVAL` behavior until guest child execution exists |
| `runtime/helm-engine/tests/aarch64_se_fd_parity.rs` | covers `dup3` and `F_DUPFD_CLOEXEC` guest-fd parity |
| `runtime/helm-engine/src/lib.rs` | AArch64 SE path owns one `Aarch64ArchState`, one `LinuxAarch64SyscallHandler`, one `FlatMem` |
| `runtime/helm-engine/src/loader/elf64.rs` | parses PT_TLS but only exposes `setup_riscv_tp`; AArch64 main-thread TLS setup is still missing |

This is the right interim state. Returning success for thread-style `clone` before the child actually runs corrupts userspace.

---

## Reference Point

`../helm.git/docs/research/se-threading-qemu-analysis.md` is the anchor.

QEMU linux-user does three things that matter here:

1. It clones full guest CPU state for the child.
2. It starts a real host thread for each guest thread-style `clone`.
3. It lets blocking syscalls and futex waits block in the host kernel.

The previous cooperative scheduler was useful for analysis, but it is not the design to carry forward. The next implementation should be direct host-thread execution.

---

## Design Decision

For AArch64 SE thread-style `clone`, use:

- one real host thread per guest thread
- one `Aarch64ArchState` per guest thread
- one shared process object for memory, fd table, PID/TID bookkeeping, and process exit state
- host-kernel blocking for `read`, `write`, `ppoll`, and `futex`

This matches the workload that motivated the work: `fish` creates real worker threads and expects them to make progress while other guest threads are blocked in syscalls.

---

## Proposed Split

### Shared process state

Add a process-scoped object, roughly:

```rust
struct Aarch64SeProcess {
    memory: Arc<Mutex<FlatMem>>,
    fds: Mutex<FdTable>,
    mmap: Mutex<MmapState>,
    next_tid: AtomicU64,
    pid: u64,
    threads: Mutex<HashMap<u64, ThreadLifecycle>>,
    exit_group: AtomicBool,
    exit_code: AtomicI32,
}
```

Why:

- `CLONE_VM` means all guest threads share one address space.
- `CLONE_FILES` means they also share one guest-fd table.
- `mmap`, `munmap`, `brk`, `dup3`, `fcntl`, and `close` must stop pretending they are thread-local process state.

### Per-thread state

Add a thread-scoped object, roughly:

```rust
struct Aarch64SeThread {
    tid: u64,
    arch: Aarch64ArchState,
    clear_tid_addr: Option<u64>,
    set_child_tid_addr: Option<u64>,
    process: Arc<Aarch64SeProcess>,
}
```

Why:

- guest GPRs, SP, PC, NZCV, SIMD, and `TPIDR_EL0` are per-thread
- `CLONE_CHILD_SETTID` and `CLONE_CHILD_CLEARTID` are per-thread lifetime rules
- the current `LinuxAarch64SyscallHandler` mixes process and thread state in one mutable struct, which does not scale to real guest threads

---

## Required Refactors

### 1. Make AArch64 architectural state cloneable

`clone()` needs a real child register image. `Aarch64ArchState` should derive or implement `Clone`.

Child setup rules:

- child gets a copy of parent architectural state
- `x0 = 0`
- `pc += 4` to step past `svc #0`
- if `child_stack != 0`, set `sp = child_stack`
- if `CLONE_SETTLS`, set `tpidr_el0 = tls_arg`, else inherit parent

### 2. Split `LinuxAarch64SyscallHandler`

The current handler owns:

- fd table
- brk/mmap state
- pid/tid
- thread pointer
- exit flags

That has to become:

- process-owned shared state behind `Arc`
- thread-owned state kept with the executing guest thread

The syscall entry point should become a pure dispatcher over:

```rust
fn handle_aarch64_syscall(
    process: &Arc<Aarch64SeProcess>,
    thread: &mut Aarch64SeThread,
    nr: u64,
    args: SyscallArgs,
) -> Result<i64, HartException>
```

### 3. Add AArch64 main-thread TLS setup

`load_aarch64_elf()` should stop leaving `TPIDR_EL0` at zero for PT_TLS binaries.

Add a loader helper mirroring `setup_riscv_tp()`:

```rust
pub fn setup_aarch64_tp(loaded: &LoadedBinary, mem: &mut FlatMem) -> u64
```

Initial requirement:

- allocate TLS block plus a pthread/self area
- copy PT_TLS template
- zero the TLS BSS region
- write the self-pointer expected by musl
- store the resulting pointer into `Aarch64ArchState::tpidr_el0`

Without this, the main thread and cloned threads do not start from the same TLS contract.

### 4. Add host-pointer translation for futex

Real guest thread execution only pays off if futex wait/wake also blocks correctly.

Needed capability:

```rust
impl FlatMem {
    fn host_ptr(&mut self, guest_addr: u64, len: usize) -> Option<*mut u8>;
}
```

Use it for:

- `FUTEX_WAIT`
- `FUTEX_WAIT_BITSET`
- `FUTEX_WAKE`

The first cut can require the futex word to live in one mapped region and return `-EFAULT` otherwise.

This is the simplest path to host-kernel blocking without reintroducing a cooperative scheduler.

---

## Execution Model

### Parent thread

On thread-style `clone`:

1. Classify flags with the existing logic in `se/threading.rs`.
2. Clone the parent `Aarch64ArchState`.
3. Allocate a guest TID from process state.
4. Apply clone semantics to the child register image.
5. Perform required guest-memory writes for `CLONE_PARENT_SETTID` / `CLONE_CHILD_SETTID`.
6. Spawn a host thread that runs the child guest loop.
7. Return the child TID in parent `x0`.

### Child host thread

Each child host thread runs a tight SE loop:

```rust
loop {
    match step_one_aarch64_instruction(&mut thread.arch, &process.memory) {
        Ok(()) => {}
        Err(HartException::EnvironmentCall { nr }) => {
            let ret = handle_aarch64_syscall(&process, &mut thread, nr, args)?;
            thread.arch.x[0] = ret as u64;
            thread.arch.pc += 4;
        }
        Err(HartException::Exit { code }) => break code,
        Err(other) => break_exception(other),
    }
}
```

This should reuse the same decode/execute path as the main engine, not a second implementation.

---

## Lifecycle Rules

### `set_tid_address`

Store the caller's clear-TID address in thread-local state.

### `clone`

Support only the existing QEMU-style thread bundle first:

- `CLONE_VM`
- `CLONE_FS`
- `CLONE_FILES`
- `CLONE_SIGHAND`
- `CLONE_THREAD`
- `CLONE_SYSVSEM`

Optional first-cut support:

- `CLONE_SETTLS`
- `CLONE_PARENT_SETTID`
- `CLONE_CHILD_SETTID`
- `CLONE_CHILD_CLEARTID`
- `CLONE_PARENT`

### `exit`

For non-last threads:

- mark thread dead
- if `CLONE_CHILD_CLEARTID`, write 0 to the guest address
- futex-wake waiters on that address
- terminate only the current host thread

### `exit_group`

For the process:

- set shared process-exit flag and exit code
- make all guest threads observe termination
- return `StopReason::Exit` once the main engine thread sees process exit

---

## Verification Plan

### Phase 1: refactor and spawn

- `clone` thread path returns a real child TID, not `-EINVAL`
- dedicated unit tests for child register image: `x0`, `sp`, `pc`, `tpidr_el0`
- loader test for AArch64 TLS setup

### Phase 2: thread lifetime and futex

- `set_tid_address`, `CLONE_CHILD_SETTID`, `CLONE_CHILD_CLEARTID`
- futex wait/wake tests using real host threads
- update `se_host_threads.rs` from rejection tests to positive execution tests

### Phase 3: workload gate

- `fish -c 'echo hello'` exits 0
- `fish` without `--no-config` exits 0
- no regression in `aarch64_se_fd_parity`

---

## File Map

Expected first implementation touches:

| File | Change |
|------|--------|
| `runtime/helm-arch/src/aarch64/arch_state.rs` | derive/implement `Clone` |
| `runtime/helm-engine/src/se/linux_aarch64.rs` | split process/thread state, real `clone` path |
| `runtime/helm-engine/src/se/threading.rs` | keep clone classification, extend runtime into real guest-thread support or rename |
| `runtime/helm-engine/src/lib.rs` | factor reusable AArch64 SE single-step / syscall-dispatch entry points |
| `runtime/helm-engine/src/loader/elf64.rs` | add `setup_aarch64_tp()` |
| `runtime/helm-engine/tests/se_host_threads.rs` | convert from `-EINVAL` assertions to success-path tests |
| `runtime/helm-engine/tests/` | add futex + clone-child-state coverage |

---

## Open Questions

1. `FlatMem` synchronization:
   First cut can use `Arc<Mutex<FlatMem>>` for correctness, but that serializes guest memory accesses. That is acceptable for the first semantic milestone if it gets `fish` unstuck. If it is too slow, the next step is a shared-memory wrapper with finer-grained synchronization.

2. Probe/plugin callbacks:
   The current SE path assumes one executing AArch64 context. Background guest threads should either emit no probe/plugin callbacks initially or tag events with synthetic thread IDs before we expose them publicly.

3. Process shutdown and joins:
   The engine needs one clear ownership rule for joining child host threads so that `exit_group` does not leak host threads or deadlock on self-join.

4. Memory translation for futex:
   Exporting raw host pointers from `FlatMem` is correct only if the pointed-to region remains stable for the duration of the syscall. The implementation must document and enforce that guarantee.

---

## Immediate Next Step

Start with the state split and loader fix, not with futex.

Reason:

- the current blocker is architectural: there is nowhere to put more than one AArch64 guest thread
- once process state and thread state are separate, `clone` can create a real child loop
- futex then becomes a focused syscall-level addition instead of a whole-engine redesign

# SE Host Threading Design

## Goal

Add a generic SE runtime threading model that matches QEMU linux-user's high-level
behavior for guest thread-style `clone()`: guest thread creation should become native
host thread creation in the SE runtime, with ISA-specific register/TLS wiring kept at
the edges.

The first implementation target is AArch64 SE mode, but the runtime design must be
generic enough to support other ISAs such as RISC-V.

## Scope

Included:

- generic SE runtime abstractions in `crates/helm-engine`
- thread-style `clone()` recognition and validation based on QEMU linux-user
- host-thread creation for supported guest thread-style clones
- per-thread CPU/register/TLS state for the active SE path
- futex and thread-exit semantics required for multithreaded user-space binaries

Excluded:

- full-system threading
- host-kernel passthrough of raw guest `clone()` flags
- non-SE CLI behavior

## Reference Behavior

QEMU linux-user does not call the host `clone()` syscall directly for guest threads.
Instead, in `assets/qemu/linux-user/syscall.c` it:

- distinguishes thread-style clone flag combinations from fork-style ones
- validates thread flags against an allowlist
- copies CPU state for the child
- applies `CLONE_SETTLS`
- uses `pthread_create()` for the new emulation thread

This is the runtime model to match.

## Architecture Decision

Implement generic host-threaded SE runtime support in `helm-engine`, with ISA-specific
hooks for:

- decoding thread/fork style from syscall arguments
- copying register state into the child thread
- applying thread-pointer / TLS state

Do not implement this as AArch64-only logic inside `linux_aarch64.rs`.

## Current Gap

The current repo has:

- no generic SE host-thread runtime
- no active-path clone thread support
- no generic per-thread runtime state in the active engine path
- a single-threaded `linux_aarch64.rs` syscall handler

The old repo has only a cooperative scheduler, which is useful for register/TLS and
futex semantics, but not as the final threading model requested here.

## Workstreams

### 1. Generic clone classification

Introduce shared logic in `helm-engine` to classify:

- thread-style clone
- fork-style clone
- invalid / unsupported combinations

The flag policy should follow QEMU linux-user's thread/fork split closely.

### 2. Generic SE thread runtime

Add a runtime representation for:

- thread identity
- per-thread architectural state
- lifecycle state
- join/exit bookkeeping
- futex wait/wake integration

The runtime should be independent of AArch64 specifics except where CPU register/TLS
copies are delegated to ISA-specific helpers.

### 3. AArch64 wiring

For AArch64:

- decode syscall `clone()` arguments correctly
- initialize child registers like the parent, with child `x0 = 0`
- apply child stack pointer override
- apply `CLONE_SETTLS` into `TPIDR_EL0`
- preserve or copy TLS according to runtime policy

### 4. Runtime verification

Verify with:

- unit tests for clone classification and TLS inheritance rules
- integration tests for thread creation and futex wakeups
- end-to-end multithreaded guest binaries

## Success Criteria

- thread-style guest `clone()` no longer returns `ENOSYS` on the supported path
- the SE runtime creates native host threads for supported guest threads
- TLS inheritance / `CLONE_SETTLS` semantics match the intended runtime model
- the design is generic enough that RISC-V can plug into the same runtime later

## Notes

- This design intentionally separates the QEMU-style host-thread runtime work from the
  earlier cooperative scheduler found in the old repo.
- The old scheduler remains a useful source of semantics tests, especially around
  `CLONE_SETTLS`, inherited thread pointer state, and futex wake behavior.

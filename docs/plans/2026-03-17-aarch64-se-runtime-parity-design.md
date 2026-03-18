# AArch64 SE Runtime Parity Design

## Goal

Bring the current AArch64 syscall-emulation runtime path up to behavioral parity with
`../helm.git` for the pieces that matter to running AArch64 user-space binaries:

- ISA decode and execute behavior on the active execution path
- Linux AArch64 syscall emulation
- ELF loading, initial process image setup, and TLS initialization

The immediate target workload is `assets/binaries/fish --no-config -c 'echo hello'`, but
the parity pass is not limited to fish-specific behavior.

## Scope

Included:

- `runtime/helm-arch/src/aarch64/decode.rs`
- `runtime/helm-arch/src/aarch64/execute.rs`
- `runtime/helm-engine/src/lib.rs` where SE runtime wires AArch64 execution
- `runtime/helm-engine/src/se/linux_aarch64.rs`
- `runtime/helm-engine/src/loader/elf64.rs`
- new regression tests that cover decode/execute, syscalls, ELF stack layout, and TLS

Excluded:

- CLI / plugin / monitor / stop-reason parity beyond what the SE runtime itself requires
- full-system behavior
- old architectural structures that are not part of the current active runtime path

## Current Findings

- The current runner had a reporting bug: it masked non-quantum stop reasons as
  `hit limit`. That has been corrected separately.
- The real current failure is a guest `BRK` inside fish's `__libc_free` at `0x6974c0`,
  which strongly suggests earlier guest-state corruption rather than instruction budget.
- The current repo contains richer AArch64 logic in alternate paths such as
  `step.rs` / `step_simd.rs`, but the engine currently uses the `decode.rs` +
  `execute.rs` path. Parity work should target the active path rather than switching
  execution pipelines mid-debug.
- The old repo includes broader AArch64 SIMD coverage, explicit SE loader TLS support
  (`PT_TLS`, `TlsInfo`, `brk_base`), and a more mature syscall layer.

## Architecture Decision

Keep the current engine on the existing AArch64 SE runtime path and port missing or
drifted behavior into that path.

Why:

- it minimizes architectural churn while debugging a correctness issue
- it avoids mixing a parity project with an execution-path swap
- it lets the old repo serve as a clean behavioral oracle for specific gaps

The old repo is the reference implementation, not the target architecture.

## Workstreams

### 1. Parity Inventory

Build a concrete parity matrix from `../helm.git` covering:

- implemented AArch64 instruction families
- implemented Linux AArch64 syscalls
- ELF process setup details including stack, auxv, brk placement, and TLS

Classify each current gap as:

- missing decode
- decoded but silently stubbed
- executed with semantic drift
- syscall missing or wrong errno/return behavior
- loader / TLS setup missing or wrong

### 2. ISA Parity

Port old-repo behavior into the active AArch64 execution path in small batches:

- SIMD / FP first, because current fish evidence points there
- then remaining integer / misc / load-store gaps

Rules:

- prefer reusing logic that already exists in this repo when it matches the old behavior
- otherwise port old logic minimally
- eliminate silent stubs for old-repo-implemented instructions on the SE path

### 3. ELF + TLS Parity

Compare the current loader against the old repo for:

- PT_LOAD placement
- zero-fill and brk tracking
- initial stack layout for argc / argv / envp / auxv
- PT_TLS parsing and TLS template capture
- thread-pointer / TLS-base initialization expected by AArch64 user-space runtimes

### 4. Syscall Parity

Diff the current AArch64 Linux syscall handler against the old implementation and bring
current behavior up to parity, prioritizing:

- syscalls seen in fish traces
- memory-management syscalls
- file / fd / stat / process-setup syscalls
- thread / TLS-adjacent syscalls if they affect allocator or runtime startup

## Verification Strategy

Verification happens at four levels:

1. decode / execute regression tests in `runtime/helm-arch/tests/`
2. loader and syscall integration tests in `runtime/helm-engine/tests/`
3. end-to-end SE workload checks with fish
4. targeted comparison against `../helm.git` and `qemu-aarch64` when diagnosing drift

Key runtime signals:

- stub-tracer counts should trend toward zero on the fish path
- fault PC should move past the current allocator abort
- fish should eventually complete `echo hello`

## Success Criteria

- the active AArch64 SE runtime no longer silently stubs instructions that the old repo
  implemented and that are required by the workload
- ELF loading and TLS setup match old-repo expectations closely enough for real static
  AArch64 user-space binaries
- required Linux syscalls behave compatibly with the old SE runtime
- `helm-aarch64 examples/se/run_binary.py` for fish progresses beyond the current
  `__libc_free` abort and ultimately succeeds

## Notes

- The workspace is currently dirty, so this design note is intentionally not committed
  separately to avoid mixing unrelated work into a planning commit.

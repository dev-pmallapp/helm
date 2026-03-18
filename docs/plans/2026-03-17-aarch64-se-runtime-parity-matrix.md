# AArch64 SE Runtime Parity Matrix

## Purpose

Track behavioral parity between the current AArch64 SE runtime path and the old
reference implementation in `../helm.git`.

This matrix covers only:

- active ISA decode / execute behavior
- Linux AArch64 syscall emulation
- ELF loading, process setup, and TLS

## Status Legend

- `done` — parity confirmed or already fixed in the active path
- `gap` — old repo implements behavior that current active path lacks
- `drift` — both paths implement behavior, but semantics or plumbing differ
- `needs-proof` — suspected gap, but still needs a targeted failing test

## Runtime Signal Summary

| Area | Current Status | Evidence |
|---|---|---|
| Runner stop reporting | done | `examples/se/run_binary.py` now stops on non-`quantum` and exits non-zero |
| Fish end-to-end run | gap | fish stops with `exception:breakpoint at pc=0x6974c0` |
| Fault location | done | guest `BRK` is inside `__libc_free`, indicating prior guest-state corruption |
| Stub tracer reliability | drift | current classifier still labels some implemented SIMD ops as stubs |

## ISA Parity

### Active-path architecture

| Item | Old repo | Current repo | Status | Notes |
|---|---|---|---|---|
| Engine execution path | `cpu.step()` / `step_fast()` with rich SIMD/ldst helpers | `decode.rs` + `execute.rs` through `aarch64_execute` | drift | parity work must target current active path |
| Alternate richer SIMD path | n/a in same form | `step.rs` / `step_simd.rs` | drift | contains useful logic, but is not engine-active |

### Confirmed instruction findings

| Instruction family | Old repo | Current active path | Status | Notes |
|---|---|---|---|---|
| Compare-against-zero SIMD forms (`CMLT0_v`, related) | implemented | partially missing, now started | drift | `0x0e20a800` test exposed this; active path now has initial support |
| Fish-observed SIMD logic ops (`AND_v`, `EOR_v`) | implemented | implemented in execute path | drift | classifier still marks them as stubs, so stub-tracer over-reports |
| Fish-observed SIMD compare/reduce (`CMEQ_v`, `UMAXV`) | implemented | implemented in execute path | drift | same classifier issue; need semantic confirmation with tests |
| Broad old SIMD surface from generated decode tests | implemented | much larger than current verified surface | gap | port tests and close real decode/execute gaps family by family |

### Immediate ISA actions

- port old fish-observed SIMD decode tests into `runtime/helm-arch/tests/aarch64_decode.rs`
- port old exec semantics tests into `runtime/helm-arch/tests/aarch64_exec.rs`
- fix classifier drift in `runtime/helm-engine/src/lib.rs` so stub-tracer reflects real gaps

## ELF / Process Setup / TLS Parity

### Loader structure comparison

| Item | Old repo | Current repo | Status | Notes |
|---|---|---|---|---|
| `PT_LOAD` parsing | yes | yes | drift | current loader is flatter and permission-less |
| `brk_base` tracking | yes | yes | done | both compute a post-load brk base |
| `PT_TLS` parsing | yes (`TlsInfo`) | no | gap | old loader records TLS template, current loader drops it |
| TLS metadata in loaded result | yes (`tls_info`) | no | gap | current `LoadedBinary` has no TLS field |
| Address-space permissions | page-mapped with perms | flat zero-filled memory | drift | may matter for protection semantics and startup assumptions |
| Stack mapping | explicit stack map plus read-only guard page | synthetic stack bytes in flat memory | drift | current path may miss guard / access behavior expected by runtime |
| Auxv construction | yes | yes | drift | needs exact parity check for entry/startup expectations |
| Initial random bytes | zeros in old sample | `0x5E` bytes currently | drift | probably not root cause, but record it |

### TLS runtime plumbing

| Item | Old repo | Current repo | Status | Notes |
|---|---|---|---|---|
| SE runtime stores `tls_info` | yes | no | gap | current `load_aarch64_elf()` drops TLS information immediately |
| Thread pointer field in arch state | yes | yes (`tpidr_el0`) | done | field exists in current arch state |
| Initial TLS allocation / TP setup | yes | no visible active-path setup | gap | likely critical for musl / fish startup correctness |
| Clone / thread TLS handling | yes | unclear / likely partial | needs-proof | current syscall layer has clone constants, but parity needs targeted tests |

### Immediate loader/TLS actions

- add `TlsInfo` and `PT_TLS` parsing to current `runtime/helm-engine/src/loader/elf64.rs`
- thread TLS metadata through `runtime/helm-engine/src/lib.rs`
- add focused loader/TLS integration tests before relying on fish

## Linux AArch64 Syscall Parity

### Handler structure comparison

| Item | Old repo | Current repo | Status | Notes |
|---|---|---|---|---|
| Stateful AArch64 SE syscall handler | yes | yes | done | both track `brk`, `mmap`, exit state, binary path |
| `brk` handling | yes | yes | needs-proof | current behavior exists; exact parity still needs tests |
| `mmap` / `mprotect` / `munmap` | yes | yes | needs-proof | fish trace includes these early; semantics must be checked |
| `set_tid_address` | yes | yes constants/path present | needs-proof | old runtime threads more state through scheduler/TLS |
| TLS-aware clone handling | yes | unclear current active path | gap | old runtime explicitly manages child TLS / TPIDR_EL0 |
| Separate syscall test suite | yes (`helm-syscall` tests) | none in current repo | gap | current repo needs targeted engine-level syscall regressions |

### Fish-observed early syscalls

Observed from current fault-detect trace before allocator abort:

- `mmap` (`222`)
- `mprotect` (`226`)
- `sigaltstack` (`132`)
- `rt_sigaction` (`134`)
- `rt_sigprocmask` (`135`)
- `brk` (`214`)
- `munmap` (`215`)
- `getrandom` (`278`)
- `ioctl` (`29`)
- `ppoll` (`73`)
- `set_tid_address` (`96`)

These should be the first syscall parity test targets.

## Highest-Probability Root-Cause Zones

Ordered by current evidence:

1. **TLS initialization gap**
   - old repo captures and propagates `PT_TLS`
   - current active path has `tpidr_el0` support but no visible TLS setup
   - musl + fish are likely sensitive to correct TLS initialization

2. **Loader / initial process image drift**
   - current flat-memory loader differs materially from old address-space loader
   - stack / auxv / guard-page differences may corrupt early runtime assumptions

3. **Syscall semantics drift**
   - early `mmap` / `mprotect` / `brk` behavior can poison allocator metadata

4. **Remaining ISA semantic drift**
   - still possible, but current stub tracer overstates missing SIMD due to classifier drift

## Next Concrete Steps

1. Add the first parity tests for old fish-observed SIMD decode/exec cases.
2. Add loader/TLS tests that fail on missing `PT_TLS` support.
3. Add syscall tests for `mmap`, `mprotect`, `brk`, and `set_tid_address`.
4. Fix stub classification drift so runtime traces reflect true remaining ISA gaps.

# L4Re JIT Plan

## Status Update: 2026-04-12

### Stack alignment fix (session 2)

### XZR stale-value fix + stencil FS helper dispatch (session 3)

- Second root cause found: the dynasm block prologue did not re-zero the XZR
  sentinel slot.  Instructions like CMP (SUBS XZR, ...) write non-zero results
  to the flat-array XZR slot.  Subsequent blocks that read XZR (e.g. MOV X19,
  X0 encoded as ORR X19, XZR, X0) pick up the stale value, corrupting the
  destination register.  This caused X19=0x30 instead of X0's value.
- Third issue: the stencil backend hardcoded SE-mode memory helper addresses.
  In FS mode, stencil-compiled loads/stores called `jit_mem_read`/`jit_mem_write`
  (which interpret the context pointer as `FlatMem*`) instead of the correct
  `jit_fs_mem_read`/`jit_fs_mem_write`.  Fixed by threading runtime-selected
  helper addresses through `DecodedFields` and the new `JitBackend::set_mem_helpers()`
  trait method.
- Live workload verified: L4Re EL2 `l4re_hello-2_arm_virt.elf` now runs 2,000,000
  instructions with `--jit` in both dynasm-only and tiered modes, matching the
  interpreter's final PC.


- Root cause of the crash at guest PC `0x4100475c` identified: **x86-64 stack
  misalignment** in the dynasm memory helper call sites.
- The block prologue leaves RSP at 8 mod 16.  The helper call wrappers
  (`emit_mem_read`, `emit_mem_write`, TLB miss slow paths) pushed 6 registers
  (48 bytes) and subtracted 16, giving RSP at 8 mod 16 before `call` -- but
  the System V ABI requires RSP at 0 mod 16 before `call`.
- Fix: change `sub rsp, 16` to `sub rsp, 8` and adjust all `[rsp+N]` stack
  offsets by -8.  All four call sites (read, write, TLB miss read, TLB miss
  write) were corrected.
- Added a regression test exercising the exact L4Re prologue pattern (pre-index
  STP, post-index LDR, indirect BR) in FS system mode.

### EL2 gate removal (session 1)
- The AArch64 FS EL2/EL3 blanket JIT deferral in `runtime/helm-engine/src/jit.rs`
  has been removed.
- Focused engine regressions now prove:
  - EL2 system mode can resume after a bounded interpreter fallback and re-enter JIT
  - tiered HAJ can compile stencil-unsupported FS load/store starts via dynasm
- The dynasm load/store emitters were fixed to honor the runtime-selected
  memory helper slots in FS mode instead of unconditionally assuming the SE
  inline-TLB path. This was required to stop immediate crashes on FS dynasm
  fallback blocks.
- A live rerun of the EL2 `l4re_hello-2_arm_virt.elf` workload now shows real
  HAJ activity, not blanket deferral: early execution compiles blocks starting
  at `0x41000000`, `0x41000040`, `0x41000044`, `0x4100d280`, `0x41006a20`,
  and many later PCs in the same boot window.
- The next real live-workload blocker is narrower: the JIT run now crashes
  after compiling and reaching the block at guest PC `0x4100475c`. Disassembly
  of that block shows a mixed complex-addressing + indirect-call prologue:
  - `stp x29, x30, [sp, #-32]!`
  - `mov x29, sp`
  - `str x19, [sp, #16]`
  - `mov x19, x0`
  - `ldr x1, [x19], #56`
  - `ldr x1, [x1, #184]`
  - `blr x1`
- That makes the next slice clear: reproduce and fix the post-`0x4100475c`
  crash with a focused regression covering FS dynasm execution of the exact
  L4Re-style pre-index pair store, post-index load, and indirect branch shape.

## Scope

This note narrows the L4Re investigation to **performance-sensitive runtime
library paths** rather than general kernel or IPC correctness. The target
workloads are the shipped AArch64 L4Re ELFs under
`assets/aarch64/boot/l4re/`, with the main emphasis on:

- `l4re_hello-2_arm_virt.elf`
- `l4re_ipcbench_arm_virt-el1.elf`
- `l4re_ipcbench_arm_virt-el2.elf`
- `l4re_vm-basic_arm_virt.elf`

The relevant source tree is the extracted L4Re snapshot in `tmp/`, especially:

- `tmp/l4re-snapshot-26.03.0/src/l4/pkg/hello/server/src/main.c`
- `tmp/l4re-snapshot-26.03.0/src/l4/pkg/ipcbench/src/ipcbench.c`
- `tmp/l4re-snapshot-26.03.0/src/l4/pkg/ipcbench/src/ipc_common.c`
- `tmp/l4re-snapshot-26.03.0/src/l4/pkg/l4re-core/libc/musl/contrib/musl/src/string/`

## Workload Shape

### `hello`

`hello` is tiny and steady-state simple: it loops over `puts("Hello World!")`
and `sleep(1)`. That makes it useful mainly as a **string / stdio / syscall
smoke workload**, not a throughput benchmark.

### `ipcbench`

`ipcbench` is split into setup and steady-state phases:

- setup uses pthread creation, scheduler queries, capability lookup, error
  reporting, and some libc formatting/string routines
- the hot loop is repeated `l4_ipc_call(...)` / `l4_ipc_reply_and_wait(...)`

That means IPC itself dominates the benchmark core, but the **startup and
measurement harness still depend on libc copy/string paths** that can poison
overall JIT behavior if they fall back too early.

## Binary Evidence

The shipped L4Re ELFs are static stripped AArch64 executables. Even without
symbols, disassembly of the shipped binaries shows abundant early use of the
same code shapes across `hello`, `ipcbench`, and `vm-*` images:

- integer pair loads/stores (`ldp` / `stp`)
- byte loads (`ldrb`)
- zero-test branches (`cbz`)
- stack-frame pair save/restore sequences

Those patterns appear directly in the first text pages of the binaries, which
strongly suggests that the bundled runtime already inlines or links
copy/string-support code into the executable image rather than keeping it fully
out-of-line.

## Source Evidence: libc Fast Paths

### AArch64 `memcpy`

`tmp/l4re-snapshot-26.03.0/src/l4/pkg/l4re-core/libc/musl/contrib/musl/src/string/aarch64/memcpy.S`
is a hand-written AArch64 implementation that relies on:

- unaligned integer loads/stores
- `ldp` / `stp` pair traffic
- `ldr` / `str` / `ldrb` / `strb`
- `tbz`, `cbz`, `b.{cond}`
- address arithmetic and loop-carried writeback

This is a good near-term JIT target because it stays in the **integer ISA**.
The current dynasm backend already has emitters for many of these shapes, so
the main risk is not raw decode coverage but **real binary-slice continuity**
and avoiding unexpected system-mode fallback at function boundaries.

### AArch64 `memset`

`tmp/l4re-snapshot-26.03.0/src/l4/pkg/l4re-core/libc/musl/contrib/musl/src/string/aarch64/memset.S`
is the more important missing piece. Its fast path relies on:

- `dup v0.16B, wN`
- `str q0, [...]`
- `stp q0, q0, [...]`
- `mrs xN, dczid_el0`
- `dc zva, xN`

This is exactly where the current JIT falls short:

- `framework/helm-jit/src/dynasm/emit/mod.rs` routes `Mrs`, `Msr`, `Sys`, and
  `DcZva` to the system emitter
- `framework/helm-jit/src/dynasm/emit/system.rs` only tolerates `Nop`; all
  other system instructions terminate JIT compilation
- there is no dynasm emitter path for AArch64 SIMD/FP load/store or `dup`
  needed by musl `memset`

So `memset` is not just an optimization opportunity. It is the clearest
**library fast-path that will still force interpreter fallback** in its current
form.

### Generic string scans

The musl implementations used here for `strlen`, `strcmp`, `memcmp`, `strchr`,
and `strnlen` are mostly generic scalar C loops or word-at-a-time C logic:

- `strlen.c` uses aligned word scanning with the `HASZERO` idiom
- `strcmp.c` and `memcmp.c` are simple byte loops
- `memmove.c` falls into `memcpy()` on the non-overlap path, otherwise runs
  scalar/aligned word copying

These are less about missing ISA coverage and more about **keeping hot loops in
compiled code without unnecessary exits**.

## Current JIT Assessment

### What is already aligned with L4Re string/copy code

The current AArch64 JIT stack already has support or tests for several shapes
that matter for the integer copy paths:

- `ldp` / `stp`
- `ldrb` / `strb` / `strh`
- `cbz` / `tbnz` / `tbz`
- integer add/sub/logical forms

That means the observed integer-heavy `memcpy`-like sequences in the shipped
ELFs are plausible JIT candidates today.

### What is still missing

The main remaining gaps for L4Re libc performance are:

1. **AArch64 system-instruction compilation boundary**
   - `mrs ... dczid_el0`
   - `dc zva`
   - more generally, user-safe system instructions that do not need a full
     interpreter handoff
2. **AArch64 AdvSIMD/vector-memory emitters**
   - `dup v0.16B`
   - `str qN`
   - `stp qN, qM`
3. **Binary-slice validation for musl `memcpy`**
   - even though coverage appears present, there is no L4Re-specific regression
     proving the real copy loop compiles end-to-end in system mode
4. **String-scan throughput tracking**
   - `strlen` / `strcmp` / `memcmp` likely compile, but there is no targeted
     measurement proving they stay in JIT and do not churn through short blocks

## Recommended Plan

### Phase 1: Prove integer libc fast paths on real L4Re slices

Goal: validate that the current JIT already handles the observed integer-heavy
copy and scan shapes from the shipped ELFs.

Tasks:

- extract small binary slices matching the observed `ldp/stp/ldrb/cbz` copy
  patterns from the shipped L4Re ELFs
- add engine or `helm-jit` regressions that run those slices under JIT and
  assert:
  - no unsupported-block fallback
  - block compilation occurs in system mode
  - guest results match interpreter
- add a tiny benchmark tier for:
  - `memcpy`-like integer pair-copy loops
  - `strlen`-like word/byte scan loops

Why first:

- this is the cheapest way to convert the current L4Re investigation from
  source inspection into hard JIT evidence
- it avoids prematurely building new emitters for code that may already be
  good enough

### Phase 2: Add a user-safe JIT path for `DCZID_EL0` / `DC ZVA`

Goal: stop musl `memset` from immediately forcing interpreter fallback.

Tasks:

- define a narrow JIT-safe subset for AArch64 user-visible system operations:
  - `mrs xN, dczid_el0`
  - `dc zva, xN`
- implement them either:
  - directly in dynasm, or
  - via explicit helper calls that preserve compiled-block continuity
- add system-mode regressions covering:
  - zero-fill correctness
  - guest-visible `DCZID_EL0` value
  - alignment-sensitive `dc zva` behavior

Why second:

- this is the single clearest blocker for libc `memset`
- it directly addresses a hot primitive used by allocators, zero-init code,
  and library setup paths

### Phase 3: Add the minimal AdvSIMD subset needed by musl `memset`

Goal: compile the actual AArch64 `memset` fast path instead of bouncing back to
the interpreter around vector setup and stores.

Minimum opcode subset:

- `dup v0.16B, wN`
- `str q0, [base + imm]`
- `stp q0, q0, [base + imm]`

Follow-up scope if needed:

- `mov xN, vM.d[0]`-style scalar extraction aliases
- additional Q-register loads/stores used by broader libc routines

Required support work:

- JIT-side vector register plumbing must stay aligned with
  `runtime/helm-arch/src/aarch64/arch_state.rs`
- add direct tests matching the musl `memset.S` control flow, not only isolated
  opcode tests

Why not larger SIMD first:

- L4Re does not need broad SIMD arithmetic for this milestone
- the priority is **library throughput**, not generic FP coverage

### Phase 4: Track string-scan continuity and specialize only if needed

Goal: quantify whether `strlen` / `strcmp` / `memcmp` need more than the
current scalar JIT path.

Tasks:

- measure block sizes and fallback counts for compiled string scans
- only if needed, add micro-optimizations for:
  - word-at-a-time zero detection
  - compare/branch fall-through continuity
  - reduced helper churn in very short scan loops

This phase should stay data-driven. Do not preemptively add broad libc-specific
special cases unless the L4Re scan benchmarks show real churn.

## Priority Order

1. prove current `memcpy`/scan coverage on real L4Re binary slices
2. compile `DCZID_EL0` / `DC ZVA` without full interpreter fallback
3. add the minimal AdvSIMD memory subset required for musl `memset`
4. optimize scalar string scans only if measured block churn justifies it

## Explicit Non-Goals For This Slice

- full AArch64 FP/SIMD JIT coverage
- broad libc function-by-function tuning without binary-backed evidence
- L4Re kernel or IPC semantic changes
- speculative SVE/SME support

## Summary

For the current L4Re binaries, the immediate performance opportunity is not
"general L4Re support". It is much narrower:

- **confirm integer bulk-copy/string loops already compile well**
- **remove the `memset` fast-path fallback on `MRS DCZID_EL0` + `DC ZVA`**
- **add only the minimal vector-memory subset needed for musl AArch64 `memset`**

That sequence keeps the work small, testable, and directly tied to the libc
paths that affect L4Re startup and benchmark behavior.

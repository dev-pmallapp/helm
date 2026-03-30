# Execution Pipeline

How guest instructions flow through helm-ng from fetch to retirement.

## Overview

The execution pipeline lives in `helm-engine` and is generic over the
timing model via `HelmEngine<T: TimingModel>`. The pipeline handles
three ISAs (AArch64, RISC-V RV64GC, AArch32) via enum dispatch in
`helm-arch`.

## Interpreter Path

The default execution path is a tight interpret loop. Each iteration:

```text
┌──────────────────────────────────────────────────┐
│  HelmEngine<T>::run(quantum)                     │
│                                                  │
│  for each instruction in quantum:                │
│    1. FETCH   — read 4 bytes at PC from FlatMem  │
│    2. DECODE  — aarch64_decode(bytes) → Insn     │
│    3. EXECUTE — aarch64_execute(state, insn, mem) │
│    4. TIMING  — T::on_insn(info)                 │
│    5. PROBES  — fire CpuProbes if active         │
│    6. RETIRE  — insns_retired += 1, advance PC   │
│    7. EVENTS  — drain EventQueue if due          │
│                                                  │
│  return StopReason                               │
└──────────────────────────────────────────────────┘
```

### Fetch

In SE mode, `FlatMem` provides direct host-pointer access to guest
memory via a flat page table (`Vec<*mut u8>`, one entry per 4K page).
The fast path uses `copy_nonoverlapping` to read instruction bytes
with zero overhead.

In FS mode, `TranslatingMem` wraps the address space. The fetch
address is first translated via the MMU page walker, then read from
`HelmAddressSpace`. A 256-entry software TLB accelerates repeated
translations.

### Decode

`helm-arch::aarch64::decode()` is a hand-written decoder covering
~100 opcode variants: data processing, branches, load/store, FP
scalar, SIMD, LSE atomics, system instructions. It returns an
`Aarch64Insn` enum variant with decoded fields.

For RISC-V, `helm-arch::riscv::decode()` handles RV64GC including
compressed (C extension) expansion via `riscv_expand_c()`.

### Execute

`helm-arch::aarch64::execute()` (~2100 lines) is organized into
submodules:

| Module | Instructions |
|--------|-------------|
| `dp.rs` | Data processing: ADD, SUB, AND, ORR, shifts, extends, bitfield |
| `branch.rs` | B, BL, BR, BLR, CBZ, TBZ, RET, SVC, HVC, SMC, PSCI stubs |
| `ldst.rs` | LDR, STR, LDP, STP, LDXR, STXR, atomics (LSE), DC ZVA |
| `fp.rs` | FP scalar: FADD, FMUL, FCVT, rounding variants, FJCVTZS |
| `simd.rs` | SIMD: vector arith, DotProduct, element-wise, across-lanes |
| `mul_div.rs` | MUL, SMULL, UMULL, SDIV, UDIV, MADD, MSUB |
| `sysreg.rs` | MRS, MSR, ~40 system registers |
| `helpers.rs` | Shared utilities (`pub(super)` functions) |

### Timing

After each instruction, the timing model receives an `on_insn()` call
with `TimingInsnInfo` (PC, instruction class, flags). The timing model
updates its cycle counter:

- **VirtualTiming** — adds 1 cycle per instruction (IPC=1)
- **IntervalTiming** — adds class-specific latency, penalizes cache
  misses and branch mispredicts
- **AccurateTiming** — full pipeline simulation (placeholder)

Because `HelmEngine<T>` is generic, the timing call is monomorphized:
zero vtable overhead in the hot loop.

### Probes

`CpuProbes` from `helm-probe` provides typed probe points that fire
on pre-step, post-step, fault, memory access, and branch events. In
release builds with no active probes, the probe check compiles to a
single branch-not-taken.

## JIT Path

When the `jit-dynasm` feature is enabled, `HelmEngine` can switch to
JIT execution via `run_jit()`. The JIT path:

1. Check `JitCache` for a compiled block at the current PC
2. On miss: decode a basic block of instructions, compile via
   `JitBackend::compile()`, store in cache
3. On hit: call the compiled block's entry point directly
4. The compiled block operates on a `[u64; 48]` register array passed
   in the first argument register (`rdi` on x86-64)

The `JitBackend` trait is pluggable. The default `dynasm` backend
generates x86-64 machine code via `dynasm-rs`. Alternative backends
(Cranelift, LLVM) can implement the same trait.

## Full-System Path

In FS mode (`step_aarch64_fs()`), additional work happens per step:

```text
1. IRQ CHECK  — if irq_pending && PSTATE.I == 0, deliver exception
2. FETCH      — MMU translate PC, check TLB, walk page table on miss
3. DECODE     — same as SE
4. EXECUTE    — uses TranslatingMem (VA→PA wrapper)
5. TIMER      — check CNTP_CVAL vs tick, fire if enabled + unmasked
6. TLB FLUSH  — if tlb_flush_pending, invalidate TLB
```

`TranslatingMem` snapshots the MMU configuration (SCTLR, TCR, TTBR)
before execute to avoid borrow conflicts between the executor
modifying arch state and the memory subsystem reading translation
registers.

## StopReason

The `run()` method returns a `StopReason` enum:

| Variant | Meaning |
|---------|---------|
| `Quantum` | Instruction budget exhausted, resume later |
| `Exit { code }` | Guest called exit syscall |
| `Exception(HartException)` | Unhandled exception |
| `Unsupported` | Encountered unimplemented instruction |

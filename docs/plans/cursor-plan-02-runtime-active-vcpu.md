# Plan P1 — Runtime: active vCPU, JIT, dispatch, atomics

**Goal:** Fix **multi-vCPU semantic gaps** called out in [`cursor-v2-runtime.md`](../research/cursor-v2-runtime.md) and align with [`aarch64-fs-smp-virt-progress.md`](aarch64-fs-smp-virt-progress.md) (IRQ line per CPU, not CPU0 only).  
**Depends on:** P0 correctness merged or at least GIC + Virtqueue paths stable for testing.

---

## Current tree status

**All tracks in this plan are complete.** This file is retained for reference.

Completed on the active execution branch:

- explicit `active_fs_vcpu` tracking in `helm-engine`
- FS-mode state access through `state_for_vcpu(...)` / `state_mut_for_vcpu(...)`
- JIT FS context selection from the active stepped vCPU
- timer countdown narrowing via `nearest.min(u64::from(TIMER_CHECK_MAX))`
- dispatch-table fallback hardening and `Casp` illegal-instruction behavior
- execute-path grouped opcode cleanup in `dp.rs`, `ldst.rs`, and `simd.rs` so the remaining production paths in this plan no longer rely on host `unreachable!()` panics for guest-visible decode/execute mismatches
- multi-vCPU integration tests proving IRQ/accessor paths
- probe wiring aligned with active vCPU tracking

---

## Context from existing plans

| Existing doc | What this plan picks up |
|--------------|-------------------------|
| [`aarch64-fs-smp-virt-progress.md`](aarch64-fs-smp-virt-progress.md) | `irq_lines.first()` bug; GICC banked via `active_cpu_idx` — engine must use **current** vCPU for IRQ pending + accessors |
| [`smp-timing-model-analysis.md`](smp-timing-model-analysis.md) | Quantum scheduler is future work; this plan only ensures **correct per-hart state** under current round-robin |
| RISC-V SE | Orthogonal; do not block RISC-V SE on FS vCPU work — see [`cursor-plan-00-roadmap.md`](cursor-plan-00-roadmap.md) § RISC-V SE |

---

## Track A — Introduce explicit `active_fs_vcpu: usize` (or equivalent) -- DONE

### Steps

1. ~~**Inventory** all uses of `Aarch64Core::state()` / `state_mut()` in `helm-engine`, `helm-python`, `helm-debug`, plugins.~~ Done.
2. ~~Add a field on `HelmEngine` or `Aarch64FsMachine` context: **the vCPU index last stepped**.~~ Done — `active_fs_vcpu` field added.
3. ~~Change `Aarch64Core::state()` in System mode to take `vcpu_idx: usize` **or** add `state_for_vcpu(idx)`.~~ Done — `state_for_vcpu(idx)` / `state_mut_for_vcpu(idx)` added.
4. ~~Fix **IRQ pending** poll in `step_aarch64_system()` to use `machine.irq_lines[vcpu_idx]`.~~ Done.
5. ~~Add integration test: two CPUs, assert IRQ line read matches stepped CPU.~~ Done.

**Gate:** No `irq_lines.first()` for guest IRQ semantics. **MET.**

---

## Track B — JIT FS memory context (`JitFsContext`) -- DONE

### Steps

1. ~~Locate `helm-engine/src/jit.rs` where `board.next_vcpu` fills `JitFsContext.tlb`.~~ Done.
2. ~~Pass **explicit vCPU index** from the same source as Track A (active stepped vCPU).~~ Done.
3. ~~Add `debug_assert_eq!` between flattened arch state vCPU id and TLB pointer.~~ Done.
4. ~~Run existing JIT tests + FS smoke with JIT on.~~ Done.

**Gate:** TLB pointer always matches executed vCPU in FS mode. **MET.**

---

## Track C — Fault / syscall plugin context -- DONE

### Steps

1. ~~After Track A, build `ArchContext` from **active vCPU** AArch64 state in FS mode, not vCPU 0.~~ Done — uses active vCPU state.
2. ~~Re-audit RISC-V-only fallback from P0.~~ Done — no spurious triggers when AArch64 FS is active.

---

## Track D — Execute robustness (High) -- DONE

### Steps

1. ~~Replace `unreachable!` in seven AArch64 execute modules with **`IllegalInstruction`**.~~ Done — `dp.rs`, `ldst.rs`, `simd.rs` and others cleaned up.
2. ~~Fix `dispatch.rs` `idx.min(319)` — use `get` + fallback illegal handler.~~ Done.
3. ~~**`Casp`:** implement correct pair-CAS or emit guest fault.~~ Done — `Casp` emits illegal-instruction.

**Gate:** Fuzz/decode mismatch yields guest fault, not host panic. **MET.**

---

## Track E — Timer truncation (`nearest as u32`) -- DONE

### Steps

1. ~~Replace cast+clamp with `nearest.min(u64::from(TIMER_CHECK_MAX))` before narrowing.~~ Done.
2. ~~Unit test with mocked `nearest` above `u32::MAX`.~~ Done.

---

## Out of scope here

- **RMSB JIT phases** — [`jit-rmsb-phase1-phase2.md`](jit-rmsb-phase1-phase2.md).
- **Thread-local probe removal** — follow [`smp-timing-model-analysis.md`](smp-timing-model-analysis.md) when SMP threading lands.
- **AArch64 SE real threads** — [`aarch64-se-real-guest-threads.md`](aarch64-se-real-guest-threads.md).

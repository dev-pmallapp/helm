# Plan P1 — Runtime: active vCPU, JIT, dispatch, atomics

**Goal:** Fix **multi-vCPU semantic gaps** called out in [`cursor-v2-runtime.md`](../research/cursor-v2-runtime.md) and align with [`aarch64-fs-smp-virt-progress.md`](aarch64-fs-smp-virt-progress.md) (IRQ line per CPU, not CPU0 only).  
**Depends on:** P0 correctness merged or at least GIC + Virtqueue paths stable for testing.

---

## Current tree status

Completed on the active execution branch:

- explicit `active_fs_vcpu` tracking in `helm-engine`
- FS-mode state access through `state_for_vcpu(...)` / `state_mut_for_vcpu(...)`
- JIT FS context selection from the active stepped vCPU
- timer countdown narrowing via `nearest.min(u64::from(TIMER_CHECK_MAX))`
- dispatch-table fallback hardening and `Casp` illegal-instruction behavior
- execute-path grouped opcode cleanup in `dp.rs`, `ldst.rs`, and `simd.rs` so the remaining production paths in this plan no longer rely on host `unreachable!()` panics for guest-visible decode/execute mismatches

Remaining work in this plan is now narrower:

- prove the multi-vCPU IRQ/accessor path with the explicit integration coverage called out below
- continue any remaining guest-fault cleanup outside these execute modules if future audits find more host-panic paths

---

## Context from existing plans

| Existing doc | What this plan picks up |
|--------------|-------------------------|
| [`aarch64-fs-smp-virt-progress.md`](aarch64-fs-smp-virt-progress.md) | `irq_lines.first()` bug; GICC banked via `active_cpu_idx` — engine must use **current** vCPU for IRQ pending + accessors |
| [`smp-timing-model-analysis.md`](smp-timing-model-analysis.md) | Quantum scheduler is future work; this plan only ensures **correct per-hart state** under current round-robin |
| RISC-V SE | Orthogonal; do not block RISC-V SE on FS vCPU work — see [`cursor-plan-00-roadmap.md`](cursor-plan-00-roadmap.md) § RISC-V SE |

---

## Track A — Introduce explicit `active_fs_vcpu: usize` (or equivalent)

### Steps

1. **Inventory** all uses of `Aarch64Core::state()` / `state_mut()` in `helm-engine`, `helm-python`, `helm-debug`, plugins — list from [`cursor-runtime.md`](../research/cursor-runtime.md).
2. Add a field on `HelmEngine` or `Aarch64FsMachine` context: **the vCPU index last stepped** (or currently stepping). Update it at the **single** point where `pick_next_fs_vcpu` returns.
3. Change `Aarch64Core::state()` in System mode to take `vcpu_idx: usize` **or** add `state_for_vcpu(idx)` and deprecate bare `state()` for FS mode (compile-time or runtime guard).
4. Fix **IRQ pending** poll in `step_aarch64_system()` to use `machine.irq_lines[vcpu_idx]` per [`aarch64-fs-smp-virt-progress.md`](aarch64-fs-smp-virt-progress.md).
5. Add integration test: two CPUs, assert IRQ line read matches stepped CPU (may require minimal GIC + synthetic assert).

**Gate:** No `irq_lines.first()` for guest IRQ semantics.

---

## Track B — JIT FS memory context (`JitFsContext`)

**Refs:** [`cursor-v2-runtime.md`](../research/cursor-v2-runtime.md) RC2, [`cursor-runtime.md`](../research/cursor-runtime.md) C3.

### Steps

1. Locate `helm-engine/src/jit.rs` where `board.next_vcpu` fills `JitFsContext.tlb`.
2. Pass **explicit vCPU index** from the same source as Track A (active stepped vCPU).
3. Add `debug_assert_eq!` between flattened arch state vCPU id and TLB pointer (debug builds).
4. Run existing JIT tests + FS smoke with JIT on.

**Gate:** TLB pointer always matches executed vCPU in FS mode.

---

## Track C — Fault / syscall plugin context

### Steps

1. After Track A, build `ArchContext` from **active vCPU** AArch64 state in FS mode, not vCPU 0.
2. Re-audit RISC-V-only fallback from P0 — must not trigger when AArch64 FS is active but accessor was wrong.

---

## Track D — Execute robustness (High)

### Steps

1. Replace `unreachable!` in seven AArch64 execute modules with **`IllegalInstruction`** (or internal fault) — paths listed in [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) §5.
2. Fix `dispatch.rs` `idx.min(319)` — use `get` + fallback illegal handler (see audit).
3. **`Casp`:** either implement minimal correct pair-CAS **or** emit guest fault / `ENOSYS`-style behavior **documented** — silent no-op is unacceptable for production.

**Gate:** Fuzz/decode mismatch yields guest fault, not host panic.

---

## Track E — Timer truncation (`nearest as u32`)

**Refs:** [`cursor-runtime.md`](../research/cursor-runtime.md) C2.

### Steps

1. Replace cast+clamp with `nearest.min(u64::from(TIMER_CHECK_MAX))` before narrowing, or use saturating math.
2. Unit test with mocked `nearest` above `u32::MAX`.

---

## Out of scope here

- **RMSB JIT phases** — [`jit-rmsb-phase1-phase2.md`](jit-rmsb-phase1-phase2.md).
- **Thread-local probe removal** — follow [`smp-timing-model-analysis.md`](smp-timing-model-analysis.md) when SMP threading lands.
- **AArch64 SE real threads** — [`aarch64-se-real-guest-threads.md`](aarch64-se-real-guest-threads.md).

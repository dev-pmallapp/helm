# JIT / Interpreter Performance Implementation Plan

> Concrete execution plan derived from the April 4, 2026 performance research.
>
> Goal: break the current ~9 MIPS plateau where the interpreter and JIT perform
> similarly, and move `helm-ng` toward a design where the JIT is structurally
> advantaged instead of frequently collapsing back to interpreter-like behavior.

---

## 1. Executive Summary

The current performance ceiling is caused less by decode cost and more by
execution-structure issues:

1. In SE mode, one unsupported block can push the rest of a huge quantum into
   the interpreter.
2. The JIT is still mostly block-oriented and branch-exit-heavy.
3. Several optimization features exist in-tree but are not wired into the live
   execution loop.
4. The interpreter still pays avoidable memory-instrumentation overhead in
   `VirtualTiming`.

This plan therefore prioritizes:

1. fixing fallback and fast-path policy,
2. activating already-designed structural optimizations,
3. improving cache and control-flow shape,
4. expanding opcode coverage only after the execution structure is sound.

---

## 2. Current Diagnosis

### Confirmed design bottlenecks

- `runtime/helm-engine/src/jit.rs`
  - SE-mode unsupported-block fallback runs the interpreter for the full
    remaining quantum.
  - `run_jit()` is block-JIT-only; trace-JIT code is not integrated.
- `examples/se/run_binary.py`
  - default JIT runner chunk size is `50_000_000`, which amplifies the cost of
    whole-quantum fallback.
- `framework/helm-jit/src/cache.rs`
  - JIT block cache is still a 4096-entry direct-mapped cache.
- `framework/helm-jit/src/dynasm/emit/branch.rs`
  - conditional branches still exit on both taken and fall-through paths.
- `framework/helm-jit/src/dynasm/emit/mod.rs`
  - opcode coverage is still narrow enough that realistic workloads frequently
    terminate compilation early.
- `framework/helm-jit/src/regs.rs`
  - adaptive binding infrastructure exists, but the access-tracking path is not
    actually feeding it useful dynamic data.
- `framework/helm-jit/src/helpers.rs`
  - inline-cache specialization scaffolding exists, but the active runtime does
    not arm it before block execution.
- `runtime/helm-engine/src/lib.rs`
  - `VirtualTiming` still routes loads/stores through instrumentation because
    `decoded.records_mem_access` is enough to enable it.

### Design consequence

The JIT is currently paying too many interpreter-like costs:

- frequent returns to the dispatch loop,
- broad fallback windows,
- limited hot-path continuity,
- cold-path mechanisms present but inactive.

That makes parity with the interpreter unsurprising.

---

## 3. Success Metrics

Use both synthetic and realistic workloads.

### Primary metrics

- SE synthetic tight-loop MIPS
- SE mixed real workload MIPS
- FS boot-phase MIPS
- JIT cache hit rate
- JIT cache eviction rate
- percentage of instructions executed in JIT vs interpreter
- unsupported-opcode dynamic frequency
- average instructions per compiled block
- average instructions per trace
- branch side-exit rate

### Acceptance targets by phase

- Phase 0:
  - metrics visible and trusted
- Phase 1:
  - JIT materially outperforms interpreter on mixed SE workloads
  - unsupported-block fallback no longer collapses most of a chunk
- Phase 2:
  - interpreter improves measurably in `VirtualTiming`
- Phase 3:
  - JIT cache evictions reduced substantially on real programs
  - average JIT block length increases
- Phase 4:
  - trace execution becomes observable in live runs
  - branch-heavy loops show clear JIT speedup over block-only mode

---

## 4. Phase Plan

## Phase 0: Measurement First

### Goal

Make performance changes evidence-driven and guard against misleading MIPS
numbers.

### Tasks

1. Add engine-level counters for:
   - `jit_blocks_compiled`
   - `jit_blocks_executed`
   - `jit_block_cache_hits`
   - `jit_block_cache_misses`
   - `jit_block_cache_evictions`
   - `jit_fallback_count`
   - `jit_fallback_insns`
   - `jit_unsupported_opcode_count`
   - `jit_trace_hits`
   - `jit_trace_misses`
   - `jit_trace_guard_exits`
2. Add per-opcode unsupported-frequency counting.
3. Add a mixed-workload benchmark in addition to the synthetic tight loop.
4. Expose counters in `sim.stats()` and/or debug output.

### Files

- `runtime/helm-engine/src/jit.rs`
- `framework/helm-jit/src/cache.rs`
- `runtime/helm-python/src/*` or stats plumbing
- benchmark files under `runtime/helm-engine/benches/`

### Verification

- benchmark runs print stable JIT/interpreter split data
- unsupported-opcode histogram is visible
- cache hit/miss/eviction data is visible

---

## Phase 1: Fix JIT Fallback Granularity

### Goal

Prevent SE-mode unsupported sites from collapsing the rest of a large quantum
into the interpreter.

### Tasks

1. Change SE-mode fallback in `run_jit()` from:
   - `batch = remaining quantum`
   to:
   - small bounded interpreter batch, similar in spirit to FS mode
2. After the small interpreter batch:
   - re-enter JIT immediately
3. Add counters for:
   - fallback batch size
   - fallback frequency
4. Tune Python runner chunking only if needed after engine-side fix.

### Files

- `runtime/helm-engine/src/jit.rs`
- optionally `examples/se/run_binary.py`

### Acceptance

- mixed SE workloads show meaningful JIT/non-JIT separation
- `jit_fallback_insns / total_insns` drops materially
- JIT resumes quickly after unsupported regions

### Risk

- too-small fallback batches can increase overhead through frequent transitions
- mitigate with bounded tuning and benchmark comparison

---

## Phase 2: Restore Interpreter Fast-Path Discipline

### Goal

Stop paying memory-recording overhead in `VirtualTiming` when it buys nothing.

### Tasks

1. Add an explicit timing-model capability for whether memory-access recording
   is required on the hot path.
2. In `step_aarch64()` and FS equivalents:
   - gate `InstrumentedMem` / `InstrumentedTranslatingMem` use on actual need
   - do not force mem instrumentation solely because the instruction is a
     load/store when the timing model does not use the records
3. Preserve existing behavior for:
   - probes
   - plugins
   - interval/accurate timing

### Files

- `framework/helm-timing/src/lib.rs`
- `runtime/helm-engine/src/lib.rs`
- `runtime/helm-engine/src/fs.rs`

### Acceptance

- interpreter MIPS improves in `VirtualTiming`
- interval/accurate timing tests remain correct
- mem callbacks/probes still fire exactly when enabled

### Tests

- extend existing timing hook tests
- add regression tests for probe/plugin behavior

---

## Phase 3: Clean Up Partial / Inert Optimizations

### Goal

Either fully wire dormant optimization systems into execution or remove them
from the active roadmap until scheduled.

### Tasks

1. Adaptive binding
   - feed `RegHeatMap::record_access()` from real dynamic use
   - update sampling by retired instructions, not by compiled-block count
   - decide whether binding changes should remain global or become per-workload
2. Inline-cache specialization
   - wire `set_ic_patch_ctx()` before block execution
   - clear it deterministically after block return
   - validate invalidation on `brk`/`mmap`/`munmap`
3. Block chaining metadata
   - wire `back_refs` population
   - verify unlink behavior on cache eviction
4. If any item cannot be finished promptly:
   - remove dead code paths and document them as deferred

### Files

- `framework/helm-jit/src/regs.rs`
- `framework/helm-jit/src/helpers.rs`
- `framework/helm-jit/src/block.rs`
- `framework/helm-jit/src/cache.rs`
- `runtime/helm-engine/src/jit.rs`

### Acceptance

- adaptive binding uses actual access data
- IC specialization can be observed in counters/logs
- chaining invalidation is correct under eviction

---

## Phase 3.5: JIT Runtime Boundary Refactor

### Goal

Move JIT execution policy out of `runtime/helm-engine` and into
`framework/helm-jit`, while keeping `helm-stats` as the typed metrics layer
instead of an execution-orchestration crate.

### Why this is a separate slice

- It is architectural work, not a narrow perf-only change.
- Mixing it into Phase 0/1 would blur root-cause measurement.
- It should happen after the first fallback/fast-path fixes so the runtime
  behavior is measurable before the boundary moves.
- It should happen before deep trace integration so trace execution is built
  on the intended crate boundary.

### Target boundary

- `framework/helm-stats`
  - owns typed counters and registries only
- `framework/helm-jit`
  - owns JIT executor / tiering loop / cache policy / fallback policy / trace policy
- `runtime/helm-engine`
  - implements a host trait for fetch, state access, memory context,
    interpreter batches, timing/probe/plugin integration

### Tasks

1. Define a minimal host trait in `framework/helm-jit`
   - examples: fetch, memory pointers, state sync, interpreter batch entry,
     stop-reason handoff
2. Move the active `run_jit()` control loop behind that trait.
3. Move JIT perf counters to typed `helm-stats` structures.
4. Keep `HelmEngine` as the host implementation and policy caller only.
5. Preserve feature-gated backend selection and existing Python-facing behavior.

### Files

- `framework/helm-jit/src/*`
- `framework/helm-stats/src/lib.rs`
- `runtime/helm-engine/src/jit.rs`
- possibly `runtime/helm-engine/src/lib.rs`

### Acceptance

- `helm-engine` no longer owns the bulk of JIT tiering/fallback logic
- JIT counters flow through typed `helm-stats` interfaces
- no behavior regression in JIT enable/disable, fallback, or stats reporting

### Recommended position in roadmap

Do this after:

1. Phase 0 instrumentation
2. Phase 1 fallback granularity
3. preferably Phase 2 interpreter fast-path cleanup

Do this before:

1. deep trace-JIT activation work
2. larger cache-policy redesign

---

## Phase 4: Improve JIT Cache And Block Continuity

### Goal

Reduce cache thrash and grow the amount of work done per dispatch return.

### Tasks

1. Replace 4096-entry direct-mapped cache with:
   - 2-way or 4-way set-associative cache, or
   - hash-based cache with collision chains
2. Track per-entry:
   - execution count
   - target/patch metadata
   - trace root eligibility
3. Improve branch structure:
   - keep hot fall-through inside compiled code
   - only exit on true side exits
4. Increase average compiled region length before returning.

### Files

- `framework/helm-jit/src/cache.rs`
- `framework/helm-jit/src/dynasm/emit/branch.rs`
- `framework/helm-jit/src/dynasm/mod.rs`
- `framework/helm-jit/src/stencil/*`

### Acceptance

- cache eviction rate drops
- average compiled block length rises
- branch-heavy workloads show improved JIT scaling

---

## Phase 5: Activate Trace JIT

### Goal

Turn the existing trace infrastructure into a live execution tier.

### Tasks

1. Integrate `TraceRecorder` into `run_jit()`
   - detect hot backward branches
   - record traces from real execution
2. Add a trace cache lookup before normal block-cache execution.
3. On guard exits:
   - update miss counts
   - retire bad traces
   - fall back to block JIT or interpreter briefly
4. Add invalidation rules for:
   - code patching
   - memory-layout changes
   - cache flushes
5. Add counters:
   - trace compiled
   - trace executed
   - average trace length
   - guard exits
   - retired traces

### Files

- `runtime/helm-engine/src/jit.rs`
- `framework/helm-jit/src/trace/mod.rs`
- `framework/helm-jit/src/trace/recorder.rs`
- `framework/helm-jit/src/trace/compiler.rs`
- `framework/helm-jit/src/trace/exit.rs`

### Acceptance

- trace execution is observable in live runs
- hot loops show materially better MIPS than block-only JIT
- guard exits remain correct and bounded

### Risk

- trace invalidation correctness
- side-exit churn on unstable branches

Mitigation:

- start with SE mode only
- keep conservative retirement thresholds

---

## Phase 6: Coverage Expansion Driven By Dynamic Data

### Goal

Expand JIT coverage based on what real workloads actually miss on.

### Tasks

1. Rank unsupported opcodes by dynamic frequency.
2. Implement emitters/stencils in this order:
   - opcodes that block loop bodies
   - addressing modes blocking common loads/stores
   - arithmetic/control ops that force frequent fallback
   - FP/SIMD only if profiling justifies it
3. Add coverage dashboards:
   - static opcode support
   - dynamic opcode support

### Files

- `framework/helm-jit/src/dynasm/emit/*`
- `framework/helm-jit/src/stencil/*`
- `framework/helm-jit/src/stencil/data/aarch64.rs`

### Acceptance

- unsupported frequency drops on target workloads
- JIT share of retired instructions rises

---

## 5. Recommended Execution Order

If only one vertical slice is executed at a time, do them in this order:

1. Phase 0 instrumentation
2. Phase 1 fallback granularity
3. Phase 2 interpreter `VirtualTiming` fast path
4. Phase 3 partial-feature cleanup
5. Phase 3.5 JIT runtime boundary refactor
6. Phase 4 cache + branch continuity
7. Phase 5 trace integration
8. Phase 6 dynamic coverage expansion

This order is deliberate:

- it fixes multiplicative losses first,
- it prevents false wins from misleading benchmarks,
- and it avoids spending weeks on opcode coverage while the runtime still
  drops out of JIT too aggressively.

---

## 6. Validation Matrix

For each phase, run:

1. Correctness
   - targeted unit tests
   - relevant integration tests
2. Performance
   - synthetic loop benchmark
   - mixed real SE workload
   - at least one FS benchmark or boot checkpoint
3. Stability
   - cache invalidation scenarios
   - code-patching or memory-layout-change scenarios

Minimum completion gate for any phase:

- no functional regressions
- explicit benchmark evidence
- documented before/after metrics

---

## 7. Out Of Scope For Early Phases

These are valuable but should not block the first wave:

- replacing the backend with a heavyweight IR framework
- broad FP/SIMD JIT before mixed-workload data demands it
- deep decoder refactors unrelated to measured bottlenecks
- multi-ISA unification work not directly tied to the active AArch64 hot path

---

## 8. Immediate Next Slice

The best first implementation slice is:

1. add JIT fallback counters,
2. change SE-mode unsupported fallback to a small bounded interpreter batch,
3. benchmark before/after on:
   - synthetic loop
   - `fish`
   - one FS workload if available.

This slice is small, measurable, and likely to produce the fastest visible
improvement in real workloads.

---

## 9. Related Documents

- `docs/research/jit-remodeled.md`
- `docs/research/jit-acceleration-no-llvm.md`
- `.workflow/.analysis/ANL-jit-interpreter-performance-2026-04-04/discussion.md`

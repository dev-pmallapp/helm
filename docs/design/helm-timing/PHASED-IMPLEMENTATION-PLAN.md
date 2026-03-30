# helm-timing — Phased Implementation Plan

## Goal

Turn `helm-timing` from a fixed-CPI placeholder layer into a real timing subsystem that is coherent with `helm-engine`, `helm-event`, and the future scheduler architecture.

This plan is specific to `helm-ng`. It is not a port plan from `../helm.git`.

---

## Phase 0: Contract Correction

### Objective

Make the design docs match the code and make the current placeholders explicit.

### Work

- Update `docs/design/helm-timing/*` to reflect current behavior
- Treat `IntervalTiming` and `AccurateTiming` as placeholders in docs and tests
- Add basic unit tests for current fixed-CPI behavior

### Exit Criteria

- No timing doc claims features that do not exist
- `helm-timing` has baseline tests for current models

---

## Phase 1: Engine Hook Completion

### Objective

Finish the basic plumbing between `helm-engine` and `helm-timing`.

### Work

- Feed precise memory accesses into `TimingModel::on_mem_access(...)`
- Feed branch outcomes into `TimingModel::on_branch(...)`
- Improve `TimingInsnInfo` so load/store direction and basic classing are correct
- Define a boundary policy in the engine run loop

### Required Decisions

- Whether `on_boundary(...)` stays on the trait
- Whether boundary detection lives in the timing model or engine
- Where event-queue draining is performed

### Exit Criteria

- All timing hooks are exercised by engine tests
- A test timing model can observe instruction, memory, branch, and boundary events

---

## Phase 2: Virtual Timing Completion

### Objective

Make `VirtualTiming` the first fully integrated timing mode, including simulated-time-driven event progression.

### Work

- Use boundary policy to advance event processing from `HelmEngine.events`
- Decide whether event draining happens every instruction, every quantum, or on configurable boundaries
- Ensure simulated cycle progression is monotonic and externally usable

### Notes

This phase should not chase realism. It should make virtual time operational and architecturally clean.

### Exit Criteria

- `VirtualTiming` drives event progression through engine-owned event dispatch
- Device/event timing behavior is tied to simulated cycles rather than retired instructions alone

---

## Phase 3: Real Interval Timing

### Objective

Replace the `IntervalTiming` wrapper with an actual analytical timing model.

### Minimum Scope

- Basic instruction classes
- Per-class latency estimates
- Branch penalty accumulation
- Memory-stall accumulation
- Fixed-size interval accounting

### Deferred from this phase

- Full cache hierarchy
- Full branch predictor model
- Per-core vendor fidelity

### Recommended Inputs

- Richer `TimingInsnInfo`
- Minimal `MicroarchProfile`

### Exit Criteria

- `IntervalTiming` no longer delegates to `VirtualTiming`
- Cycle output differs meaningfully on mixed workloads
- Direct and engine-level tests validate interval accounting

---

## Phase 4: Minimal `MicroarchProfile`

### Objective

Introduce immutable profile-backed configuration only after interval timing has real consumers.

### Work

- Add a small JSON-backed `MicroarchProfile`
- Provide generic built-in presets
- Parameterize `VirtualTiming` and `IntervalTiming` from the profile

### Constraints

- Keep schema intentionally small
- Do not design a full cache/OOO resource schema yet

### Exit Criteria

- Profile values measurably affect timing behavior
- Schema is validated and documented

---

## Phase 5: Accurate Timing, In-Order First

### Objective

Make `AccurateTiming` an actual timing model, starting with a simple in-order pipeline.

### Work

- Add per-stage or per-latency pipeline accounting
- Model basic hazards
- Model branch flushes
- Integrate memory stalls with pipeline timing

### Explicit Deferrals

- Register renaming
- ROB
- LSQ
- Full OoO scheduling

### Exit Criteria

- `AccurateTiming` no longer delegates to `VirtualTiming`
- Deterministic pipeline tests pass

---

## Phase 6: Scheduler and Multi-Hart Timing

### Objective

Make timing coherent in multi-hart execution and shared event scheduling.

### Work

- Reconcile local hart time with shared event time
- Decide whether `EventQueue` remains in `HelmEngine` or moves to a scheduler-owned layer
- Define quantum/boundary synchronization semantics

### Why this is late

Until single-hart timing semantics are correct, multi-hart timing only compounds ambiguity.

### Exit Criteria

- Shared event progression is well-defined
- Timing boundaries are coherent across harts

---

## Sequencing Rationale

This order is deliberate:

1. Fix the contract first
2. Complete engine plumbing second
3. Finish virtual timing before analytical timing
4. Add profile/config only when there are real consumers
5. Build accurate timing after interval timing
6. Tackle scheduler-scale timing last

Doing this in reverse would create large configuration and architecture surfaces with little executable value.

---

## Risks

### Over-design risk

The previous docs drifted by describing future architecture as current implementation. This plan avoids that by requiring each phase to close with tests and updated docs.

### Trait churn risk

`TimingModel` may need one API adjustment around boundary/event ownership. That should happen early, before interval or accurate logic lands.

### Metadata risk

If `TimingInsnInfo` stays too coarse, both interval and accurate timing will either be wrong or forced to duplicate decode logic. That is a Phase 1 issue, not a later optimization.

---

## Immediate Next Slice

The next implementation slice after this documentation update should be:

1. Add crate tests for current timing behavior
2. Add a test timing model in `helm-engine`
3. Wire at least one missing hook path, preferably branch outcomes

That is the smallest slice that converts this plan from documentation into executable progress.

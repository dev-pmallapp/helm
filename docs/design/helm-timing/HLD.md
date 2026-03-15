# helm-timing — High-Level Design

## Purpose

`helm-timing` provides the timing infrastructure for the Helm-ng simulator. It defines the `TimingModel` trait and three concrete implementations that span the accuracy/performance tradeoff space: `Virtual` (event-driven, fastest), `Interval` (Sniper-style analytical, moderate accuracy), and `Accurate` (cycle-accurate pipeline, highest fidelity). It also owns `MicroarchProfile`, the pluggable JSON microarchitecture configuration used by all three models.

The crate is the primary knob users turn to trade simulation speed for timing accuracy without changing any other component. A generic parameter `T: TimingModel` on `HelmEngine<T>` selects the model at compile time; no runtime dispatch cost is paid on the hot path.

---

## Crate Position in the DAG

```
helm-core ──► helm-timing ──► helm-engine
                    │
                    └──► helm-memory (CacheModel)
                    └──► helm-event (EventQueue)
```

`helm-timing` depends on `helm-core` (for `Cycles`, `Insn`, `MemAccess` types), `helm-memory` (for `CacheModel`), and `helm-event` (for `EventQueue`). It has no upward dependency on `helm-engine` or `helm-arch`.

---

## API Overview

### `TimingModel` Trait

```rust
pub trait TimingModel: Send + Sync + 'static {
    fn on_insn(&mut self, insn: &InsnInfo) -> Cycles;
    fn on_mem_access(&mut self, access: &MemAccess) -> Cycles;
    fn current_cycles(&self) -> Cycles;
    fn on_branch_outcome(&mut self, taken: bool, predicted: bool);
    fn on_interval_boundary(&mut self, eq: &mut EventQueue);
    fn profile(&self) -> &MicroarchProfile;
}
```

`on_insn` is the hot-path hook called once per committed instruction. It returns the estimated cycle delta for that instruction. The caller accumulates the total cycle count.

### Timing Model Implementations

| Struct | Strategy | Speed | Accuracy |
|--------|----------|-------|----------|
| `Virtual` | IPC=1.0 (configurable), drives EventQueue | Fastest | Low |
| `Interval` | OoO window + CPI stack, exact at miss events | Fast | Medium |
| `Accurate` | 5-stage in-order pipeline (Phase 0) | Slowest | High |

### `MicroarchProfile`

Immutable after construction (Q48). Loaded from a JSON file. Provides per-ISA latency tables, cache geometry, branch penalty, and prefetch hints used by all three timing models.

---

## Design Decisions Answered

**Q38 — Virtual tick = estimated cycles.**
`Virtual::on_insn` advances the cycle counter by `ceil(1.0 / ipc)` where `ipc` defaults to 1.0 and is set in `MicroarchProfile.virtual_ipc`. This gives a deterministic, reproducible pseudo-clock that device timers can consume without any pipeline model.

**Q39 — Virtual mode drives EventQueue.**
`Virtual::on_interval_boundary` calls `EventQueue::drain_until(current_cycles)` so device timer events fire at the right simulated time. Without draining the queue, UART baud-rate timers and interrupt controllers would never fire in virtual mode.

**Q40 — OoOWindow tracks RAW dependency chains.**
`Interval::OoOWindow` maintains a `reg_ready: [Cycles; 64]` table. Each instruction's issue cycle = `max(all_src_reg_ready_cycles)`. This is the Sniper "in-order issue, out-of-order completion" approximation of the critical path.

**Q41 — Interval boundary = fixed instruction count + miss events.**
Default interval length is 10,000 committed instructions (configurable). A cache miss also triggers an immediate boundary, because miss latency is the main source of CPI variance. Both triggers call `on_interval_boundary`.

**Q42 — Branch misprediction penalty = fixed value from MicroarchProfile.**
`MicroarchProfile.branch_mispredict_penalty_cycles` (default 15) is charged on every misprediction. This avoids needing a full predictor model in Interval mode while capturing the dominant penalty term.

**Q43 — Interval mode wraps helm-memory CacheModel.**
`IntervalTimed` holds an `Arc<CacheModel>` shared with `helm-memory`. Miss/hit outcomes come from the real cache model (Q32 answered in helm-memory: state persists between intervals). No separate software cache is maintained.

**Q44 — Accurate = 5-stage in-order pipeline in Phase 0.**
`AccuratePipeline` implements IF→ID→EX→MEM→WB with stall and forwarding logic. Full OoO (ROB, reservation stations, LSQ) is deferred to Phase 3 as a separate struct that implements the same `TimingModel` trait.

**Q45 — Structural hazards: functional unit latency table.**
`MicroarchProfile.fu_latencies: HashMap<FuClass, u8>` provides latency per functional-unit class (INT, BRANCH, MUL, DIV, FP, LD, ST). The EX stage stalls for the indicated number of cycles.

**Q46 — AccuratePipeline reuses helm-memory CacheModel.**
Same `Arc<CacheModel>` pattern as Interval. Cache state is consistent regardless of which timing model is active, enabling mode-switching mid-simulation for future tooling.

**Q47 — LSQ deferred.**
Phase 0 in-order pipeline serializes all loads and stores. Memory ordering is trivially correct in 5-stage in-order. LSQ and RVWMO/weak-memory enforcement are deferred to the OoO Phase 3 implementation.

**Q48 — MicroarchProfile is immutable after construction.**
Fields are private with getter methods. Python can read values but not write them after the profile is built. A new profile requires constructing a new `HelmEngine`.

**Q49 — Shipped profiles.**
`generic-inorder.json`, `generic-ooo.json`, `sifive-u74.json`, `cortex-a72.json` are embedded via `include_str!` in the `profiles/` module and also installable as files.

**Q50 — helm validate uses pre-collected perf counter JSON.**
`helm validate --profile sifive-u74.json --counters collected.json --workload dhrystone` replays the workload in simulation and compares simulated counter values against `collected.json`. No live hardware access is required.

---

## Non-Goals for Phase 0

- OoO ROB/reservation-station/LSQ (deferred to Phase 3 as `AccurateOoO`).
- Memory consistency model enforcement in Accurate mode (deferred with LSQ).
- NUMA modeling.
- Power/thermal modeling (future crate).

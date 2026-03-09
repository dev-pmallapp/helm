# CAE Model

Cycle-Accurate Emulation — full pipeline model.

## Pipeline Stages

The `helm-pipeline` crate models an OoO processor:

| Stage | Module | Description |
|-------|--------|-------------|
| Fetch | — | Driven by the ISA frontend |
| Decode | — | `MicroOp` classification |
| Rename | `rename.rs` | Architectural → physical register mapping |
| Dispatch | `rob.rs` | ROB allocation |
| Issue | `scheduler.rs` | Ready-to-issue selection |
| Execute | timing model | Latency from `TimingModel` |
| Complete | `rob.rs` | Mark ROB entry done |
| Commit | `rob.rs` | In-order retirement |

## Key Structures

- **ReorderBuffer** — circular buffer tracking in-flight uops.
  States: Dispatched → Executing → Complete → (commit).
- **RenameUnit** — RAT (Register Alias Table) + free list.
- **Scheduler** — issue queue with wakeup and width-limited select.
- **BranchPredictor** — Static, Bimodal, GShare, TAGE, Tournament.

## Pipeline Configuration

Via `CoreConfig`:

```rust
CoreConfig {
    name: "big-core",
    width: 4,        // issue/dispatch width
    rob_size: 192,
    iq_size: 64,
    lq_size: 32,
    sq_size: 32,
    branch_predictor: BranchPredictorConfig::TAGE { history_length: 16 },
}
```

## When to Use

- Microarchitectural research.
- Bottleneck analysis (ROB-bound, IQ-bound, memory-bound).
- Branch predictor evaluation.

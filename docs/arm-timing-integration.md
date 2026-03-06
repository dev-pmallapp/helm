# ARM Timing Integration — Future Work

This document describes changes needed in `helm-isa` and `helm-core` to
fully integrate timing into the AArch64 execution path.  These changes
are **not yet implemented** — they are a plan for follow-up work.

## 1. InstructionOutcome from `Aarch64Cpu::step()`

Currently `step()` returns `Result<(), HelmError>`.  To feed the timing
model with precise information, it should return a structured outcome:

```rust
pub struct InstructionOutcome {
    pub pc: u64,
    pub insn_word: u32,
    pub class: InsnClass,
    pub mem_accesses: Vec<MemAccess>,
    pub branch_taken: Option<bool>,
}

pub struct MemAccess {
    pub addr: u64,
    pub size: usize,
    pub is_write: bool,
}
```

This lets the engine feed real data addresses to the timing model
instead of using PC-based proxies.

## 2. Memory Access Address Extraction

After `step()`, the engine currently cannot determine what memory
addresses were accessed.  Options:

1. **Return from step()**: Modify `step()` to collect and return
   `MemAccess` records as part of `InstructionOutcome`.
2. **Callback-based**: Register a memory-access callback on the CPU
   that fires during `step()`.
3. **Post-hoc decode**: Re-decode the instruction after step() and
   compute the effective address from register state — fragile but
   avoids modifying `step()`.

**Recommendation**: Option 1 is the cleanest.

## 3. Branch Prediction Integration

The current timed SE runner uses a simple hash-based misprediction
model.  For APE/CAE accuracy:

- Add `BranchPredictor` trait to `helm-timing`
- Wire predictor state into the timed loop
- Feed branch outcomes (taken/not-taken, target) from `InstructionOutcome`
- Support Bimodal, GShare, TAGE, Tournament (configs already exist)

## 4. MicroOp Pipeline Feed (CAE Mode)

For full cycle-accurate emulation, each instruction must be decomposed
into `MicroOp`s and fed through the pipeline model:

1. `step()` emits `InstructionOutcome`
2. Decode phase maps `insn_word` → `Vec<MicroOp>`
3. Pipeline stages (fetch, decode, rename, dispatch, execute, writeback,
   commit) consume MicroOps
4. Each stage adds latency via the timing model

This requires `helm-pipeline` to be connected to the timed SE loop.

## 5. Cache Integration

The timed SE runner currently uses a probabilistic memory latency model
in `ApeModelDetailed`.  For real cache simulation:

1. Create `helm-memory::Cache` instance during SE setup
2. On each Load/Store, probe the cache hierarchy
3. Feed cache hit/miss result to `TimingModel::memory_latency()`
4. Report cache statistics via `helm-stats`

# Timing Model

HELM supports three accuracy levels through the `TimingModel` trait.
Any level can be combined with any execution mode (SE or FS).

## Accuracy Levels

| Level | Name | Speed | What Is Modelled |
|-------|------|-------|------------------|
| L0 | **FE** | 100–1000 MIPS | IPC = 1, flat memory, no timing |
| L1–L2 | **ITE** | 1–100 MIPS | Per-class instruction latency, cache-level stalls, branch penalty |
| L3 | **CAE** | 0.1–1 MIPS | Full pipeline stages, bypass network, store buffer |

## TimingModel Trait

Every timing model implements `TimingModel` (in `helm-timing::model`):

```rust
pub trait TimingModel: Send + Sync {
    fn accuracy(&self) -> AccuracyLevel;
    fn instruction_latency(&mut self, uop: &MicroOp) -> u64;
    fn instruction_latency_for_class(&mut self, class: InsnClass) -> u64;
    fn memory_latency(&mut self, addr: Addr, size: usize, is_write: bool) -> u64;
    fn branch_misprediction_penalty(&mut self) -> u64;
    fn end_of_quantum(&mut self);
    fn reset(&mut self);
}
```

## Built-In Models

### FeModel (L0)

Every instruction costs 1 cycle. Memory latency is 0. Branch penalty
is 0. Maximum simulation throughput.

### IteModel (L1)

Configurable cache-level latencies (`l1`, `l2`, `l3`, `dram`).
Instruction latency is 1 for all classes. Useful when you only care
about memory-hierarchy effects.

### IteModelDetailed (L2)

Per-instruction-class latencies:

| Class | Default Cycles |
|-------|---------------|
| IntAlu | 1 |
| IntMul | 3 |
| IntDiv | 12 |
| FpAlu | 4 |
| FpMul | 5 |
| FpDiv | 15 |
| Load | 4 |
| Store | 1 |
| Branch penalty | 10 |

Memory latency uses a probabilistic model based on address-bit
hashing: 85% L1, 10% L2, 4% L3, 1% DRAM. Real cache simulation can
be layered on top via `helm-memory::Cache`.

### Pipeline Model (L3 / CAE)

At CAE level, `helm-pipeline` provides a full OoO pipeline:
`ReorderBuffer`, `RenameUnit`, `Scheduler`, `BranchPredictor`. Each
pipeline stage implements the `Stage` trait and advances one cycle at a
time. See [execution-pipeline.md](execution-pipeline.md) for details.

## CPU Type Presets

The `helm-aarch64` CLI maps human-readable CPU types to timing models:

| `--cpu` | Model | Notes |
|---------|-------|-------|
| `atomic` | `FeModel` | Default, fastest |
| `timing` | `IteModelDetailed` (defaults) | General-purpose |
| `minor` | `IteModelDetailed` (reduced div/branch) | In-order-like |
| `o3` | `IteModelDetailed` (full latencies) | OoO-like |
| `big` | `IteModelDetailed` (big-core tuned) | ARM big.LITTLE big core |

## Integration Points

The SE runner calls `timing.instruction_latency_for_class(class)` once
per retired instruction and accumulates virtual cycles. The FS session
does the same within its block-dispatch loop. The `Simulation` driver
in `helm-engine` wires the timing model into either the SE or
microarchitectural run loop.

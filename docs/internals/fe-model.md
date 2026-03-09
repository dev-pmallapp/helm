# FE Model

Functional Emulation — the fastest timing model.

## Behaviour

- `instruction_latency()` → 1 cycle for all instructions.
- `memory_latency()` → 0 cycles (no cache simulation).
- `branch_misprediction_penalty()` → 0 cycles.

## When to Use

- Maximum simulation throughput (100–1000 MIPS).
- Functional correctness testing.
- Boot-time fast-forward before switching to detailed mode.
- Plugin development and debugging.

## Implementation

```rust
pub struct FeModel;

impl TimingModel for FeModel {
    fn accuracy(&self) -> AccuracyLevel { AccuracyLevel::FE }
    fn instruction_latency(&mut self, _uop: &MicroOp) -> u64 { 1 }
    fn memory_latency(&mut self, _: Addr, _: usize, _: bool) -> u64 { 0 }
    fn branch_misprediction_penalty(&mut self) -> u64 { 0 }
}
```

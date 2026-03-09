# Timing Integration

How timing models plug into the execution loop.

## SE Mode Integration

The SE runner calls the timing model once per retired instruction:

```text
instruction → classify(insn) → InsnClass
            → timing.instruction_latency_for_class(class) → cycles
            → virtual_cycles += cycles
```

For memory accesses: `timing.memory_latency(addr, size, is_write)`.

## FS Mode Integration

The FS session integrates timing at block boundaries:

1. Execute a TCG block (N instructions).
2. For each instruction, accumulate latency.
3. `timing.end_of_quantum()` at sync points.

## EventQueue

`helm-timing::EventQueue` provides a min-heap event queue for
event-driven simulation:

- `schedule(timestamp, priority, tag)` — add event.
- `pop()` — remove earliest event.
- Events with equal timestamps are ordered by priority.

## TemporalDecoupler

For multi-core simulation:

- Each core has a `CoreTiming` with atomic virtual time.
- `quantum_size` limits the maximum skew between cores.
- `needs_sync(core_id)` checks if a core has exceeded its quantum.
- `global_time()` = min across all cores.

## SamplingController

Multi-phase sampled simulation:

1. **FastForward** — skip N instructions at FE speed.
2. **Warmup** — run with caches/predictors but no stats.
3. **Detailed** — collect statistics.
4. **Cooldown** — drain pipeline.
5. **Done** — all phases complete.

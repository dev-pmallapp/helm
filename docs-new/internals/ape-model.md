# APE Model

Approximate Emulation — configurable per-class latencies.

## ApeModel (Basic)

Cache-level latencies only; all instructions cost 1 cycle:

| Parameter | Default |
|-----------|---------|
| L1 latency | 3 |
| L2 latency | 12 |
| L3 latency | 40 |
| DRAM latency | 200 |

## ApeModelDetailed

Per-instruction-class latencies with memory hierarchy:

| Parameter | Default | Description |
|-----------|---------|-------------|
| `int_alu_latency` | 1 | Integer ALU |
| `int_mul_latency` | 3 | Integer multiply |
| `int_div_latency` | 12 | Integer divide |
| `fp_alu_latency` | 4 | Floating-point ALU |
| `fp_mul_latency` | 5 | FP multiply |
| `fp_div_latency` | 15 | FP divide |
| `load_latency` | 4 | Load |
| `store_latency` | 1 | Store |
| `branch_penalty` | 10 | Branch misprediction |

Memory latency uses address-bit hashing to approximate cache-hit
distribution: 85% L1, 10% L2, 4% L3, 1% DRAM.

## When to Use

- Performance estimation without full pipeline model.
- Parameter sweeps over latency configurations.
- Workload characterisation (IPC, memory-boundedness).

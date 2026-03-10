# Plugin Catalog

Built-in plugins shipped with HELM.

## Trace Plugins

### insn-count (`plugin.trace.insn-count`)

Counts instructions per vCPU using a lock-free scoreboard.

| Method | Returns | Description |
|--------|---------|-------------|
| `total()` | `u64` | Total instructions across all vCPUs |
| `per_vcpu()` | `Vec<u64>` | Per-vCPU counts |

### execlog (`plugin.trace.execlog`)

Logs every executed instruction.

| Parameter | Default | Description |
|-----------|---------|-------------|
| `regs` | `false` | Include register dump |
| `max` | unlimited | Maximum log lines |

### hotblocks (`plugin.trace.hotblocks`)

Ranks translation blocks by execution count.

| Method | Returns | Description |
|--------|---------|-------------|
| `top(n)` | `Vec<(pc, count, insns)>` | Top N blocks |

### howvec (`plugin.trace.howvec`)

Instruction-class histogram (integer ALU, FP, load, store, branch, …).

### syscall-trace (`plugin.trace.syscall-trace`)

Logs syscall entries and returns with arguments.

## Debug Plugins

### fault-detect (`plugin.debug.fault-detect`)

Catches execution anomalies:
- Jump to NULL (PC = 0).
- Wild jumps (PC in unmapped region).
- Undefined instructions.
- Stack pointer corruption.
- TLS aliasing across threads.
- Critical unsupported syscalls.

Produces `FaultReport` with PC history, syscall log, and register dump.

## Memory Plugins

### cache (`plugin.memory.cache`)

Set-associative L1/L2 cache simulation via memory-access callbacks.

| Method | Returns | Description |
|--------|---------|-------------|
| `hit_rate()` | `f64` | Overall hit rate |
| `hits` | `AtomicU64` | Hit count |
| `misses` | `AtomicU64` | Miss count |

## CLI Usage

```bash
helm-aarch64 --plugin insn-count ./binary
helm-aarch64 --plugin execlog:regs=true,max=1000 ./binary
helm-aarch64 --plugin hotblocks --plugin howvec ./binary
helm-aarch64 --plugin fault-detect ./binary
helm-aarch64 --plugin cache ./binary
```

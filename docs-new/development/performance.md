# Performance

## Profiling

Build with debug symbols in release mode:

```bash
cargo build --profile profiling
```

The `profiling` profile inherits from `release` with `debug = 2`.

Use `perf record` / `perf report` for Linux profiling:

```bash
perf record ./target/profiling/helm-arm ./binary
perf report
```

## MIPS Benchmarks

Approximate throughput by timing model:

| Model | Backend | Typical MIPS |
|-------|---------|-------------|
| FE | JIT | 10–100 |
| FE | Interpreter | 1–10 |
| FE | Threaded interp | 5–50 |
| APE | Interpreter | 0.5–5 |
| CAE | Pipeline | 0.1–1 |

## Hot Paths

The most performance-critical code paths:

1. **TCG interpreter loop** — `TcgInterp::execute()`.
2. **JIT block lookup** — direct-mapped cache indexed by PC.
3. **MMU translation** — TLB lookup + page-table walk.
4. **Syscall dispatch** — `Aarch64SyscallHandler::handle()`.
5. **Plugin callbacks** — instruction-level callbacks add per-insn overhead.

## Optimisation Tips

- Use JIT backend (`--backend jit`) for maximum throughput.
- Minimise plugin callbacks in hot loops.
- Use `SamplingController` to fast-forward through uninteresting phases.
- Increase TLB size for workloads with high TLB miss rates.
- Use `FeModel` for functional testing; switch to APE/CAE only for
  timing-sensitive analysis.

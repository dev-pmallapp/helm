# Debugging

Tips for debugging HELM simulations.

## Kernel Boot Issues

1. Use `--serial stdio` for console output.
2. Add `earlycon=pl011,0x09000000` to kernel command line.
3. Use `--backend interp` for deterministic execution.
4. Check PC against `System.map` (pass via `--sysmap`).
5. Use `--monitor` for the debug monitor.

## JIT vs Interpreter Divergence

1. Run with `--backend interp` first to establish baseline.
2. Run with `--backend jit` and compare results.
3. Use parity tests in `helm-tcg/src/tests/jit_parity.rs`.
4. Set `RUST_LOG=debug` for detailed translation logging.

## Tracing

Enable Rust logging: `RUST_LOG=helm_isa=debug cargo run ...`

Log levels:
- `trace` — every instruction, every register write.
- `debug` — block translation, syscalls, exceptions.
- `info` — session start/stop, major events.
- `warn` — unimplemented instructions, fallbacks.
- `error` — fatal conditions.

## Plugins for Debugging

- `--plugin execlog:regs=true` — log every instruction with registers.
- `--plugin fault-detect` — catch wild jumps, NULL PC, stack corruption.
- `--strace` — log all syscalls with arguments.

## GDB-Style Monitor

`helm-system-aarch64 --monitor` provides an interactive debug console:
- Register inspection.
- Memory dumps.
- Breakpoints.
- Single-stepping.

## Common Issues

| Symptom | Likely Cause |
|---------|-------------|
| Kernel hangs at boot | Missing device or incorrect DTB |
| "unmapped address" error | MMIO access to unmodelled peripheral |
| Incorrect syscall result | Unsupported or misimplemented syscall |
| JIT crash | TCG emitter bug for specific instruction |
| Timer not firing | Sysreg sync issue (see sysreg-sync.md) |

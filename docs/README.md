# helm-ng

**Next-generation, research-grade hardware simulator.** Rust core,
Python configuration, multi-ISA, multi-mode, multi-timing.

helm-ng targets the same problem space as gem5, QEMU, and Simics but is
designed from first principles for clarity, correctness, and
composability. It runs AArch64 and RISC-V binaries in syscall emulation
(SE) or full-system (FS) mode, with selectable timing fidelity from
functional (IPC=1) through interval to cycle-accurate.

For interval timing, plain `timing="interval"` or `--timing interval`
selects the default two-level L1D/L2 estimator. Use the Python
`interval:...` string or the example launchers' explicit `--l1d-*` /
`--l2-*` flags when you need to override the cache hierarchy.

## At a Glance

| Dimension | Value |
|-----------|-------|
| Language | Rust (simulation) + Python (configuration) |
| ISAs | AArch64, RISC-V RV64GC, AArch32 (planned) |
| Execution modes | FE (functional), SE (syscall emulation), FS (full system) |
| Timing models | VirtualTiming, IntervalTiming, AccurateTiming |
| Device model | `Device` trait + bus tree, DLD dynamic loading |
| Config layer | gem5-style Python via PyO3 |
| Workspace | 27 Rust crates across `framework/`, `runtime/`, `hw/`, `debug/` |

## Design Principles

1. **Monomorphize timing only** — `HelmEngine<T: TimingModel>` is the
   sole generic parameter; timing is inlined, not vtable-dispatched.
2. **ISA/mode are enum-dispatched** — one `match` per Python call, zero
   per instruction.
3. **No dark state** — every persistent field is a registered attribute.
4. **Device knows no base address or IRQ** — platform wires placement.
5. **Determinism by default** — no wall-clock, no background threads in
   the hot loop.
6. **Python describes; Rust simulates** — config is frozen after
   `build_simulator()`.

## Quick Links

- [Architecture Overview](architecture/overview.md) — system design and
  positioning vs QEMU/gem5/Simics
- [Crate Map](architecture/crate-map.md) — all 27 crates with
  dependency graph
- [Execution Pipeline](architecture/execution-pipeline.md) —
  fetch-decode-execute data flow
- [Memory Model](architecture/memory-model.md) — address spaces, MMU,
  TLB, cache hierarchy
- [Timing Model](architecture/timing-model.md) — VirtualTiming through
  AccurateTiming
- [Device Model](architecture/device-model.md) — Device trait, MMIO,
  interrupt wiring
- [Comparisons](architecture/comparison-qemu.md) — how helm-ng relates
  to QEMU, gem5, Simics, and Higan

## Building the Docs

```bash
cd docs && mdbook build      # output in docs/book/
mdbook serve                 # local preview at http://localhost:3000
```

## Building the Simulator

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --all --all-targets -- -D warnings
```

# Comparison with gem5

Architectural parallels and differences between helm-ng and gem5.

## Overview

gem5 is the dominant academic microarchitecture simulator. helm-ng
borrows gem5's Python SimObject configuration model and multi-fidelity
timing approach, but uses Rust instead of C++, monomorphized generics
instead of virtual dispatch, and a cleaner memory subsystem.

## Configuration Layer

| Aspect | gem5 | helm-ng |
|--------|------|---------|
| Language | Python (SWIG/pybind11 bridge) | Python (PyO3 bridge) |
| Object model | `SimObject` tree with `Param` types | `SimObject` tree with `__setattr__` tracking |
| Config freeze | At `simulate()` call | At `instantiate()` call |
| Platform scripts | `fs.py`, `se.py` | Python scripts via `helm-cli` |
| Parameterization | `Param.UInt64`, `Param.String`, etc. | Python kwargs → Rust structs |

Both use a "Python describes, engine simulates" model. The key
difference is binding technology: gem5 uses SWIG/pybind11 to expose C++
classes to Python; helm-ng uses PyO3 for zero-overhead Rust-Python
interop.

## Timing Models

| Aspect | gem5 | helm-ng |
|--------|------|---------|
| Dispatch | Virtual (`BaseCPU*` → `AtomicSimpleCPU`, `MinorCPU`, `O3CPU`) | Monomorphized (`HelmEngine<T: TimingModel>`) |
| Models | AtomicSimple, TimingSimple, Minor, O3 | VirtualTiming, IntervalTiming, AccurateTiming |
| Hot-loop cost | vtable call per instruction | Zero (inlined by compiler) |
| Switching | Drain + swap CPU object | Change `TimingChoice` at construction |

gem5's virtual dispatch allows runtime CPU swapping (e.g., KVM fast-forward
then switch to O3). helm-ng's monomorphization trades this flexibility
for zero-overhead timing in the hot loop — a deliberate choice for a
research simulator where timing overhead directly affects experiment
wall-clock time.

## Memory System

| Aspect | gem5 | helm-ng |
|--------|------|---------|
| Architecture | Port-based (`MasterPort` ↔ `SlavePort`) | Direct: `FlatMem` (SE) / `HelmAddressSpace` (FS) |
| Access modes | Atomic, Timing, Functional | Read/write via `MemInterface` trait |
| Cache model | Classic (coherent) / Ruby (protocol-driven) | Engine-owned L1D/L2 estimator for `IntervalTiming` today; broader memory/cache unification still evolving |
| TLB | `TLB` class per CPU model | 256-entry `Tlb` struct |
| MMIO | Via `PioDevice` port binding | Via `AddressMap` → `Device::transact()` |

gem5's dual memory subsystem (Classic vs Ruby) is a known pain point —
they have different APIs and cannot be easily mixed. helm-ng avoids
this by using a single memory interface trait. Today the live timed
cache hierarchy is the interval-timing L1D/L2 estimator configured
through `TimingChoice`, Python timing strings such as
`interval:interval_len=256,l1d_size=64KiB,l2_size=1MiB`, or the example
launchers' explicit `--l1d-*` / `--l2-*` flags.

## Device Model

| Aspect | gem5 | helm-ng |
|--------|------|---------|
| Base class | `SimObject` (C++) | `Device` trait (Rust) |
| MMIO | Port-based: `PioDevice::read/write` | `Device::transact()` with `Transaction` |
| IRQ | Port signaling | `InterruptPin` → `InterruptSink` |
| Bus | `Bus` SimObject | `MmioBus`, AMBA, I2C, SPI |
| Address binding | Port binding in Python config | Platform-driven `AddressMap` |
| Dynamic loading | Compile-time only | DLD `.so` runtime loading |

## SimObject Lifecycle

| Phase | gem5 | helm-ng |
|-------|------|---------|
| Construction | `SimObject.__init__()` | `SimObject::new()` |
| Initialization | `init()` | `init()` (self-contained) |
| Wiring | Port binding in Python | `elaborate(system)` stores `Arc` refs |
| Startup | `startup()` | `startup()` |
| Config freeze | `simulate()` | `instantiate()` |
| Cross-component | Port resolution (runtime) | `elaborate()` (pre-run, stored refs) |

gem5 resolves cross-component references at runtime via port resolution.
helm-ng resolves everything during `elaborate()` and stores direct
`Arc` pointers — zero dynamic lookup in the hot loop.

## Event System

| Aspect | gem5 | helm-ng |
|--------|------|---------|
| Scheduled events | `Event` class + `EventQueue` | `EventQueue` (BinaryHeap) |
| Observable events | Same `Event` class | Separate `HelmEventBus` |
| Separation | No (one system) | Yes (scheduling ≠ observation) |
| Checkpoint | Event queue serialized | EventQueue saved, EventBus re-registers |

## Testing

| Aspect | gem5 | helm-ng |
|--------|------|---------|
| ISA tests | In-tree test programs | riscv-tests vectors + AArch64 torture tests |
| Differential | Limited (vs hardware) | QEMU/Spike trace comparison |
| Property-based | No | `proptest` for memory layouts |
| Benchmarks | Internal regression suite | `criterion` for IPC regressions |

## Plugin / Instrumentation

| Aspect | gem5 | helm-ng |
|--------|------|---------|
| API | C++ probes | `HelmPlugin` trait + `Probe<T>` |
| Zero-cost | No | Yes (probes compile away when inactive) |
| Analysis | Statistics framework | `helm-stats` + `helm-spy` + `helm-report` |
| Instrumentation stack | Flat | 3-layer: probe → spy → report |

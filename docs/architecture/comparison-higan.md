# Comparison with Higan

Architectural parallels and differences between helm-ng and Higan
(formerly bsnes).

## Overview

Higan is a multi-system emulator focused on cycle-accurate console
emulation. While targeting a different domain (retro gaming consoles
vs modern ARM/RISC-V systems), Higan's approach to multi-fidelity
emulation directly influenced helm-ng's timing model design.

## Multi-Fidelity Emulation

Higan's key insight: a single emulator can support multiple accuracy
levels for the same target, letting users trade speed for fidelity.
This directly inspired helm-ng's three-tier timing model.

| Fidelity | Higan | helm-ng |
|----------|-------|---------|
| Fast / functional | Performance profile | `VirtualTiming` (IPC=1) |
| Balanced | Balanced profile | `IntervalTiming` (instruction-class latencies) |
| Cycle-accurate | Accuracy profile | `AccurateTiming` (full pipeline) |

### How Each Achieves Multi-Fidelity

**Higan:** Maintains separate emulation cores per accuracy level. The
SNES has three processor cores (CPU, SFC, DSP) each with fast and
accurate implementations. The "accuracy" profile synchronizes all cores
cycle-by-cycle; the "performance" profile batches execution.

**helm-ng:** Uses a single engine (`HelmEngine<T>`) parameterized by a
`TimingModel` trait. The Rust compiler generates specialized code for
each timing level via monomorphization. No separate cores needed — the
same decode/execute logic serves all fidelity levels.

| Aspect | Higan | helm-ng |
|--------|-------|---------|
| Approach | Separate cores per accuracy | Single core, parameterized timing |
| Code duplication | High (separate implementations) | None (monomorphized generic) |
| Switching cost | Select profile at startup | Select `TimingChoice` at construction |
| Binary size | One binary, multiple cores | One binary, multiple specializations |

## Determinism

Both Higan and helm-ng are **deterministic by design**:

| Property | Higan | helm-ng |
|----------|-------|---------|
| Wall-clock independence | Yes | Yes |
| Thread isolation | Single-threaded | No background threads in hot loop |
| Reproducibility | Exact same output per input | Exact same output per input |
| Save states | Per-frame snapshots | Checkpoint save/restore |

## Per-Component Timing

Higan's per-chip timing model gives each hardware component (CPU, PPU,
APU, etc.) its own clock rate and synchronization protocol. helm-ng's
approach differs:

| Aspect | Higan | helm-ng |
|--------|-------|---------|
| Clock domains | Per-chip (CPU: 3.58 MHz, PPU: 5.37 MHz) | Single tick domain with `set_tick_scale()` |
| Synchronization | Cycle-locked or batched | EventQueue-driven |
| Device timing | Hardware-accurate | Instruction-class approximation |

## Device Model

| Aspect | Higan | helm-ng |
|--------|-------|---------|
| Device abstraction | Per-chip C++ classes | `Device` trait |
| MMIO | Per-chip memory map (hardcoded) | Platform-driven `AddressMap` |
| Reusability | Chip-specific (SNES PPU ≠ GBA PPU) | Reusable across platforms |
| Dynamic loading | No | DLD `.so` loading |

Higan's devices are tightly coupled to specific console hardware.
helm-ng's `Device` trait enables the same device (e.g., PL011 UART)
to be used across multiple platforms without modification.

## Target Domain

| Aspect | Higan | helm-ng |
|--------|-------|---------|
| Targets | Retro consoles (SNES, GBA, etc.) | Modern CPUs (AArch64, RISC-V) |
| ISAs | 65C816, ARM7TDMI, SPC700, etc. | AArch64, RV64GC, AArch32 |
| Primary use | Preservation + gaming | Research + development |
| OS support | Console firmware | Linux (SE + FS mode) |
| Scale | ~10 MHz processors | GHz-class processors |

## What helm-ng Learned from Higan

1. **Multi-fidelity is a first-class feature** — don't bolt on timing
   as an afterthought. Design the engine to support multiple accuracy
   levels from day one.

2. **Determinism must be structural** — if determinism is enforced by
   the type system and architecture (no wall-clock, no threads), it
   cannot be accidentally broken.

3. **Accuracy profiles serve different users** — the same codebase can
   serve both "run fast to test something" and "measure cycle-accurate
   behavior" use cases.

4. **Per-component clocking matters** — even if helm-ng uses a single
   tick domain today, the EventQueue provides the foundation for
   per-device clock rates in the future.

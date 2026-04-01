# Timing Model

How helm-ng models microarchitectural timing — from zero-overhead
functional emulation to cycle-accurate pipeline simulation.

## Design Principle: Monomorphization

The timing model is the **sole generic parameter** in the simulation
engine:

```rust
pub struct HelmEngine<T: TimingModel> { ... }
```

This means:
- `HelmEngine<VirtualTiming>` and `HelmEngine<IntervalTiming>` are
  **separate compiled types** — the Rust compiler generates specialized
  code for each, eliminating all vtable dispatch in the hot loop.
- Timing callbacks (`on_insn()`, `on_mem_access()`, `on_branch()`)
  are inlined directly into the instruction loop.
- For `VirtualTiming` (IPC=1), the timing call typically compiles away
  to a single counter increment.

This is a deliberate design choice. Other approaches:
- **gem5** uses virtual dispatch (`BaseCPU*`) — flexible but adds
  vtable overhead per instruction.
- **Simics** uses transaction-level callbacks — low overhead but cannot
  reach cycle accuracy.
- **QEMU** has no timing model at all.

## The TimingModel Trait

Defined in `helm-timing`:

```rust
pub trait TimingModel: Send + 'static {
    fn on_insn(&mut self, info: &TimingInsnInfo) -> u64;
    fn on_mem_access(&mut self, access: &MemAccess);
    fn on_branch(&mut self, taken: bool, predicted: bool);
    fn current_cycles(&self) -> u64;
    fn advance_to(&mut self, tick: u64);
    fn on_boundary(&mut self, eq: &mut EventQueue);
}
```

Supporting types:

| Type | Purpose |
|------|---------|
| `TimingInsnClass` | Instruction classification: IntAlu, IntMul, Branch, Load, Store, FpAlu, SimdAlu, System, Nop, Atomic, Unknown |
| `TimingInsnInfo` | Per-instruction metadata: PC, class, flags |
| `MemAccess` | Memory access descriptor: addr, size, is_store, cache hit levels |

`current_cycles()` is the externally visible simulated-time contract used
by engine-owned timed callbacks and the Python `System.current_cycles`
getter. `advance_to()` is used when the engine can prove guest time
should fast-forward without retiring instructions, such as WFI idle.

## VirtualTiming

The simplest model. Time advances at an ideal IPC with fractional-cycle
accumulation when `ipc > 1.0`. No cache model, no branch prediction, no
pipeline stalls.

**When to use:** ISA validation, fast-forward, workload
characterization, any case where timing fidelity is not needed.

**Speed:** 100M–1B instructions/sec on modern hosts.

**Implementation:** `on_insn()` accumulates exact cycles and exposes the
floored integer tick via `current_cycles()`. All other callbacks are
no-ops except `advance_to()`, which monotonically fast-forwards time.

## IntervalTiming

Sniper-style interval simulation. Instructions execute in intervals;
timing penalties are applied only at miss events:

- **Cache misses** — L1, L2, L3 miss penalties
- **Branch mispredicts** — pipeline flush penalty
- **Memory ordering** — load-to-use stalls

**When to use:** Workload studies where ~5% IPC error vs cycle-accurate
is acceptable and 10x speedup matters.

**Speed:** 10–100M instructions/sec.

**Implementation:** Maintains class-weighted interval work, branch
penalties, and a small engine-owned cache-locality model. `on_insn()`
adds class-specific work, `on_mem_access()` applies penalties from
observed L1/L2 outcomes, and `on_branch()` applies misprediction costs.

### Interval Cache Hierarchy

The current `IntervalTiming` path consumes an engine-owned two-level
cache estimator:

| Level | Default |
|-------|---------|
| L1D | 32 KiB, 8-way, 64-byte lines |
| L2 | 256 KiB, 8-way, 64-byte lines |

These defaults can now be overridden through `TimingChoice` and the
Python/example timing string surface:

```text
interval:interval_len=256,l1d_size=64KiB,l1d_assoc=4,l1d_line=128,l2_size=1MiB,l2_assoc=16,l2_line=128
```

The documented runner surfaces expose these as explicit flags:

```bash
target/debug/helm-aarch64 examples/se/run_binary.py \
  --binary assets/aarch64/binaries/fish \
  --cpu timing \
  --interval-len 256 \
  --l1d-size 64KiB \
  --l2-size 1MiB
```

## AccurateTiming

Full cycle-accurate pipeline model (placeholder — Phase 3). Will
simulate:

- Out-of-order pipeline: ROB, rename, issue queue, LSQ
- Branch predictor (bimodal, TAGE)
- Cache coherence (MOESI)
- Precise cycle counting per pipeline stage

**When to use:** Microarchitecture research, RTL correlation, IPC
accuracy studies.

**Speed:** 0.1–1M instructions/sec.

## HelmSim Enum

`HelmSim` wraps the three timing variants for Python interop:

```rust
pub enum HelmSim {
    VirtualTiming(HelmEngine<VirtualTiming>),
    IntervalTiming(HelmEngine<IntervalTiming>),
    AccurateTiming(HelmEngine<AccurateTiming>),
}
```

The `build_simulator()` factory function selects the variant based on
`TimingChoice`:

```rust
pub enum TimingChoice {
    VirtualTiming { ipc: f64 },
    IntervalTiming {
        ipc: f64,
        interval_len: u64,
        mem_model: TimingMemModelConfig,
    },
    AccurateTiming,
}
```

## Timing Tick Scale

`HelmEngine` supports `set_tick_scale(scale)` to convert instruction
ticks to nanoseconds. This enables device timers (CNTP_CVAL,
SP804) to operate in simulated time regardless of which timing model
is active.

## Current User-Facing Paths

The preferred timing configuration surface is now:

- Python/system config: `timing="interval:..."`
- FS example launchers: `--timing interval` plus explicit `--interval-len`,
  `--l1d-*`, `--l2-*` flags
- SE example launcher: `--cpu timing` or `--cpu minor` plus the same
  explicit interval cache flags

The legacy SE flags `--caches` and `--l2cache` still work as deprecated
compatibility shorthands, but they only exist to preserve older scripts.

## Comparison

| Aspect | QEMU | gem5 | Simics | Higan | helm-ng |
|--------|------|------|--------|-------|---------|
| Timing dispatch | None | Virtual (`BaseCPU*`) | Callback-based | Hardcoded per chip | Monomorphized generic |
| Models | FE only | Atomic, Minor, O3 | Transaction-level | Cycle-exact | Virtual, Interval, Accurate |
| Hot-loop overhead | Zero | vtable call | Callback | Zero | Zero (inlined) |
| Switching cost | N/A | Rebuild | Reconfig | Different binary | Change `TimingChoice` |

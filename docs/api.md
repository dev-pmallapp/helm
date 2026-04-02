# helm-ng API Reference

> **See also:** [`traits.md`](traits.md) for trait definitions, [`object-model.md`](object-model.md) for the SimObject hierarchy.

helm-ng is a Rust-core, Python-config, multi-ISA simulator. The Rust crates implement all simulation logic; the `helm` Python package (backed by the `_helm_ng` PyO3 extension) exposes a high-level configuration and control surface.

---

## Table of Contents

- [Part 1: Rust API Reference](#part-1-rust-api-reference)
  - [helm-core](#helm-core)
  - [helm-engine](#helm-engine)
  - [helm-memory](#helm-memory)
  - [helm-timing](#helm-timing)
  - [helm-event](#helm-event)
  - [helm-debug](#helm-debug)
  - [helm-stats](#helm-stats)
- [Part 2: Python API Reference](#part-2-python-api-reference)
  - [Package Structure](#package-structure)
  - [SimObject Base Class](#simobject-base-class)
  - [Cpu](#cpu)
  - [Cache](#cache)
  - [Ram and MemorySpace](#ram-and-memoryspace)
  - [Board Composition](#board-composition)
  - [System](#system)
  - [Enumerations](#enumerations)
  - [Param System](#param-system)
  - [Complete Worked Example](#complete-worked-example)
  - [Error Handling](#error-handling)
- [Part 3: Error Reference](#part-3-error-reference)
  - [Rust Error Types](#rust-error-types)
  - [PyO3 Error Propagation](#pyo3-error-propagation)
  - [Error Recovery Patterns](#error-recovery-patterns)

---

# Part 1: Rust API Reference

## helm-core

**Purpose.** `helm-core` defines the portable architectural state that every ISA implementation reads and writes. It is intentionally ISA-agnostic: it holds the physical register file, floating-point register file, program counter, and CSR file. Higher-level crates depend on `helm-core` but `helm-core` itself has no dependencies on any particular instruction set.

### `ArchState`

The complete architectural state of a single hardware thread (hart).

```rust
pub struct ArchState {
    pub int_regs: [u64; 32],
    pub float_regs: [f64; 32],
    pub pc: u64,
    pub csrs: CsrFile,
}
```

| Field | Type | Description |
|---|---|---|
| `int_regs` | `[u64; 32]` | Integer register file. Index 0 is always `x0` (hard-wired zero for RISC-V; wired behavior is enforced by `write_int`). |
| `float_regs` | `[f64; 32]` | Floating-point register file. For AArch64, the upper 64 bits of 128-bit SIMD registers are not represented here. |
| `pc` | `u64` | Current program counter. |
| `csrs` | `CsrFile` | Control and status register bank. See `CsrFile` below. |

#### Methods

```rust
impl ArchState {
    pub fn new() -> Self
```

Creates an `ArchState` with all integer and floating-point registers zeroed, `pc` set to `0`, and `CsrFile` at reset values.

```rust
    pub fn reset(&mut self)
```

Resets all fields to their power-on defaults without reallocating. Equivalent to `*self = ArchState::new()` but avoids a heap allocation if `csrs` is already allocated.

```rust
    pub fn read_int(&self, idx: usize) -> u64
```

Returns the value of integer register `idx`. For RISC-V, `idx == 0` always returns `0` regardless of what was written.

```rust
    pub fn write_int(&mut self, idx: usize, val: u64)
```

Writes `val` to integer register `idx`. Silently discards writes to index `0` (RISC-V `x0` semantics).

```rust
    pub fn read_pc(&self) -> u64
```

Returns the current program counter.

```rust
    pub fn write_pc(&mut self, val: u64)
}
```

Sets the program counter to `val`.

#### Worked Example

```rust
use helm_core::ArchState;

let mut state = ArchState::new();
state.write_pc(0x8000_0000);
state.write_int(1, 42);           // x1 = 42
state.write_int(0, 99);           // silently ignored; x0 stays 0

assert_eq!(state.read_int(0), 0);
assert_eq!(state.read_int(1), 42);
assert_eq!(state.read_pc(), 0x8000_0000);

state.reset();
assert_eq!(state.read_int(1), 0);
```

---

### `CsrFile`

An opaque bank of 64-bit control and status registers, indexed by 12-bit CSR address. Accessible through `ArchState::csrs`.

```rust
pub struct CsrFile { /* private */ }
```

`CsrFile` is constructed by `ArchState::new()` and should not be constructed directly. Reads and writes go through the ISA executor; direct field access is available for testing and state inspection.

---

### Traits

`helm-core` exports two traits that ISA back-ends must implement. See [`traits.md`](traits.md) for full specifications.

| Trait | Implemented by |
|---|---|
| `Decoder` | Per-ISA instruction decode stage |
| `Executor` | Per-ISA instruction execution stage |

---

## helm-engine

**Purpose.** `helm-engine` is the simulation orchestrator. It owns an `ArchState`, a `MemoryMap`, a timing model, and optional plug-in handlers (syscall, trace), and it drives the fetch-decode-execute loop. The `HelmSim` enum erases the timing-model type parameter for use from Python and in generic contexts.

### `Isa`

```rust
pub enum Isa {
    RiscV,
    AArch64,
    AArch32,
}
```

Selects the instruction set architecture. The chosen variant determines which `Decoder` and `Executor` implementations are instantiated inside `HelmEngine`.

### `ExecMode`

```rust
pub enum ExecMode {
    Functional,
    Syscall,
    System,
}
```

| Variant | Description |
|---|---|
| `Functional` | No OS or syscall support. The simulation runs bare-metal code and halts on an unhandled exception. |
| `Syscall` | User-space binaries only. Syscalls are intercepted and emulated by a `SyscallHandler`. |
| `System` | Full machine emulation including privilege levels, device models, and interrupt controllers. |

### `TimingChoice`

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

Passed to `build_simulator` to select the timing model without requiring the caller to name the concrete type. See [helm-timing](#helm-timing) for model semantics.

### `HelmEngine<T>`

The generic simulation kernel. `T` must implement the `TimingModel` trait (see [`traits.md`](traits.md)).

```rust
pub struct HelmEngine<T: TimingModel> {
    pub isa: Isa,
    pub mode: ExecMode,
    pub timing: T,
    pub arch: ArchState,
    pub memory: MemoryMap,
}
```

#### Methods

```rust
impl<T: TimingModel> HelmEngine<T> {
    pub fn new(isa: Isa, mode: ExecMode, timing: T) -> Self
```

Constructs a kernel with freshly reset `ArchState` and empty `MemoryMap`. The caller is responsible for populating memory before calling `run`.

```rust
    pub fn run(&mut self, n_insns: u64)
```

Executes exactly `n_insns` instructions, or fewer if the hart halts first.

```rust
    pub fn run_until_halt(&mut self) -> HaltReason
```

Runs until the hart halts (breakpoint, `wfi`, exception, or end-of-program) and returns the reason.

```rust
    pub fn step_once(&mut self) -> Result<(), HartException>
```

Executes a single instruction. Returns `Err(HartException)` if the instruction raises an exception that is not handled by the current privilege level.

```rust
    pub fn set_syscall_handler(&mut self, handler: Box<dyn SyscallHandler>)
```

Attaches a syscall handler. Required when `mode == ExecMode::Syscall`; a panic occurs if a syscall is encountered without a handler.

```rust
    pub fn set_trace_logger(&mut self, logger: Arc<TraceLogger>)
}
```

Attaches a `TraceLogger`. All `TraceEvent`s produced during execution are forwarded to `logger`.

### `HelmSim`

Type-erased enum wrapper over the three concrete `HelmEngine` instantiations. This is what `build_simulator` returns and what the PyO3 layer holds.

```rust
pub enum HelmSim {
    VirtualTiming(HelmEngine<VirtualTiming>),
    IntervalTiming(HelmEngine<IntervalTiming>),
    AccurateTiming(HelmEngine<AccurateTiming>),
}
```

#### Methods

All methods delegate to the inner `HelmEngine`.

```rust
impl HelmSim {
    pub fn run(&mut self, n_insns: u64)
    pub fn run_until_halt(&mut self) -> HaltReason
    pub fn step_once(&mut self) -> Result<(), HartException>
    pub fn arch_state(&self) -> &ArchState
    pub fn arch_state_mut(&mut self) -> &mut ArchState
    pub fn memory(&self) -> &MemoryMap
    pub fn memory_mut(&mut self) -> &mut MemoryMap
}
```

### `build_simulator`

```rust
pub fn build_simulator(
    isa: Isa,
    mode: ExecMode,
    timing: TimingChoice,
    mem_base: u64,
    mem_size: usize,
) -> HelmSim
```

The primary factory function. Constructs and returns a `HelmSim` variant matching `timing`. Use this instead of constructing `HelmEngine` directly when the timing model is not known at compile time.

#### Worked Example

```rust
use helm_engine::{build_simulator, ExecMode, Isa, StopReason, TimingChoice};

// Build a small RISC-V functional simulator with virtual timing.
let mut sim = build_simulator(
    Isa::RiscV,
    ExecMode::Functional,
    TimingChoice::VirtualTiming { ipc: 1.0 },
    0,
    0x2000,
);

// Encode `addi x0, x0, 0` / nop at 0x100.
sim.load_bytes(0x100, &[0x13, 0x00, 0x00, 0x00]);
sim.set_pc(0x100);

assert_eq!(sim.run(1), StopReason::Quantum);
assert_eq!(sim.current_cycles(), 1);
```

---

## helm-memory

**Purpose.** `helm-memory` defines the address-space model: a composable tree of `MemoryRegion` variants that is flattened into a `FlatView` for fast linear access. It is the single source of truth for all load/store operations in the simulation.

### `MemoryRegion`

```rust
pub enum MemoryRegion {
    Ram { data: Vec<u8> },
    Mmio { handler: Box<dyn MmioHandler> },
    Alias { target: Arc<MemoryRegion>, offset: u64, size: u64 },
    Container { subregions: Vec<(u64, MemoryRegion)> },
}
```

| Variant | Description |
|---|---|
| `Ram` | Flat byte array. Reads and writes go directly into `data`. |
| `Mmio` | Delegates to a `MmioHandler` trait object. Suitable for device registers. |
| `Alias` | Maps a window of another region into a different address. `offset` is the byte offset into `target`; `size` constrains the visible window. |
| `Container` | Holds child regions at relative offsets. Used to group related regions (e.g., all peripheral registers under one base address). |

`MmioHandler` is defined in [`traits.md`](traits.md).

### `MemoryMap`

The top-level address space. Regions are stored in a sorted interval tree; the `FlatView` cache is rebuilt lazily after modifications.

```rust
pub struct MemoryMap { /* private */ }
```

#### Methods

```rust
impl MemoryMap {
    pub fn new() -> Self
```

Creates an empty address space.

```rust
    pub fn add_region(&mut self, base: u64, size: u64, region: MemoryRegion)
```

Inserts `region` spanning `[base, base + size)`. Panics if the range overlaps an existing region.

```rust
    pub fn remove_region(&mut self, base: u64) -> Option<MemoryRegion>
```

Removes and returns the region whose base address is exactly `base`. Returns `None` if no such region exists. Invalidates the `FlatView` cache.

```rust
    pub fn read(&self, addr: u64, size: usize) -> Result<u64, MemFault>
```

Reads `size` bytes (1, 2, 4, or 8) from `addr`, returns them zero-extended to `u64`. Returns `MemFault::Misaligned` if `addr % size != 0`.

```rust
    pub fn write(&mut self, addr: u64, size: usize, val: u64) -> Result<(), MemFault>
```

Writes the low `size * 8` bits of `val` to `addr`. Returns `MemFault::Misaligned` if `addr % size != 0`.

```rust
    pub fn read_bytes(&self, addr: u64, buf: &mut [u8]) -> Result<(), MemFault>
```

Bulk read; copies `buf.len()` bytes starting at `addr` into `buf`. No alignment requirement.

```rust
    pub fn write_bytes(&mut self, addr: u64, data: &[u8]) -> Result<(), MemFault>
```

Bulk write; copies `data` into the address space starting at `addr`.

```rust
    pub fn flat_view(&self) -> &FlatView
}
```

Returns a reference to the cached `FlatView`. The view is a sorted list of contiguous physical ranges; it is used internally by the executor for fast instruction fetch and by debuggers for memory inspection.

### `FlatView`

```rust
pub struct FlatView { /* private */ }
```

Read-only. Obtain via `MemoryMap::flat_view`. Provides iteration over physically contiguous ranges; the exact iteration API is documented in the `FlatView` source.

### `MemFault`

```rust
pub enum MemFault {
    UnmappedAddress(u64),
    Misaligned { addr: u64, size: usize },
    ReadOnly(u64),
}
```

| Variant | Meaning |
|---|---|
| `UnmappedAddress(addr)` | No region covers `addr`. |
| `Misaligned { addr, size }` | `addr` is not naturally aligned for an access of `size` bytes. |
| `ReadOnly(addr)` | A write was attempted to a read-only region. |

#### Worked Example

```rust
use helm_memory::{MemoryMap, MemoryRegion, MemFault};

let mut map = MemoryMap::new();

// Add 4 MiB of RAM at the standard RISC-V boot address.
map.add_region(
    0x8000_0000,
    4 * 1024 * 1024,
    MemoryRegion::Ram { data: vec![0u8; 4 * 1024 * 1024] },
);

// Write a 32-bit value and read it back.
map.write(0x8000_0000, 4, 0xDEAD_BEEF).unwrap();
let val = map.read(0x8000_0000, 4).unwrap();
assert_eq!(val, 0xDEAD_BEEF);

// Misaligned access returns an error.
assert!(matches!(
    map.read(0x8000_0001, 4),
    Err(MemFault::Misaligned { .. })
));
```

---

## helm-timing

**Purpose.** `helm-timing` provides timing model implementations consumed by `HelmEngine`. All three concrete models implement the `TimingModel` trait (see [`traits.md`](traits.md)), which the kernel uses to advance simulated time and enforce pipeline stalls.

### `VirtualTiming`

Advances time at an ideal IPC. When `ipc > 1.0`, fractional cycles are
accumulated internally and exposed as a floored integer tick through
`current_cycles()`. Suitable for functional runs, fast-forward, and any
case where timing fidelity is not needed.

```rust
pub struct VirtualTiming { /* private */ }
```

Constructed automatically by
`build_simulator(…, TimingChoice::VirtualTiming { ipc })`.

### `IntervalTiming`

Sniper-style interval simulation. Tracks class-weighted instruction
work, branch penalties, and an engine-owned two-level cache-locality
estimator. Useful for long-running workloads where approximate timing
is needed without paying for a full cycle-accurate model.

```rust
pub struct IntervalTiming { /* private */ }
```

Constructed by
`build_simulator(…, TimingChoice::IntervalTiming { .. })`.

The current default cache geometry is:

| Level | Default |
|---|---|
| L1D | 32 KiB, 8-way, 64-byte lines |
| L2 | 256 KiB, 8-way, 64-byte lines |

These values can be overridden through `TimingMemModelConfig` in Rust or
through the Python/example timing string surface:

```text
interval:interval_len=256,l1d_size=64KiB,l1d_assoc=4,l1d_line=128,l2_size=1MiB,l2_assoc=16,l2_line=128
```

### `AccurateTiming`

Full cycle-accurate pipeline model. Tracks in-order pipeline stages, cache miss penalties, and branch mispredictions. Significantly slower than `VirtualTiming` but produces realistic cycle counts.

```rust
pub struct AccurateTiming { /* private */ }
```

Constructed by `build_simulator(…, TimingChoice::AccurateTiming)`.

### `TimingModel` Trait

Defined in `helm-timing` and re-exported from `helm-engine`. See
[`traits.md`](traits.md) for the full method list. The key externally
visible contract is `current_cycles()`, which is what engine-owned timed
events and the Python `System.current_cycles` getter observe.

#### Worked Example

```rust
use helm_engine::{
    build_simulator, ExecMode, Isa, TimingCacheConfig, TimingChoice,
    TimingMemModelConfig,
};

let sim = build_simulator(
    Isa::RiscV,
    ExecMode::Functional,
    TimingChoice::IntervalTiming {
        ipc: 2.0,
        interval_len: 256,
        mem_model: TimingMemModelConfig {
            l1d: TimingCacheConfig::new(64 * 1024, 4, 128),
            l2: TimingCacheConfig::new(1024 * 1024, 16, 128),
        },
    },
    0,
    0x2000,
);
```

---

## helm-event

**Purpose.** `helm-event` implements a discrete-event queue keyed by simulator tick. Components (device models, DMA engines, timers) schedule callbacks at future ticks; the kernel drains the queue after each instruction or at configurable intervals.

### `EventQueue`

```rust
pub struct EventQueue { /* private */ }
```

#### Methods

```rust
impl EventQueue {
    pub fn new() -> Self
```

Creates an empty queue with current tick set to `0`.

```rust
    pub fn schedule<F: FnOnce() + 'static>(&mut self, tick: u64, f: F)
```

Schedules closure `f` to fire at `tick`. Multiple closures may be scheduled at the same tick; they fire in insertion order. `tick` must be `>= current_tick()`, otherwise the closure fires immediately on the next `drain_until` call.

```rust
    pub fn drain_until(&mut self, tick: u64)
```

Fires all pending events with a scheduled tick `<= tick`, advancing `current_tick` to `tick`. Safe to call with `tick < current_tick()`; this is a no-op.

```rust
    pub fn peek_next_tick(&self) -> Option<u64>
```

Returns the tick of the earliest pending event, or `None` if the queue is empty.

```rust
    pub fn current_tick(&self) -> u64
}
```

Returns the current tick counter.

### `TimedEvent`

An opaque handle returned by future versions of `schedule` for cancellation support. Currently informational only.

```rust
pub struct TimedEvent { /* private */ }
```

#### Worked Example

```rust
use helm_event::EventQueue;

let mut q = EventQueue::new();

q.schedule(100, || println!("tick 100 fired"));
q.schedule(200, || println!("tick 200 fired"));
q.schedule(100, || println!("tick 100 (second handler) fired"));

q.drain_until(150);
// Prints:
//   tick 100 fired
//   tick 100 (second handler) fired

assert_eq!(q.current_tick(), 150);
assert_eq!(q.peek_next_tick(), Some(200));
```

---

## helm-debug

**Purpose.** `helm-debug` provides three independent debugging facilities: a ring-buffered `TraceLogger` that records simulation events, a GDB RSP server that lets external debuggers attach over TCP, and a `CheckpointManager` that snapshots and restores full simulator state.

### `TraceEvent`

```rust
pub enum TraceEvent {
    InsnFetch { pc: u64, raw: u32 },
    MemRead  { addr: u64, size: u8, val: u64, cycle: u64 },
    MemWrite { addr: u64, size: u8, val: u64, cycle: u64 },
    Exception { vector: u32, pc: u64, tval: u64 },
    Syscall  { nr: u64, args: [u64; 6], ret: u64 },
    BranchMiss { pc: u64, target: u64, penalty: u32 },
}
```

| Variant | Fields | Description |
|---|---|---|
| `InsnFetch` | `pc`, `raw` | Instruction fetched at `pc`; `raw` is the 32-bit encoding. |
| `MemRead` | `addr`, `size`, `val`, `cycle` | Completed load of `size` bytes from `addr` returning `val`. |
| `MemWrite` | `addr`, `size`, `val`, `cycle` | Completed store of `size` bytes to `addr` with value `val`. |
| `Exception` | `vector`, `pc`, `tval` | Exception or interrupt taken; `tval` is the trap value (e.g., faulting address). |
| `Syscall` | `nr`, `args`, `ret` | Syscall emulation intercept. `args` are the six argument registers; `ret` is the return value written back. |
| `BranchMiss` | `pc`, `target`, `penalty` | Branch misprediction at `pc`; `target` is the correct target; `penalty` is the cycle penalty applied. |

### `TraceLogger`

```rust
pub struct TraceLogger { /* private */ }
```

#### Methods

```rust
impl TraceLogger {
    pub fn new(ring_capacity: usize) -> Self
```

Creates a logger with an in-memory ring buffer of `ring_capacity` events. Once full, the oldest events are overwritten.

```rust
    pub fn log(&self, event: TraceEvent)
```

Appends `event` to the ring buffer and dispatches it to all registered subscribers. This method is `&self` (not `&mut self`) so it can be called from multiple threads without locking the caller; internal synchronization uses a lock-free ring.

```rust
    pub fn flush_to_file(&mut self, path: &Path) -> io::Result<()>
```

Serializes all events currently in the ring buffer to `path` in a newline-delimited JSON format, then clears the buffer.

```rust
    pub fn subscribe<F: Fn(&TraceEvent) + Send + 'static>(&mut self, f: F)
```

Registers a subscriber callback that is called synchronously inside `log`. Subscribers must not block. Multiple subscribers are called in registration order.

```rust
    pub fn recent(&self, n: usize) -> Vec<TraceEvent>
}
```

Returns up to `n` of the most recent events from the ring buffer, newest last.

### `GdbServer`

Implements the GDB Remote Serial Protocol over TCP.

```rust
pub struct GdbServer { /* private */ }
```

#### Methods

```rust
impl GdbServer {
    pub fn bind(port: u16) -> io::Result<Self>
```

Binds a TCP listener on `127.0.0.1:port`. Returns an error if the port is already in use.

```rust
    pub fn accept_and_serve(&mut self, target: &mut dyn GdbTarget) -> io::Result<()>
}
```

Blocks until a GDB client connects, then enters the RSP serve loop. `target` receives commands (read/write registers, read/write memory, step, continue, set breakpoints). The loop exits when the client disconnects or sends a detach packet. `GdbTarget` is defined in [`traits.md`](traits.md).

### Supporting Types

```rust
pub struct CheckpointManager { /* private */ }
```

Saves and restores complete `HelmSim` state to disk. API: `save(path)` and `load(path)` (see source for full signatures). Used by `Simulation::checkpoint` and `Simulation::restore` in the Python layer.

```rust
pub enum StopReason {
    Breakpoint { pc: u64 },
    Watchpoint { addr: u64 },
    SingleStep,
    Exited { code: i32 },
    Halted,
}
```

```rust
pub enum BreakpointKind { Software, Hardware }
pub struct GdbReg { pub idx: u32, pub val: u64 }
```

#### Worked Example

```rust
use helm_debug::{TraceLogger, TraceEvent, GdbServer};
use std::sync::Arc;
use std::path::Path;

// --- TraceLogger ---
let mut logger = TraceLogger::new(4096);
logger.subscribe(|ev| {
    if let TraceEvent::Exception { vector, pc, .. } = ev {
        eprintln!("Exception vector=0x{:x} at pc=0x{:x}", vector, pc);
    }
});

let logger = Arc::new(logger);
// Pass Arc clone to HelmEngine:
// kernel.set_trace_logger(Arc::clone(&logger));

// After running, flush to disk:
// Arc::get_mut(&mut logger).unwrap().flush_to_file(Path::new("trace.jsonl")).unwrap();

// --- GdbServer ---
// In a thread:
// let mut gdb = GdbServer::bind(1234).unwrap();
// gdb.accept_and_serve(&mut my_gdb_target).unwrap();
```

---

## helm-stats

**Purpose.** `helm-stats` provides lock-free performance counters, histograms, and formula-based derived metrics. All statistics are registered through a `StatsRegistry` that can dump a JSON snapshot or print a human-readable table.

### `PerfCounter`

```rust
pub struct PerfCounter {
    pub name: String,
    pub desc: String,
    value: AtomicU64,  // private
}
```

#### Methods

```rust
impl PerfCounter {
    pub fn new(name: &str, desc: &str) -> Self
    pub fn inc(&self)
    pub fn inc_by(&self, n: u64)
    pub fn get(&self) -> u64
    pub fn reset(&self)
}
```

`inc` and `inc_by` use `Ordering::Relaxed` atomics; cross-thread ordering is the caller's responsibility. `reset` sets the counter to zero.

### `PerfHistogram`

```rust
pub struct PerfHistogram { /* private */ }
```

Tracks a distribution over configurable bucket boundaries. Obtain via `StatsRegistry::perf_histogram`. Key methods: `record(value: u64)`, `percentile(p: f64) -> u64`, `mean() -> f64`.

### `PerfFormula`

```rust
pub struct PerfFormula { /* private */ }
```

A derived metric defined as an expression over other registered statistics (e.g., `ipc = insns / cycles`). Evaluated lazily at dump time.

### `StatsRegistry`

```rust
pub struct StatsRegistry { /* private */ }
```

#### Methods

```rust
impl StatsRegistry {
    pub fn new() -> Self
```

Creates an empty registry.

```rust
    pub fn perf_counter(&mut self, name: &str, desc: &str) -> Arc<PerfCounter>
```

Creates and registers a `PerfCounter`. Returns an `Arc` that components hold to increment the counter. Panics if `name` is already registered.

```rust
    pub fn perf_histogram(&mut self, name: &str, desc: &str, buckets: &[u64]) -> Arc<PerfHistogram>
```

Creates a histogram with the given bucket upper bounds. `buckets` must be sorted and non-empty.

```rust
    pub fn dump_json(&self, path: &Path) -> io::Result<()>
```

Writes all statistics to `path` as a JSON object. Keys are stat names; values are current readings.

```rust
    pub fn print_table(&self)
}
```

Prints a formatted table of all statistics to stdout, suitable for end-of-run reporting.

#### Worked Example

```rust
use helm_stats::StatsRegistry;
use std::path::Path;

let mut reg = StatsRegistry::new();

let insns  = reg.perf_counter("sim.insns",  "Total instructions retired");
let cycles = reg.perf_counter("sim.cycles", "Total simulated cycles");

// In the execute loop:
insns.inc();
cycles.inc_by(2);  // e.g., a 2-cycle instruction

// At end of run:
reg.print_table();
reg.dump_json(Path::new("stats.json")).unwrap();
```

---

# Part 2: Python API Reference

## Package Structure

```python
from helm import (
    System,              # Top-level SimObject / controller
    Simulation,          # Backward-compatible alias for System
    Cpu,                 # CPU SimObject
    Ram,                 # RAM size descriptor
    MemorySpace,         # Physical address map
    Cache,               # Generic cache descriptor
    GicV2,               # ARM interrupt controller
    Pl011,               # ARM UART
    PortRef,             # Wiring helper
    HelmSpy,             # Observation session
    build_simulation,    # Backward-compatible factory
    set_sim_trace,       # Diagnostics sink control
)
```

The user-facing package is `helm`; it re-exports the `_helm_ng` PyO3
extension module. `Simulation` remains as a backward-compatible alias
for `System`.

---

## SimObject Base Class

```python
class SimObject:
    name: str
```

All SimObjects share this base type. `System.instantiate()` walks the
attached child objects, freezes the configuration, and constructs the
underlying Rust `HelmSim`.

---

## Cpu

Represents a single hardware thread. Timing selection lives on
`System.timing`; the current interval-timing cache hierarchy is
configured through that timing string rather than `Cpu` fields.

```python
class Cpu(SimObject):
    isa: str
    model: str
    width: int
    rob_size: int
    iq_size: int
    lq_size: int
    sq_size: int
```

### Parameters

| Parameter | Type | Default | Valid Values | Description |
|---|---|---|---|---|
| `isa` | `str` | `"aarch64"` | `"aarch64"`, `"arm64"`, `"riscv"`, `"riscv64"`, `"rv64"`, `"aarch32"`, `"arm32"` | Selects the instruction set. |
| `model` | `str` | `"cortex-a55"` | CPU model names from `list_cpu_models()` | Selects the architectural core model exposed through ID registers. |
| `width` | `int` | `4` | Positive integer | Front-end / issue width hint stored on the Python descriptor. |
| `rob_size` | `int` | `128` | Positive integer | Reorder-buffer size hint stored on the Python descriptor. |
| `iq_size` | `int` | `64` | Positive integer | Issue-queue size hint stored on the Python descriptor. |
| `lq_size` | `int` | `32` | Positive integer | Load-queue size hint stored on the Python descriptor. |
| `sq_size` | `int` | `32` | Positive integer | Store-queue size hint stored on the Python descriptor. |

---

## Cache

```python
class Cache(SimObject):
    size: str
    assoc: int
    latency: int
    line_size: int
```

### Parameters

| Parameter | Type | Default | Valid Range | Description |
|---|---|---|---|---|
| `size` | `str` | `"32KiB"` | Memory-size string such as `"32KiB"` | Cache capacity descriptor. |
| `assoc` | `int` | `8` | Positive integer | Set associativity descriptor. |
| `latency` | `int` | `4` | Positive integer | Cache-hit latency descriptor. |
| `line_size` | `int` | `64` | Positive integer | Cache line size in bytes. |

`Cache` is a generic SimObject descriptor. The current live
`IntervalTiming` hierarchy is configured through `System.timing` or the
`build_simulation(..., timing="interval:...")` string, for example:

```python
system = helm.System(
    "virt",
    timing="interval:interval_len=256,l1d_size=64KiB,l1d_assoc=4,l2_size=1MiB",
)
```

---

## Ram and MemorySpace

`Ram` describes a RAM object by size. `MemorySpace` holds explicit
physical address mappings for FS-style system composition.

```python
class Ram(SimObject):
    size: str

class MemorySpace(SimObject):
    def add_map(self, base: int, device: object, size: int, *, bank: int = 0) -> None: ...
```

### Parameters

| Parameter | Type | Default | Valid Range | Description |
|---|---|---|---|---|
| `Ram.size` | `str` | `"512MiB"` | Memory-size string such as `"512MiB"` | Total RAM size descriptor. |
| `MemorySpace.add_map(..., base, size, bank)` | `int` | `bank=0` | 64-bit base + size | Adds a device or RAM object to the physical address map. |

---

## Board Composition

There is no standalone `Board` pyclass in the current public surface.
Compose a machine by attaching `Cpu`, `Ram`, `MemorySpace`, and device
objects directly to `System`. For example, FS launchers attach
`system.cpu`, `system.mem`, `system.gic`, and `system.uart` before
calling `system.instantiate()`.

---

## System

`System` is the top-level controller and SimObject root. `Simulation` is
kept as a backward-compatible alias to this class.

```python
class System(SimObject):
    def __init__(self, name: str, *, timing: str = "virtual", mode: str = "se", ipc: float = 4.0): ...
    def instantiate(self) -> None: ...
    def run(self, max_insns: int) -> str: ...
```

`timing` accepts both simple model names and interval-timing override
strings:

```python
"virtual"
"interval"
"interval:interval_len=256,l1d_size=64KiB,l2_size=1MiB"
"accurate"
```

Key methods and properties:

| Member | Description |
|---|---|
| `instantiate()` | Freeze config and construct the underlying `HelmSim`. |
| `run(max_insns)` | Execute up to `max_insns` instructions and return a string stop reason such as `"quantum"` or `"exit:0"`. |
| `load_elf(binary, argv=None, envp=None)` | Configure SE mode from a static AArch64 ELF. |
| `load_kernel(kernel, dtb=None, dtb_bytes=None, initrd=None, append=None, num_cpus=1, gic_version="v3")` | Configure FS mode from a Linux kernel image plus DTB bytes or path. |
| `stats()` | Return a small dictionary containing `insn_count`, `tick_count`, `virtual_cycles`, `sim_freq`, and derived `ipc`. |
| `pc`, `insn_count`, `current_cycles` | Read-only execution state exposed as properties. |
| `set_cpu_model(name)` | Select the architectural CPU model exposed through ID registers. |
| `add_plugin(name, args="")` | Install a built-in plugin such as `stub-tracer`, `syscall-trace`, `cache`, or `mem-trace`. |

---

```python
def read_mem(self, addr: int, size: int) -> int:
```

Reads `size` bytes from physical address `addr`, returns the value zero-extended to a Python `int`.

| Argument | Type | Constraint |
|---|---|---|
| `addr` | `int` | Any mapped address |
| `size` | `int` | `1`, `2`, `4`, or `8` |

**Returns:** Unsigned integer value.

**Raises:** `MemoryError` on `MemFault::UnmappedAddress` or `MemFault::Misaligned`. `ValueError` if `size` is not 1, 2, 4, or 8.

---

```python
def write_mem(self, addr: int, size: int, val: int) -> None:
```

Writes the low `size * 8` bits of `val` to physical address `addr`.

| Argument | Type | Constraint |
|---|---|---|
| `addr` | `int` | Any mapped, writable address |
| `size` | `int` | `1`, `2`, `4`, or `8` |
| `val` | `int` | `0` – `2^(size*8) - 1` |

**Raises:** `MemoryError` on `MemFault`. `ValueError` for bad `size`. `OverflowError` if `val` exceeds `size`.

---

```python
def attach_gdb(self, port: int = 1234) -> None:
```

Starts a GDB RSP server on `port` (localhost only) and blocks until a client connects. The simulation is paused while waiting for the client; execution continues when the client sends a `continue` or `step` command.

| Argument | Type | Default | Constraint |
|---|---|---|---|
| `port` | `int` | `1234` | `1024` – `65535` |

**Raises:** `SimulationError` if the port cannot be bound (e.g., already in use or insufficient privileges).

---

```python
def enable_trace(self, path: str) -> None:
```

Enables tracing and directs trace output to `path` (newline-delimited JSON). Must be called before `run()`. Trace events include instruction fetches, memory accesses, exceptions, syscalls, and branch mispredictions.

**Raises:** `SimulationError` if `path` cannot be opened for writing.

---

## Enumerations

### `Isa`

```python
class Isa:
    RiscV   = ...
    AArch64 = ...
    AArch32 = ...
```

### `ExecMode`

```python
class ExecMode:
    Functional = ...
    Syscall    = ...
    System     = ...
```

### `TimingModel`

```python
class TimingModel:
    Virtual = ...
    Interval = ...
    Accurate = ...
```

### `StopReason`

```python
class StopReason:
    Breakpoint  = ...   # Execution stopped at a software or hardware breakpoint
    Watchpoint  = ...   # A watched memory address was accessed
    SingleStep  = ...   # One instruction was executed (after attach_gdb step command)
    Exited      = ...   # Program called exit(); check StopReason.exit_code
    Halted      = ...   # Simulator reached an unrecoverable halt state
```

`StopReason` instances carry extra attributes depending on the variant:

| Variant | Extra Attribute | Type | Description |
|---|---|---|---|
| `Breakpoint` | `.pc` | `int` | Address of the breakpoint. |
| `Watchpoint` | `.addr` | `int` | Watched address that was touched. |
| `Exited` | `.exit_code` | `int` | Program exit code (may be negative). |

---

## Param System

`Param` is a namespace for parameter descriptors. When you set
`cpu.isa = Isa.RiscV`, the `Param.Isa` descriptor validates the value
and stores it. Invalid values raise `TypeError` or `ValueError`
immediately at assignment time, not at `instantiate()` time.

### `Param.Isa`

Accepts only members of the `Isa` enumeration.

```python
cpu.isa = Isa.RiscV    # ok
cpu.isa = "riscv"      # raises TypeError
```

### `Param.ExecMode`

Accepts only members of `ExecMode`.

### `Param.Timing`

Accepts only members of `TimingModel`.

### `Param.MemorySize`

Accepts a string with an IEC binary suffix or a plain integer in bytes.

```python
mem.size = "256MiB"     # ok — 268435456 bytes
mem.size = "1GiB"       # ok — 1073741824 bytes
mem.size = 67108864     # ok — 64 MiB as integer
mem.size = "256 MB"     # ok — 256000000 bytes (note: MB != MiB)
mem.size = "3KiB"       # raises ValueError — not a power of two
```

Accepted suffixes: `B`, `KiB`, `MiB`, `GiB`, `TiB`, `KB`, `MB`, `GB`. Values must be powers of two (after suffix conversion).

### `Param.Int`

Accepts a Python `int` within the range specified on the field.

```python
cache.assoc = 8     # ok
cache.assoc = 33    # example invalid value for a bounded associativity field
cache.assoc = 3     # raises ValueError (must be power of two)
```

### `Param.Cycles`

Accepts a positive `int`. Same as `Param.Int` but semantically represents a cycle count.

### `Param.Addr`

Accepts a Python `int` in the range `[0, 2^64)`. Must be page-aligned (multiple of 4096) where noted.

```python
mem.base_addr = 0x80000000     # ok
mem.base_addr = 0x80000001     # raises ValueError — not page-aligned
```

---

## Complete Worked Example

The following example runs a statically linked AArch64 ELF binary in
syscall-emulation mode using interval timing with explicit cache
hierarchy overrides.

```python
import _helm_ng

sim = _helm_ng.build_simulation(
    isa="aarch64",
    mode="se",
    timing="interval:interval_len=256,l1d_size=64KiB,l1d_assoc=4,l1d_line=128,l2_size=1MiB,l2_assoc=16,l2_line=128",
    mem_mib=512,
    ipc=2.0,
)

sim.load_elf(
    "assets/aarch64/binaries/fish",
    ["fish", "-c", "echo hello"],
    ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C", "USER=helm"],
)

result = sim.run(1_000_000)
print("stop =", result)
print("insns =", sim.insn_count)
print("cycles =", sim.current_cycles)
print("pc =", hex(sim.pc))
```

---

## Error Handling

The current PyO3 bindings primarily raise standard Python exceptions
rather than a custom `helm.*Error` hierarchy.

| Exception | Typical source |
|---|---|
| `ValueError` | Unknown ISA / mode / timing string, bad interval-timing overrides, conflicting system configuration, invalid plugin or kernel arguments |
| `RuntimeError` | Using a system before `instantiate()`, mutating a `SimObject` after `instantiate()`, ELF/kernel loading failures returned from Rust |
| `AttributeError` | Accessing a missing child on a `SimObject`, assigning a non-child attribute through the SimObject child hook |
| `TypeError` | Python-side call/signature mismatch reported by PyO3 |

---

# Part 3: Error Reference

## Python-Facing Contract

The current Python API does not expose a stable custom exception
taxonomy for internal Rust enums such as `MemFault`,
`HartException`, or `SyscallError`. Treat the public contract as:

- Standard Python exception class (`ValueError`, `RuntimeError`,
  `AttributeError`, `TypeError`)
- The human-readable error message returned by the binding

For normal simulator control flow, prefer the explicit status surfaces
over exceptions:

- `System.run(max_insns)` returns strings such as `"quantum"` and
  `"exit:0"`
- `System.stats()` returns post-run counters
- `System.pc`, `System.insn_count`, and `System.current_cycles` expose
  basic execution state for debugging

## Recovery Patterns

### Catching configuration errors early

```python
import helm

system = helm.System(
    "virt",
    timing="interval:interval_len=256,l1d_size=bogus",
    mode="se",
)
system.cpu = helm.Cpu("cpu0", isa="aarch64")
system.ram = helm.Ram("ram0", size="512MiB")

try:
    system.instantiate()
except ValueError as exc:
    print(f"Configuration problem: {exc}")
```

### Handling runtime failures

```python
import helm

system = helm.System("virt", timing="virtual", mode="se")
system.cpu = helm.Cpu("cpu0", isa="aarch64")
system.ram = helm.Ram("ram0", size="512MiB")
system.instantiate()

try:
    reason = system.run(10_000_000)
    print(f"stop reason: {reason}")
except RuntimeError as exc:
    print(f"runtime failure: {exc}")
    print(system.stats())
    print(hex(system.pc))
```

---

*Generated for helm-ng. Cross-references: [`traits.md`](traits.md), [`object-model.md`](object-model.md).*

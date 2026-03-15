# helm-ng API Reference

> **See also:** [`traits.md`](traits.md) for trait definitions, [`object-model.md`](object-model.md) for the SimObject hierarchy.

helm-ng is a Rust-core, Python-config, multi-ISA simulator. The Rust crates implement all simulation logic; the `helm_ng` Python package (built via PyO3) exposes a high-level configuration and control surface.

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
  - [L1Cache](#l1cache)
  - [L2Cache](#l2cache)
  - [Memory](#memory)
  - [Board](#board)
  - [Simulation](#simulation)
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
    Virtual,
    Interval { interval_ns: u64 },
    Accurate,
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
    Virtual(HelmEngine<Virtual>),
    Interval(HelmEngine<Interval>),
    Accurate(HelmEngine<Accurate>),
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
pub fn build_simulator(isa: Isa, mode: ExecMode, timing: TimingChoice) -> HelmSim
```

The primary factory function. Constructs and returns a `HelmSim` variant matching `timing`. Use this instead of constructing `HelmEngine` directly when the timing model is not known at compile time.

#### Worked Example

```rust
use helm_engine::{build_simulator, Isa, ExecMode, TimingChoice};

// Build a RISC-V syscall-emulation simulator with virtual time.
let mut sim = build_simulator(
    Isa::RiscV,
    ExecMode::Syscall,
    TimingChoice::Virtual,
);

// Load a binary into memory, set the PC, then run.
sim.memory_mut().add_region(
    0x8000_0000,
    4 * 1024 * 1024,
    helm_memory::MemoryRegion::Ram { data: vec![0u8; 4 * 1024 * 1024] },
);
sim.arch_state_mut().write_pc(0x8000_0000);

let halt = sim.run_until_halt();
println!("Halted: {:?}", halt);
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

### `Virtual`

Advances time by a fixed cost per instruction type. The simplest model: each instruction has a configurable CPI derived from a static table. Suitable for functional runs where wall-clock correlation is not needed.

```rust
pub struct Virtual { /* private */ }
```

Constructed automatically by `build_simulator(…, TimingChoice::Virtual)`.

### `Interval`

Samples real wall-clock time every `interval_ns` nanoseconds and scales the simulated clock proportionally. Balances simulation accuracy with low overhead. Useful for long-running workloads where per-instruction timing is too expensive.

```rust
pub struct Interval { /* private */ }
```

Constructed by `build_simulator(…, TimingChoice::Interval { interval_ns })`.

The `interval_ns` field controls the sampling granularity. Smaller values increase timing accuracy at the cost of more frequent OS interactions.

### `Accurate`

Full cycle-accurate pipeline model. Tracks in-order pipeline stages, cache miss penalties, and branch mispredictions. Significantly slower than `Virtual` but produces realistic cycle counts.

```rust
pub struct Accurate { /* private */ }
```

Constructed by `build_simulator(…, TimingChoice::Accurate)`.

### `TimingModel` Trait

Defined in `helm-timing` and re-exported from `helm-engine`. See [`traits.md`](traits.md) for the full method list. The key contract: every timing model must implement `advance_cycle`, `stall`, and `current_tick`.

#### Worked Example

```rust
use helm_engine::{build_simulator, Isa, ExecMode, TimingChoice};

// Interval timing: sample every 1 ms of real time.
let sim = build_simulator(
    Isa::RiscV,
    ExecMode::Functional,
    TimingChoice::Interval { interval_ns: 1_000_000 },
);

// The inner timing object is accessible if you construct HelmEngine directly.
use helm_timing::Interval;
use helm_engine::HelmEngine;
let kernel: HelmEngine<Interval> = HelmEngine::new(
    Isa::RiscV,
    ExecMode::Functional,
    Interval::new(1_000_000),
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
from helm_ng import (
    Simulation,          # Top-level simulation controller
    Cpu,                 # CPU SimObject
    L1Cache,             # L1 cache SimObject
    L2Cache,             # L2 cache SimObject
    Memory,              # DRAM SimObject
    Board,               # Optional system board SimObject
    Isa,                 # Enum: RiscV | AArch64 | AArch32
    ExecMode,            # Enum: Functional | Syscall | System
    TimingModel,         # Enum: Virtual | Interval | Accurate
    Param,               # Namespace for parameter type descriptors
    StopReason,          # Enum returned by run() / run_until_halt()
    SimulationError,     # Base exception class
)
```

The `helm_ng` package is a PyO3 extension module. All classes are Python wrappers around Rust structs; method calls cross the FFI boundary and may raise `SimulationError` (or subclasses) on failure.

---

## SimObject Base Class

```python
class SimObject:
    name: str
    def elaborate(self): ...
```

All SimObjects share this interface. `elaborate()` is called automatically by `Simulation.elaborate()`; you should not call it directly unless building a custom simulation graph. After elaboration, parameter values are frozen — modifying them raises `SimulationError`.

---

## Cpu

Represents a single hardware thread. Attach optional cache objects to model memory hierarchy latency.

```python
class Cpu(SimObject):
    isa: Param.Isa           # Instruction set architecture
    mode: Param.ExecMode     # Execution mode
    timing: Param.Timing     # Timing model selection
    icache: Optional[L1Cache]  # Instruction cache (None = no icache model)
    dcache: Optional[L1Cache]  # Data cache (None = no dcache model)
```

### Parameters

| Parameter | Type | Default | Valid Values | Description |
|---|---|---|---|---|
| `isa` | `Param.Isa` | `Isa.RiscV` | `Isa.RiscV`, `Isa.AArch64`, `Isa.AArch32` | Selects the instruction set. |
| `mode` | `Param.ExecMode` | `ExecMode.Syscall` | `ExecMode.Functional`, `ExecMode.Syscall`, `ExecMode.System` | Determines OS and privilege level support. |
| `timing` | `Param.Timing` | `TimingModel.Virtual` | `TimingModel.Virtual`, `TimingModel.Interval`, `TimingModel.Accurate` | Timing model. See [helm-timing](#helm-timing) for semantics. |
| `icache` | `Optional[L1Cache]` | `None` | Any `L1Cache` instance or `None` | Instruction cache. When `None`, fetch latency is not modeled. |
| `dcache` | `Optional[L1Cache]` | `None` | Any `L1Cache` instance or `None` | Data cache. When `None`, load/store latency is not modeled. |

---

## L1Cache

```python
class L1Cache(SimObject):
    size: Param.MemorySize
    assoc: Param.Int
    hit_latency: Param.Cycles
```

### Parameters

| Parameter | Type | Default | Valid Range | Description |
|---|---|---|---|---|
| `size` | `Param.MemorySize` | `"32KiB"` | `"4KiB"` – `"4MiB"` | Cache capacity. Accepts strings like `"32KiB"`, `"256KiB"`. Must be a power of two. |
| `assoc` | `Param.Int` | `8` | `1` – `32` | Set associativity. Must be a power of two. |
| `hit_latency` | `Param.Cycles` | `4` | `1` – `100` | Latency on a cache hit, in simulated cycles. Only meaningful when `timing` is `Accurate` or `Interval`. |

---

## L2Cache

```python
class L2Cache(SimObject):
    size: Param.MemorySize
    assoc: Param.Int
    hit_latency: Param.Cycles
```

### Parameters

| Parameter | Type | Default | Valid Range | Description |
|---|---|---|---|---|
| `size` | `Param.MemorySize` | `"256KiB"` | `"64KiB"` – `"64MiB"` | Cache capacity. Must be a power of two. |
| `assoc` | `Param.Int` | `16` | `1` – `64` | Set associativity. Must be a power of two. |
| `hit_latency` | `Param.Cycles` | `12` | `1` – `500` | Latency on an L2 hit, in simulated cycles. |

---

## Memory

Represents the physical DRAM. Exactly one `Memory` object must be attached to each `Simulation`.

```python
class Memory(SimObject):
    size: Param.MemorySize
    base_addr: Param.Addr
```

### Parameters

| Parameter | Type | Default | Valid Range | Description |
|---|---|---|---|---|
| `size` | `Param.MemorySize` | `"256MiB"` | `"1MiB"` – `"256GiB"` | Total DRAM size. Must be a power of two. |
| `base_addr` | `Param.Addr` | `0x80000000` | Any 64-bit address | Physical base address of DRAM. Must be page-aligned (4 KiB). Default matches the standard RISC-V boot address. |

---

## Board

An optional container for multi-component system configurations. Holds references to a `Cpu`, `Memory`, and optional cache hierarchy. For simple single-CPU simulations, use `Simulation` directly without a `Board`.

```python
class Board(SimObject): ...
```

See [`object-model.md`](object-model.md) for the full `Board` API.

---

## Simulation

The top-level controller. Instantiate with a configured `Cpu` and `Memory`, call `elaborate()`, then drive execution with `run()` or `run_until_halt()`.

```python
class Simulation:
    def __init__(self, cpu: Cpu, memory: Memory, **kwargs): ...
```

`kwargs` are forwarded to the underlying `build_simulator` call. Currently recognized keys:

| Key | Type | Default | Description |
|---|---|---|---|
| `name` | `str` | `"sim"` | Human-readable simulation name used in stats output. |
| `stats_path` | `str \| None` | `None` | If set, stats are written to this path on `__del__`. |

### Methods

```python
def elaborate(self) -> None:
```

Finalizes the SimObject graph and constructs the underlying Rust `HelmSim`. Must be called exactly once before any `run`, `read_reg`, or memory method. Raises `SimulationError` if any parameter is invalid or if required components are missing.

---

```python
def run(self, n_instructions: int) -> StopReason:
```

Runs at most `n_instructions` instructions. Returns a `StopReason` explaining why execution stopped.

| Argument | Type | Constraint |
|---|---|---|
| `n_instructions` | `int` | Must be `>= 1`. |

**Returns:** `StopReason`

**Raises:** `SimulationError` if the simulator has not been elaborated, or if an unrecoverable internal error occurs.

---

```python
def run_until_halt(self) -> StopReason:
```

Runs until the simulation halts naturally (program exit, unhandled exception, or `wfi` with no pending interrupts).

**Returns:** `StopReason`

**Raises:** `SimulationError` on internal error.

---

```python
def checkpoint(self, path: str) -> None:
```

Saves the complete simulator state to `path`. The file is in an opaque binary format. Can be called at any point after `elaborate()`.

**Raises:** `SimulationError` if `path` cannot be written, or if the state cannot be serialized.

---

```python
def restore(self, path: str) -> None:
```

Restores simulator state from a checkpoint file written by `checkpoint`. The `Cpu` and `Memory` configuration of the current `Simulation` must match the configuration stored in the checkpoint; mismatches raise `SimulationError`.

**Raises:** `SimulationError` on IO failure, version mismatch, or configuration mismatch.

---

```python
def stats(self) -> dict[str, int | float]:
```

Returns a snapshot of all registered statistics as a plain Python dictionary. Keys are stat names (e.g., `"sim.insns"`, `"sim.cycles"`); values are `int` or `float`.

---

```python
def read_reg(self, idx: int) -> int:
```

Reads the integer register at index `idx`. Returns an unsigned 64-bit value.

| Argument | Type | Constraint |
|---|---|---|
| `idx` | `int` | `0` – `31` |

**Raises:** `IndexError` if `idx` is out of range. `SimulationError` if not elaborated.

---

```python
def write_reg(self, idx: int, val: int) -> None:
```

Writes `val` to integer register `idx`. For RISC-V, writes to `idx == 0` are silently discarded.

| Argument | Type | Constraint |
|---|---|---|
| `idx` | `int` | `0` – `31` |
| `val` | `int` | `0` – `2^64 - 1` |

**Raises:** `IndexError` if `idx` is out of range. `OverflowError` if `val` does not fit in 64 bits.

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

`Param` is a namespace for parameter descriptors. When you set `cpu.isa = Isa.RiscV`, the `Param.Isa` descriptor validates the value and stores it. Invalid values raise `TypeError` or `ValueError` immediately at assignment time, not at `elaborate()` time.

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
cache.assoc = 33    # raises ValueError (max for L1Cache.assoc is 32)
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

The following example runs a statically linked RISC-V ELF binary in syscall-emulation mode with a two-level cache hierarchy and tracing enabled.

```python
import helm_ng
from helm_ng import (
    Simulation, Cpu, L1Cache, L2Cache, Memory,
    Isa, ExecMode, TimingModel, StopReason,
)

# 1. Configure the cache hierarchy.
l1i = L1Cache()
l1i.size = "32KiB"
l1i.assoc = 8
l1i.hit_latency = 4

l1d = L1Cache()
l1d.size = "32KiB"
l1d.assoc = 8
l1d.hit_latency = 4

l2 = L2Cache()
l2.size = "512KiB"
l2.assoc = 16
l2.hit_latency = 12

# 2. Configure the CPU.
cpu = Cpu()
cpu.isa = Isa.RiscV
cpu.mode = ExecMode.Syscall
cpu.timing = TimingModel.Accurate
cpu.icache = l1i
cpu.dcache = l1d

# 3. Configure memory.
mem = Memory()
mem.size = "256MiB"
mem.base_addr = 0x80000000

# 4. Build the Simulation.
sim = Simulation(cpu=cpu, memory=mem, name="hello_world")

# 5. Enable trace output before elaboration.
sim.enable_trace("/tmp/helm_trace.jsonl")

# 6. Elaborate (finalizes configuration, builds Rust objects).
sim.elaborate()

# 7. Write the binary into memory.
binary_data: bytes = open("hello.elf", "rb").read()
# (In practice, use the ELF loader helper — see object-model.md)
for offset, chunk in enumerate_elf_segments(binary_data):
    for i, byte in enumerate(chunk):
        sim.write_mem(mem.base_addr + offset + i, 1, byte)

# Set the program counter to the ELF entry point.
entry_point = 0x80000000  # replace with actual ELF e_entry
sim.write_reg(0, 0)       # x0 always 0
# (RISC-V PC is not a GPR; write via the internal API)

# 8. Run until the program exits.
reason = sim.run_until_halt()

if reason == StopReason.Exited:
    print(f"Program exited with code {reason.exit_code}")
elif reason == StopReason.Halted:
    pc = sim.read_reg(0)  # inspect PC via stats or GDB
    print(f"Simulation halted unexpectedly")

# 9. Inspect statistics.
stats = sim.stats()
insns  = stats["sim.insns"]
cycles = stats["sim.cycles"]
ipc    = insns / cycles if cycles else 0.0
print(f"Instructions: {insns:,}")
print(f"Cycles:       {cycles:,}")
print(f"IPC:          {ipc:.3f}")
```

---

## Error Handling

### Python Exception Hierarchy

```
SimulationError          — base class for all helm_ng errors
├── ConfigurationError   — invalid parameter values or missing required objects
├── ElaborationError     — error during Simulation.elaborate()
├── ExecutionError       — runtime error during run() / run_until_halt()
├── MemoryError          — corresponds to helm_memory::MemFault
└── DebugError           — GDB server errors, checkpoint I/O failures
```

Standard Python exceptions are also raised in some cases:

| Situation | Exception |
|---|---|
| Register index out of range | `IndexError` |
| `val` too large for `size` in `write_mem` | `OverflowError` |
| Bad `size` argument (not 1/2/4/8) | `ValueError` |
| Wrong type assigned to a `Param` | `TypeError` |
| Parameter out of valid range | `ValueError` |
| File not found in `restore()` | `FileNotFoundError` |

---

# Part 3: Error Reference

## Rust Error Types

### `MemFault` (helm-memory)

```rust
pub enum MemFault {
    UnmappedAddress(u64),
    Misaligned { addr: u64, size: usize },
    ReadOnly(u64),
}
```

| Variant | Meaning | Python mapping |
|---|---|---|
| `UnmappedAddress(addr)` | The address is not covered by any `MemoryRegion`. | `MemoryError` with message `"unmapped address 0x…"` |
| `Misaligned { addr, size }` | The address is not naturally aligned for the access width. | `MemoryError` with message `"misaligned access …"` |
| `ReadOnly(addr)` | A write was directed at a region with no write handler. | `MemoryError` with message `"read-only address 0x…"` |

### `HartException` (helm-engine)

Returned by `HelmEngine::step_once` and `HelmSim::step_once`.

```rust
pub enum HartException {
    IllegalInstruction { pc: u64, raw: u32 },
    InstructionAddressMisaligned { pc: u64 },
    LoadAddressMisaligned { addr: u64 },
    StoreAddressMisaligned { addr: u64 },
    EnvironmentCall,
    Breakpoint,
    // … additional RISC-V / AArch64 exception codes
}
```

In Python, `HartException` surfaces as `ExecutionError` with the variant name in the message string and the relevant address in `ExecutionError.address` (when applicable).

### `SyscallError` (helm-engine/se)

```rust
pub enum SyscallError {
    UnknownSyscall(u64),
    BadArgument { nr: u64, arg_idx: usize, val: u64 },
    IoError(io::Error),
    NotImplemented(u64),
}
```

Surfaces in Python as `ExecutionError` with `ExecutionError.syscall_nr` set to the syscall number.

### `HaltReason` (helm-engine)

Returned by `run_until_halt`. Not an error, but included here for completeness.

```rust
pub enum HaltReason {
    Breakpoint { pc: u64 },
    ExitCall { code: i32 },
    Faulted { exception: HartException },
    WaitForInterrupt,
}
```

Maps to `StopReason` on the Python side:

| `HaltReason` | `StopReason` |
|---|---|
| `Breakpoint` | `StopReason.Breakpoint` |
| `ExitCall { code }` | `StopReason.Exited` with `.exit_code = code` |
| `Faulted` | `StopReason.Halted` (and may raise `ExecutionError` if `step_once` was used) |
| `WaitForInterrupt` | `StopReason.Halted` |

---

## PyO3 Error Propagation

All fallible Rust functions return `Result<T, E>`. The PyO3 layer converts errors according to this table:

| Rust error type | Python exception class |
|---|---|
| `MemFault::UnmappedAddress` | `helm_ng.MemoryError` |
| `MemFault::Misaligned` | `helm_ng.MemoryError` |
| `MemFault::ReadOnly` | `helm_ng.MemoryError` |
| `HartException` | `helm_ng.ExecutionError` |
| `SyscallError` | `helm_ng.ExecutionError` |
| `io::Error` (checkpoint/trace) | `helm_ng.DebugError` (wraps OS message) |
| Parameter validation failures | `TypeError` or `ValueError` (raised in Python descriptor) |
| Configuration validation in `elaborate` | `helm_ng.ElaborationError` |

All `helm_ng.*Error` classes carry a `.message: str` attribute with the Rust `Display` output, and a `.rust_source: str` attribute with the error type name for programmatic inspection.

---

## Error Recovery Patterns

### Handling a memory fault

```python
from helm_ng import Simulation, MemoryError as HelmMemoryError

sim: Simulation = ...
try:
    val = sim.read_mem(0xDEAD_BEEF, 4)
except HelmMemoryError as exc:
    print(f"Memory fault: {exc.message}")
    # Inspect where the PC is at the point of fault:
    pc = sim.stats().get("sim.last_pc", 0)
    print(f"PC at fault: 0x{pc:016x}")
```

### Handling an unexpected halt

```python
from helm_ng import StopReason, ExecutionError

try:
    reason = sim.run_until_halt()
except ExecutionError as exc:
    print(f"Execution error: {exc.message} (type: {exc.rust_source})")
    raise

if reason == StopReason.Halted:
    # Dump stats and trace for post-mortem analysis.
    print(sim.stats())
    # Trace was already flushed if enable_trace() was called.
```

### Checkpoint / restore on error

```python
import tempfile, os

sim.elaborate()
ckpt = tempfile.mktemp(suffix=".helm")
sim.checkpoint(ckpt)

try:
    reason = sim.run(10_000_000)
except Exception:
    # Roll back to the pre-run state.
    sim.restore(ckpt)
    raise
finally:
    os.unlink(ckpt)
```

### Catching configuration errors early

```python
from helm_ng import Cpu, Memory, Simulation, Isa, ExecMode
from helm_ng import ElaborationError, SimulationError

cpu = Cpu()
cpu.isa = Isa.RiscV
cpu.mode = ExecMode.Syscall

mem = Memory()
mem.size = "256MiB"

sim = Simulation(cpu=cpu, memory=mem)
try:
    sim.elaborate()
except ElaborationError as exc:
    print(f"Configuration problem: {exc.message}")
    # Fix parameters and retry.
```

---

*Generated for helm-ng. Cross-references: [`traits.md`](traits.md), [`object-model.md`](object-model.md).*

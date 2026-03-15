# helm-ng — System High-Level Design

> Canonical top-level design document for the helm-ng multi-ISA, multi-mode, multi-timing hardware simulator.
> Cross-references: [`ARCHITECTURE.md`](../ARCHITECTURE.md) · [`docs/design/helm-python/HLD.md`](./helm-python/HLD.md) · [`docs/design/helm-engine/LLD-world.md`](./helm-engine/LLD-world.md)

---

## Table of Contents

1. [Project Overview](#1-project-overview)
2. [The 4-Item Irreducible Core](#2-the-4-item-irreducible-core)
3. [Crate Map](#3-crate-map)
4. [Crate Dependency DAG](#4-crate-dependency-dag)
5. [Execution Modes](#5-execution-modes)
6. [Timing Models](#6-timing-models)
7. [The World](#7-the-world)
8. [HelmEventBus](#8-helmeventbus)
9. [Python Config Model](#9-python-config-model)
10. [Plugin System](#10-plugin-system)
11. [Phased Build Plan](#11-phased-build-plan)
12. [Key Design Principles](#12-key-design-principles)

---

## 1. Project Overview

helm-ng is a next-generation, research-grade hardware simulator written in Rust with Python configuration. It targets the same problem space as gem5, SIMICS, and QEMU, but is designed from first principles for a single developer: clean architecture over feature sprawl, correctness before performance, and Python ergonomics over C++ complexity.

**Primary users:**

- Computer architecture researchers who need a fast, accurate, multi-mode simulator they can modify.
- Hardware/software co-design engineers who need to test device models without a full platform.
- Systems programmers who want to validate ISA-level code before running on real hardware.

**What helm-ng provides:**

- Multi-ISA support (RISC-V RV64GC, AArch64, AArch32) in a single binary, selected at runtime.
- Three orthogonal execution modes: Functional, Syscall Emulation, Full System.
- Three composable timing models: Virtual (event-driven), Interval (Sniper-style), Accurate (cycle-level).
- A headless device simulation environment (`World`) for device unit testing and fuzzing without a CPU.
- A Gem5-style Python configuration layer via PyO3, allowing platform description in Python while simulation runs in Rust.
- Deterministic-by-default operation: no threads, no wall-clock dependence, reproducible across runs.
- First-class checkpointing, GDB RSP debugging, event tracing, and a statistics system.

**What helm-ng explicitly is not:**

- A JIT/binary translator (planned for a future phase after correctness is established).
- A production emulator for end users (design goal is research and device validation, not user-facing speed).
- A complete QEMU or gem5 replacement in scope (no USB, audio, GPU, or display subsystem).

---

## 2. The 4-Item Irreducible Core

First-principles analysis of every existing simulator (gem5, QEMU, SIMICS, Spike, Dromajo) reveals that all simulation complexity decomposes to exactly four irreducible abstractions. Everything else — caches, timing, events, devices, OS interfaces, config layers — composes on top of these four.

```
1. ArchState     — All architecturally-visible state: integer registers, FP registers,
                   PC, CSRs (RISC-V) or system registers (AArch64), PSTATE, CPSR.
                   ISA-specific fields live inside typed sub-structs.

2. Decoder       — bytes → Instruction enum
                   Consumes raw bytes from the fetch address, returns a decoded
                   instruction variant. ISA-specific. Pure function: no side effects.

3. Executor      — (ArchState, Instruction, MemInterface) → (ΔArchState, EffectList)
                   Applies the instruction semantics. Reads/writes ArchState.
                   Calls MemInterface for load/store. Raises exceptions on fault.

4. MemInterface  — read(addr, size) → bytes  |  write(addr, size, bytes) → ()
                   The bridge between the CPU model and the memory subsystem.
                   Three access modes: Timing (async), Atomic (sync+latency), Functional (instant).
```

**Architectural consequence:** Build and validate these four items first. A passing RISC-V test suite against just these four (flat RAM, no events, no timing, no devices) is Phase 0 complete. Every other crate is layered on top and can be added incrementally.

---

## 3. Crate Map

All crates live under `crates/` in the Cargo workspace (`workspace.members = ["crates/*"]`).

| Crate | Purpose | Key Types |
|---|---|---|
| `helm-core` | The 4-item core: register file, memory interface contracts, execution context traits, exception types. No ISA specifics. | `ArchState`, `ExecContext`, `ThreadContext`, `MemInterface`, `MemFault`, `HartException` |
| `helm-arch` | All ISA implementations: decode + execute for RISC-V, AArch64, AArch32. Each ISA in its own sub-module with its own test vectors. | `RiscvDecoder`, `Aarch64Decoder`, `Instruction` (per-ISA enum), `execute()` |
| `helm-memory` | Unified memory subsystem: region tree, flat address-space view, MMIO dispatch, cache model, TLB. | `MemoryRegion`, `MemoryMap`, `FlatView`, `MmioHandler`, `CacheModel`, `TlbModel` |
| `helm-timing` | Three timing model implementations. `TimingModel` trait + `Virtual`, `Interval`, `Accurate` structs. `MicroarchProfile` for Interval/Accurate configuration. | `TimingModel`, `Virtual`, `Interval`, `Accurate`, `MicroarchProfile` |
| `helm-event` | Discrete event queue: time-ordered callbacks scheduled by devices and the timing model. | `EventQueue`, `TimedEvent`, `EventClass`, `EventHandle` |
| `helm-devices/src/bus/event_bus` | Observable pub-sub event system (SIMICS HAP-style). Synchronous: subscribers run inline when an event fires. | `HelmEventBus`, `HelmEvent`, `HelmEventKind`, `SubscriberId` |
| `helm-engine` | The simulation kernel. `HelmEngine<T: TimingModel>` drives the instruction loop. `HelmSim` is the PyO3-boundary enum. The factory `build_simulator()` is here. | `HelmEngine<T>`, `HelmSim`, `ExecMode`, `Isa`, `build_simulator()` |
| `helm-devices` | Device trait, interrupt pin/wire/sink model, device parameter schema, device registry, `.so` plugin loader, bus sub-module (PCI, AMBA/I2C/SPI). | `Device`, `SimObject`, `InterruptPin`, `InterruptWire`, `InterruptSink`, `DeviceRegistry`, `Bus`, `BusDevice` |
| `helm-engine/se` | Syscall emulation: intercepts syscall instructions and dispatches to host OS or a simulated table. | `LinuxSyscallHandler`, `SyscallTable`, `SyscallDispatch` |
| `helm-debug` | GDB RSP server stub, trace logger (ring buffer), checkpoint manager. | `GdbServer`, `TraceLogger`, `TraceEvent`, `CheckpointManager` |
| `helm-stats` | Performance counter registration, histograms, derived formula counters, JSON/CSV dump. | `PerfCounter`, `PerfHistogram`, `PerfFormula`, `StatsRegistry` |
| `helm-engine` | Headless device simulation: no CPU, no ISA. Drives devices via MMIO, advances a virtual clock, observes interrupts. | `World`, `HelmObjectId`, `VirtualClock`, `EventHandle` |
| `helm-python` | PyO3 bindings + `helm_ng` Python package. Two layers: raw `#[pyclass]` bindings and a high-level Python DSL. | `PySimulation`, `#[pyfunction] build_simulator`, Python `Simulation`, `Cpu`, `Cache`, `Memory`, `Param.*` |

---

## 4. Crate Dependency DAG

Arrows point from dependent → dependency. There are no cycles.

```
helm-python
  ├── helm-engine
  │     ├── helm-core
  │     ├── helm-arch
  │     │     └── helm-core
  │     ├── helm-memory
  │     │     └── helm-core
  │     ├── helm-timing
  │     │     ├── helm-core
  │     │     └── helm-event
  │     ├── helm-event
  │     ├── helm-devices/bus
  │     ├── helm-devices
  │     │     ├── helm-memory
  │     │     └── helm-event
  │     ├── helm-engine/se
  │     │     └── helm-core
  │     ├── helm-debug
  │     │     ├── helm-core
  │     │     └── helm-devices/bus
  │     └── helm-stats
  └── helm-engine
        ├── helm-devices
        ├── helm-memory
        ├── helm-event
        ├── helm-devices/bus
        └── helm-stats

helm-stats    (no helm-* deps)
helm-event    (no helm-* deps)
helm-devices/bus (no helm-* deps)
helm-core     (no helm-* deps)
```

**Zero-dependency leaf crates** (safe to build and test independently):
- `helm-core` — pure trait and type definitions
- `helm-event` — pure event queue, no simulation types
- `helm-devices/src/bus/event_bus` — pure pub-sub, no simulation types
- `helm-stats` — pure counter/histogram, no simulation types

**Key invariants enforced by the DAG:**
- `helm-arch` never imports `helm-engine`. ISAs do not know about the kernel.
- `helm-devices` never imports `helm-arch` or `helm-engine`. Devices do not know about CPUs or ISAs.
- `helm-engine` never imports `helm-engine`, `helm-arch`, or `helm-core`. World is CPU-free by construction.
- `helm-memory` never imports `helm-arch`. The memory system is ISA-agnostic.

---

## 5. Execution Modes

Execution mode (`ExecMode` enum) controls how the simulation handles events that escape the 4-item core: syscalls, interrupts, exceptions, and I/O. The CPU model (`HelmEngine<T>`) is identical across all modes; only the handler dispatch changes.

### ExecMode::Functional (FE)

Executes instructions correctly with no timing and no OS interface. When a syscall instruction is encountered, the engine raises `HartException::UnhandledSyscall` and stops. When a trap vector would fire, the engine either injects an exception into architectural state or panics (mode-dependent).

**Use cases:** ISA test suite validation, fast-forward front-end, correctness testing before adding a higher-level mode.

**Speed:** 100M–1B instructions/sec (no cache model, no event queue overhead).

**Constraints:** Cannot run code that makes syscalls or relies on device I/O. No OS model, no device drivers.

### ExecMode::Syscall (SE)

FE plus a `SyscallHandler` that intercepts syscall instructions and dispatches them to a host OS emulation layer (`helm-engine/se`). The guest's register file is read to extract syscall number and arguments; the handler calls the appropriate host or simulated syscall; return values are written back to the guest register file.

**Use cases:** Running statically-linked Linux userspace binaries without booting a kernel. The canonical Phase 0 mode.

**Speed:** 50M–500M instructions/sec (syscall overhead amortized over millions of instructions between calls).

**Constraints:** Limited syscall coverage (Phase 0: ~50 essential syscalls). No dynamic linking support until virtual filesystem is added. No signal delivery.

### ExecMode::System (FS)

Full system simulation. The CPU model boots a real kernel. Device models handle I/O. Interrupt controllers route device signals to CPU exception vectors. The MMU enforces page table permissions and generates page faults.

**Use cases:** OS research, driver development, full-stack accuracy, booting Linux.

**Speed:** 1M–50M instructions/sec (full device model overhead, interrupt routing, TLB).

**Constraints:** Requires complete device tree (PLIC/GIC, CLINT/generic timer, storage, serial). Phase 3 deliverable.

### World (no HelmEngine) (World)

Not a mode of `HelmEngine`. `World` replaces `HelmEngine` entirely: no CPU, no ISA, no `ArchState`. The user drives devices directly via `mmio_write`/`mmio_read` and advances a virtual clock. Devices use the same `Device` trait, `SimObject` lifecycle, `EventQueue`, and `HelmEventBus` as in a full system.

**Use cases:** Device unit testing, fuzzing, bus protocol simulation, RTL co-simulation, SoC bring-up without a functional CPU.

**Speed:** Millions of MMIO transactions/sec (no instruction decode overhead).

**Constraints:** No CPU-side validation. Tests must manually drive all transactions that firmware would have generated.

### Mode Comparison

| Aspect | FE | SE | FS | Device |
|---|---|---|---|---|
| CPU model | Yes | Yes | Yes | No |
| ISA decode+execute | Yes | Yes | Yes | No |
| Syscall handler | No | Yes | N/A (kernel) | No |
| Device models | Optional | Optional | Required | Yes (primary) |
| Interrupt routing | No | No | Yes | Optional |
| MMU/page tables | No | No | Yes | No |
| Kernel boot | No | No | Yes | No |
| Phase available | Phase 0 | Phase 0 | Phase 3 | Phase 1 |

---

## 6. Timing Models

Timing model (`TimingModel` trait, `T` generic parameter on `HelmEngine<T>`) controls how simulated time is tracked and how memory access latencies are modeled. Timing is orthogonal to execution mode: any `ExecMode` can be combined with any `TimingModel`.

The key architectural decision: **timing is the only axis that is monomorphized.** `HelmEngine<Virtual>`, `HelmEngine<Interval>`, and `HelmEngine<Accurate>` are three distinct types. The compiler inlines `T::on_memory_access()` into the hot loop with zero overhead. Switching timing model requires constructing a new `HelmSim` — there is no runtime switching.

### Virtual (Event-Driven)

A global virtual clock driven by a `BinaryHeap<Reverse<TimedEvent>>` priority queue. Every latency is a scheduled future event. When a memory access completes, the timing model posts an event at `now + latency_cycles` and continues executing. Devices' timer callbacks are also posted to this queue.

There is no real-time relationship. The virtual clock advances at the rate that events are drained, which is bounded by `EventQueue` throughput.

**Accuracy:** No IPC model. All instructions take 1 virtual tick. Cache misses are modeled as latency events but do not stall the pipeline.

**Target:** Functional correctness with observable simulated time for device timers and event ordering. Not suitable for performance research.

**Speed:** 10M–100M instructions/sec.

### Interval (Sniper-Style)

The Interval model executes instructions functionally in chunks (intervals), then applies timing corrections at interval boundaries — cache misses, branch mispredictions, and TLB misses. Between miss events, the model assumes IPC from a `MicroarchProfile` (configurable, loaded from JSON). At a miss event, it computes the penalty, advances the virtual clock by the penalty, and resumes functional execution.

This matches the Sniper simulator's core insight: most instructions execute at the predicted IPC; only miss events perturb that prediction.

**Accuracy:** ~5% IPC error vs. cycle-accurate on SPEC CPU benchmarks (Sniper reference result).

**Target:** Performance research where exact cycle accuracy is not required but IPC trends, cache miss rates, and branch behavior must be correct.

**Speed:** 10M–100M instructions/sec.

### Accurate (Cycle-Accurate)

A cycle-accurate pipeline model. Every instruction flows through pipeline stages; structural hazards, data hazards, and control hazards are modeled per cycle. The Phase 3 initial implementation is a 5-stage in-order pipeline; out-of-order with ROB/RS is deferred.

**Accuracy:** Cycle-accurate for in-order pipelines. ~10–20% IPC error for OoO workloads on the Phase 3 5-stage model.

**Target:** Microarchitecture research, RTL correlation.

**Speed:** 0.1M–2M instructions/sec.

### Timing Model Comparison

| Model | IPC Source | Miss Modeling | Speed | Accuracy Target |
|---|---|---|---|---|
| Virtual | 1 insn/tick (no model) | Latency events, no stall | Fastest | Event ordering only |
| Interval | `MicroarchProfile.ipc` | Penalty at miss boundary | Fast | ~5% IPC error |
| Accurate | Pipeline stages | Exact cycle stall | Slowest | RTL-correlatable |

---

## 7. The World

`World` (in the full-system context, realized as `System` inside `HelmEngine<T>`) is the owner of all simulation state. Nothing in the simulation exists outside of `World`'s ownership graph. This is the "no dark state" principle: if it is not reachable from `World`, it does not participate in checkpointing, reset, or statistics.

### What World Owns

```
World / System
├── objects: IndexMap<String, Box<dyn SimObject>>   — component tree, keyed by full path
├── memory:  MemoryMap                              — unified address space (RAM, MMIO, ROM, alias)
├── event_queue: EventQueue                         — time-ordered callbacks (helm-event)
├── event_bus:   Arc<HelmEventBus>                  — synchronous observable events (helm-devices/bus)
├── stats:       StatsRegistry                      — performance counter registry (helm-stats)
├── interface_registry: InterfaceRegistry           — named typed interfaces between objects
└── attr_registry:      AttrRegistry                — named typed attributes (checkpointed state)
```

`HelmEngine<T>` additionally owns:
- `arch: ArchState` — the architectural register file
- `timing: T` — the timing model instance
- The GDB server handle and trace logger (if enabled)

### Instantiate Flow

`World::instantiate(pending_objects)` is called by Python (via `PySimulation::elaborate()`) after Python has assembled the object graph. The flow:

```
1. Python builds PendingObject list (component type + param values)
2. Python calls sim.elaborate()  →  PyO3  →  World::instantiate()
3. World creates Box<dyn SimObject> for each PendingObject via DeviceRegistry factory
4. World::register() — inserts each component into the component tree under its path
5. Lifecycle: for each component in registration order → component.init()
6. Lifecycle: for each component in registration order → component.elaborate(&mut system)
   - MemoryMap regions registered here
   - Cross-component Arc references stored here
   - Interrupt wires connected here
7. System::validate_wiring() — check no MMIO overlap, no dangling interrupt lines
8. Lifecycle: for each component in registration order → component.startup()
   - Initial events scheduled here
   - Initial signal states asserted here
9. Return initialized HelmEngine<T> wrapped in HelmSim enum variant
```

After step 9, Python may call `sim.run()` but cannot modify the component graph. All configuration is frozen.

### Reset Path

`world.reset()` calls `component.reset()` on every `SimObject` in registration order. Wiring is not rebuilt; the component graph is preserved. The architectural state is reset to power-on defaults. This enables repeated simulation runs with the same platform configuration and different workloads.

---

## 8. HelmEventBus

`HelmEventBus` is the observability bus for helm-ng — the equivalent of SIMICS HAPs (Hardware Action Points). It is a synchronous, named, typed pub-sub system: any component fires a `HelmEvent`; any subscriber (tool, debugger, Python script) receives it inline.

### Relationship to EventQueue

These are two distinct systems with different purposes:

| System | Crate | Purpose | Timing |
|---|---|---|---|
| `EventQueue` | `helm-event` | Schedule future callbacks at simulated tick T | Asynchronous — deferred |
| `HelmEventBus` | `helm-devices/src/bus/event_bus` | Observable named events fired by components now | Synchronous — inline |

The `EventQueue` is how devices schedule "fire interrupt at tick 5000." The `HelmEventBus` is how a tracer observes "a MemWrite just happened."

### Event Taxonomy

```rust
pub enum HelmEvent {
    Exception    { cpu: &'static str, vector: u32, pc: u64, tval: u64 },
    MemWrite     { addr: u64, size: usize, val: u64, cycle: u64 },
    MemRead      { addr: u64, size: usize, val: u64, cycle: u64 },
    CsrWrite     { csr: u16, old: u64, new: u64 },
    MagicInsn    { pc: u64, value: u64 },       // SIMICS-style debug marker
    SyscallEnter { nr: u64, args: [u64; 6] },
    SyscallReturn{ nr: u64, ret: u64 },
    ModeChange   { from: ExecMode, to: ExecMode },
    DeviceSignal { device: &'static str, port: &'static str, val: u64 },
    Custom       { name: &'static str, data: Vec<u8> },
}
```

### Who Fires

- `HelmEngine` fires: `Exception`, `MemWrite`, `MemRead`, `CsrWrite`, `MagicInsn`, `SyscallEnter`, `SyscallReturn`, `ModeChange`.
- Device implementations fire: `DeviceSignal`, `Custom`.
- Tests and World fire: `Custom` (for test-driven events).

### Who Subscribes

- `TraceLogger` — subscribes to all events, writes to ring buffer.
- GDB stub — subscribes to `Exception` to break on traps.
- Python user scripts — subscribe via `sim.event_bus.subscribe("Exception", callback)`.
- `World` tests — subscribe via `world.on_event(kind, callback)`.

### Ownership

`System` owns the `HelmEventBus`. `HelmEngine<T>` holds `Arc<HelmEventBus>`. All device implementations hold `Arc<HelmEventBus>` clones acquired at `elaborate()` time. No locking is needed during `fire()` because the bus is synchronous and single-threaded.

### Python Integration

```python
def on_exception(event):
    print(f"Exception vector={event.vector:#x} at pc={event.pc:#x}")
    sim.pause()

sim.event_bus.subscribe("Exception", on_exception)
sim.event_bus.subscribe("MagicInsn", lambda e: print(f"ROI marker at {e.pc:#x}"))
```

Subscribers are called from the Rust simulation thread. The PyO3 GIL is re-acquired before calling Python callbacks and released on return.

---

## 9. Python Config Model

helm-ng uses a two-phase model inspired by gem5: Python describes the machine; Rust simulates it.

### Phase 1 — Python Configuration

The user writes a Python script that imports `helm_ng`, instantiates components as Python objects, sets parameters, wires connections, and calls `sim.elaborate()`.

```python
from helm_ng import Simulation, Cpu, L1Cache, Memory, Board, Isa, ExecMode, Timing

cpu   = Cpu(isa=Isa.RiscV, mode=ExecMode.Syscall, timing=Timing.Virtual)
icache = L1Cache(size="32KiB", assoc=8, hit_latency=4)
dcache = L1Cache(size="32KiB", assoc=8, hit_latency=4)
mem   = Memory(size="256MiB", base=0x8000_0000)

cpu.icache = icache
cpu.dcache = dcache

board = Board(cpu=cpu, memory=mem)
sim = Simulation(root=board)
sim.elaborate()
sim.run(n_instructions=1_000_000_000)
```

After `sim.elaborate()` returns, Python configuration is complete. The `Simulation` object becomes opaque; the Rust engine owns all simulation state.

### Phase 2 — Rust Simulation

`sim.run()` (via PyO3) calls `HelmSim::run()` which calls `HelmEngine::run()`. The hot loop runs entirely in Rust. Python is not invoked during the hot loop unless a `HelmEventBus` subscriber fires a Python callback.

`sim.run()` releases the Python GIL before entering the Rust loop. Python threads can do other work during simulation.

### PendingObject Protocol

Python component objects (`Cpu`, `L1Cache`, `Memory`) are Python dataclasses that accumulate parameter values. When `sim.elaborate()` is called, each Python object is serialized to a `PendingObject` (a Rust type: component type name + `HashMap<String, AttrValue>`) and passed across the PyO3 boundary. `World::instantiate(pending_objects)` then materializes the Rust `SimObject` instances.

This means Python objects are short-lived configuration holders, not long-lived proxy objects. After `elaborate()`, the Python objects are no longer connected to the simulation.

### Param System

All configurable fields on Python component classes are typed via `Param.*` descriptors. See [`docs/design/helm-python/LLD-param-system.md`](./helm-python/LLD-param-system.md) for the full type list. Key principle: type-check at Python attribute-set time; range-check and conversion at `elaborate()` time on the Rust side.

---

## 10. Plugin System

Devices can be shipped as external `.so` (shared library) plugins. Each plugin bundles:

1. A Rust `SimObject + Device` implementation compiled to `crate-type = ["cdylib"]`.
2. A C-ABI entry point `helm_device_register()` that registers the device with `DeviceRegistry`.
3. An embedded Python class definition string (the Python-side param schema, no address or IRQ fields).

### Plugin Contract

```rust
// Every plugin exports this symbol
#[no_mangle]
pub extern "C" fn helm_device_register(registry: *mut DeviceRegistry) {
    let r = unsafe { &mut *registry };
    r.register(DeviceDescriptor {
        name:         "uart16550",
        version:      "1.0.0",
        description:  "16550-compatible UART",
        factory:      |params| Box::new(Uart16550::from_params(params)),
        param_schema: || ParamSchema::new()
                            .field("clock_hz", ParamType::Int, 1_843_200),
    });
}

// Embedded Python class (no base_addr, no irq — those are system-level)
pub static PYTHON_CLASS: &str = r#"
class Uart16550(Device):
    clock_hz: Param.Int = 1_843_200
"#;
```

### Plugin Loading

```python
helm_ng.load_plugin("./libhelm_uart16550.so")   # registers Uart16550 Python class
devices = helm_ng.list_devices()                 # ["uart16550", ...]
schema  = helm_ng.device_schema("uart16550")     # {"clock_hz": {"type": "int", "default": 1843200}}

uart = helm_ng.Uart16550(clock_hz=3_686_400)     # device-internal params only
```

### ABI Versioning

Plugin `.so` files embed a `HELM_DEVICE_ABI_VERSION` constant. `DeviceRegistry::load_plugin()` checks this against the current ABI version and rejects mismatches with a clear error message. Plugins must be recompiled when the `helm-devices` API changes.

### Conflict Resolution

If a plugin registers a device name that already exists in the registry, `load_plugin()` returns `Err(PluginError::NameConflict { name })`. The existing registration is preserved; the new one is rejected.

---

## 11. Phased Build Plan

### Phase 0 — MVP: Correct RISC-V SE Simulator (4–6 weeks)

**Goal:** Execute real RISC-V Linux binaries (statically linked) with correct output. No timing.

**Deliverables:**
- `helm-core`: `ArchState` (RV64GC register file + CSRs), `MemInterface` trait, flat `Vec<u8>` memory.
- `helm-arch/riscv`: Full RV64IMACFD decode + execute (match + bit ops, no DSL).
- `helm-engine`: `HelmEngine<Virtual>` with `ExecMode::Syscall`, Virtual timing (1 tick/insn).
- `helm-engine/se`: ~50 Linux syscalls (read, write, open, mmap, brk, exit, clone, wait4, getcwd).
- GDB RSP stub: read/write registers, read/write memory, step, continue, software breakpoints.
- Validation: RISC-V official test suite + riscv-tests; run `hello_world`, `ls`, statically-linked bash.

**Does NOT include:** Caches, event queue, device models, Python config, ARM, tracing.

### Phase 1 — Timing + Events + World (6–10 weeks)

**Goal:** Timing-accurate memory simulation + first device models + World.

**Deliverables:**
- `helm-event`: `EventQueue` (`BinaryHeap<Reverse<TimedEvent>>`).
- `helm-devices/src/bus/event_bus`: `HelmEventBus`, synchronous pub-sub.
- `helm-memory`: `MemoryRegion` tree, `FlatView`, `MmioHandler`, three access modes.
- `helm-timing`: `Interval` model (Sniper-style, `MicroarchProfile` JSON).
- `helm-devices`: `Device` trait, `InterruptPin`/`Wire`/`Sink`, `DeviceRegistry`, `.so` loader, UART16550.
- `helm-engine`: `World`, built-in interrupt sink, `VirtualClock`.
- `helm-stats`: `PerfCounter`, `PerfHistogram`, JSON dump.
- Validation: Cache miss rate vs. Cachegrind; UART TX/RX unit tests via World.

### Phase 2 — Python Config + AArch64 (8–12 weeks)

**Goal:** Gem5-style Python config layer + ARM AArch64 ISA.

**Deliverables:**
- `helm-python`: PyO3 bindings, `#[pymodule]`, `PySimulation`, `build_simulator()`.
- `helm_ng` Python package: `Simulation`, `Cpu`, `Cache`, `Memory`, `Board`, `Param.*` types.
- Python `World` bindings.
- `helm-arch/aarch64`: AArch64 decode (using `deku`) + execute, system register file.
- `helm-debug`: Trace logger (ring buffer, `serde`-serialized `TraceEvent`), trace export.
- Checkpoint/restore via `checkpoint_save()`/`checkpoint_restore()` on all SimObjects.
- Validation: AArch64 ISA test suite; Python integration tests; `Simulation.run()` GIL release verified.

### Phase 3 — Full System + Cycle-Accurate (Future)

**Goal:** Boot Linux. Cycle-accurate pipeline model.

**Deliverables:**
- `helm-devices`: PLIC, CLINT, VirtIO disk, VirtIO network.
- `helm-engine`: `ExecMode::System` — interrupt delivery, page table walker, MMU.
- `helm-timing`: `Accurate` — 5-stage in-order pipeline (OoO deferred).
- AArch64 Full System: boot Linux kernel on VirtIO disk.
- AArch32 / Thumb: register banking, CPSR mode tracking.

---

## 12. Key Design Principles

### No Dark State

Every piece of simulation state that affects correctness must be reachable from `World`'s ownership graph. If a device has internal state that is not registered via the attribute system (or serialized in `checkpoint_save()`), that state is "dark" — invisible to checkpointing, reset, and debugging. Dark state is a bug.

**Corollary:** Performance counters are explicitly excluded from the `checkpoint_save()` blob. They are not architectural state; they do not affect correctness; they never appear in diffs.

### Attribute System Owns Persistence

Architectural state (register file, device register banks, memory contents) is exposed via the attribute system — named, typed fields that can be read, written, and serialized. The checkpoint protocol reads from and writes to the attribute registry, not from arbitrary struct fields. This means any field that survives a checkpoint must be registered as an attribute.

### Device Knows No Addresses

A device has no knowledge of its base address in the system address space. The `Device` trait receives only byte offsets within its mapped region. The `MemoryMap` handles base address translation. This mirrors real hardware: a UART IP block has no `#define BASE_ADDR` in its RTL.

### Interrupt Routing is a Platform Concern

A device asserts or deasserts its `InterruptPin` output. It has no knowledge of IRQ numbers, interrupt controllers, or routing tables. The platform configuration (Python config, or `wire_interrupt()` in World) connects `InterruptPin` to an `InterruptSink`. The UART does not know it is wired to PLIC input 33; the PLIC does not know the device type that drives input 33.

### Monomorphize Only Timing

The timing model is the only axis that is generic over (monomorphized). `ISA` and `ExecMode` are enum-dispatched: one `match` per Python-boundary call, zero overhead per simulated instruction. Adding a new ISA does not require a new binary; adding a new timing model requires a new `HelmSim` variant and a new `HelmEngine<NewTiming>` type.

### Python Describes; Rust Simulates

All configuration — ISA selection, mode, component topology, parameter values, interrupt routing — lives in Python. After `sim.elaborate()`, Python has no influence on the simulation unless a `HelmEventBus` subscriber fires a Python callback. The Python interpreter is not required to be running during `sim.run()`.

### Determinism by Default

No wall-clock dependence. No background threads. No non-deterministic allocation. Given identical inputs (binary, params, ISA), two simulation runs produce identical outputs. This is a hard constraint, not a best-effort target. Fuzzing, regression testing, and checkpoint/restore all depend on it.

---

*This document is the authoritative top-level design reference for helm-ng. For crate-level detail, see the individual HLD and LLD documents under `docs/design/`.*

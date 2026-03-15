# HLD: helm-engine

> High-Level Design for the `helm-engine` crate — the simulation kernel.

**Status:** Draft
**Phase:** Phase 0 (MVP)
**Crate path:** `crates/helm-engine/`

---

## Table of Contents

1. [Crate Purpose](#1-crate-purpose)
2. [Scope Boundaries](#2-scope-boundaries)
3. [Type Hierarchy](#3-type-hierarchy)
4. [Temporal Decoupling Design](#4-temporal-decoupling-design)
5. [Design Questions Answered](#5-design-questions-answered)
6. [Dependency List](#6-dependency-list)
7. [Public API Surface](#7-public-api-surface)
8. [Architecture Diagram](#8-architecture-diagram)

---

## 1. Crate Purpose

`helm-engine` is the **simulation kernel**. It owns the instruction execution loop, owns the hart's architectural state and memory map, drives the `Scheduler` for temporal decoupling across multiple harts, and exposes the `HelmSim` PyO3 boundary enum to the Python configuration layer.

The crate answers one question: **given an ISA, an execution mode, and a timing model, execute instructions as fast as possible while keeping simulation state consistent.**

### Responsibilities

| Responsibility | Owner |
|---|---|
| Instruction dispatch loop (fetch → decode → execute) | `HelmEngine<T>` |
| Timing model integration (memory latency, branch penalty) | `HelmEngine<T>` via `T: TimingModel` |
| ISA selection at runtime | `Isa` enum dispatch inside `step_*()` |
| Execution mode selection (Functional / Syscall / System) | `ExecMode` enum, cold path |
| Syscall dispatch | `SyscallHandler` trait object, cold path |
| Exception and event notification | `HelmEventBus::fire()` |
| Hart scheduling and quantum management | `Scheduler` |
| PyO3 boundary | `HelmSim` enum + `build_simulator()` factory |
| Checkpoint save/restore of architectural state | `HelmEngine<T>` via `HelmAttr` |

---

## 2. Scope Boundaries

### What `helm-engine` Does NOT Do

- **ISA decode and execution logic.** Instruction decode and per-instruction semantic execution live in `helm-arch`. `helm-engine` calls into `helm-arch` via the `step_riscv()`, `step_aarch64()`, `step_aarch32()` methods, which are implemented in `helm-arch` and monomorphized into `HelmEngine<T>`.

- **Device modeling.** No MMIO handlers, interrupt controllers, or DMA engines live here. Those are `helm-devices` and are accessed only through `MemoryMap`.

- **Memory region management.** `MemoryMap`, `MemoryRegion`, and `FlatView` are owned by `helm-memory`. `helm-engine` holds a `MemoryMap` instance but does not implement address translation or the region tree.

- **Cache simulation.** Cache hit/miss modeling and eviction policy live in `helm-memory` (for `Interval` and `Accurate` timing models). `helm-engine` calls `timing.on_memory_access()` and the timing model handles cache state.

- **Timing model implementation.** `Virtual`, `Interval`, and `Accurate` timing model logic lives in `helm-timing`. `helm-engine` receives a `T: TimingModel` and calls its interface — it does not implement timing.

- **Discrete event queue.** The global event queue for future-timed events (`helm-event`) is not owned by `helm-engine`. The `Scheduler` drives the event queue boundary.

- **OS syscall implementation.** Linux syscall tables and host dispatch live in `helm-engine/se`. `helm-engine` calls `syscall_handler.handle(tc)` on a cold path.

- **GDB stub and trace logging.** `helm-debug` implements those. `helm-engine` fires `HelmEvent` events that the trace logger subscribes to.

- **Python-side param parsing.** `helm-python` owns all PyO3 bindings. `build_simulator()` in this crate receives already-parsed Rust types.

---

## 3. Type Hierarchy

```
TimingModel (trait, helm-timing)
  ├── Virtual
  ├── Interval { interval_ns: u64 }
  └── Accurate

Isa (enum, helm-engine)
  ├── RiscV
  ├── AArch64
  └── AArch32

ExecMode (enum, helm-engine)
  ├── Functional
  ├── Syscall
  └── System

HelmEngine<T: TimingModel>  ← simulation kernel, owns all hart state
  ├── isa: Isa
  ├── mode: ExecMode
  ├── timing: T              ← monomorphized, zero vtable
  ├── arch: ArchState        ← owned, not borrowed
  ├── memory: MemoryMap      ← owned, not borrowed
  ├── syscall_handler: Option<Box<dyn SyscallHandler>>
  ├── event_bus: Arc<HelmEventBus>
  ├── quantum_budget: u64
  └── insns_executed: u64

HelmSim (enum)             ← PyO3 boundary, one variant per timing model
  ├── Virtual(HelmEngine<Virtual>)
  ├── Interval(HelmEngine<Interval>)
  └── Accurate(HelmEngine<Accurate>)

Scheduler                  ← temporal decoupling, multi-hart coordination
  ├── harts: Vec<HelmSim>
  ├── quantum_size: u64
  └── current_tick: u64

build_simulator(isa, mode, timing) -> HelmSim   ← factory, sole creation path
```

### Key Rule: Monomorphize Timing Only

The generic parameter `T` in `HelmEngine<T>` is the **only** compile-time polymorphism in the hot loop. `Isa` and `ExecMode` are dispatched via enum match inside the loop body. This design was the result of the council debate between generic-over-everything and enum-only approaches:

- **Timing is monomorphized** because `T::on_memory_access()` is called on nearly every instruction. A vtable call there would add 3–5 ns per instruction — unacceptable at 100M+ insns/sec targets.
- **ISA and mode are enum-dispatched** because the branch predictor learns the constant value after a few iterations, making the branch effectively free. Adding ISA and mode as generic parameters would produce 9 monomorphized variants (3 ISA × 3 mode) without measurable benefit.

---

## 4. Temporal Decoupling Design

### Problem

In a multi-hart simulation (e.g., 4-core RISC-V), naive implementation would synchronize harts at every instruction. Synchronization requires acquiring locks or barriers, imposing overhead that makes multi-hart simulation slower than single-hart.

### Solution: Quantum-Based Execution

Each hart runs **independently for one quantum** (a fixed instruction budget, default 1000 instructions) before synchronization. This is called *temporal decoupling* — the harts' local clocks are allowed to diverge by up to one quantum, then resync at the quantum boundary.

```
Quantum loop (Scheduler):

  Round 1:
    hart0.run(quantum=1000)   → executes 1000 instructions, local tick advances
    hart1.run(quantum=1000)   → executes 1000 instructions, local tick advances
    hart2.run(quantum=1000)   → executes 1000 instructions, local tick advances
    hart3.run(quantum=1000)   → executes 1000 instructions, local tick advances
    synchronize()             → resolve shared memory events, fire pending IRQs

  Round 2: repeat
```

### Properties

- **No synchronization inside a quantum.** Harts do not communicate during their quantum. Shared memory accesses are buffered and reconciled at the quantum boundary.
- **Quantum size is configurable.** Default is 1000 instructions (calibrated to balance simulation speed and synchronization accuracy for SE mode). FS mode with devices may use smaller quanta.
- **Single-hart mode skips the Scheduler.** When only one hart exists, `HelmEngine::run()` is called directly without `Scheduler` overhead.
- **Temporal decoupling is a correctness approximation.** For SE mode (userspace only, no shared memory), decoupling is exact. For FS mode (shared memory, DMA), the approximation introduces at most one quantum of latency in observable side effects — within the accuracy bounds of the `Virtual` timing model.

### Breakpoint Interruption

A hart's quantum can be cut short by a `HelmEventBus` event. When the engine fires `HelmEvent::Breakpoint`, the engine sets an internal `stop_requested: bool` flag and exits the instruction loop before the quantum is exhausted. The `Scheduler` detects `StopReason::Breakpoint` and pauses all harts.

---

## 5. Design Questions Answered

### Q10: Does HelmEngine own ArchState and MemoryMap, or borrow them?

**Own.** `HelmEngine<T>` owns both `ArchState` and `MemoryMap` by value. Ownership is required for:

1. **Checkpoint scope.** `checkpoint_save()` must serialize the complete hart state. If `ArchState` were borrowed, the checkpoint boundary would be unclear — who saves and who restores?
2. **PyO3 safety.** `HelmSim` is a `#[pyclass]`. PyO3 requires the wrapped type to be `'static`. Borrowed references cannot be `'static` without unsafe lifetime extensions.
3. **Simplicity.** A single-hart simulator (Phase 0 target) does not need shared `MemoryMap` access from multiple concurrent owners.

For multi-hart FS mode (Phase 3), harts share a `MemoryMap` via `Arc<MemoryMap>`. The field type changes from `MemoryMap` to `Arc<MemoryMap>` at that point; the ownership model does not change for single-hart.

### Q11: Does HelmEngine implement SimObject?

**No.** `HelmEngine<T>` does not implement `SimObject`. It is the **driver** of the SimObject lifecycle, not a participant in it.

`HelmEngine<T>` owns the `System` tree during Phase 2+ (when device models are present). It calls `init()`, `elaborate()`, `startup()` on all registered `SimObject` instances. It is not itself checkpointed via the attribute system — its checkpoint is done directly via `HelmEngine::checkpoint_save()`, which serializes `ArchState` and delegates to `System::checkpoint_save_all()` for device state.

This matches the pattern documented in `object-model.md` (Section 7): `HelmEngine<T>` is the engine, not a component.

### Q12: How does HelmSim expose ArchState inspection without knowing T?

Via a `thread_context()` method on `HelmSim` that returns `&mut dyn ThreadContext`. `ThreadContext` is a trait in `helm-core` that provides cold-path access to the hart's architectural state (read/write registers, read/write PC, read/write CSRs). `HelmEngine<T>` implements `ThreadContext` independent of `T`.

```rust
impl HelmSim {
    pub fn thread_context(&mut self) -> &mut dyn ThreadContext {
        match self {
            Self::Virtual(k)   => k as &mut dyn ThreadContext,
            Self::Interval(k)  => k as &mut dyn ThreadContext,
            Self::Accurate(k)  => k as &mut dyn ThreadContext,
        }
    }
}
```

The Python call `sim.read_reg(0)` resolves to `HelmSim::thread_context().read_int_reg(0)`. One enum dispatch at the `HelmSim` level; then a `dyn ThreadContext` vtable call — acceptable because register inspection is a cold path (GDB, Python, not the inner loop).

### Q13: Is build_simulator the only creation path?

**Yes, for all Python-initiated simulations.** `build_simulator(isa, mode, timing) -> HelmSim` is the sole factory callable from Python. Python cannot directly construct `HelmEngine<Virtual>` or any other concrete variant.

In Rust unit tests, `HelmEngine::new(isa, mode, timing)` may be called directly for testing purposes. This is intentional — tests should not require PyO3 initialization.

### Q14: Who owns the Scheduler?

**The Scheduler is owned by the caller** — either `HelmSim` (when multi-hart) or the test harness. In the common single-hart case, no `Scheduler` exists; `HelmSim::run()` calls `HelmEngine::run()` directly.

In multi-hart configuration, `HelmSim` grows a `Scheduler` field:

```rust
pub enum HelmSim {
    Virtual(HelmEngine<Virtual>),             // single-hart
    VirtualMulti(Scheduler<Virtual>),         // multi-hart
    // ...
}
```

For Phase 0 (single-hart SE), this complexity is not present. The `Scheduler` is introduced in Phase 1 when multi-core support is added.

### Q15: How does HelmEventBus::fire(Exception) interrupt Execute::run()?

The engine checks a `stop_flag: AtomicBool` after every instruction. When `HelmEventBus::fire()` is called with an event that has a registered stop-on-fire subscriber (e.g., breakpoints, exceptions in GDB mode), the subscriber sets the `stop_flag`. The inner loop exits at the top of its next iteration.

The `AtomicBool` uses `Relaxed` ordering for the write (from the subscriber) and `Relaxed` for the read (in the loop). On x86_64 and AArch64, relaxed atomics compile to plain loads/stores — no fence, no overhead per instruction. The flag is only checked once per instruction, not per memory access.

Exceptions in the architectural sense (trap vectors, privilege transitions) are handled inline in `step_riscv()` / `step_aarch64()` without going through the event bus. The event bus is for observable notifications, not for control flow.

### Q16: Do harts share MemoryMap?

**In Phase 0 (single-hart SE): no sharing, each hart owns its MemoryMap.**

**In Phase 3 (multi-hart FS): shared via `Arc<MemoryMap>`.** All harts on the same physical machine share the same physical address space. For SE mode, each hart sees an independent virtual address space (the host process provides isolation), so separate maps are fine.

The field type in `HelmEngine<T>` is designed to accommodate both:

```rust
pub struct HelmEngine<T: TimingModel> {
    // Phase 0-2: MemoryMap
    // Phase 3:   Arc<MemoryMap>  (field rename or newtype wrapper)
    pub memory: MemoryMap,
    // ...
}
```

### Q17: Default quantum size?

**Default: 1000 instructions.** Rationale:

- At 100M insns/sec (realistic for Virtual mode), 1000 instructions = 10 µs of simulated wall time per quantum.
- Synchronization overhead is ~100 ns per quantum boundary (atomic fence + hart loop iteration). At 1000 insns/quantum, overhead is <0.1% of simulation time.
- 1000 instructions is short enough that device timer events (which fire every ~10K–100K cycles) are not delayed by more than one quantum.

The quantum size is configurable per `Scheduler`:

```rust
pub struct Scheduler<T: TimingModel> {
    quantum_size: u64,  // default: 1000
    // ...
}
```

---

## 6. Dependency List

```
helm-engine
  ├── helm-core       [ArchState, ExecContext, ThreadContext, MemInterface, Isa, ExecMode]
  ├── helm-timing     [TimingModel trait, Virtual, Interval, Accurate]
  ├── helm-memory     [MemoryMap, MemoryRegion, FlatView, MemFault]
  ├── helm-event      [EventQueue — used by Scheduler for time advancement]
  ├── helm-devices/bus   [HelmEventBus, HelmEvent, HelmEventKind]
  └── helm-arch       [step_riscv, step_aarch64, step_aarch32 — ISA execution]

helm-engine is depended upon by:
  ├── helm-python         [PyO3 bindings — wraps HelmSim as #[pyclass]]
  ├── helm-engine/se         [LinuxSyscallHandler implements SyscallHandler]
  └── helm-debug      [GdbServer, TraceLogger subscribe to HelmEventBus]
```

`helm-engine` does **not** depend on:
- `helm-devices` (devices are registered in `MemoryMap` at configuration time; engine calls memory, not devices)
- `helm-stats` (stats are registered externally; engine increments via `PerfCounter` shared refs)
- `helm-debug` (the debug layer subscribes to the engine's event bus; not the reverse)

---

## 7. Public API Surface

```rust
// ── Enums ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isa { RiscV, AArch64, AArch32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode { Functional, Syscall, System }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingChoice { Virtual, Interval { interval_ns: u64 }, Accurate }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    QuantumExhausted,
    Breakpoint { pc: u64 },
    Exception { vector: u32, pc: u64 },
    SimExit { code: i32 },
}

// ── Factory ───────────────────────────────────────────────────────────────────

pub fn build_simulator(isa: Isa, mode: ExecMode, timing: TimingChoice) -> HelmSim;

// ── HelmSim (PyO3 boundary enum) ─────────────────────────────────────────────

pub enum HelmSim {
    Virtual(HelmEngine<Virtual>),
    Interval(HelmEngine<Interval>),
    Accurate(HelmEngine<Accurate>),
}

impl HelmSim {
    pub fn run(&mut self, budget: u64) -> StopReason;
    pub fn step_once(&mut self) -> StopReason;
    pub fn thread_context(&mut self) -> &mut dyn ThreadContext;
    pub fn memory(&self) -> &MemoryMap;
    pub fn memory_mut(&mut self) -> &mut MemoryMap;
    pub fn checkpoint_save(&self) -> Vec<u8>;
    pub fn checkpoint_restore(&mut self, data: &[u8]);
    pub fn reset(&mut self);
    pub fn set_syscall_handler(&mut self, h: Box<dyn SyscallHandler>);
    pub fn set_event_bus(&mut self, bus: Arc<HelmEventBus>);
}

// ── HelmEngine<T> ─────────────────────────────────────────────────────────────

pub struct HelmEngine<T: TimingModel> { /* see LLD-helm-engine.md */ }

impl<T: TimingModel> HelmEngine<T> {
    pub fn new(isa: Isa, mode: ExecMode, timing: T) -> Self;
    pub fn run(&mut self, budget: u64) -> StopReason;
    pub fn step_once(&mut self) -> StopReason;
}

// ── Scheduler ────────────────────────────────────────────────────────────────

pub struct Scheduler { /* see LLD-scheduler.md */ }
```

---

## 8. Architecture Diagram

```
Python (helm_ng package)
  │
  │  build_simulator(isa, mode, timing)
  ▼
HelmSim enum  ←── PyO3 #[pyclass] wrapper (helm-python)
  │
  │  one match arm per Python call
  ▼
HelmEngine<T: TimingModel>
  │
  ├── inner loop: for _ in 0..budget { match isa { RiscV => step_riscv(), ... } }
  │                                     T::on_memory_access() ← inlined, zero vtable
  │
  ├── ArchState (owned)         ← int_regs, fp_regs, pc, csrs
  │
  ├── MemoryMap (owned)         ← FlatView, MemoryRegion tree (helm-memory)
  │
  ├── T: TimingModel            ← Virtual | Interval | Accurate (helm-timing)
  │                               monomorphized into HelmEngine, no vtable
  │
  ├── SyscallHandler (Box<dyn>) ← cold path: LinuxSyscallHandler (helm-engine/se)
  │
  └── HelmEventBus (Arc<>)      ← fire(Exception), fire(Breakpoint), fire(MagicInsn)
                                   subscribers: TraceLogger, GdbServer, Python callbacks

Scheduler (optional, multi-hart)
  ├── Vec<HelmSim>              ← one per hart
  ├── quantum_size: u64         ← default 1000
  └── round-robin quantum loop → StopReason per hart
```

---

*See [`LLD-helm-engine.md`](LLD-helm-engine.md) for HelmEngine struct fields and inner loop detail.*
*See [`LLD-helm-sim.md`](LLD-helm-sim.md) for HelmSim enum and build_simulator factory.*
*See [`LLD-scheduler.md`](LLD-scheduler.md) for Scheduler design and temporal decoupling.*
*See [`TEST.md`](TEST.md) for the test plan.*

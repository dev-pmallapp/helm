# helm-ng Object Model — Developer Reference

> Cross-references: [`traits.md`](./traits.md) · [`api.md`](./api.md)

---

## Table of Contents

1. [Overview](#1-overview)
2. [SimObject Trait](#2-simobject-trait)
3. [Lifecycle Phases](#3-lifecycle-phases)
4. [System Tree and Path Naming](#4-system-tree-and-path-naming)
5. [Component Wiring](#5-component-wiring)
6. [Checkpoint Protocol](#6-checkpoint-protocol)
7. [HelmSim and HelmEngine](#7-helmsim-and-helmengine)
8. [Implementing a New Component](#8-implementing-a-new-component)
9. [Common Mistakes](#9-common-mistakes)

---

## 1. Overview

### Purpose

The helm-ng object model defines how simulation components are described, composed, wired together, and driven through a simulation run. Every device, cache, memory controller, CPU, and bus that participates in a simulation is a **SimObject** — a Rust struct that implements the `SimObject` trait and registers itself in the `System` component tree.

The model is designed around three non-negotiable constraints:

- **No dynamic dispatch in the hot loop.** Component references acquired during `elaborate()` are stored as direct Rust references (or `Arc` pointers for shared ownership). Runtime name-based lookup is forbidden after startup.
- **Strict phase ordering.** Each lifecycle method has a defined window of validity. Calling across phase boundaries is a logic error and will panic in debug builds.
- **Python describes; Rust simulates.** Configuration — ISA selection, mode selection, component topology, parameter values — lives entirely in Python. The Rust engine consumes a fully resolved configuration and never re-reads Python after `build_simulator()` returns.

### How It Differs from gem5's SimObject

| Aspect | gem5 | helm-ng |
|---|---|---|
| Configuration language | Python (SimObject class hierarchy) | Python (dataclass / TOML config) |
| Component base | C++ `SimObject` with SWIG bindings | Rust `SimObject` trait with PyO3 boundary |
| Cross-component refs | `param` descriptors resolved at C++ elaborate time | Direct Rust references stored at `elaborate()` |
| Timing models | Built into every object | Separate `TimingModel` type parameter on `HelmEngine<T>` |
| Checkpoint format | gem5 checkpoint files (key-value text) | Binary blob per object (user-defined serialization) |
| ISA | Single ISA per binary | Runtime-selectable (`Isa` enum); multi-ISA in one binary |

### Two-Phase Model: Python Config → Rust Simulation

```
┌────────────────────────────────────────────────┐
│  PHASE 1 — Python Configuration                │
│                                                │
│  User writes config.py (or config.toml)        │
│  Specifies: ISA, ExecMode, component list,     │
│  parameters, memory map, device tree           │
│                                                │
│  build_simulator() called via PyO3             │
│  Returns: HelmSim enum variant           │
└───────────────────┬────────────────────────────┘
                    │  (PyO3 boundary — Python data
                    │   converted to Rust types here)
                    ▼
┌────────────────────────────────────────────────┐
│  PHASE 2 — Rust Simulation                     │
│                                                │
│  HelmEngine<T> owns System + all SimObjects     │
│  Lifecycle: init → elaborate → startup → run   │
│  No Python interpreter active during run()     │
└────────────────────────────────────────────────┘
```

Python is **not** present during the simulation run. After `build_simulator()` returns, all component parameters are immutable Rust values.

---

## 2. SimObject Trait

Full trait definition (see [`traits.md`](./traits.md) for the authoritative source):

```rust
pub trait SimObject: Send {
    fn name(&self) -> &str;
    fn init(&mut self);
    fn elaborate(&mut self, system: &mut System);
    fn startup(&mut self);
    fn reset(&mut self);
    fn checkpoint_save(&self) -> Vec<u8>;
    fn checkpoint_restore(&mut self, data: &[u8]);
}
```

The trait is `Send` because components may be moved across threads during construction (though not during the hot loop). It is **not** `Sync` — a component is owned by exactly one thread at a time.

---

### `fn name(&self) -> &str`

**Signature:** `fn name(&self) -> &str`

**When called:** At any time, including before `init()`. The name is the final path segment as registered with `System` (e.g., `"icache"`, not `"system.cpu0.icache"`). The full path is assembled by `System` at registration time.

**Must do:** Return a string slice with a lifetime tied to `&self`. The name must be stable — it must not change between calls and must match the string passed to `System::register()`.

**Must not do:** Allocate on every call. Store the name as a field; return `&self.name`.

```rust
pub struct L1Cache {
    name: String,
    // ...
}

impl SimObject for L1Cache {
    fn name(&self) -> &str {
        &self.name
    }
    // ...
}
```

---

### `fn init(&mut self)`

**Signature:** `fn init(&mut self)`

**When called:** After all `SimObject` instances have been constructed and registered with `System`, but before any cross-component wiring. The simulator calls `init()` on each component in registration order.

**Must do:**
- Initialize all internal state that does not depend on any other component.
- Reset all data structures to their default/empty state.
- Allocate resources owned exclusively by this component (e.g., internal buffers).

**Must not do:**
- Call methods on other `SimObject` instances. Other objects exist but are not yet wired; their own `init()` may not have run yet.
- Perform memory-mapped I/O registration. That happens in `elaborate()`.
- Access `System`'s component tree by path. Path resolution is not valid until `elaborate()`.

```rust
impl SimObject for L1Cache {
    fn init(&mut self) {
        // OK: initialize internal state only
        self.lines.iter_mut().for_each(|l| l.clear());
        self.stats.reset();
        // NOT OK: self.system.lookup("cpu0") — panics in debug
    }
}
```

---

### `fn elaborate(&mut self, system: &mut System)`

**Signature:** `fn elaborate(&mut self, system: &mut System)`

**When called:** After all components have completed `init()`. The simulator calls `elaborate()` on each component in registration order. This is the **only** phase where cross-component references may be acquired and stored.

**Must do:**
- Register any memory-mapped regions with `system.memory_map_mut()`.
- Resolve references to other components via `system.get::<T>("path")` and store them as direct references or `Arc` pointers.
- Register interrupt lines, port connections, and other cross-component bindings.
- Complete all wiring before returning.

**Must not do:**
- Retain a reference to `system` beyond the `elaborate()` call. Store component references, not the `System` handle.
- Perform work that belongs in `startup()` (e.g., sending initial events, loading firmware).

```rust
impl SimObject for L1Cache {
    fn elaborate(&mut self, system: &mut System) {
        // Acquire reference to the memory bus and store it
        self.membus = Some(system.get::<MemBus>("system.membus")
            .expect("membus not registered"));

        // Register our MMIO control region
        system.memory_map_mut().register(
            CACHE_CTRL_BASE,
            CACHE_CTRL_SIZE,
            Arc::clone(&self.mmio_handler),
        );
    }
}
```

---

### `fn startup(&mut self)`

**Signature:** `fn startup(&mut self)`

**When called:** After all components have completed `elaborate()`. Wiring is fully resolved and frozen. `startup()` is where a component performs its first actions in simulation time — scheduling initial events, asserting initial signal states, loading ROM content.

**Must do:**
- Schedule any initial events with the timing model.
- Assert initial pin/signal states.
- Perform one-time, run-time initialization that requires wiring to be complete.

**Must not do:**
- Modify cross-component wiring. The wiring graph is frozen after `elaborate()`.
- Call `system.get()`. Path resolution is over.

```rust
impl SimObject for Timer {
    fn startup(&mut self) {
        // Schedule our first tick
        self.timing.schedule(self.period_ns, TimerEvent::Tick);
    }
}
```

---

### `fn reset(&mut self)`

**Signature:** `fn reset(&mut self)`

**When called:** Between simulation runs (e.g., after a `RESET` signal, or when the harness re-runs the same configuration with different workloads). Wiring is still valid. `reset()` returns the component to its post-`startup()` state without rebuilding the object graph.

**Must do:**
- Reset all architectural state (registers, queues, state machines) to their power-on defaults.
- Reschedule any initial events if the timing model was also reset.
- Clear all buffers that represent in-flight microarchitectural state.

**Must not do:**
- Re-register memory regions or re-wire components. Wiring survives reset.
- Preserve any state from the previous run. `reset()` must be idempotent — calling it twice must yield the same result as calling it once.

```rust
impl SimObject for UartDevice {
    fn reset(&mut self) {
        self.tx_buffer.clear();
        self.rx_buffer.clear();
        self.status_reg = UartStatus::IDLE;
        self.pending_irq = false;
    }
}
```

---

### `fn checkpoint_save(&self) -> Vec<u8>`

**Signature:** `fn checkpoint_save(&self) -> Vec<u8>`

**When called:** When the harness requests a checkpoint snapshot. May be called at any point after `startup()`, including mid-run. The call does not pause simulation — the simulator quiesces the component before calling.

**Must do:**
- Serialize all **architectural state** that must survive a restore.
- Return a self-describing byte blob (include a version tag as the first byte or first 4 bytes).

**Must not do:**
- Serialize performance counters, statistics, or tracing data (breaks the differential checkpoint model — see [Section 6](#6-checkpoint-protocol)).
- Serialize direct pointers or OS-level handles. All references must be reconstructed from names at restore time.
- Allocate large intermediate structures. Serialize directly to a `Vec<u8>`.

---

### `fn checkpoint_restore(&mut self, data: &[u8])`

**Signature:** `fn checkpoint_restore(&mut self, data: &[u8])`

**When called:** When restoring from a saved checkpoint. Called after `elaborate()` (wiring is valid) but before `startup()` — `startup()` is skipped on restore because the checkpoint already contains post-startup state.

**Must do:**
- Deserialize the byte blob produced by `checkpoint_save()`.
- Check the version tag and return an `Err`-style panic with a clear message if the version is incompatible (see [Section 6](#6-checkpoint-protocol)).
- Fully replace all serialized fields with deserialized values.

**Must not do:**
- Silently ignore unknown fields. Unknown fields in a checkpoint indicate a version mismatch and must be treated as an error.

---

## 3. Lifecycle Phases

### Phase Diagram

```
  ┌───────────┐
  │ CONSTRUCT │  SimObject instances created, names set.
  │           │  No cross-component knowledge.
  └─────┬─────┘
        │ System::register() called for every component
        ▼
  ┌───────────┐
  │   INIT    │  init() called on each component.
  │           │  Internal state only. No cross-component refs.
  └─────┬─────┘
        │ All init() calls complete
        ▼
  ┌───────────┐
  │ ELABORATE │  elaborate(system) called on each component.
  │           │  Cross-component wiring happens here.
  │           │  MemoryMap regions registered here.
  └─────┬─────┘
        │ All elaborate() calls complete — wiring FROZEN
        ▼
  ┌───────────┐
  │  STARTUP  │  startup() called on each component.
  │           │  Initial events scheduled. Simulation clock starts.
  └─────┬─────┘
        │
        ▼
  ┌───────────┐
  │    RUN    │  Simulation executes. Hot loop active.
  │           │  No SimObject lifecycle calls during run.
  └─────┬─────┘
        │  (either normal exit or checkpoint request)
       / \
      /   \
     ▼     ▼
 ┌──────┐  ┌────────────────┐
 │ END  │  │ CHECKPOINT     │
 │      │  │ checkpoint_save│
 └──────┘  │ called on each │
            │ component      │
            └───────┬────────┘
                    │
          ┌─────────┴──────────────┐
          │  RESET (optional)      │
          │  reset() called.       │
          │  Return to post-startup│
          │  state. May re-run.    │
          └─────────┬──────────────┘
                    │
                    ▼
             ┌────────────┐
             │  RUN again │
             └────────────┘

  Restore path (from checkpoint):
  CONSTRUCT → INIT → ELABORATE → CHECKPOINT_RESTORE → RUN
  (startup() is skipped on restore)
```

### Phase State Table

| Phase | cross-component calls | system.get() | memory map | events | arch state |
|---|---|---|---|---|---|
| CONSTRUCT | forbidden | forbidden | forbidden | forbidden | uninitialized |
| INIT | forbidden | forbidden | forbidden | forbidden | self only |
| ELABORATE | allowed | allowed | register here | forbidden | self only |
| STARTUP | allowed (wired refs) | forbidden | frozen | schedule here | initialized |
| RUN | allowed (wired refs) | forbidden | frozen | allowed | live |
| CHECKPOINT | read-only self | forbidden | frozen | forbidden | live |
| RESET | allowed (wired refs) | forbidden | frozen | reschedule | reset to default |
| RESTORE | allowed (wired refs) | forbidden | frozen | forbidden | from blob |

### Out-of-Order Call Behavior

In **debug builds**, the simulator tracks the current phase in `System::phase: SimPhase`. Any SimObject method called outside its valid phase triggers:

```
thread 'main' panicked at 'SimObject::init() called in phase Elaborate: component system.cpu0.icache'
```

In **release builds**, phase tracking is elided for performance. Out-of-order calls produce undefined behavior.

---

## 4. System Tree and Path Naming

### Component Registration

`System` owns the component tree as a flat map from full path to `Box<dyn SimObject>`:

```rust
pub struct System {
    components: IndexMap<String, Box<dyn SimObject>>,
    // ... memory_map, timing, phase tracker
}

impl System {
    /// Register a component under a parent path.
    /// Full path = parent_path + "." + component.name()
    /// Root-level components use parent = "system".
    pub fn register(&mut self, parent: &str, component: Box<dyn SimObject>) {
        let full_path = format!("{}.{}", parent, component.name());
        self.components.insert(full_path, component);
    }

    /// Resolve a component by full path. Only valid during elaborate().
    pub fn get<T: SimObject + 'static>(&self, path: &str) -> Option<&T> {
        self.components.get(path)
            .and_then(|c| c.as_any().downcast_ref::<T>())
    }
}
```

### Path Conventions

- The root of the tree is always `system`.
- Every component name is a single lowercase identifier segment: `cpu0`, `icache`, `membus`.
- Hierarchy is expressed by the registration call, not the component's name field.
- Full paths use dot separators: `system.cpu0.icache`, `system.membus`, `system.dram`.

```rust
// During build_simulator():
let mut system = System::new();

// Register top-level components
system.register("system", Box::new(MemBus::new("membus")));
system.register("system", Box::new(Dram::new("dram")));

// Register CPU and its children
system.register("system", Box::new(Cpu::new("cpu0")));
system.register("system.cpu0", Box::new(L1Cache::new("icache")));
system.register("system.cpu0", Box::new(L1Cache::new("dcache")));
```

This produces the tree:

```
system
├── membus
├── dram
└── cpu0
    ├── icache
    └── dcache
```

### Path Resolution Rules

1. `system.get(path)` is **only callable during `elaborate()`**. The call panics (debug) or returns garbage (release) outside that phase.
2. Resolution is exact-match on the full path string. There is no wildcard or prefix matching.
3. The returned reference is valid only for the duration of the `elaborate()` call. To use it after `elaborate()`, store it as an `Arc<T>` or a raw pointer with a lifetime bound to `System`.
4. **Runtime lookup in the hot loop is forbidden.** Store all cross-component references during `elaborate()` and access them directly during `run()`.

### Storing References Safely

The recommended pattern for holding a reference to another component after `elaborate()`:

```rust
use std::sync::Arc;

pub struct L1Cache {
    name: String,
    membus: Option<Arc<MemBus>>,  // None until elaborate()
}

impl SimObject for L1Cache {
    fn elaborate(&mut self, system: &mut System) {
        let bus = system.get_arc::<MemBus>("system.membus")
            .expect("system.membus must be registered before system.cpu0.icache");
        self.membus = Some(bus);
    }
}

// During run(): direct call, no lookup
fn handle_miss(&mut self, addr: u64) {
    self.membus.as_ref().unwrap().request(addr, self.line_size);
}
```

If `Arc` is inappropriate (e.g., the target is exclusively owned), use a raw pointer with a `PhantomData` lifetime tied to `System`'s lifetime. See [`api.md`](./api.md) for `system.get_raw()`.

---

## 5. Component Wiring

### The Rule

> All cross-component connections are resolved in `elaborate()` and are frozen after `startup()`.

There are no exceptions to this rule. If a component needs a reference to another component, it stores that reference in `elaborate()`. It never acquires new references after `startup()` returns.

### Why Wiring Is Frozen After Startup

The simulation hot loop assumes that all pointer indirections through component references are unconditionally valid. Allowing dynamic re-wiring after startup would require either:

- Locking every cross-component call (unacceptable overhead), or
- A quiescence protocol (breaks determinism guarantees).

Neither is acceptable. The wiring graph is a compile-time-fixed structure by the time simulation begins.

### Wiring Patterns

**One-to-one (CPU → MemBus):** Store as `Arc<MemBus>` in `elaborate()`.

**One-to-many (MemBus → [L1Cache, L1Cache, DRAM]):** Store as `Vec<Arc<dyn MemPort>>` in `elaborate()`.

**Interrupt lines:** Components register interrupt callbacks as `Box<dyn Fn(IrqLevel)>` closures. The interrupt controller stores these closures after `elaborate()`. See [`api.md`](./api.md) for `IrqController::register_line()`.

**Bidirectional:** Both components hold `Arc<>` to each other. `Arc` cycles are intentional here; the entire graph is dropped at once when `System` is dropped.

### Wiring Validation

After all `elaborate()` calls complete, `System` calls `System::validate_wiring()`. This method:

- Verifies that no registered `MemoryMap` region overlaps with another.
- Verifies that every interrupt line has exactly one driver and at least one listener.
- Panics with a diagnostic if any wiring invariant is violated.

Components may also implement `fn validate(&self)` (optional extension, see [`traits.md`](./traits.md)) to perform component-local wiring checks.

---

## 6. Checkpoint Protocol

### What Must Be Serialized

A checkpoint captures **architectural state** — the state that software (the simulated workload) can observe. It does not capture microarchitectural state that is invisible to software.

**Serialize:**
- Architectural registers (`ArchState::int_regs`, `pc`, `csrs`)
- Memory contents (RAM, NVRAM)
- Device register state visible to software (UART status/data registers, timer counters)
- Interrupt pending state
- DMA descriptor state (software-visible)

**Do not serialize:**
- Performance counters (`stats.cache_hits`, `stats.branch_mispredicts`, etc.)
- Timing model internal state (`Virtual` tick counts, `Interval` interval counters)
- Prefetch queues, speculative state, branch predictor tables
- Internal pipeline stage latches

### Serialization Format

helm-ng does not mandate a serialization library. Components may use `serde` with `bincode`, or manual byte serialization. The only requirements are:

1. The blob must begin with a 4-byte version tag (`u32` little-endian). Increment on every breaking schema change.
2. The blob must be self-contained. No external references.
3. `checkpoint_restore()` must reject blobs with a different version tag with a clear panic message.

Example using manual serialization:

```rust
const UART_CKPT_VERSION: u32 = 1;

impl SimObject for UartDevice {
    fn checkpoint_save(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64);
        buf.extend_from_slice(&UART_CKPT_VERSION.to_le_bytes());
        // Serialize tx_buffer length + contents
        let tx_len = self.tx_buffer.len() as u32;
        buf.extend_from_slice(&tx_len.to_le_bytes());
        buf.extend(self.tx_buffer.iter().copied());
        // Serialize status register
        buf.extend_from_slice(&(self.status_reg as u32).to_le_bytes());
        buf
    }

    fn checkpoint_restore(&mut self, data: &[u8]) {
        let version = u32::from_le_bytes(data[0..4].try_into().unwrap());
        assert_eq!(
            version, UART_CKPT_VERSION,
            "UartDevice checkpoint version mismatch: got {}, expected {}",
            version, UART_CKPT_VERSION
        );
        let tx_len = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
        self.tx_buffer.clear();
        self.tx_buffer.extend(&data[8..8 + tx_len]);
        let status_raw = u32::from_le_bytes(data[8 + tx_len..12 + tx_len].try_into().unwrap());
        self.status_reg = UartStatus::try_from(status_raw).expect("invalid UartStatus in checkpoint");
    }
}
```

### Differential Checkpoint Model

helm-ng supports differential checkpoints: only the delta from the last full checkpoint is stored per subsequent snapshot. The checkpoint coordinator (owned by `HelmEngine`) manages delta tracking. Component implementations do not need to implement delta logic themselves — they always serialize their full state in `checkpoint_save()`. The coordinator computes the diff.

**Implication:** Performance counters must not appear in `checkpoint_save()` output. If they did, every simulation tick would produce a non-empty delta (counters always change), negating the compression benefit of differential checkpoints.

### Compatibility Rules

A checkpoint is compatible with a `HelmSim` configuration if and only if:

1. The `Isa` variant matches exactly (`RiscV` checkpoint cannot restore into `AArch64`).
2. The `ExecMode` variant matches exactly.
3. The component set is identical (same registration paths in the same order).
4. Each component's version tag is compatible (typically: same version; future policy TBD).

Attempting to restore an incompatible checkpoint panics with a compatibility report listing all mismatches.

---

## 7. HelmSim and HelmEngine

### The Distinction

`HelmEngine<T>` is the **simulation engine**. It drives the `SimObject` lifecycle, owns the `System` tree, and runs the instruction dispatch loop. It is not itself a `SimObject` — it does not implement the trait.

Devices (CPU, cache, UART, memory controller, DMA, timer) **do** implement `SimObject`. They are leaves in the component tree.

```
HelmSim
└── HelmEngine<T>          ← engine, NOT a SimObject
    ├── System             ← owns component tree
    │   ├── Cpu            ← implements SimObject
    │   ├── L1Cache        ← implements SimObject
    │   ├── MemBus         ← implements SimObject
    │   └── UartDevice     ← implements SimObject
    ├── ArchState          ← architectural register file
    ├── MemoryMap          ← address space layout
    └── T: TimingModel     ← Virtual | Interval | Accurate
```

### Timing Model Type Parameter

`HelmEngine<T: TimingModel>` selects the timing model at compile time. The three variants correspond to the `HelmSim` enum:

| Variant | `T` | Use case |
|---|---|---|
| `HelmSim::Virtual` | `Virtual` | Functional-only, no timing (fast) |
| `HelmSim::Interval` | `Interval` | Interval-based performance modeling |
| `HelmSim::Accurate` | `Accurate` | Cycle-accurate simulation (slow) |

Switching timing model requires constructing a new `HelmSim` — there is no runtime switching. This is intentional: the type system ensures timing-model-specific optimizations (e.g., eliding event scheduling in `Virtual`) are resolved at compile time with zero overhead.

### The PyO3 Boundary

Python calls `build_simulator()` via PyO3. This is the single crossing point between the Python configuration world and the Rust simulation world:

```rust
#[pyfunction]
pub fn build_simulator(config: &PyDict) -> PyResult<HelmSim> {
    let isa = parse_isa(config)?;
    let mode = parse_exec_mode(config)?;
    let timing = parse_timing(config)?;

    match timing {
        TimingVariant::Virtual => Ok(HelmSim::Virtual(
            build_kernel::<Virtual>(isa, mode, config)?
        )),
        TimingVariant::Interval => Ok(HelmSim::Interval(
            build_kernel::<Interval>(isa, mode, config)?
        )),
        TimingVariant::Accurate => Ok(HelmSim::Accurate(
            build_kernel::<Accurate>(isa, mode, config)?
        )),
    }
}
```

After `build_simulator()` returns, Python may call `HelmSim::run()` (exposed via PyO3) but cannot inspect or modify internal Rust state. The `HelmSim` is opaque to Python beyond its public PyO3-exposed methods (`run()`, `reset()`, `checkpoint_save()`, `checkpoint_restore()`).

### `build_kernel<T>()` Factory

`build_kernel<T>()` constructs the `HelmEngine<T>` by:

1. Creating all `SimObject` instances from config parameters.
2. Calling `System::register()` for each component.
3. Running the lifecycle: `init()` on all → `elaborate()` on all → `startup()` on all.
4. Returning the fully-initialized `HelmEngine<T>`.

See [`api.md`](./api.md) for the full `build_kernel` signature and the `ComponentFactory` registry that maps Python config type names to Rust constructors.

---

## 8. Implementing a New Component

This section walks through a complete `UartDevice` implementation.

### Component Goals

- Implements `SimObject`.
- Holds a `tx_buffer: VecDeque<u8>` for outgoing bytes.
- Implements `MmioHandler` for software-visible register access.
- Registers its MMIO region with `MemoryMap` during `elaborate()`.
- Saves and restores `tx_buffer` state in checkpoint operations.

### Full Implementation

```rust
use std::collections::VecDeque;
use std::sync::Arc;
use crate::sim::{SimObject, System, MmioHandler, MmioAccess, MemoryMap};

// ── MMIO register offsets ────────────────────────────────────────────────────
const UART_BASE: u64 = 0x1000_0000;
const UART_SIZE: u64 = 0x1000;

const REG_TX_DATA:   u64 = 0x00;  // W: write byte to tx_buffer
const REG_STATUS:    u64 = 0x04;  // R: bit 0 = TX empty, bit 1 = TX full
const REG_TX_FLUSH:  u64 = 0x08;  // W: any write flushes tx_buffer

const TX_CAPACITY: usize = 256;

// ── Status register bit flags ────────────────────────────────────────────────
const STATUS_TX_EMPTY: u32 = 1 << 0;
const STATUS_TX_FULL:  u32 = 1 << 1;

// ── Checkpoint version tag ───────────────────────────────────────────────────
const UART_CKPT_VERSION: u32 = 1;

// ── UartDevice struct ────────────────────────────────────────────────────────
pub struct UartDevice {
    name: String,
    tx_buffer: VecDeque<u8>,
}

impl UartDevice {
    pub fn new(name: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            tx_buffer: VecDeque::with_capacity(TX_CAPACITY),
        })
    }
}

// ── SimObject implementation ─────────────────────────────────────────────────
impl SimObject for UartDevice {
    fn name(&self) -> &str {
        &self.name
    }

    fn init(&mut self) {
        // Internal state only — no cross-component access.
        self.tx_buffer.clear();
    }

    fn elaborate(&mut self, system: &mut System) {
        // Register our MMIO region. The memory map will route reads/writes
        // in [UART_BASE, UART_BASE + UART_SIZE) to our MmioHandler impl.
        // Note: MmioHandler requires Arc<Self>; use Arc::new_cyclic or
        // split the handler into a separate type if needed.
        system.memory_map_mut().register_mmio(
            UART_BASE,
            UART_SIZE,
            // In real usage: Arc::clone(&self_arc) passed via constructor.
            // Shown here as a conceptual example.
            self as *mut _ as *mut dyn MmioHandler,
        );
        // No other cross-component dependencies for a basic UART.
    }

    fn startup(&mut self) {
        // Nothing to schedule — UART is purely reactive (driven by MMIO writes).
    }

    fn reset(&mut self) {
        // Clear all mutable state. Wiring (memory map registration) survives.
        self.tx_buffer.clear();
    }

    fn checkpoint_save(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + self.tx_buffer.len());

        // 4-byte version tag
        buf.extend_from_slice(&UART_CKPT_VERSION.to_le_bytes());

        // tx_buffer: 4-byte length followed by contents
        let len = self.tx_buffer.len() as u32;
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend(self.tx_buffer.iter().copied());

        buf
    }

    fn checkpoint_restore(&mut self, data: &[u8]) {
        assert!(
            data.len() >= 8,
            "UartDevice checkpoint blob too short: {} bytes", data.len()
        );

        let version = u32::from_le_bytes(data[0..4].try_into().unwrap());
        assert_eq!(
            version, UART_CKPT_VERSION,
            "UartDevice checkpoint version mismatch: blob={} current={}",
            version, UART_CKPT_VERSION
        );

        let len = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
        assert!(
            data.len() >= 8 + len,
            "UartDevice checkpoint blob truncated: need {} bytes, got {}",
            8 + len, data.len()
        );

        self.tx_buffer.clear();
        self.tx_buffer.extend(&data[8..8 + len]);
    }
}

// ── MmioHandler implementation ───────────────────────────────────────────────
impl MmioHandler for UartDevice {
    fn mmio_read(&self, offset: u64, _size: u8) -> u32 {
        match offset {
            REG_STATUS => {
                let mut status = 0u32;
                if self.tx_buffer.is_empty() {
                    status |= STATUS_TX_EMPTY;
                }
                if self.tx_buffer.len() >= TX_CAPACITY {
                    status |= STATUS_TX_FULL;
                }
                status
            }
            _ => 0,  // reads to write-only registers return 0
        }
    }

    fn mmio_write(&mut self, offset: u64, _size: u8, value: u32) {
        match offset {
            REG_TX_DATA => {
                if self.tx_buffer.len() < TX_CAPACITY {
                    self.tx_buffer.push_back(value as u8);
                }
                // Silently drop if full — matches hardware behavior.
            }
            REG_TX_FLUSH => {
                self.tx_buffer.clear();
            }
            _ => {}  // writes to read-only registers are ignored
        }
    }
}
```

### Registration in `build_kernel`

```rust
// In build_kernel() or your config-driven factory:
let uart = Box::new(UartDevice::new("uart0"));
system.register("system", uart);
```

After registration, the full path is `system.uart0`. During `elaborate()`, `system.memory_map_mut().register_mmio(...)` maps `[0x1000_0000, 0x1001_0000)` to the UART's `MmioHandler`.

---

## 9. Common Mistakes

### Calling Another Component from `init()`

**Wrong:**
```rust
fn init(&mut self) {
    // BUG: other components may not have called init() yet.
    // In debug builds this panics. In release builds: UB.
    let bus = system.get::<MemBus>("system.membus").unwrap();
    self.membus = Some(bus);
}
```

**Correct:** Move cross-component access to `elaborate()`. `init()` is for self-contained initialization only.

---

### Dynamic Lookup at Runtime

**Wrong:**
```rust
fn handle_miss(&mut self, addr: u64) {
    // BUG: system is not available here, and even if it were,
    // this is O(log n) inside the hot loop.
    let bus = GLOBAL_SYSTEM.get::<MemBus>("system.membus").unwrap();
    bus.request(addr, self.line_size);
}
```

**Correct:** Store `Arc<MemBus>` in `self.membus` during `elaborate()`. Call `self.membus.as_ref().unwrap().request(...)` in the hot loop.

---

### Forgetting to Register with System in `elaborate()`

**Wrong:**
```rust
fn elaborate(&mut self, _system: &mut System) {
    // BUG: MMIO region never registered.
    // Software writes to UART_BASE will hit unmapped address fault.
}
```

**Correct:** Always call `system.memory_map_mut().register_mmio(...)` (or equivalent) for every MMIO-visible device in `elaborate()`.

---

### Including Performance Counters in Checkpoint

**Wrong:**
```rust
fn checkpoint_save(&self) -> Vec<u8> {
    let mut buf = vec![];
    buf.extend_from_slice(&self.tx_buffer.len().to_le_bytes());
    // BUG: stats included — every tick changes these, breaking delta compression
    buf.extend_from_slice(&self.stats.total_bytes_sent.to_le_bytes());
    buf.extend_from_slice(&self.stats.flush_count.to_le_bytes());
    buf
}
```

**Correct:** Serialize only architectural state. Performance counters live outside the checkpoint blob and are reset independently.

---

### Not Implementing `reset()` Properly

**Wrong:**
```rust
fn reset(&mut self) {
    // BUG: tx_buffer not cleared. Bytes from the previous run
    // are visible to the next run's software. Non-deterministic behavior.
}
```

**Correct:** Every mutable field that represents architectural state must be reset to its power-on default. `reset()` must be idempotent: calling it N times must yield the same state as calling it once.

---

### Storing References Before `elaborate()`

**Wrong:**
```rust
impl SimObject for L1Cache {
    fn init(&mut self) {
        // BUG: system is not passed to init(). This won't even compile.
        // Even if it did, other components may not exist yet.
        self.membus = system.get::<MemBus>("system.membus");
    }
}
```

`init()` does not receive a `&mut System` parameter. The signature enforces the phase rule at the type level. If you find yourself wanting to call `system.get()` in `init()`, the work belongs in `elaborate()`.

---

*End of object-model.md — see [`traits.md`](./traits.md) for full trait signatures and [`api.md`](./api.md) for the public API surface.*

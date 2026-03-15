# helm-python — High-Level Design

> PyO3 bindings and Python DSL for the helm-ng simulator.
> Cross-references: [`docs/design/HLD.md`](../HLD.md) · [`LLD-sim-objects.md`](./LLD-sim-objects.md) · [`LLD-param-system.md`](./LLD-param-system.md) · [`LLD-factory.md`](./LLD-factory.md) · [`TEST.md`](./TEST.md)

---

## Table of Contents

1. [Purpose and Scope](#1-purpose-and-scope)
2. [Two-Layer Architecture](#2-two-layer-architecture)
3. [Package Structure](#3-package-structure)
4. [PendingObject Protocol](#4-pendingobject-protocol)
5. [Error Propagation Strategy](#5-error-propagation-strategy)
6. [GIL Management](#6-gil-management)
7. [Key Design Decisions](#7-key-design-decisions)

---

## 1. Purpose and Scope

`helm-python` is the crate that exposes helm-ng's Rust simulation engine to Python. It serves two audiences:

- **End users** who write platform configuration scripts in Python (the `helm_ng` DSL).
- **Tool authors** who integrate helm-ng into larger Python workflows (event callbacks, statistics collection, checkpoint management).

The crate does two things:

1. Compiles the `helm_ng` Python extension module (a `.so` / `.pyd` file) via PyO3.
2. Ships a pure-Python package (`helm_ng/`) that wraps the raw extension with an ergonomic DSL.

`helm-python` does not implement any simulation logic. It is a pure translation layer between Python and the Rust crates (`helm-engine`, `helm-engine`, `helm-devices`, `helm-devices/src/bus/event_bus`, `helm-stats`).

---

## 2. Two-Layer Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  User Python Script                                             │
│                                                                 │
│  from helm_ng import Simulation, Cpu, L1Cache, Memory           │
│  cpu = Cpu(isa=Isa.RiscV, mode=ExecMode.Syscall)               │
│  sim = Simulation(root=cpu)                                     │
│  sim.elaborate()                                                │
│  sim.run(n_instructions=1_000_000)                              │
└─────────────────────────┬───────────────────────────────────────┘
                          │  imports
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│  Layer 2 — Python DSL  (helm_ng/ Python package)                │
│                                                                 │
│  helm_ng/__init__.py     — public API re-exports                │
│  helm_ng/components.py   — Simulation, Board, Cpu, Cache,       │
│                            Memory, World classes          │
│  helm_ng/params.py       — Param.MemorySize, Param.Int, etc.   │
│  helm_ng/enums.py        — Isa, ExecMode, Timing Python enums  │
│  helm_ng/exceptions.py   — HelmMemFault, HelmConfigError, etc. │
└─────────────────────────┬───────────────────────────────────────┘
                          │  calls into
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│  Layer 1 — Raw PyO3 Bindings  (_helm_ng extension module)       │
│                                                                 │
│  #[pyclass] PySimulation    — wraps HelmSim + World            │
│  #[pyclass] PyWorld   — wraps World                 │
│  #[pyclass] PyEventBus      — wraps Arc<HelmEventBus>           │
│  #[pyfunction] build_simulator(isa, mode, timing) -> HelmSim    │
│  #[pyfunction] load_plugin(path: str)                           │
│  #[pyfunction] list_devices() -> list[str]                      │
│  #[pyfunction] device_schema(name: str) -> dict                 │
└─────────────────────────┬───────────────────────────────────────┘
                          │  Rust FFI
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│  Rust Simulation Core                                           │
│  helm-engine / helm-engine / helm-devices / helm-devices/bus │
└─────────────────────────────────────────────────────────────────┘
```

**Layer 1 (raw bindings)** is minimal — thin wrappers that convert PyO3 types to Rust types and propagate errors. It exposes `_helm_ng` as a private internal extension module. Users should not import from it directly.

**Layer 2 (Python DSL)** provides the `Simulation`, `Cpu`, `L1Cache`, `L2Cache`, `Memory`, `Board`, and `World` classes that users interact with. It builds `PendingObject` lists from user configuration and calls into Layer 1 at `elaborate()` time.

This separation has two advantages: the Python DSL can be evolved, renamed, and extended without touching Rust code; and the raw extension module can be used by advanced users who need fine-grained control.

---

## 3. Package Structure

```
crates/helm-python/
├── Cargo.toml                    # [lib] crate-type = ["cdylib"]
│                                 # dependencies: pyo3, helm-engine, helm-engine, ...
└── src/
    ├── lib.rs                    # #[pymodule] fn _helm_ng(py, m) — registers all pyclass/pyfn
    ├── simulation.rs             # #[pyclass] PySimulation
    ├── world.rs           # #[pyclass] PyWorld
    ├── event_bus.rs              # #[pyclass] PyEventBus, #[pyclass] PyEventHandle
    ├── params.rs                 # AttrValue <-> PyObject conversions, PendingObject builder
    ├── factory.rs                # #[pyfunction] build_simulator, load_plugin, list_devices
    └── errors.rs                 # Rust HelmError -> Python exception class mapping

python/
└── helm_ng/
    ├── __init__.py               # Public API: Simulation, Cpu, L1Cache, L2Cache, Memory,
    │                             # Board, World, Isa, ExecMode, Timing, Param
    ├── components.py             # Simulation, Board, Cpu, L1Cache, L2Cache, Memory, World
    ├── params.py                 # Param.MemorySize, Param.Int, Param.Hz, Param.Cycles, etc.
    ├── enums.py                  # Isa, ExecMode, Timing — Python enum wrappers
    └── exceptions.py             # HelmError, HelmMemFault, HelmConfigError, HelmDeviceError
```

The compiled extension (`_helm_ng.so`) is placed alongside the `helm_ng/` package directory at install time (via `maturin` or manual `cargo build`).

---

## 4. PendingObject Protocol

Python component objects are configuration holders, not live simulation proxies. After `sim.elaborate()`, the Python objects are disconnected from the Rust simulation.

### Python side

Every Python component class (e.g. `Cpu`, `L1Cache`) is a dataclass-style object with typed `Param.*` fields. At attribute-set time, each field's `Param.*` descriptor validates the value type (e.g. `Param.MemorySize` rejects a negative integer). Values are stored as plain Python objects in an internal dict.

### Conversion at elaborate()

When `sim.elaborate()` is called, the Python `Simulation` object:

1. Walks the component tree (breadth-first, `root` first).
2. For each component, calls `component._to_pending()` which returns a `(type_name: str, params: dict[str, Any])` tuple.
3. Passes the list of tuples across the PyO3 boundary to `PySimulation::elaborate()`.

### Rust side

`PySimulation::elaborate()` receives `Vec<(String, HashMap<String, PyObject>)>`. For each tuple:

1. Converts `PyObject` values to `AttrValue` (the Rust enum: `AttrValue::Int(i64)`, `AttrValue::Str(String)`, `AttrValue::Bool(bool)`, `AttrValue::Bytes(Vec<u8>)`).
2. Looks up the component factory in `DeviceRegistry` by type name.
3. Calls `factory(params)` to produce `Box<dyn SimObject>`.
4. Registers the component with `World`.

After all components are registered, `World::elaborate()` drives the full lifecycle (`init → elaborate → startup`).

### Wiring

Port connections (e.g. `cpu.icache = icache`) are represented as special `PendingConnection` entries: `(from_path, port_name, to_path)`. These are resolved in the Rust elaborate pass after all objects exist.

---

## 5. Error Propagation Strategy

Rust `Result<T, E>` errors are mapped to Python exception classes at the PyO3 boundary in `src/errors.rs`. The mapping is one-to-one: each Rust error enum variant becomes a Python exception class.

### Exception Hierarchy

```
BaseException
└── Exception
    └── HelmError                (base for all helm-ng errors)
        ├── HelmConfigError      (invalid parameter, wiring mismatch, unknown component)
        ├── HelmMemFault         (memory access fault: access violation, bus error, alignment)
        │     attributes: addr: int, fault_kind: str, pc: int
        ├── HelmDeviceError      (device MMIO error, device returned error response)
        │     attributes: device_name: str, offset: int
        └── HelmCheckpointError  (version mismatch, truncated blob, incompatible config)
```

### Mapping Table

| Rust error | Python exception | When raised |
|---|---|---|
| `HelmError::Config(ConfigError::UnknownComponent)` | `HelmConfigError` | Unknown type name at elaborate |
| `HelmError::Config(ConfigError::ParamRange)` | `HelmConfigError` | Out-of-range param at elaborate |
| `HelmError::Config(ConfigError::WiringConflict)` | `HelmConfigError` | MMIO overlap or dangling IRQ |
| `HelmError::MemFault(MemFault { addr, kind })` | `HelmMemFault` | Load/store to unmapped address |
| `HelmError::Device(DeviceError { name, .. })` | `HelmDeviceError` | Device returns error from read/write |
| `HelmError::Checkpoint(CheckpointError::Version)` | `HelmCheckpointError` | Version tag mismatch on restore |

### Rule

No Rust `panic!` reaches Python. All `Result::Err` paths at PyO3 boundaries are converted to `PyErr` via `impl From<HelmError> for PyErr`. The simulation may still panic internally (e.g., on `debug_assert!` violations) — those are bugs, not recoverable errors.

---

## 6. GIL Management

The Python GIL (Global Interpreter Lock) must be managed carefully at the PyO3 boundary. Two rules govern this:

### Rule 1: sim.run() releases the GIL

`PySimulation::run()` calls `py.allow_threads(|| helm_sim.run(n))`. This releases the GIL for the duration of the Rust simulation loop. Python threads can run other work (e.g., a progress reporter, a log consumer) while the simulation executes.

The GIL is re-acquired on return from `allow_threads`, before any Python object is accessed.

**Consequence:** The Rust simulation loop must not hold any Python objects, call Python code, or access `PyObject` values while the GIL is released. All Python-side data must be converted to owned Rust types before entering `allow_threads`.

### Rule 2: HelmEventBus Python callbacks re-acquire the GIL

When a Python function is registered as a `HelmEventBus` subscriber (e.g. `sim.event_bus.subscribe("Exception", callback)`), the callback is stored as a `PyObject` (a Python callable). When the event fires during `sim.run()` (while the GIL is released), the Rust side must re-acquire the GIL before calling the Python callable.

This is implemented via `Python::with_gil(|py| callback.call1(py, (event_obj,)))` inside the subscriber closure.

**Performance note:** Every `HelmEventBus` callback re-acquire/release of the GIL is expensive. Subscribe sparingly. If a subscriber processes events at high frequency (e.g., every instruction), consider a Rust-side subscriber that buffers events and a Python-side pull API instead.

### Rule 3: until_callback in sim.run()

`sim.run(until=callback)` accepts a Python callable that is called after each `HelmEvent` to decide whether to stop. This callback is also called with GIL re-acquisition on each event. The callback must be fast; it runs on the simulation hot path.

---

## 7. Key Design Decisions

### Q94 — High-Level Python DSL (not raw HelmObject API)

**Decision:** Expose a Gem5-style Python DSL (`Cpu`, `Cache`, `Memory`, `Board`) rather than a raw `HelmObject`/`World` API.

**Rationale:** The target user is a researcher writing a platform configuration, not a simulator internals developer. A high-level DSL requires zero knowledge of Rust types, trait objects, or PyO3 internals. The raw binding layer (`_helm_ng`) remains available for advanced users.

### Q95 — Custom Exception Classes

**Decision:** Rust errors propagate as custom Python exception classes (`HelmMemFault`, `HelmConfigError`, `HelmDeviceError`) rather than generic `RuntimeError`.

**Rationale:** Users writing `try/except` blocks need to distinguish a memory fault (recoverable if caught, perhaps for fault injection) from a config error (always fatal). Generic `RuntimeError` prevents this. Custom classes also carry structured attributes (`.addr`, `.pc`) that are useless on a string message.

### Q96 — sim.run() Releases GIL

**Decision:** `sim.run()` releases the Python GIL via `py.allow_threads()`.

**Rationale:** A long simulation run (billions of instructions) must not block other Python threads. A GUI, progress reporter, or data pipeline should be able to run concurrently. GIL release is safe because the Rust simulation loop accesses no Python objects after the release.

### Q97 — until Condition is a Python Callback

**Decision:** `sim.run(until=callback)` accepts a Python callable invoked with each `HelmEvent`. The callback returns `True` to stop, `False` to continue.

**Rationale:** "Run until ROI start" is the canonical use case. The ROI start is a `MagicInsn` event. The callback pattern is flexible: it can match on any event field, count events, or implement complex stopping logic without requiring a new API for every use case.

### Q98 — Param Validation Split

**Decision:** Type-check at Python attribute-set time (Param descriptor `__set__`); range-check and unit conversion at `elaborate()` time (Rust side).

**Rationale:** Type errors (passing a string to `Param.Int`) should fail fast with a readable Python traceback. Range errors (cache size of 0 bytes) require the full Rust parameter context and are caught at elaborate time with a `HelmConfigError`.

---

*For implementation detail on the `#[pyclass]` sim objects, see [`LLD-sim-objects.md`](./LLD-sim-objects.md). For the Param type system, see [`LLD-param-system.md`](./LLD-param-system.md). For the factory and plugin loader, see [`LLD-factory.md`](./LLD-factory.md). For tests, see [`TEST.md`](./TEST.md).*

# helm-python — High-Level Design

> PyO3 bindings exposing a first-class SimObject hierarchy to Python.
> Cross-references: [`PYTHON-API-REDESIGN.md`](../PYTHON-API-REDESIGN.md) · [`LLD-sim-objects.md`](./LLD-sim-objects.md) · [`LLD-param-system.md`](./LLD-param-system.md) · [`LLD-instantiate.md`](./LLD-instantiate.md) · [`TEST.md`](./TEST.md)

---

## Table of Contents

1. [Purpose and Scope](#1-purpose-and-scope)
2. [Two-Layer Architecture](#2-two-layer-architecture)
3. [Package Structure](#3-package-structure)
4. [SimObject Hierarchy](#4-simobject-hierarchy)
5. [Two-Phase Construction](#5-two-phase-construction)
6. [Error Propagation Strategy](#6-error-propagation-strategy)
7. [GIL Management](#7-gil-management)
8. [Key Design Decisions](#8-key-design-decisions)

---

## 1. Purpose and Scope

`helm-python` exposes helm-ng's Rust simulation engine to Python via a **first-class SimObject hierarchy** inspired by gem5 and Simics. Every major Rust type — CPU, memory, GIC, UART, memory space — has a 1:1 Python counterpart backed by a `#[pyclass]`.

The crate serves two audiences:

- **End users** who compose platforms in Python by creating, parameterizing, and wiring SimObjects.
- **Tool authors** who integrate helm-ng into larger Python workflows (instrumentation, statistics, checkpoint management).

The crate does two things:

1. Compiles the `_helm_ng` Python extension module (`.so` / `.pyd`) via PyO3, containing all `#[pyclass]` SimObject types.
2. Ships a pure-Python package (`python/helm/`) with pre-built boards and convenience wrappers.

`helm-python` does not implement simulation logic. It is a translation layer between Python and the Rust crates (`helm-engine`, `helm-arch`, `helm-hw-*`).

---

## 2. Two-Layer Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│  User Python Script                                              │
│                                                                  │
│  import helm                                                     │
│  from helm.boards import ArmVirt                                 │
│                                                                  │
│  system = ArmVirt(mem="1GiB", cpu_model="cortex-a55")           │
│  system.instantiate()                                            │
│  system.load_kernel("Image", dtb="virt.dtb")                    │
│  system.run(10_000_000)                                         │
│  print(f"PC = {system.cpu.pc:#x}")                              │
└─────────────────────────┬────────────────────────────────────────┘
                          │  imports
                          ▼
┌──────────────────────────────────────────────────────────────────┐
│  Layer 2 — Pure Python  (python/helm/)                           │
│                                                                  │
│  helm/__init__.py          — re-exports all SimObject classes     │
│  helm/boards/arm_virt.py   — ArmVirt pre-built board             │
│  helm/boards/__init__.py   — board registry                      │
└─────────────────────────┬────────────────────────────────────────┘
                          │  backed by
                          ▼
┌──────────────────────────────────────────────────────────────────┐
│  Layer 1 — PyO3 SimObject Classes  (_helm_ng extension module)   │
│                                                                  │
│  #[pyclass(subclass)]     SimObject       — base class           │
│  #[pyclass(extends=...)]  System          — wraps HelmSim        │
│  #[pyclass(extends=...)]  Cpu             — wraps ArchState      │
│  #[pyclass(extends=...)]  Ram             — wraps FlatMem        │
│  #[pyclass(extends=...)]  MemorySpace     — wraps SystemMem      │
│  #[pyclass(extends=...)]  GicV2           — wraps GicDistributor │
│  #[pyclass(extends=...)]  Pl011           — wraps Pl011          │
│  #[pyclass(extends=...)]  Sp804           — wraps Sp804Timer     │
│  #[pyclass(extends=...)]  Cache           — descriptor only      │
│  #[pyclass]               PortRef         — connection descriptor │
│  #[pyclass]               SpySession      — standalone observer   │
│  #[pyfunction]            build_simulation — backward-compat      │
└─────────────────────────┬────────────────────────────────────────┘
                          │  Rust FFI
                          ▼
┌──────────────────────────────────────────────────────────────────┐
│  Rust Simulation Core                                            │
│  helm-engine / helm-arch / helm-hw-intc / helm-hw-char           │
└──────────────────────────────────────────────────────────────────┘
```

**Layer 1 (PyO3 SimObject classes)** — each `#[pyclass]` maps 1:1 to a Rust type. Before `instantiate()`, objects are configuration descriptors. After `instantiate()`, they become live handles to Rust objects with property getters/setters that read/write Rust state directly.

**Layer 2 (Pure Python)** — pre-built board classes (e.g. `ArmVirt`) that compose Layer 1 objects. These replace hardcoded Rust platform wiring. Users can subclass or write new boards without touching Rust.

---

## 3. Package Structure

```
runtime/helm-python/
├── Cargo.toml                    # [lib] crate-type = ["cdylib"]
└── src/
    ├── lib.rs                    # #[pymodule] fn _helm_ng — registers all classes
    ├── simobject.rs              # #[pyclass(subclass)] SimObject base
    ├── system.rs                 # #[pyclass(extends=SimObject)] System
    ├── cpu.rs                    # #[pyclass(extends=SimObject)] Cpu
    ├── ram.rs                    # #[pyclass(extends=SimObject)] Ram
    ├── memory_space.rs           # #[pyclass(extends=SimObject)] MemorySpace
    ├── devices/
    │   ├── mod.rs                # device module root
    │   ├── gicv2.rs              # #[pyclass(extends=SimObject)] GicV2
    │   ├── pl011.rs              # #[pyclass(extends=SimObject)] Pl011
    │   └── sp804.rs              # #[pyclass(extends=SimObject)] Sp804
    ├── cache.rs                  # #[pyclass(extends=SimObject)] Cache (descriptor only)
    ├── port.rs                   # #[pyclass] PortRef, MapEntry
    ├── instantiate.rs            # system.instantiate() — builds Rust objects from descriptors
    ├── spy.rs                    # #[pyclass] SpySession (standalone observer)
    ├── compat.rs                 # build_simulation() backward-compat wrapper
    └── errors.rs                 # Rust HelmError → Python exception mapping

python/
└── helm/
    ├── __init__.py               # ISA-neutral: System, Cpu, Ram, MemorySpace, Cache, SpySession
    ├── aarch64/
    │   ├── __init__.py           # re-exports: A55Cpu, A73Cpu, GicV2, Pl011
    │   ├── cpu.py                # AArch64 CPU models
    │   ├── devices.py            # AArch64 devices: GicV2, Pl011, Sp804
    │   └── boards/
    │       ├── __init__.py
    │       └── arm_virt.py       # ArmVirt pre-built board
    └── riscv/                    # (future)
```

---

## 4. SimObject Hierarchy

Every simulatable component inherits from `SimObject`. The hierarchy is flat on `System` — components are assigned as named children:

```
System("virt")
├── .cpu     = Cpu("cpu0")
├── .gic     = GicV2("gic0")
├── .uart    = Pl011("uart0")
├── .ram     = Ram("ram0")
└── .mem     = MemorySpace("phys_mem")
                ├── MapEntry(0x4000_0000, ram,  "1GiB")
                ├── MapEntry(0x0800_0000, gic,  0x1_0000, bank=0)
                ├── MapEntry(0x0801_0000, gic,  0x1_0000, bank=1)
                └── MapEntry(0x0900_0000, uart, 0x1000)
```

### Python-to-Rust Type Mapping

| Python Class | Rust Backing Type | Crate |
|---|---|---|
| `SimObject` | (base, no Rust type) | helm-python |
| `System` | `HelmSim` (enum over `HelmEngine<T>`) | helm-engine |
| `Cpu` | `Aarch64ArchState` / `RiscvArchState` | helm-arch |
| `Ram` | `FlatMem` | helm-engine |
| `MemorySpace` | `SystemMem` (FlatMem + AddressMap + devices) | helm-engine |
| `GicV2` | `GicDistributor` + `GicCpuInterface` + `GicState` | helm-hw-intc |
| `Pl011` | `Pl011` | helm-hw-char |
| `Sp804` | `Sp804Timer` | helm-hw-timer |
| `Cache` | (descriptor only — consumed by timing model) | helm-python |
| `Board` | (pure Python composition) | python/helm/ |

**Note:** `SpySession` is NOT a SimObject -- it is a standalone observer that attaches to a System's probes. It does not appear in the child hierarchy.

---

## 5. Two-Phase Construction

Inspired by gem5's `m5.instantiate()` and Simics's `pre_conf_object` → `SIM_add_configuration()`.

### Phase 1: Describe (Python)

Python objects are configuration descriptors. No Rust simulation objects exist yet. Port wiring and memory map entries are stored as descriptors.

```python
system = helm.System("virt", timing="virtual", mode="fs")
system.cpu  = helm.Cpu("cpu0", isa="aarch64", model="cortex-a55")
system.gic  = helm.GicV2("gic0", num_irqs=96)
system.uart = helm.Pl011("uart0")
system.ram  = helm.Ram("ram0", size="1GiB")
system.mem  = helm.MemorySpace("phys_mem")
system.mem.add_map(0x4000_0000, system.ram,  "1GiB")
system.mem.add_map(0x0800_0000, system.gic,  0x1_0000, bank=0)
system.mem.add_map(0x0801_0000, system.gic,  0x1_0000, bank=1)
system.mem.add_map(0x0900_0000, system.uart,  0x1000)
system.uart.irq = system.gic.spi(33)
```

### Phase 2: Instantiate (Rust)

`system.instantiate()` freezes configuration and creates all Rust objects:

1. Create `HelmEngine<T>` based on `timing` parameter
2. Create `FlatMem` for each `Ram`
3. Create `AddressMap` from `MemorySpace` map entries
4. Create device instances (`Pl011`, `GicDistributor`, etc.)
5. Build `SystemMem` from `FlatMem` + `AddressMap` + devices
6. Resolve `PortRef`s → wire `Arc<GicState>` into device interrupt outputs
7. Run SimObject lifecycle: `init()` → `elaborate(system)` → `startup()`
8. Freeze — mutations to children/map/ports raise errors

After instantiation, Python objects become **live handles**: property access reads/writes Rust state directly.

### Phase 3: Load and Run

```python
system.load_kernel("Image", dtb="virt.dtb", initrd="initrd.cpio")
while True:
    result = system.run(10_000_000)
    if result != "quantum":
        break
print(f"PC = {system.cpu.pc:#x}")
```

---

## 6. Error Propagation Strategy

### Exception Hierarchy

```
BaseException
└── Exception
    └── HelmError                (base for all helm-ng errors)
        ├── HelmConfigError      (invalid parameter, wiring mismatch, unknown component)
        ├── HelmMemFault         (memory access fault)
        │     attributes: addr: int, fault_kind: str, pc: int
        ├── HelmDeviceError      (device MMIO error)
        │     attributes: device_name: str, offset: int
        └── HelmCheckpointError  (version mismatch, truncated blob)
```

### When Errors Are Raised

| Error | Phase | Example |
|---|---|---|
| `HelmConfigError` | `instantiate()` | Duplicate map entry, unresolved PortRef, unknown device |
| `HelmConfigError` | property set | Wrong type for Rust `#[pyo3(get, set)]` field |
| `HelmMemFault` | `run()` | Load/store to unmapped address |
| `HelmDeviceError` | `run()` | Device returns error from MMIO access |
| `RuntimeError` | any | Method called before `instantiate()` |

### Rule

No Rust `panic!` reaches Python. All `Result::Err` paths at PyO3 boundaries are converted to `PyErr`.

---

## 7. GIL Management

### Rule 1: system.run() releases the GIL

`System::run()` calls `py.allow_threads(|| sim.run(n))`. Python threads can run concurrently (progress reporters, log consumers) while the simulation executes.

**Consequence:** The Rust simulation loop must not hold any Python objects while the GIL is released.

### Rule 2: Plugin callbacks re-acquire the GIL

When a Python callable is registered as a plugin callback (e.g., `add_plugin`, `spy()`, `trace_after()`), the callback is stored as a `PyObject`. When it fires during `run()`, the Rust side re-acquires the GIL via `Python::with_gil(|py| callback.call1(py, args))`.

**Performance note:** Every callback GIL re-acquisition is expensive. Use Rust-side plugins for high-frequency events.

---

## 8. Key Design Decisions

### D1 — 1:1 Python ↔ Rust Type Mapping (gem5-inspired)

**Decision:** Every major Rust type gets its own `#[pyclass(extends=SimObject)]` — not a single monolithic `Simulation` wrapper.

**Rationale:** Users should see the system composition in Python. Devices, CPU, memory, and memory map are first-class objects that can be created, parameterized, wired, and inspected individually. This mirrors gem5's SimObject pattern where every C++ class has a Python counterpart.

### D2 — Rust Struct Fields as Typed Params (no Python descriptors)

**Decision:** Use `#[pyo3(get, set)]` on Rust struct fields instead of Python-side `Param.*` descriptors.

**Rationale:** Type checking happens at compile time in Rust and at runtime via PyO3's automatic type conversion. No need for a separate Python descriptor layer — Rust IS the type system. Simpler, fewer layers, one source of truth.

### D3 — PortRef Wiring Resolved at instantiate()

**Decision:** Port assignments (`uart.irq = gic.spi(33)`) store `PortRef` descriptors. These are resolved into `Arc` refs during `instantiate()`.

**Rationale:** During the configuration phase, Rust objects don't exist yet, so there's nothing to wire. The PortRef pattern keeps the config phase borrow-free (no `Arc`, no `Mutex`, no Rust lifetimes in Python). Resolution happens in a single pass during `instantiate()`, where all objects are available.

### D4 — MemorySpace.add_map() with Bank Numbers (Simics-inspired)

**Decision:** `mem.add_map(base, device, size, bank=0)` maps a device at a base address with an optional bank number.

**Rationale:** Simics's memory-space map entries have a `function` field that selects which register bank handles the access. This is exactly what GICv2 needs: distributor at bank 0 (0x0800_0000), CPU interface at bank 1 (0x0801_0000). The device knows no base address — the memory map assigns it.

### D5 — Pre-Built Boards Replace Hardcoded Rust Platforms

**Decision:** Platform wiring (address map, device creation, interrupt routing) moves from Rust (`build_arm_virt()`) to pure Python classes in `python/helm/boards/`.

**Rationale:** Custom platforms should not require writing Rust. The `ArmVirt` board is a Python class that composes SimObject primitives — users can subclass it, modify the address map, add/remove devices, or write entirely new boards. This is analogous to gem5's stdlib `Board` classes.

### D6 — system.run() Releases GIL

**Decision:** `system.run()` releases the Python GIL via `py.allow_threads()`.

**Rationale:** Long simulation runs (billions of instructions) must not block Python threads. A GUI, progress reporter, or data pipeline should run concurrently. GIL release is safe because the Rust simulation loop accesses no Python objects.

### D7 — build_simulation() Remains as Backward-Compatible Sugar

**Decision:** Keep `build_simulation(isa, mode, timing, ...)` as a convenience function that internally creates a System + Cpu + Ram and calls `instantiate()`.

**Rationale:** Existing scripts and examples use `build_simulation()`. Breaking them is unnecessary. The function becomes a thin wrapper over the new API. Advanced users migrate to the SimObject API when they need custom platforms.

### D8 — Post-Instantiate Introspection

**Decision:** After `instantiate()`, Python object properties delegate to live Rust state. `system.cpu.pc` reads `Aarch64ArchState.pc` directly.

**Rationale:** Debugging and scripting require inspecting simulation state without stopping the hot path. Properties are implemented as `#[getter]` methods that read the Rust object behind an `Arc` or stored reference.

### D9 -- SpySession is Standalone/Attachable (not System.spy())

**Decision:** SpySession is constructed independently as `helm.SpySession(system, ...)`, not as `system.spy()`.

**Rationale:** SpySession is an observation tool, not a simulation component. It should not be part of the SimObject hierarchy. Making it standalone lets users create multiple independent observation sessions, detach/reattach them, and pass them across function boundaries. It also avoids polluting System's API with spy-specific methods.

### D10 -- ISA-Namespaced Python Package

**Decision:** ISA-specific classes live under `helm.aarch64.*` (and future `helm.riscv.*`), while ISA-neutral classes live directly under `helm.*`.

**Rationale:** As helm-ng supports multiple ISAs, device classes (GicV2, PLIC) and CPU models (A55Cpu, RV64Core) are ISA-specific. Namespacing prevents name collisions and makes the ISA scope clear in user code.

---

*For SimObject class definitions, see [`LLD-sim-objects.md`](./LLD-sim-objects.md). For param types and wiring, see [`LLD-param-system.md`](./LLD-param-system.md). For the instantiate flow, see [`LLD-instantiate.md`](./LLD-instantiate.md). For tests, see [`TEST.md`](./TEST.md).*

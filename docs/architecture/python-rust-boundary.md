# Python-Rust Boundary

How helm-ng bridges Python configuration with Rust simulation via PyO3.

## Design Principle

**Python describes; Rust simulates.** Python scripts define what to
simulate (ISA, platform, devices, parameters). Rust code executes the
simulation. Configuration is frozen at `instantiate()` — no Python
mutation during the simulation loop.

This follows gem5's model where Python `fs.py` scripts construct
SimObject trees but the C++ engine runs the simulation.

## Module Structure

The PyO3 module is `_helm_ng`, exposed as the Python package
`helm` (via `python/helm/`):

```text
python/helm/
├── __init__.py          # Re-exports from _helm_ng
├── aarch64/             # ISA-namespaced layout
│   └── ...
└── ...

rust: helm-python crate
├── lib.rs               # #[pymodule] _helm_ng
├── simobject.rs         # SimObject base class
├── system.rs            # HelmSystem (Python: "System")
├── cpu.rs               # Cpu descriptor
├── ram.rs               # Ram descriptor
├── memory_space.rs      # MemorySpace + MapEntry
├── cache.rs             # Cache descriptor
├── devices.rs           # GicV2, Pl011 wrappers
├── port.rs              # PortRef connections
├── spy.rs               # HelmSpy observation
├── compat.rs            # Backward-compatible helpers
└── instantiate.rs       # Instantiation logic
```

## HelmSim — The Boundary Object

`HelmSim` (defined in `helm-engine`) is an enum that wraps the three
timing variants:

```rust
pub enum HelmSim {
    VirtualTiming(HelmEngine<VirtualTiming>),
    IntervalTiming(HelmEngine<IntervalTiming>),
    AccurateTiming(HelmEngine<AccurateTiming>),
}
```

This is the **sole object that crosses the Rust-Python boundary**.
All Python calls enter through `HelmSim`; ISA and timing mode are
dispatched once per call (not per instruction).

## SimObject Base Class

`SimObject` is a `#[pyclass(subclass)]` that serves as the root of
the Python object hierarchy:

```python
class SimObject:
    # Children attached via __setattr__
    # State: Pending → Instantiated
    # Parameters frozen after instantiate()
```

### Child Attachment

```python
system = System(isa="aarch64")
system.cpu = Cpu(model="cortex-a55")    # __setattr__ tracks child
system.ram = Ram(size="256M")           # __setattr__ tracks child
```

Children are tracked internally via `__setattr__` override. The
`instantiate()` method walks the tree and wires all components.

## HelmSystem

`HelmSystem` (Rust struct name) is exposed to Python as `"System"`.
It extends `SimObject` and wraps `HelmSim`:

```python
system = System(isa="aarch64", timing="virtual", mem_size="256M")
system.instantiate()    # Freezes config, builds HelmSim
system.run(1000000)     # Runs quantum of instructions
```

Key methods:

| Python Method | Rust Method | Description |
|---------------|-------------|-------------|
| `instantiate()` | `HelmSystem::instantiate()` | Build engine, freeze config |
| `run(quantum)` | `HelmSim::run()` | Execute instructions |
| `set_cpu_model(name)` | `HelmSim::set_cpu_model()` | Set CPU core model |
| `pc` (property) | `HelmSim::pc()` | Current program counter |

## HelmSpy

`HelmSpy` is a standalone observation class (not a SimObject child):

```python
spy = HelmSpy(system, addr=0x1000, size=0x100)
# Monitors memory region for reads/writes
```

It is constructed via `helm.HelmSpy(system, ...)`, not attached as
a child.

## Exported Functions

`helm-python::compat` provides backward-compatible factory functions:

| Function | Purpose |
|----------|---------|
| `build_simulation()` | Legacy factory, returns `Py<HelmSystem>` |
| `set_sim_trace()` | Configure diagnostics |
| `list_cpu_models()` | Enumerate available CPU core models |
| `list_platforms()` | Enumerate available platforms |

## Helper Functions

`helm-python::system` provides parsing utilities:

| Function | Purpose |
|----------|---------|
| `parse_mode()` | String → `ExecMode` enum |
| `parse_timing()` | String → `TimingChoice` enum |

`parse_timing()` now accepts both simple model names and interval timing
override strings. Examples:

```python
timing="virtual"
timing="interval"
timing="interval:interval_len=256,l1d_size=64KiB,l2_size=1MiB"
```

The interval override string is the narrow Python-facing bridge to
`TimingChoice::IntervalTiming { mem_model, .. }`.

## Comparison

| Aspect | QEMU | gem5 | Simics | helm-ng |
|--------|------|------|--------|---------|
| Binding tech | None (C API) | SWIG/pybind11 | Custom C bridge | PyO3 |
| Config language | CLI + QOM | Python | Python + DML | Python |
| Object hierarchy | QOM tree | SimObject tree | Namespace tree | SimObject tree |
| Config freeze | `realize()` | `simulate()` | `continue` | `instantiate()` |
| Boundary object | N/A | SimObject | conf_object_t | `HelmSim` enum |
| ISA dispatch | Per-instruction | Per-instruction | Per-instruction | Per-call (once) |

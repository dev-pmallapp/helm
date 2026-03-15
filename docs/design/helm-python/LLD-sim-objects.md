# helm-python — LLD: Simulation Objects

> Low-level design for the PyO3 `#[pyclass]` sim objects and their Python DSL counterparts.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-param-system.md`](./LLD-param-system.md) · [`LLD-factory.md`](./LLD-factory.md)

---

## Table of Contents

1. [PySimulation](#1-pysimulation)
2. [Python Simulation Class](#2-python-simulation-class)
3. [Python Component Classes](#3-python-component-classes)
4. [PendingObject Conversion](#4-pendingobject-conversion)
5. [PyEventBus](#5-pyeventbus)
6. [PyWorld](#6-pydeviceworld)

---

## 1. PySimulation

`PySimulation` is the primary `#[pyclass]` that Python code interacts with after `elaborate()`. It wraps a `HelmSim` enum (which wraps `HelmEngine<T>`) and provides the Python-callable simulation methods.

### Rust Definition

```rust
// src/simulation.rs

use pyo3::prelude::*;
use helm_engine::{HelmSim, StopReason};
use crate::errors::map_helm_error;

/// Owns the Rust simulation engine after elaborate() completes.
/// Python holds a reference to this object for the lifetime of the simulation.
#[pyclass(name = "Simulation")]
pub struct PySimulation {
    /// None before elaborate(), Some after.
    sim: Option<HelmSim>,
    /// Shared reference to the event bus — Python can subscribe before run().
    event_bus: PyEventBus,
}

#[pymethods]
impl PySimulation {
    /// Called by Python Simulation.elaborate() after assembling PendingObjects.
    ///
    /// pending: list of (type_name: str, params: dict[str, Any]) tuples.
    /// Drives World::instantiate() → full SimObject lifecycle.
    #[pyo3(name = "elaborate")]
    fn elaborate_py(
        &mut self,
        py: Python<'_>,
        pending: Vec<(String, HashMap<String, PyObject>)>,
    ) -> PyResult<()> {
        let rust_pending = convert_pending(py, pending)?;
        let sim = HelmSim::build(rust_pending).map_err(map_helm_error)?;
        self.event_bus = PyEventBus::from_arc(sim.event_bus());
        self.sim = Some(sim);
        Ok(())
    }

    /// Run the simulation for `n_instructions` instructions.
    ///
    /// Releases the GIL for the duration of the Rust loop.
    /// `until` is an optional Python callable: (HelmEvent) -> bool.
    /// If `until` returns True for an event, simulation stops immediately.
    ///
    /// Returns: StopReason as a Python string ("completed", "until_hit", "exception").
    #[pyo3(name = "run", signature = (n_instructions=1_000_000, until=None))]
    fn run_py(
        &mut self,
        py: Python<'_>,
        n_instructions: u64,
        until: Option<PyObject>,
    ) -> PyResult<String> {
        let sim = self.sim.as_mut().ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "sim.run() called before sim.elaborate()"
            )
        })?;

        // Convert Python `until` callback to a Rust closure.
        // The closure re-acquires the GIL each time it is called.
        let until_fn: Option<Box<dyn Fn(&HelmEvent) -> bool + Send>> =
            until.map(|cb| {
                let cb = cb.clone();
                Box::new(move |event: &HelmEvent| -> bool {
                    Python::with_gil(|py| {
                        let py_event = event_to_pyobject(py, event);
                        cb.call1(py, (py_event,))
                            .and_then(|r| r.extract::<bool>(py))
                            .unwrap_or(false)
                    })
                }) as Box<dyn Fn(&HelmEvent) -> bool + Send>
            });

        // Release GIL — Rust simulation loop runs without Python
        let stop = py.allow_threads(|| {
            sim.run(n_instructions, until_fn)
        });

        Ok(match stop {
            StopReason::Completed       => "completed".to_string(),
            StopReason::UntilHit        => "until_hit".to_string(),
            StopReason::Exception(v)    => format!("exception:{v:#x}"),
            StopReason::Breakpoint(pc)  => format!("breakpoint:{pc:#x}"),
        })
    }

    /// Reset all SimObjects to power-on state. Wiring is preserved.
    #[pyo3(name = "reset")]
    fn reset_py(&mut self) -> PyResult<()> {
        self.sim.as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "sim.reset() called before sim.elaborate()"
            ))?
            .reset();
        Ok(())
    }

    /// Save a full checkpoint of all SimObjects. Returns bytes.
    #[pyo3(name = "checkpoint_save")]
    fn checkpoint_save_py(&self, py: Python<'_>) -> PyResult<PyObject> {
        let blob = self.sim.as_ref()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "checkpoint_save() called before elaborate()"
            ))?
            .checkpoint_save();
        Ok(PyBytes::new(py, &blob).into())
    }

    /// Restore a checkpoint from bytes produced by checkpoint_save().
    #[pyo3(name = "checkpoint_restore")]
    fn checkpoint_restore_py(&mut self, data: &PyBytes) -> PyResult<()> {
        self.sim.as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "checkpoint_restore() called before elaborate()"
            ))?
            .checkpoint_restore(data.as_bytes())
            .map_err(map_helm_error)
    }

    /// Attach a GDB RSP server on the given TCP port.
    #[pyo3(name = "attach_gdb", signature = (port = 1234))]
    fn attach_gdb_py(&mut self, port: u16) -> PyResult<()> {
        self.sim.as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "attach_gdb() called before elaborate()"
            ))?
            .attach_gdb(port)
            .map_err(map_helm_error)
    }

    /// Return the HelmEventBus proxy for subscribing to events.
    #[getter]
    fn event_bus(&self) -> PyResult<PyEventBus> {
        Ok(self.event_bus.clone())
    }

    /// Return current simulated instruction count.
    #[getter]
    fn instruction_count(&self) -> PyResult<u64> {
        Ok(self.sim.as_ref().map(|s| s.instruction_count()).unwrap_or(0))
    }
}
```

### State Machine

```
PySimulation created by Python Simulation.__init__()
       │
       │  sim.elaborate() called
       ▼
  elaborate_py() — converts PendingObjects, calls HelmSim::build()
       │
       │  HelmSim::build() → World::instantiate() → lifecycle complete
       ▼
  self.sim = Some(helm_sim)   ← simulation ready
       │
       │  sim.run() called
       ▼
  run_py() — releases GIL, calls HelmSim::run()
       │
       │  returns StopReason
       ▼
  returns stop reason string to Python
       │
       │  sim.reset() or sim.checkpoint_save/restore()
       ▼
  reset_py() / checkpoint_*_py()   ← GIL held throughout
```

---

## 2. Python Simulation Class

The Python `Simulation` class in `helm_ng/components.py` wraps `PySimulation` and provides the user-facing API.

```python
# helm_ng/components.py

from __future__ import annotations
from dataclasses import dataclass, field
from typing import Optional, Callable
from . import _helm_ng
from .params import _collect_params
from .exceptions import HelmConfigError

class Simulation:
    """
    Top-level simulation handle.

    Usage:
        board = Board(cpu=Cpu(isa=Isa.RiscV), memory=Memory(size="256MiB"))
        sim = Simulation(root=board)
        sim.elaborate()
        sim.run(n_instructions=1_000_000_000)
    """

    def __init__(self, root):
        self._root = root
        self._py_sim = _helm_ng.PySimulation()
        self._elaborated = False

    def elaborate(self):
        """
        Finalize the component graph and hand off to Rust.

        Converts all Python component objects to PendingObjects and
        calls PySimulation.elaborate(). After this call, the Python
        component objects are decoupled from the simulation.

        Raises:
            HelmConfigError: if any parameter is invalid or wiring conflicts.
        """
        pending = _collect_pending_objects(self._root)
        self._py_sim.elaborate(pending)
        self._elaborated = True

    def run(
        self,
        n_instructions: int = 1_000_000,
        until: Optional[Callable] = None,
    ) -> str:
        """
        Run the simulation.

        Releases the Python GIL during the Rust simulation loop.
        Other Python threads may run concurrently.

        Args:
            n_instructions: Maximum number of instructions to execute.
            until: Optional callable(event) -> bool. Called for each
                   HelmEvent. Return True to stop simulation early.

        Returns:
            Stop reason string: "completed", "until_hit",
            "exception:<vector_hex>", or "breakpoint:<pc_hex>".

        Raises:
            HelmMemFault: on simulated memory access fault.
        """
        self._require_elaborated()
        return self._py_sim.run(n_instructions, until)

    def reset(self):
        """Reset all components to power-on state. Wiring preserved."""
        self._require_elaborated()
        self._py_sim.reset()

    def checkpoint_save(self) -> bytes:
        """Serialize full architectural state. Returns bytes."""
        self._require_elaborated()
        return self._py_sim.checkpoint_save()

    def checkpoint_restore(self, data: bytes):
        """Restore from bytes produced by checkpoint_save()."""
        self._require_elaborated()
        self._py_sim.checkpoint_restore(data)

    def attach_gdb(self, port: int = 1234):
        """Start a GDB RSP server on the given TCP port."""
        self._require_elaborated()
        self._py_sim.attach_gdb(port)

    @property
    def event_bus(self):
        """HelmEventBus proxy. Subscribe to events before or after elaborate()."""
        return self._py_sim.event_bus

    @property
    def instruction_count(self) -> int:
        """Current simulated instruction count."""
        return self._py_sim.instruction_count

    def _require_elaborated(self):
        if not self._elaborated:
            raise HelmConfigError(
                "sim.elaborate() must be called before simulation methods"
            )


def _collect_pending_objects(root) -> list[tuple[str, dict]]:
    """
    Walk the component tree breadth-first and return a flat list of
    (type_name, params_dict) tuples suitable for PySimulation.elaborate().
    """
    result = []
    queue = [root]
    while queue:
        obj = queue.pop(0)
        if obj is None:
            continue
        result.append((obj._type_name(), obj._to_params()))
        for child in obj._children():
            queue.append(child)
    return result
```

---

## 3. Python Component Classes

Each component class follows a consistent pattern:

- Class-level `Param.*` descriptors define the field types.
- `__init__` accepts keyword arguments matching the field names.
- `_type_name()` returns the Rust factory name.
- `_to_params()` returns a dict of field name → Python value.
- `_children()` returns sub-components (for tree walking).

```python
# helm_ng/components.py (continued)

from .params import Param
from .enums import Isa, ExecMode, Timing

class Cpu:
    """CPU model configuration."""

    isa:     Param.Isa      = Param.Isa(Isa.RiscV)
    mode:    Param.ExecMode = Param.ExecMode(ExecMode.Syscall)
    timing:  Param.Timing   = Param.Timing(Timing.Virtual)

    # Sub-components (wired via Python attribute assignment)
    icache: "L1Cache | None" = None
    dcache: "L1Cache | None" = None
    l2cache: "L2Cache | None" = None

    def __init__(
        self,
        isa:    Isa      = Isa.RiscV,
        mode:   ExecMode = ExecMode.Syscall,
        timing: Timing   = Timing.Virtual,
        icache           = None,
        dcache           = None,
        l2cache          = None,
    ):
        self.isa     = isa
        self.mode    = mode
        self.timing  = timing
        self.icache  = icache
        self.dcache  = dcache
        self.l2cache = l2cache

    def _type_name(self): return "Cpu"
    def _to_params(self):
        return {
            "isa":    self.isa,
            "mode":   self.mode,
            "timing": self.timing,
        }
    def _children(self):
        return [self.icache, self.dcache, self.l2cache]


class L1Cache:
    """L1 instruction or data cache configuration."""

    size:        Param.MemorySize = Param.MemorySize("32KiB")
    assoc:       Param.Int        = Param.Int(8)
    hit_latency: Param.Cycles     = Param.Cycles(4)

    def __init__(
        self,
        size:        str | int = "32KiB",
        assoc:       int       = 8,
        hit_latency: int       = 4,
    ):
        self.size        = size
        self.assoc       = assoc
        self.hit_latency = hit_latency

    def _type_name(self): return "L1Cache"
    def _to_params(self):
        return {
            "size":        self.size,
            "assoc":       self.assoc,
            "hit_latency": self.hit_latency,
        }
    def _children(self): return []


class L2Cache:
    """Unified L2 cache configuration."""

    size:        Param.MemorySize = Param.MemorySize("256KiB")
    assoc:       Param.Int        = Param.Int(8)
    hit_latency: Param.Cycles     = Param.Cycles(12)

    def __init__(
        self,
        size:        str | int = "256KiB",
        assoc:       int       = 8,
        hit_latency: int       = 12,
    ):
        self.size        = size
        self.assoc       = assoc
        self.hit_latency = hit_latency

    def _type_name(self): return "L2Cache"
    def _to_params(self):
        return {
            "size":        self.size,
            "assoc":       self.assoc,
            "hit_latency": self.hit_latency,
        }
    def _children(self): return []


class Memory:
    """Main memory (DRAM) configuration."""

    size: Param.MemorySize = Param.MemorySize("256MiB")
    base: Param.Addr       = Param.Addr(0x8000_0000)

    def __init__(
        self,
        size: str | int = "256MiB",
        base: int       = 0x8000_0000,
    ):
        self.size = size
        self.base = base

    def _type_name(self): return "Memory"
    def _to_params(self):
        return {"size": self.size, "base": self.base}
    def _children(self): return []


class Board:
    """Platform board — top-level component container."""

    def __init__(self, cpu=None, memory=None, devices=None):
        self.cpu     = cpu
        self.memory  = memory
        self.devices = devices or []

    def _type_name(self): return "Board"
    def _to_params(self): return {}
    def _children(self):
        return [self.cpu, self.memory] + self.devices
```

---

## 4. PendingObject Conversion

The `_collect_params()` helper in `params.py` converts Python param values to the types that the Rust `AttrValue` enum accepts. This is the sole location where Python → Rust type conversion is defined.

```python
# helm_ng/params.py (conversion helper)

def _to_attr_value(key: str, value, descriptor) -> object:
    """
    Convert a Python param value to a Rust-compatible primitive.

    The descriptor knows the expected type and performs any needed
    normalization (e.g., "32KiB" -> 32768 for MemorySize).
    Returns a Python int, float, bool, or str — PyO3 knows how to
    convert these to AttrValue on the Rust side.
    """
    return descriptor.to_rust_value(value)
```

On the Rust side, the conversion from `PyObject` to `AttrValue`:

```rust
// src/params.rs

use pyo3::prelude::*;
use helm_engine::AttrValue;

pub fn pyobject_to_attr_value(py: Python<'_>, obj: &PyObject) -> PyResult<AttrValue> {
    // Try int first (most common)
    if let Ok(v) = obj.extract::<i64>(py) {
        return Ok(AttrValue::Int(v));
    }
    // Try bool before int (Python bool is a subclass of int)
    if let Ok(v) = obj.extract::<bool>(py) {
        return Ok(AttrValue::Bool(v));
    }
    if let Ok(v) = obj.extract::<f64>(py) {
        return Ok(AttrValue::Float(v));
    }
    if let Ok(v) = obj.extract::<String>(py) {
        return Ok(AttrValue::Str(v));
    }
    Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
        format!("Cannot convert {:?} to AttrValue", obj)
    ))
}
```

---

## 5. PyEventBus

`PyEventBus` wraps `Arc<HelmEventBus>` and exposes subscription to Python.

```rust
// src/event_bus.rs

#[pyclass(name = "EventBus")]
#[derive(Clone)]
pub struct PyEventBus {
    bus: Arc<HelmEventBus>,
}

#[pymethods]
impl PyEventBus {
    /// Subscribe a Python callable to a HelmEvent kind.
    ///
    /// kind: str — one of "Exception", "MemWrite", "MemRead", "CsrWrite",
    ///             "MagicInsn", "SyscallEnter", "SyscallReturn",
    ///             "DeviceSignal", "Custom"
    /// callback: callable(event: dict) -> None
    ///
    /// Returns an EventHandle. Keep alive to stay subscribed; drop to unsubscribe.
    #[pyo3(name = "subscribe")]
    fn subscribe_py(
        &self,
        py: Python<'_>,
        kind: &str,
        callback: PyObject,
    ) -> PyResult<PyEventHandle> {
        let kind = parse_event_kind(kind)?;

        // Clone callback into the closure (Py<PyAny> is Send + Sync).
        let cb = callback.clone_ref(py);

        let id = self.bus.subscribe(kind, move |event: &HelmEvent| {
            // Re-acquire GIL to call Python. This fires on the simulation thread.
            Python::with_gil(|py| {
                let py_event = event_to_dict(py, event);
                if let Err(e) = cb.call1(py, (py_event,)) {
                    // Log but do not panic — subscriber errors must not crash Rust
                    eprintln!("helm-python: event subscriber error: {e}");
                }
            });
        });

        Ok(PyEventHandle { id, bus: Arc::clone(&self.bus) })
    }
}

/// Holds a subscription ID. Dropping this unsubscribes the callback.
#[pyclass(name = "EventHandle")]
pub struct PyEventHandle {
    id:  SubscriberId,
    bus: Arc<HelmEventBus>,
}

impl Drop for PyEventHandle {
    fn drop(&mut self) {
        self.bus.unsubscribe(self.id);
    }
}
```

### Python Usage

```python
# Subscribe to exceptions — keep handle alive or subscription is dropped
handle = sim.event_bus.subscribe("Exception", lambda e: print(f"Exception: {e}"))

# Subscribe to magic instructions (ROI markers)
roi_entered = False
def on_magic(event):
    global roi_entered
    if event["value"] == 0xDEAD_BEEF:
        roi_entered = True

sim.event_bus.subscribe("MagicInsn", on_magic)
sim.run(n_instructions=10_000_000_000)
```

### Event Dict Schema

Python callbacks receive the event as a dict. Field names match the `HelmEvent` enum variants:

| Event kind | Dict keys |
|---|---|
| `Exception` | `cpu: str`, `vector: int`, `pc: int`, `tval: int` |
| `MemWrite` | `addr: int`, `size: int`, `val: int`, `cycle: int` |
| `MemRead` | `addr: int`, `size: int`, `val: int`, `cycle: int` |
| `CsrWrite` | `csr: int`, `old: int`, `new: int` |
| `MagicInsn` | `pc: int`, `value: int` |
| `SyscallEnter` | `nr: int`, `args: list[int]` |
| `SyscallReturn` | `nr: int`, `ret: int` |
| `DeviceSignal` | `device: str`, `port: str`, `val: int` |
| `Custom` | `name: str`, `data: bytes` |

---

## 6. PyWorld

`PyWorld` wraps `World` for Python access.

```rust
// src/world.rs

#[pyclass(name = "World")]
pub struct PyWorld {
    world: World,
}

#[pymethods]
impl PyWorld {
    #[new]
    fn new_py() -> Self {
        PyWorld { world: World::new() }
    }

    /// Add a device to the world. Returns an integer object ID.
    #[pyo3(name = "add_device")]
    fn add_device_py(
        &mut self,
        py: Python<'_>,
        device_config: PyObject,
        name: &str,
    ) -> PyResult<u64> {
        let (type_name, params) = extract_device_config(py, device_config)?;
        let device = build_device(&type_name, params).map_err(map_helm_error)?;
        let id = self.world.add_device(name, device);
        Ok(id.0)
    }

    /// Map a device at a base address.
    #[pyo3(name = "map_device")]
    fn map_device_py(&mut self, id: u64, base: u64) -> PyResult<()> {
        self.world.map_device(HelmObjectId(id), base);
        Ok(())
    }

    /// Finalize the component graph.
    #[pyo3(name = "elaborate")]
    fn elaborate_py(&mut self) -> PyResult<()> {
        self.world.elaborate();
        Ok(())
    }

    /// Write `size` bytes of `val` to `addr`.
    #[pyo3(name = "mmio_write")]
    fn mmio_write_py(&mut self, addr: u64, size: usize, val: u64) -> PyResult<()> {
        self.world.mmio_write(addr, size, val);
        Ok(())
    }

    /// Read `size` bytes from `addr`. Returns the value.
    #[pyo3(name = "mmio_read")]
    fn mmio_read_py(&self, addr: u64, size: usize) -> PyResult<u64> {
        Ok(self.world.mmio_read(addr, size))
    }

    /// Advance the virtual clock by `cycles` ticks.
    #[pyo3(name = "advance")]
    fn advance_py(&mut self, cycles: u64) -> PyResult<()> {
        self.world.advance(cycles);
        Ok(())
    }

    /// Return list of (device_id: int, pin_name: str) for asserted interrupts.
    #[pyo3(name = "pending_interrupts")]
    fn pending_interrupts_py(&self) -> PyResult<Vec<(u64, String)>> {
        Ok(self.world.pending_interrupts()
            .into_iter()
            .map(|(id, name)| (id.0, name))
            .collect())
    }

    /// Return the current virtual clock tick.
    #[getter]
    fn current_tick(&self) -> PyResult<u64> {
        Ok(self.world.current_tick())
    }

    /// Subscribe a Python callable to HelmEvent kind.
    #[pyo3(name = "on_event")]
    fn on_event_py(
        &self,
        py: Python<'_>,
        kind: &str,
        callback: PyObject,
    ) -> PyResult<PyEventHandle> {
        let kind = parse_event_kind(kind)?;
        let cb = callback.clone_ref(py);
        let handle = self.world.on_event(kind, move |event| {
            Python::with_gil(|py| {
                let py_event = event_to_dict(py, event);
                let _ = cb.call1(py, (py_event,));
            });
        });
        Ok(PyEventHandle::from_handle(handle))
    }
}
```

### Python Usage

```python
from helm_ng import World, Uart16550

world = World()
uart = world.add_device(Uart16550(clock_hz=1_843_200), name="uart")
world.map_device(uart, base=0x10000000)
world.elaborate()

world.mmio_write(0x10000000, 1, ord('A'))
world.advance(cycles=2000)

irqs = world.pending_interrupts()
assert (uart, "irq_out") in irqs
```

---

*For the Param type system, see [`LLD-param-system.md`](./LLD-param-system.md). For `build_simulator` and plugin loading, see [`LLD-factory.md`](./LLD-factory.md). For tests, see [`TEST.md`](./TEST.md).*

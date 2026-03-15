# helm-python — LLD: Factory, Plugin Loader, and HelmProtocol

> Low-level design for `build_simulator()`, `helm_ng.load_plugin()`, device introspection, and the HelmProtocol server attachment.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-sim-objects.md`](./LLD-sim-objects.md) · [`LLD-param-system.md`](./LLD-param-system.md)

---

## Table of Contents

1. [build_simulator()](#1-build_simulator)
2. [Plugin Loader](#2-plugin-loader)
3. [Device Introspection](#3-device-introspection)
4. [HelmProtocol Server Attachment](#4-helmprotocol-server-attachment)
5. [Module Registration](#5-module-registration)

---

## 1. build_simulator()

`build_simulator` is the `#[pyfunction]` that creates a `HelmSim` from a minimal set of ISA/mode/timing parameters. It is the low-level alternative to `Simulation.elaborate()` for users who want to bypass the Python DSL and drive the Rust engine directly.

### Rust Definition

```rust
// src/factory.rs

use pyo3::prelude::*;
use helm_engine::{HelmSim, Isa, ExecMode, TimingChoice};
use crate::errors::map_helm_error;

/// Low-level factory — create a HelmSim from string parameters.
///
/// Args:
///   isa:    "riscv" | "aarch64" | "aarch32"
///   mode:   "functional" | "syscall" | "system"
///   timing: "virtual" | "interval" | "accurate"
///
/// Returns:
///   PySimulation wrapping the built HelmSim.
///
/// Raises:
///   HelmConfigError: if isa/mode/timing combination is unsupported.
#[pyfunction]
#[pyo3(name = "build_simulator")]
pub fn build_simulator_py(
    isa:    &str,
    mode:   &str,
    timing: &str,
) -> PyResult<PySimulation> {
    let isa = parse_isa(isa).map_err(map_helm_error)?;
    let mode = parse_exec_mode(mode).map_err(map_helm_error)?;
    let timing = parse_timing(timing).map_err(map_helm_error)?;

    let sim = HelmSim::build_minimal(isa, mode, timing)
        .map_err(map_helm_error)?;

    Ok(PySimulation::from_sim(sim))
}

fn parse_isa(s: &str) -> Result<Isa, HelmError> {
    match s {
        "riscv"   => Ok(Isa::RiscV),
        "aarch64" => Ok(Isa::AArch64),
        "aarch32" => Ok(Isa::AArch32),
        other     => Err(HelmError::Config(ConfigError::UnknownIsa(other.to_string()))),
    }
}

fn parse_exec_mode(s: &str) -> Result<ExecMode, HelmError> {
    match s {
        "functional" => Ok(ExecMode::Functional),
        "syscall"    => Ok(ExecMode::Syscall),
        "system"     => Ok(ExecMode::System),
        other        => Err(HelmError::Config(ConfigError::UnknownMode(other.to_string()))),
    }
}

fn parse_timing(s: &str) -> Result<TimingChoice, HelmError> {
    match s {
        "virtual"  => Ok(TimingChoice::Virtual),
        "interval" => Ok(TimingChoice::Interval),
        "accurate" => Ok(TimingChoice::Accurate),
        other      => Err(HelmError::Config(ConfigError::UnknownTiming(other.to_string()))),
    }
}
```

### Rust HelmSim::build_minimal()

This is the variant of the build path that skips the full `PendingObject` elaboration and creates a minimal `HelmEngine<T>` with only a CPU and flat memory — suitable for ISA testing without a device tree.

```rust
// crates/helm-engine/src/lib.rs

impl HelmSim {
    /// Build a minimal simulation with no devices — suitable for ISA unit tests.
    ///
    /// Creates: ArchState (for `isa`) + flat 256 MiB RAM + no event queue.
    /// Does NOT run the SimObject lifecycle (no devices to init/elaborate).
    pub fn build_minimal(
        isa: Isa,
        mode: ExecMode,
        timing: TimingChoice,
    ) -> Result<HelmSim, HelmError> {
        let memory = MemoryMap::with_flat_ram(0x0000_0000, 256 * 1024 * 1024);
        let arch   = ArchState::new(isa);

        match timing {
            TimingChoice::Virtual  => Ok(HelmSim::Virtual(
                HelmEngine::new_minimal(isa, mode, Virtual, arch, memory)
            )),
            TimingChoice::Interval => Ok(HelmSim::Interval(
                HelmEngine::new_minimal(isa, mode, Interval::default(), arch, memory)
            )),
            TimingChoice::Accurate => Ok(HelmSim::Accurate(
                HelmEngine::new_minimal(isa, mode, Accurate, arch, memory)
            )),
        }
    }
}
```

### Python Usage

```python
import _helm_ng

# Low-level: minimal RISC-V functional simulator, no device tree
sim = _helm_ng.build_simulator(isa="riscv", mode="syscall", timing="virtual")
sim.run(n_instructions=1_000_000)

# High-level: use the Simulation DSL instead (preferred for most users)
from helm_ng import Simulation, Cpu, Memory, Board
sim = Simulation(root=Board(cpu=Cpu(), memory=Memory()))
sim.elaborate()
sim.run(1_000_000)
```

---

## 2. Plugin Loader

`helm_ng.load_plugin(path)` loads a device `.so` plugin at runtime, registers the Rust device factory with `DeviceRegistry`, and injects the embedded Python class into the `helm_ng` namespace.

### Rust Implementation

```rust
// src/factory.rs (continued)

use std::path::PathBuf;
use helm_devices::DeviceRegistry;
use pyo3::types::PyModule;

/// Load a device plugin from a .so file.
///
/// The .so must export:
///   - `helm_device_register(registry: *mut DeviceRegistry)` — Rust C-ABI entry
///   - `PYTHON_CLASS: &'static str` — embedded Python class definition
///   - `HELM_DEVICE_ABI_VERSION: u32` — must match current ABI version
///
/// After loading:
///   - The Rust factory is registered in the global DeviceRegistry.
///   - The Python class is exec()'d into the helm_ng module namespace.
///
/// Raises:
///   HelmConfigError: if the .so is not found, ABI version mismatches,
///                    or the device name conflicts with an existing registration.
#[pyfunction]
#[pyo3(name = "load_plugin")]
pub fn load_plugin_py(py: Python<'_>, path: &str) -> PyResult<()> {
    let path = PathBuf::from(path);

    // 1. dlopen the .so
    let lib = unsafe { libloading::Library::new(&path) }
        .map_err(|e| map_config_error(format!("failed to open plugin {path:?}: {e}")))?;

    // 2. Check ABI version
    let abi_ver: libloading::Symbol<*const u32> = unsafe {
        lib.get(b"HELM_DEVICE_ABI_VERSION")
    }.map_err(|_| map_config_error("plugin missing HELM_DEVICE_ABI_VERSION"))?;
    let abi_ver = unsafe { **abi_ver };
    if abi_ver != CURRENT_DEVICE_ABI_VERSION {
        return Err(map_config_error(format!(
            "plugin ABI mismatch: plugin={abi_ver}, current={CURRENT_DEVICE_ABI_VERSION}"
        )));
    }

    // 3. Call helm_device_register()
    let register_fn: libloading::Symbol<unsafe extern "C" fn(*mut DeviceRegistry)> = unsafe {
        lib.get(b"helm_device_register")
    }.map_err(|_| map_config_error("plugin missing helm_device_register"))?;

    let registry = global_device_registry();
    unsafe { register_fn(registry as *mut DeviceRegistry) };

    // 4. Inject Python class into helm_ng namespace
    let python_class: libloading::Symbol<*const u8> = unsafe {
        lib.get(b"PYTHON_CLASS")
    }.map_err(|_| map_config_error("plugin missing PYTHON_CLASS"))?;
    let class_str = unsafe { std::ffi::CStr::from_ptr(*python_class as *const i8) }
        .to_str()
        .map_err(|_| map_config_error("PYTHON_CLASS is not valid UTF-8"))?;

    let helm_ng_module = py.import("helm_ng")?;
    py.run(class_str, Some(helm_ng_module.dict()), None)?;

    // 5. Keep the library alive (leak intentionally — plugins are permanent)
    std::mem::forget(lib);

    Ok(())
}
```

### Plugin .so Structure

A plugin crate must have `crate-type = ["cdylib"]` and export these symbols:

```rust
// examples/plugin-uart/src/lib.rs

use helm_devices::{DeviceRegistry, DeviceDescriptor, ParamSchema, ParamType};

/// ABI version — must match helm-devices' CURRENT_DEVICE_ABI_VERSION.
#[no_mangle]
pub static HELM_DEVICE_ABI_VERSION: u32 = 1;

/// Registration entry point — called by load_plugin().
#[no_mangle]
pub extern "C" fn helm_device_register(registry: *mut DeviceRegistry) {
    let r = unsafe { &mut *registry };
    r.register(DeviceDescriptor {
        name:        "uart16550",
        version:     "1.0.0",
        description: "16550-compatible UART",
        factory:     |params| Box::new(Uart16550::from_params(params)),
        param_schema: || {
            ParamSchema::new()
                .field("clock_hz", ParamType::Int, 1_843_200i64)
        },
    });
}

/// Embedded Python class — exec()'d into helm_ng namespace after load.
/// Must not include base_addr or irq — those are system-level.
#[no_mangle]
pub static PYTHON_CLASS: &std::ffi::CStr = {
    const BYTES: &[u8] = b"class Uart16550(Device):\n    clock_hz: Param.Int = 1_843_200\n\0";
    match std::ffi::CStr::from_bytes_with_nul(BYTES) {
        Ok(s) => s,
        Err(_) => panic!("invalid PYTHON_CLASS"),
    }
};
```

### Python Usage

```python
import helm_ng

# Load plugin — Uart16550 class is now in helm_ng namespace
helm_ng.load_plugin("./libhelm_uart16550.so")

# Use the loaded class
uart = helm_ng.Uart16550(clock_hz=3_686_400)

world = helm_ng.World()
uart_id = world.add_device(uart, name="uart")
world.map_device(uart_id, base=0x10000000)
world.elaborate()
```

---

## 3. Device Introspection

Three functions allow Python scripts to discover what devices are available without knowing the device types at import time. These are useful for generic tooling (configuration generators, test harnesses) and plugin discovery.

### list_devices()

```rust
/// Return the names of all registered device types.
#[pyfunction]
#[pyo3(name = "list_devices")]
pub fn list_devices_py() -> Vec<String> {
    global_device_registry()
        .list()
        .iter()
        .map(|d| d.name.to_string())
        .collect()
}
```

```python
>>> import helm_ng
>>> helm_ng.list_devices()
['uart16550', 'plic', 'clint', 'virtio_disk', 'virtio_net']
```

### device_schema()

Returns the parameter schema for a named device as a Python dict. Each entry maps field name to a dict with keys `type`, `default`, `description`.

```rust
/// Return the parameter schema for a named device as a Python dict.
///
/// Schema dict format:
///   { field_name: { "type": str, "default": Any, "description": str } }
///
/// Raises:
///   HelmConfigError: if device_name is not registered.
#[pyfunction]
#[pyo3(name = "device_schema")]
pub fn device_schema_py(py: Python<'_>, device_name: &str) -> PyResult<PyObject> {
    let registry = global_device_registry();
    let schema = registry.param_schema(device_name)
        .ok_or_else(|| map_config_error(
            format!("unknown device: {device_name}")
        ))?;

    let dict = pyo3::types::PyDict::new(py);
    for field in schema.fields() {
        let field_dict = pyo3::types::PyDict::new(py);
        field_dict.set_item("type",        param_type_str(field.kind))?;
        field_dict.set_item("default",     param_value_to_py(py, &field.default)?)?;
        field_dict.set_item("description", field.description)?;
        dict.set_item(field.name, field_dict)?;
    }
    Ok(dict.into())
}
```

```python
>>> helm_ng.device_schema("uart16550")
{
    "clock_hz": {
        "type": "int",
        "default": 1843200,
        "description": "UART baud clock in Hz"
    }
}
```

---

## 4. HelmProtocol Server Attachment

After `elaborate()`, two debug protocols can be attached to the simulation.

### sim.attach_gdb(port)

Starts a GDB RSP server listening on the given TCP port. The server runs in a separate OS thread; the simulation pauses at the next instruction boundary and waits for GDB to connect.

```rust
// On PySimulation — see LLD-sim-objects.md for full context

#[pyo3(name = "attach_gdb", signature = (port = 1234))]
fn attach_gdb_py(&mut self, port: u16) -> PyResult<()> {
    let sim = self.require_elaborated()?;
    // Starts the GDB RSP server thread, hooks into HelmEngine's step boundary
    sim.debug_mut().attach_gdb(port).map_err(map_helm_error)
}
```

```python
sim.elaborate()
sim.attach_gdb(port=1234)   # blocks until gdb connects if blocking=True (default)
# In another terminal: gdb -ex 'target remote :1234' ./my_binary
sim.run(n_instructions=10_000_000)
```

### sim.enable_trace(path)

Enables the `TraceLogger` — a `HelmEventBus` subscriber that writes all `TraceEvent` records to a file in JSON Lines format.

```rust
#[pyo3(name = "enable_trace", signature = (path, ring_buffer_size = 65536))]
fn enable_trace_py(&mut self, path: &str, ring_buffer_size: usize) -> PyResult<()> {
    let sim = self.require_elaborated()?;
    sim.debug_mut()
        .enable_trace(PathBuf::from(path), ring_buffer_size)
        .map_err(map_helm_error)
}
```

```python
sim.elaborate()
sim.enable_trace("/tmp/trace.jsonl", ring_buffer_size=131072)
sim.run(n_instructions=1_000_000)
# /tmp/trace.jsonl now contains JSON Lines of TraceEvent records
```

**Trace file format (JSON Lines, one record per line):**

```json
{"kind":"InsnFetch","pc":4194304,"bytes":19778867}
{"kind":"MemRead","addr":134217728,"size":4,"value":0,"cycle":1}
{"kind":"SyscallEnter","nr":64,"args":[1,135000,12,0,0,0]}
{"kind":"SyscallReturn","nr":64,"ret":12}
{"kind":"Exception","cpu":"cpu0","vector":8,"pc":4194304,"tval":0}
```

---

## 5. Module Registration

All `#[pyclass]` and `#[pyfunction]` items are registered in the `#[pymodule]` function in `src/lib.rs`:

```rust
// src/lib.rs

use pyo3::prelude::*;

/// The internal extension module — imported as `_helm_ng`.
/// Users should import from `helm_ng` (the Python DSL package), not from here.
#[pymodule]
fn _helm_ng(py: Python<'_>, m: &PyModule) -> PyResult<()> {
    // Classes
    m.add_class::<PySimulation>()?;
    m.add_class::<PyWorld>()?;
    m.add_class::<PyEventBus>()?;
    m.add_class::<PyEventHandle>()?;

    // Factory functions
    m.add_function(wrap_pyfunction!(build_simulator_py, m)?)?;
    m.add_function(wrap_pyfunction!(load_plugin_py, m)?)?;
    m.add_function(wrap_pyfunction!(list_devices_py, m)?)?;
    m.add_function(wrap_pyfunction!(device_schema_py, m)?)?;

    // Exception classes
    m.add("HelmError",           py.get_type::<exceptions::PyHelmError>())?;
    m.add("HelmConfigError",     py.get_type::<exceptions::PyHelmConfigError>())?;
    m.add("HelmMemFault",        py.get_type::<exceptions::PyHelmMemFault>())?;
    m.add("HelmDeviceError",     py.get_type::<exceptions::PyHelmDeviceError>())?;
    m.add("HelmCheckpointError", py.get_type::<exceptions::PyHelmCheckpointError>())?;

    Ok(())
}
```

The public `helm_ng/__init__.py` imports selectively from `_helm_ng` and re-exports under clean names:

```python
# helm_ng/__init__.py

from ._helm_ng import (
    build_simulator,
    load_plugin,
    list_devices,
    device_schema,
    HelmError,
    HelmConfigError,
    HelmMemFault,
    HelmDeviceError,
    HelmCheckpointError,
)
from .components import Simulation, Board, Cpu, L1Cache, L2Cache, Memory, World
from .params import Param
from .enums import Isa, ExecMode, Timing

__all__ = [
    "Simulation", "Board", "Cpu", "L1Cache", "L2Cache", "Memory", "World",
    "Param", "Isa", "ExecMode", "Timing",
    "build_simulator", "load_plugin", "list_devices", "device_schema",
    "HelmError", "HelmConfigError", "HelmMemFault", "HelmDeviceError",
    "HelmCheckpointError",
]
```

---

*For the `#[pyclass]` sim objects, see [`LLD-sim-objects.md`](./LLD-sim-objects.md). For the Param type system, see [`LLD-param-system.md`](./LLD-param-system.md). For tests, see [`TEST.md`](./TEST.md).*

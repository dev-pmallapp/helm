# helm-python — LLD: Instantiate Flow, DLD Loader, and Module Registration

> Low-level design for `system.instantiate()`, backward-compat `build_simulation()`, DLD loading, and `#[pymodule]` registration.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-sim-objects.md`](./LLD-sim-objects.md) · [`LLD-param-system.md`](./LLD-param-system.md)

---

## Table of Contents

1. [system.instantiate() Flow](#1-systeminstantiate-flow)
2. [Backward-Compatible build_simulation()](#2-backward-compatible-build_simulation)
3. [DLD Loader](#3-dld-loader)
4. [Device Introspection](#4-device-introspection)
5. [Debug Protocol Attachment](#5-debug-protocol-attachment)
6. [Module Registration](#6-module-registration)

---

## 1. system.instantiate() Flow

`system.instantiate()` is the bridge between Python configuration and Rust simulation. It converts the Python SimObject tree into live Rust objects.

### Implementation

```rust
// src/instantiate.rs

use pyo3::prelude::*;
use helm_engine::{HelmSim, HelmEngine, build_simulator, TimingChoice};
use crate::simobject::SimObjectState;
use crate::system::HelmSystem;
use crate::cpu::Cpu;
use crate::ram::Ram;
use crate::memory_space::MemorySpace;
use crate::devices::gicv2::GicV2;
use crate::devices::pl011::Pl011;

pub fn do_instantiate(mut system: PyRefMut<HelmSystem>, py: Python) -> PyResult<()> {
    let base = system.as_ref();  // access SimObject base
    base.require_pending()?;

    // ── Step 1: Validate required children ──

    let mode = system.mode.clone();
    let timing = system.timing.clone();
    let ipc = system.ipc;

    // Extract children from the SimObject base
    let children = &base.children;

    let cpu_obj = children.get("cpu")
        .ok_or_else(|| config_error("System requires a 'cpu' child"))?;
    let cpu: PyRef<Cpu> = cpu_obj.extract(py)?;

    // ── Step 2: Parse ISA and timing ──

    let isa = parse_isa(&cpu.isa)?;
    let timing_choice = parse_timing(&timing, ipc)?;

    // ── Step 3: Build HelmEngine<T> ──

    let mem_size = resolve_ram_size(children, py)?;
    let sim = build_simulator(isa, parse_mode(&mode)?, timing_choice,
                              0x0, mem_size);

    // ── Step 4: Apply CPU model ──

    sim.set_cpu_model(&cpu.model)?;

    // ── Step 5: Build memory map and devices (FS mode) ──

    if mode == "fs" {
        let mem_obj = children.get("mem")
            .ok_or_else(|| config_error("FS mode requires a 'mem' child"))?;
        let mem_space: PyRef<MemorySpace> = mem_obj.extract(py)?;

        // Process map entries → create HelmAddressSpace, AddressMap, device instances
        for entry in &mem_space.entries {
            // Dispatch on device type:
            // - Ram → add RAM region to FlatMem
            // - GicV2 → create GicDistributor/GicCpuInterface, map at base+bank
            // - Pl011 → create Pl011 device, map at base
            process_map_entry(&mut sim, entry, py)?;
        }

        // ── Step 6: Resolve PortRefs ──

        resolve_ports(&mut sim, children, py)?;
    }

    // ── Step 7: Store live references back on Python objects ──

    // cpu.arch_state = Some(Arc to ArchState inside sim)
    // gic.gic_state = Some(Arc to GicState inside sim)
    // etc.
    wire_back_references(&sim, children, py)?;

    // ── Step 8: Mark all objects as Instantiated ──

    system.sim = Some(sim);
    // Mark base and all children as Instantiated
    base.state = SimObjectState::Instantiated;
    for (_name, child) in &base.children {
        if let Ok(mut obj) = child.extract::<PyRefMut<SimObject>>(py) {
            obj.state = SimObjectState::Instantiated;
        }
    }

    Ok(())
}
```

### Sequence Diagram

```
Python                                  Rust
──────                                  ────
system.instantiate()
  │
  ├─ validate children exist ─────────► require "cpu" child
  │                                     require "mem" child (FS mode)
  │
  ├─ parse ISA, mode, timing ────────► Isa::AArch64, ExecMode::System,
  │                                     TimingChoice::Virtual
  │
  ├─ build HelmEngine<T> ────────────► HelmEngine::new(isa, mode, timing, mem)
  │
  ├─ apply CPU model ────────────────► set_cpu_model("cortex-a55")
  │
  ├─ process map entries ────────────► for each MapEntry:
  │                                       create device, add to HelmAddressSpace
  │
  ├─ resolve PortRefs ───────────────► for each device with irq: Option<PortRef>:
  │                                       find target GIC → create GicSink
  │                                       → wire into device.irq_out
  │
  ├─ wire back references ──────────► store Arc refs on Python objects
  │                                    (cpu.arch_state, gic.gic_state, etc.)
  │
  └─ mark Instantiated ─────────────► all SimObjects → Instantiated state
```

### Port Resolution Detail

```rust
fn resolve_ports(
    sim: &mut HelmSim,
    children: &HashMap<String, PyObject>,
    py: Python,
) -> PyResult<()> {
    // Collect all devices with PortRef fields
    for (name, child) in children {
        // Check if child has an `irq` field with a PortRef
        if let Ok(pl011) = child.extract::<PyRef<Pl011>>(py) {
            if let Some(ref port_ref) = pl011.irq {
                // Find the target GIC
                let target = children.get(&port_ref.target_name)
                    .ok_or_else(|| config_error(format!(
                        "unresolved port: {name}.irq references '{}.{}' \
                         but no child '{}' exists",
                        port_ref.target_name, port_ref.port_name,
                        port_ref.target_name
                    )))?;

                // Parse port_name "spi[33]" → INTID 33
                let intid = parse_spi_port(&port_ref.port_name)?;

                // Create GicSink and wire
                let gic: PyRef<GicV2> = target.extract(py)?;
                let gic_state = gic.gic_state.as_ref().unwrap();
                let sink = GicSink::new(Arc::clone(gic_state), intid);

                // Wire the device's IRQ output to the sink
                sim.wire_irq(name, WireId::from(intid), sink)?;
            }
        }
        // ... repeat for Sp804 and other devices with ports
    }
    Ok(())
}
```

---

## 2. Backward-Compatible build_simulation()

The existing `build_simulation()` function remains as sugar. Internally it creates a System + Cpu + Ram and calls `instantiate()`.

```rust
// src/compat.rs

/// Backward-compatible factory — creates a minimal System and instantiates it.
///
/// This is the v1 API. New code should use the SimObject hierarchy directly.
#[pyfunction]
#[pyo3(name = "build_simulation")]
#[pyo3(signature = (*, isa="aarch64", mode="se", timing="virtual",
                    mem_base=0, mem_mib=512, ipc=4.0))]
pub fn build_simulation_py(
    py: Python,
    isa: &str,
    mode: &str,
    timing: &str,
    mem_base: u64,
    mem_mib: u64,
    ipc: f64,
) -> PyResult<PyObject> {
    // Create System + Cpu + Ram
    let system = HelmSystem::new("default", timing, mode, ipc);
    let cpu = Cpu::new("cpu0", isa, "cortex-a55", 4, 128, 64, 32, 32);
    let ram = Ram::new("ram0", &format!("{}MiB", mem_mib));

    // Assign children
    // ... (via Python-level assignment to maintain consistency)

    // Instantiate
    // ...

    // Return the System (which has .run(), .load_elf(), etc.)
    Ok(system.into_py(py))
}
```

### Python Usage (backward compat)

```python
# Old API — still works
sim = helm.build_simulation(isa="aarch64", mode="se", timing="virtual")
sim.load_elf("./hello", argv=["hello"])
sim.run(10_000_000)

# New API — preferred
system = helm.System("se", timing="virtual", mode="se")
system.cpu = helm.Cpu("cpu0", isa="aarch64")
system.ram = helm.Ram("ram0", size="512MiB")
system.instantiate()
system.load_elf("./hello", argv=["hello"])
system.run(10_000_000)
```

---

## 3. DLD Loader

`helm.load_dld(path)` loads a Dynamically Loaded Device (`.so` plugin) at runtime.

```rust
// src/compat.rs (or src/dld.rs)

/// Load a device DLD from a .so file.
///
/// The .so must export:
///   - helm_device_register(registry: *mut DeviceRegistry)
///   - HELM_DEVICES_ABI_VERSION: u32
///
/// After loading, the device type is available for use with MemorySpace.add_map().
#[pyfunction]
#[pyo3(name = "load_dld")]
pub fn load_dld_py(path: &str) -> PyResult<()> {
    let path = PathBuf::from(path);
    // 1. dlopen
    // 2. Check ABI version
    // 3. Call helm_device_register()
    // 4. std::mem::forget(lib) — plugins are permanent
    Ok(())
}
```

---

## 4. Device Introspection

```rust
/// Return names of all registered device types (built-in + loaded DLDs).
#[pyfunction]
#[pyo3(name = "list_devices")]
pub fn list_devices_py() -> Vec<String> {
    global_device_registry().list().iter().map(|d| d.name.to_string()).collect()
}

/// Return the parameter schema for a named device.
#[pyfunction]
#[pyo3(name = "device_schema")]
pub fn device_schema_py(py: Python, device_name: &str) -> PyResult<PyObject> {
    // Returns dict of { field_name: { "type": str, "default": Any, "description": str } }
    ...
}
```

---

## 5. Debug Protocol Attachment

### system.attach_gdb(port)

Starts a GDB RSP server on the given TCP port. Available after `instantiate()`.

```python
system.instantiate()
system.attach_gdb(port=1234)
# In another terminal: gdb -ex 'target remote :1234' ./my_binary
system.run(10_000_000)
```

### system.trace_after(...)

Deprecated legacy plugin-backed event logging with trigger conditions.
Prefer `helm.HelmSpy(system, ...)` or `system.observe()` for primary
observation flows.

```python
system.trace_after(insn_count=1000, events=["mem", "branch"], max=5000)
system.run(10_000_000)
```

---

## 6. Module Registration

All `#[pyclass]` and `#[pyfunction]` items are registered in the `#[pymodule]`:

```rust
// src/lib.rs

#[pymodule]
fn _helm_ng(py: Python, m: &PyModule) -> PyResult<()> {
    // SimObject hierarchy
    m.add_class::<SimObject>()?;
    m.add_class::<HelmSystem>()?;
    m.add_class::<Cpu>()?;
    m.add_class::<Ram>()?;
    m.add_class::<MemorySpace>()?;
    m.add_class::<Cache>()?;

    // Device classes
    m.add_class::<GicV2>()?;
    m.add_class::<Pl011>()?;
    m.add_class::<Sp804>()?;

    // Support classes
    m.add_class::<PortRef>()?;
    m.add_class::<MapEntry>()?;
    // Standalone observer (NOT a SimObject)
    m.add_class::<HelmSpy>()?;

    // Backward-compat factory
    m.add_function(wrap_pyfunction!(build_simulation_py, m)?)?;

    // DLD and introspection
    m.add_function(wrap_pyfunction!(load_dld_py, m)?)?;
    m.add_function(wrap_pyfunction!(list_devices_py, m)?)?;
    m.add_function(wrap_pyfunction!(device_schema_py, m)?)?;

    // Exceptions
    m.add("HelmError",           py.get_type::<exceptions::PyHelmError>())?;
    m.add("HelmConfigError",     py.get_type::<exceptions::PyHelmConfigError>())?;
    m.add("HelmMemFault",        py.get_type::<exceptions::PyHelmMemFault>())?;
    m.add("HelmDeviceError",     py.get_type::<exceptions::PyHelmDeviceError>())?;
    m.add("HelmCheckpointError", py.get_type::<exceptions::PyHelmCheckpointError>())?;

    Ok(())
}
```

### Python Package Re-exports

```python
# python/helm/__init__.py

from _helm_ng import (
    SimObject, System, Cpu, Ram, MemorySpace, Cache,
    GicV2, Pl011, Sp804,
    PortRef, HelmSpy,
    build_simulation, load_dld, list_devices, device_schema,
    HelmError, HelmConfigError, HelmMemFault, HelmDeviceError,
    HelmCheckpointError,
)

from helm.boards import *

__version__ = "0.2.0"

__all__ = [
    # SimObject hierarchy
    "SimObject", "System", "Cpu", "Ram", "MemorySpace", "Cache",
    # Devices
    "GicV2", "Pl011", "Sp804",
    # Support
    "PortRef", "HelmSpy",
    # Functions
    "build_simulation", "load_dld", "list_devices", "device_schema",
    # Exceptions
    "HelmError", "HelmConfigError", "HelmMemFault", "HelmDeviceError",
    "HelmCheckpointError",
]
```

---

*For SimObject class definitions, see [`LLD-sim-objects.md`](./LLD-sim-objects.md). For param types and wiring, see [`LLD-param-system.md`](./LLD-param-system.md). For tests, see [`TEST.md`](./TEST.md).*

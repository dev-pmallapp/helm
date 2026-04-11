# helm-python — LLD: SimObject Classes

> Low-level design for all `#[pyclass]` SimObject types and their Python-to-Rust mapping.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-param-system.md`](./LLD-param-system.md) · [`LLD-instantiate.md`](./LLD-instantiate.md)

---

## Table of Contents

1. [SimObject Base Class](#1-simobject-base-class)
2. [System](#2-system)
3. [Cpu](#3-cpu)
4. [Ram](#4-ram)
5. [MemorySpace](#5-memoryspace)
6. [GicV2](#6-gicv2)
7. [Pl011](#7-pl011)
8. [Sp804](#8-sp804)
9. [Cache](#9-cache)
10. [HelmSpy](#10-spysession)

---

## 1. SimObject Base Class

The base class for all simulatable components. Manages the child hierarchy and tracks instantiation state.

```rust
// src/simobject.rs

use pyo3::prelude::*;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq)]
pub enum SimObjectState {
    Pending,       // config phase — children/params mutable
    Instantiated,  // Rust objects created — mutations raise error
}

#[pyclass(subclass)]
pub struct SimObject {
    pub name: String,
    pub children: HashMap<String, PyObject>,
    pub state: SimObjectState,
}

#[pymethods]
impl SimObject {
    #[new]
    fn new(name: &str) -> Self {
        SimObject {
            name: name.to_string(),
            children: HashMap::new(),
            state: SimObjectState::Pending,
        }
    }

    #[getter]
    fn name(&self) -> &str { &self.name }

    #[getter]
    fn instantiated(&self) -> bool { self.state == SimObjectState::Instantiated }

    fn __setattr__(&mut self, py: Python, name: &str, value: PyObject) -> PyResult<()> {
        // If value is a SimObject subclass, store as child
        if value.extract::<PyRef<SimObject>>(py).is_ok() {
            self.require_pending()?;
            self.children.insert(name.to_string(), value);
            return Ok(());
        }
        // Otherwise, delegate to normal __setattr__
        Err(PyErr::new::<pyo3::exceptions::PyAttributeError, _>(
            format!("cannot set '{name}' on SimObject")
        ))
    }

    fn __getattr__(&self, py: Python, name: &str) -> PyResult<PyObject> {
        self.children.get(name)
            .cloned()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyAttributeError, _>(
                format!("'{}' has no child '{name}'", self.name)
            ))
    }
}

impl SimObject {
    pub fn require_pending(&self) -> PyResult<()> {
        if self.state == SimObjectState::Instantiated {
            return Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "cannot modify SimObject after instantiate()"
            ));
        }
        Ok(())
    }
}
```

### Child Assignment Protocol

Setting an attribute that is a `SimObject` subclass registers it as a child:

```python
system = helm.System("virt", timing="virtual", mode="fs", num_cpus=4, gic_version="v2")
system.cpu = helm.Cpu("cpu0", isa="aarch64")  # stored as child "cpu"
system.gic = helm.GicV2("gic0")               # stored as child "gic"
```

After `instantiate()`, child assignments raise `RuntimeError`.

---

## 2. System

Top-level container. Wraps `HelmSim` after instantiation.

```rust
// src/system.rs

#[pyclass(extends=SimObject)]
pub struct HelmSystem {
    // Config params — mutable before instantiate()
    #[pyo3(get, set)]
    pub timing: String,     // "virtual" | "interval" | "accurate"
    #[pyo3(get, set)]
    pub mode: String,       // "se" | "fe" | "fs"
    #[pyo3(get, set)]
    pub ipc: f64,           // instructions per cycle (for interval timing)

    // Live state — populated by instantiate()
    pub(crate) sim: Option<HelmSim>,
}

#[pymethods]
impl HelmSystem {
    #[new]
    #[pyo3(signature = (name, *, timing="virtual", mode="se", ipc=4.0))]
    fn new(name: &str, timing: &str, mode: &str, ipc: f64) -> (Self, SimObject) {
        (
            HelmSystem { timing: timing.into(), mode: mode.into(), ipc, sim: None },
            SimObject::new(name),
        )
    }

    /// Freeze config and create all Rust simulation objects.
    fn instantiate(slf: PyRefMut<Self>, py: Python) -> PyResult<()> {
        // See LLD-instantiate.md for full flow
        crate::instantiate::do_instantiate(slf, py)
    }

    /// Run simulation for up to max_insns instructions.
    /// Returns StopReason: "quantum", "exit:N", "exception:...", "unsupported".
    fn run(&mut self, max_insns: u64, py: Python) -> PyResult<String> {
        let sim = self.sim.as_mut().ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "run() called before instantiate()"
            )
        })?;
        let stop = py.allow_threads(|| sim.run(max_insns));
        Ok(stop.to_string())
    }

    /// Load ELF binary for SE mode.
    #[pyo3(signature = (binary, *, argv=None, envp=None))]
    fn load_elf(&mut self, binary: &str,
                argv: Option<Vec<String>>, envp: Option<Vec<String>>) -> PyResult<()> { ... }

    /// Load kernel for FS mode.
    #[pyo3(signature = (kernel, *, dtb, initrd=None, append=None))]
    fn load_kernel(&mut self, kernel: &str, dtb: &str,
                   initrd: Option<&str>, append: Option<&str>) -> PyResult<()> { ... }

    /// Install a built-in legacy plugin by name.
    /// Deprecated compatibility path: prefer observe()/HelmSpy-backed flows.
    fn add_plugin(&mut self, name: &str, args: Option<&str>) -> PyResult<()> { ... }

    // HelmSpy is standalone — created via helm.HelmSpy(system, ...), not system.spy().
    // See LLD-sim-objects.md Section 10 for the HelmSpy class definition.

    // ── Properties (live state after instantiate) ──

    #[getter]
    fn insn_count(&self) -> u64 { ... }

    #[getter]
    fn has_exited(&self) -> bool { ... }

    #[getter]
    fn exit_code(&self) -> i32 { ... }

    fn finish(&mut self) -> PyResult<()> { ... }
}
```

### Python Usage

```python
system = helm.System("virt", timing="virtual", mode="fs")
system.cpu = helm.Cpu("cpu0", isa="aarch64", model="cortex-a55")
system.instantiate()
system.load_kernel("Image", dtb="virt.dtb")
result = system.run(10_000_000)
```

---

## 3. Cpu

Wraps `Aarch64ArchState` (or `RiscvArchState`). Before instantiation, holds config params. After instantiation, provides register access.

```rust
// src/cpu.rs

#[pyclass(extends=SimObject)]
pub struct Cpu {
    // Config params
    #[pyo3(get, set)]
    pub isa: String,       // "aarch64" | "riscv64" | "aarch32"
    #[pyo3(get, set)]
    pub model: String,     // "cortex-a55" | "generic" | etc.
    #[pyo3(get, set)]
    pub width: u32,        // issue width hint; currently not consumed by runtime execution
    #[pyo3(get, set)]
    pub rob_size: u32,     // reserved reorder-buffer hint; currently no runtime effect
    #[pyo3(get, set)]
    pub iq_size: u32,      // reserved issue-queue hint; currently no runtime effect
    #[pyo3(get, set)]
    pub lq_size: u32,      // reserved load-queue hint; currently no runtime effect
    #[pyo3(get, set)]
    pub sq_size: u32,      // reserved store-queue hint; currently no runtime effect

    // Live state (set by instantiate, read by getters)
    pub(crate) arch_state: Option<Arc<Mutex<Aarch64ArchState>>>,
}

#[pymethods]
impl Cpu {
    #[new]
    #[pyo3(signature = (name, *, isa="aarch64", model="cortex-a55",
                        width=4, rob_size=128, iq_size=64, lq_size=32, sq_size=32))]
    fn new(name: &str, isa: &str, model: &str,
           width: u32, rob_size: u32, iq_size: u32,
           lq_size: u32, sq_size: u32) -> (Self, SimObject) {
        (
            Cpu {
                isa: isa.into(), model: model.into(),
                width, rob_size, iq_size, lq_size, sq_size,
                arch_state: None,
            },
            SimObject::new(name),
        )
    }

    // ── Post-instantiate register access ──

    #[getter]
    fn pc(&self) -> PyResult<u64> { ... }

    #[getter]
    fn sp(&self) -> PyResult<u64> { ... }

    #[getter]
    fn nzcv(&self) -> PyResult<u32> { ... }

    fn xn(&self, n: usize) -> PyResult<u64> { ... }

    fn vn(&self, n: usize) -> PyResult<(u64, u64)> { ... }
}
```

### Python Usage

```python
cpu = helm.Cpu("cpu0", isa="aarch64", model="cortex-a55")
cpu.width = 3
cpu.rob_size = 60

# After instantiate:
print(f"PC = {system.cpu.pc:#x}")
print(f"X0 = {system.cpu.xn(0):#x}")
```

---

## 4. Ram

Wraps `FlatMem`. Configuration: base address (assigned by MemorySpace, not Ram) and size.

```rust
// src/ram.rs

#[pyclass(extends=SimObject)]
pub struct Ram {
    #[pyo3(get, set)]
    pub size: String,      // "512MiB", "1GiB", etc. (parsed at instantiate)

    pub(crate) flat_mem: Option<Arc<Mutex<FlatMem>>>,
}

#[pymethods]
impl Ram {
    #[new]
    #[pyo3(signature = (name, *, size="512MiB"))]
    fn new(name: &str, size: &str) -> (Self, SimObject) {
        (Ram { size: size.into(), flat_mem: None }, SimObject::new(name))
    }
}
```

Ram intentionally has no `base` parameter — the memory map assigns its address (design rule: "device knows no base address").

---

## 5. MemorySpace

Wraps `HelmAddressSpace` (FlatMem + AddressMap + devices). Holds map entries before instantiation.

```rust
// src/memory_space.rs

#[pyclass]
pub struct MapEntry {
    pub base: u64,
    pub device: PyObject,     // reference to the SimObject being mapped
    pub size: u64,            // size in bytes
    pub bank: u32,            // register bank selector (Simics-style function number)
}

#[pyclass(extends=SimObject)]
pub struct MemorySpace {
    pub(crate) entries: Vec<MapEntry>,
    pub(crate) sys_mem: Option<Arc<Mutex<HelmAddressSpace>>>,
}

#[pymethods]
impl MemorySpace {
    #[new]
    fn new(name: &str) -> (Self, SimObject) {
        (MemorySpace { entries: vec![], sys_mem: None }, SimObject::new(name))
    }

    /// Add a device to the memory map.
    ///
    /// Args:
    ///   base: Physical address where the device is mapped.
    ///   device: SimObject to map (Ram, GicV2, Pl011, etc.).
    ///   size: Size of the mapping in bytes (or string like "1GiB").
    ///   bank: Register bank selector. Default 0.
    ///         GicV2 uses bank=0 for distributor, bank=1 for CPU interface.
    #[pyo3(signature = (base, device, size, *, bank=0))]
    fn add_map(&mut self, base: u64, device: PyObject, size: PyObject,
               bank: u32) -> PyResult<()> {
        // Parse size (int or string)
        let size_bytes = parse_size(size)?;
        self.entries.push(MapEntry { base, device, size: size_bytes, bank });
        Ok(())
    }
}
```

### Python Usage

```python
mem = helm.MemorySpace("phys_mem")
mem.add_map(0x4000_0000, ram,  "1GiB")
mem.add_map(0x0800_0000, gic,  0x1_0000, bank=0)   # distributor
mem.add_map(0x0801_0000, gic,  0x1_0000, bank=1)   # CPU interface
mem.add_map(0x0900_0000, uart, 0x1000)
```

---

## 6. GicV2

Wraps `GicDistributor` + `GicCpuInterface` + `GicState`. Provides `spi()` method for interrupt wiring.

```rust
// src/devices/gicv2.rs

#[pyclass(extends=SimObject)]
pub struct GicV2 {
    #[pyo3(get, set)]
    pub num_irqs: u32,

    pub(crate) gic_state: Option<Arc<Mutex<GicState>>>,
}

#[pymethods]
impl GicV2 {
    #[new]
    #[pyo3(signature = (name, *, num_irqs=96))]
    fn new(name: &str, num_irqs: u32) -> (Self, SimObject) {
        (GicV2 { num_irqs, gic_state: None }, SimObject::new(name))
    }

    /// Return a PortRef for SPI interrupt number `n`.
    /// Used for wiring: `uart.irq = gic.spi(33)`
    fn spi(&self, n: u32) -> PortRef {
        PortRef {
            target_name: self.name.clone(),  // resolved at instantiate
            port_name: format!("spi[{n}]"),
        }
    }

    // ── Post-instantiate getters ──

    #[getter]
    fn ctlr(&self) -> PyResult<u32> { ... }

    #[getter]
    fn pending_mask(&self) -> PyResult<u32> { ... }

    #[getter]
    fn enabled_mask(&self) -> PyResult<u32> { ... }
}
```

### Python Usage

```python
gic = helm.GicV2("gic0", num_irqs=96)
uart.irq = gic.spi(33)         # wire UART IRQ to SPI #33

# After instantiate:
print(f"GICD_CTLR = {system.gic.ctlr:#x}")
```

---

## 7. Pl011

Wraps the PL011 UART device. Has an `irq` port for interrupt wiring.

```rust
// src/devices/pl011.rs

#[pyclass(extends=SimObject)]
pub struct Pl011 {
    #[pyo3(get, set)]
    pub irq: Option<PortRef>,    // wired to GIC SPI

    pub(crate) device: Option<Arc<Mutex<helm_hw_char::Pl011>>>,
}

#[pymethods]
impl Pl011 {
    #[new]
    fn new(name: &str) -> (Self, SimObject) {
        (Pl011 { irq: None, device: None }, SimObject::new(name))
    }

    // ── Post-instantiate getters ──

    #[getter]
    fn tx_fifo_empty(&self) -> PyResult<bool> { ... }

    #[getter]
    fn rx_fifo_full(&self) -> PyResult<bool> { ... }
}
```

### Python Usage

```python
uart = helm.Pl011("uart0")
uart.irq = gic.spi(33)

# After instantiate:
print(f"TX FIFO empty: {system.uart.tx_fifo_empty}")
```

---

## 8. Sp804

Wraps the SP804 dual timer device.

```rust
// src/devices/sp804.rs

#[pyclass(extends=SimObject)]
pub struct Sp804 {
    #[pyo3(get, set)]
    pub irq: Option<PortRef>,

    pub(crate) device: Option<Arc<Mutex<helm_hw_timer::Sp804Timer>>>,
}

#[pymethods]
impl Sp804 {
    #[new]
    fn new(name: &str) -> (Self, SimObject) {
        (Sp804 { irq: None, device: None }, SimObject::new(name))
    }
}
```

---

## 9. Cache

Descriptor-only class — consumed by the timing model, no live Rust backing. Present because timing models need cache hierarchy descriptions.

```rust
// src/cache.rs

#[pyclass(extends=SimObject)]
pub struct Cache {
    #[pyo3(get, set)]
    pub size: String,       // "32KiB", "256KiB", "1MiB"
    #[pyo3(get, set)]
    pub assoc: u32,         // associativity
    #[pyo3(get, set)]
    pub latency: u32,       // hit latency in cycles
    #[pyo3(get, set)]
    pub line_size: u32,     // cache line size in bytes
}

#[pymethods]
impl Cache {
    #[new]
    #[pyo3(signature = (name, *, size="32KiB", assoc=8, latency=4, line_size=64))]
    fn new(name: &str, size: &str, assoc: u32,
           latency: u32, line_size: u32) -> (Self, SimObject) {
        (Cache { size: size.into(), assoc, latency, line_size }, SimObject::new(name))
    }
}
```

### Python Usage

```python
system.l1i = helm.Cache("l1i", size="32KiB", assoc=8, latency=4)
system.l1d = helm.Cache("l1d", size="32KiB", assoc=8, latency=4)
system.l2  = helm.Cache("l2",  size="256KiB", assoc=8, latency=12)
```

---

## 10. HelmSpy

Standalone observation session -- attaches to a System's probes independently.
Created as `helm.HelmSpy(system, ...)`, NOT via `system.spy()`.
Not part of the SimObject hierarchy.

```rust
// src/spy.rs

/// Standalone observation session -- attaches to a System's probes.
///
/// Created independently: `helm.HelmSpy(system, cache_l1d_size=32768, predictor="gshare")`
/// Not part of SimObject hierarchy.
#[pyclass]
pub struct HelmSpy {
    session: helm_spy::session::HelmSpy,
}

#[pymethods]
impl HelmSpy {
    /// Create a new observation session attached to the given system.
    ///
    /// Parameters
    /// ----------
    /// system : System
    ///     The system to observe. Must be instantiated.
    /// cache_l1d_size : int, optional
    ///     L1D cache size in bytes. No cache model if omitted.
    /// cache_l1d_ways : int
    ///     L1D set associativity (default 8).
    /// cache_l1d_line : int
    ///     Cache line size in bytes (default 64).
    /// predictor : str, optional
    ///     Branch predictor kind: "bimodal" or "gshare". No predictor if omitted.
    /// predictor_bits : int
    ///     Table index bits (default 10).
    #[new]
    #[pyo3(signature = (system, *, cache_l1d_size=None, cache_l1d_ways=8,
                        cache_l1d_line=64, predictor=None, predictor_bits=10,
                        predictor_table_bits=None))]
    fn new(system: &mut System, ...) -> PyResult<Self> {
        let sim = system.sim.as_mut().ok_or_else(|| ...)?;
        // Build HelmSpy, wire to probes
        Ok(HelmSpy { session })
    }

    /// Detach from the system's probes. Metrics are frozen at detach time.
    fn detach(&mut self) -> PyResult<()> { ... }

    #[getter]
    fn insn_count(&self) -> u64 { ... }

    #[getter]
    fn cache_hit_rate(&self) -> Option<f64> { ... }

    #[getter]
    fn branch_miss_rate(&self) -> Option<f64> { ... }

    fn insn_mix(&self) -> Vec<(String, u64, f64)> { ... }

    fn hot_pcs(&self, n: Option<usize>) -> Vec<(u64, u64)> { ... }

    fn snapshot(&self, py: Python) -> PyResult<PyObject> { ... }
}
```

### Python Usage

```python
system.instantiate()
system.load_elf("./hello", argv=["hello"])

spy = helm.HelmSpy(system, cache_l1d_size=32768, predictor="gshare")
system.run(10_000_000)

print(f"Insns: {spy.insn_count}")
print(f"L1D hit rate: {spy.cache_hit_rate:.2%}")
print(f"Branch miss rate: {spy.branch_miss_rate:.2%}")

spy.detach()  # freeze metrics, unsubscribe from probes
```

---

*For port wiring and memory map internals, see [`LLD-param-system.md`](./LLD-param-system.md). For the instantiate flow, see [`LLD-instantiate.md`](./LLD-instantiate.md). For tests, see [`TEST.md`](./TEST.md).*

# helm-python — LLD: Parameters, Ports, and Memory Maps

> Low-level design for parameter typing, port wiring, and memory map construction.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-sim-objects.md`](./LLD-sim-objects.md) · [`LLD-instantiate.md`](./LLD-instantiate.md)

---

## Table of Contents

1. [Design Overview](#1-design-overview)
2. [Rust-Native Parameters](#2-rust-native-parameters)
3. [PortRef — Connection Descriptors](#3-portref--connection-descriptors)
4. [MapEntry — Memory Map Descriptors](#4-mapentry--memory-map-descriptors)
5. [Size String Parsing](#5-size-string-parsing)
6. [Validation Strategy](#6-validation-strategy)

---

## 1. Design Overview

Unlike the previous design (which used Python-side `Param.*` descriptors), the new API uses **Rust struct fields as the sole parameter system**. Each field is marked `#[pyo3(get, set)]`, making it a Python property. Type checking happens in two places:

1. **At property-set time:** PyO3 automatically rejects wrong Python types (e.g., passing a list to a `u32` field). This produces immediate `TypeError` at the assignment line.
2. **At instantiate() time:** Semantic validation (ranges, cross-param consistency, size string parsing) happens in Rust.

This eliminates the Python `Param.*` descriptor layer entirely. Rust IS the type system.

### Comparison with Previous Design

| Aspect | Previous (Param descriptors) | New (Rust-native) |
|---|---|---|
| Type definition | Python class-level `Param.Int`, `Param.MemorySize` | Rust `#[pyo3(get, set)] pub field: u32` |
| Type checking | Python descriptor `__set__` | PyO3 automatic extraction |
| Range checking | Rust `elaborate()` | Rust `instantiate()` |
| Source of truth | Split (Python descriptors + Rust AttrValue) | Single (Rust struct) |
| Layers | 3 (Python descriptor → PyObject → AttrValue) | 1 (PyO3 property) |

---

## 2. Rust-Native Parameters

### Parameter Categories

| Type | Rust Type | Python Accepts | Example |
|---|---|---|---|
| Integer | `u32`, `u64`, `i64` | `int` | `num_irqs=96` |
| Float | `f64` | `float`, `int` | `ipc=4.0` |
| String | `String` | `str` | `isa="aarch64"` |
| Boolean | `bool` | `bool` | (future use) |
| Size string | `String` | `str` | `size="1GiB"` — parsed to bytes at instantiate() |
| Port | `Option<PortRef>` | `PortRef` or `None` | `irq=gic.spi(33)` |

### Example: Cpu Parameters

```rust
#[pyclass(extends=SimObject)]
pub struct Cpu {
    #[pyo3(get, set)]
    pub isa: String,       // "aarch64", "riscv64", "aarch32"
    #[pyo3(get, set)]
    pub model: String,     // "cortex-a55", "cortex-a73", "generic"
    #[pyo3(get, set)]
    pub width: u32,        // issue width (1-16)
    #[pyo3(get, set)]
    pub rob_size: u32,     // reorder buffer entries (0-512)
    #[pyo3(get, set)]
    pub iq_size: u32,      // instruction queue entries
    #[pyo3(get, set)]
    pub lq_size: u32,      // load queue entries
    #[pyo3(get, set)]
    pub sq_size: u32,      // store queue entries
}
```

Python usage:

```python
cpu = helm.Cpu("cpu0", isa="aarch64", model="cortex-a55")
cpu.width = 3         # OK — int → u32
cpu.width = "three"   # TypeError raised by PyO3 immediately
cpu.width = -1        # OverflowError — u32 rejects negative
```

### Post-Instantiate Properties

After `instantiate()`, some fields become read-only because they're backed by live Rust objects:

```rust
impl Cpu {
    #[getter]
    fn pc(&self) -> PyResult<u64> {
        let state = self.arch_state.as_ref()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "pc not available before instantiate()"
            ))?;
        Ok(state.lock().unwrap().pc)
    }
}
```

Config params (isa, model, width, etc.) remain readable but raise `RuntimeError` on write after instantiation.

---

## 3. PortRef — Connection Descriptors

`PortRef` is a lightweight descriptor that records a connection intent. It is **not** a live Rust reference — it's resolved into `Arc` refs during `instantiate()`.

### Rust Definition

```rust
// src/port.rs

#[pyclass]
#[derive(Clone)]
pub struct PortRef {
    /// Name of the target SimObject (resolved by walking the System's children)
    pub target_name: String,
    /// Port identifier on the target (e.g., "spi[33]", "timer[0]")
    pub port_name: String,
}

#[pymethods]
impl PortRef {
    fn __repr__(&self) -> String {
        format!("PortRef({}.{})", self.target_name, self.port_name)
    }
}
```

### How PortRefs Are Created

Devices with output ports provide methods that return `PortRef`:

```rust
// On GicV2:
fn spi(&self, n: u32) -> PortRef {
    PortRef {
        target_name: self.name.clone(),
        port_name: format!("spi[{n}]"),
    }
}
```

### How PortRefs Are Used

Devices with input ports store `Option<PortRef>`:

```rust
// On Pl011:
#[pyo3(get, set)]
pub irq: Option<PortRef>,
```

Python:

```python
uart.irq = gic.spi(33)     # stores PortRef("gic0", "spi[33]")
```

### Resolution at instantiate()

During `instantiate()`, the system walks all children, collects PortRefs, and resolves them:

```
PortRef("gic0", "spi[33]")
  → find child named "gic0" → get GicV2 → get Arc<GicState>
  → create GicSink(gic_state, intid=33)
  → wire into Pl011.irq_out
```

If a PortRef references a nonexistent child, `instantiate()` raises `HelmConfigError`.

---

## 4. MapEntry — Memory Map Descriptors

`MapEntry` records a memory mapping: which device is mapped at which base address with which bank number.

### Rust Definition

```rust
// src/port.rs

#[pyclass]
pub struct MapEntry {
    /// Base physical address in the memory space
    pub base: u64,
    /// Reference to the SimObject being mapped (Ram, GicV2, Pl011, etc.)
    pub device: PyObject,
    /// Size of the mapping in bytes
    pub size: u64,
    /// Register bank selector (Simics-style function number)
    /// GicV2: bank=0 → distributor, bank=1 → CPU interface
    pub bank: u32,
}
```

### How MapEntries Are Created

`MemorySpace.add_map()` creates entries:

```python
mem.add_map(0x4000_0000, ram,  "1GiB")              # bank=0 default
mem.add_map(0x0800_0000, gic,  0x1_0000, bank=0)    # GIC distributor
mem.add_map(0x0801_0000, gic,  0x1_0000, bank=1)    # GIC CPU interface
mem.add_map(0x0900_0000, uart, 0x1000)               # UART
```

### Resolution at instantiate()

During `instantiate()`, map entries are converted to `AddressMap` entries:

```
MapEntry(base=0x0900_0000, device=Pl011("uart0"), size=0x1000, bank=0)
  → find or create Pl011 Rust device
  → add_to_address_map(base=0x0900_0000, device_idx, size=0x1000)
```

### Bank Number Semantics

The `bank` field selects which register bank handles accesses to this mapping. This is inspired by Simics's `function` field in memory space map entries.

| Device | Bank 0 | Bank 1 |
|---|---|---|
| `GicV2` | Distributor (GICD) | CPU Interface (GICC) |
| `Sp804` | Timer 1 | Timer 2 |
| `Ram` | (always 0) | — |
| `Pl011` | (always 0) | — |

For devices with a single register bank, `bank` is always 0 (the default).

### Overlap Detection

At `instantiate()` time, overlapping map entries are detected and raise `HelmConfigError`:

```python
mem.add_map(0x1000, ram, 0x2000)
mem.add_map(0x1800, uart, 0x100)   # overlaps with ram
system.instantiate()                # HelmConfigError: overlapping mappings
```

---

## 5. Size String Parsing

Size strings (e.g., `"1GiB"`, `"32KiB"`, `"512MiB"`) are parsed to byte counts at `instantiate()` time.

### Accepted Formats

| Input | Parsed Value |
|---|---|
| `"32KiB"` / `"32KB"` / `"32 KiB"` | 32,768 |
| `"256MiB"` / `"256MB"` | 268,435,456 |
| `"1GiB"` / `"1GB"` | 1,073,741,824 |
| `"32768"` (decimal string) | 32,768 |
| `32768` (int — via Python) | 32,768 |

### Implementation

```rust
// src/port.rs (or src/util.rs)

pub fn parse_size(value: &PyAny) -> PyResult<u64> {
    if let Ok(n) = value.extract::<u64>() {
        return Ok(n);
    }
    let s: String = value.extract()?;
    parse_size_str(&s)
        .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
            format!("cannot parse size: {s:?}")
        ))
}

fn parse_size_str(s: &str) -> Option<u64> {
    let s = s.trim();
    let suffixes = [
        ("tib", 1u64 << 40), ("tb", 1u64 << 40),
        ("gib", 1u64 << 30), ("gb", 1u64 << 30),
        ("mib", 1u64 << 20), ("mb", 1u64 << 20),
        ("kib", 1u64 << 10), ("kb", 1u64 << 10),
        ("b", 1),
    ];
    let lower = s.to_lowercase();
    for (suffix, mult) in &suffixes {
        if lower.ends_with(suffix) {
            let num = s[..s.len() - suffix.len()].trim();
            let n: f64 = num.parse().ok()?;
            return Some((n * *mult as f64) as u64);
        }
    }
    s.parse::<u64>().ok()
}
```

---

## 6. Validation Strategy

### At Property-Set Time (automatic via PyO3)

| Check | How | Error |
|---|---|---|
| Wrong Python type | PyO3 extraction fails | `TypeError` |
| Negative on `u32`/`u64` | PyO3 overflow check | `OverflowError` |

### At instantiate() Time (explicit Rust checks)

| Check | What | Error |
|---|---|---|
| Unknown ISA string | `isa` not in `["aarch64", "riscv64", "aarch32"]` | `HelmConfigError` |
| Unknown timing | `timing` not in `["virtual", "interval", "accurate"]` | `HelmConfigError` |
| Unknown CPU model | `model` not recognized | `HelmConfigError` |
| Size parse failure | `Ram.size` is not a valid size string | `HelmConfigError` |
| Overlapping maps | Two map entries overlap in address space | `HelmConfigError` |
| Unresolved PortRef | `uart.irq` references nonexistent child | `HelmConfigError` |
| Missing required child | FS mode requires `cpu`, `mem`, and at least one RAM | `HelmConfigError` |
| Cache size not power-of-two | (when timing model consumes cache descriptor) | `HelmConfigError` |

### Example Error Messages

```
HelmConfigError: isa 'mips' is not supported (valid: aarch64, riscv64, aarch32)
HelmConfigError: overlapping memory map entries: ram0@0x4000_0000-0x8000_0000 and gic0@0x7FFF_0000-0x8000_0000
HelmConfigError: unresolved port: uart0.irq references 'gic0.spi[33]' but no child 'gic0' exists
HelmConfigError: FS mode requires a Cpu child on System
```

---

*For SimObject class definitions, see [`LLD-sim-objects.md`](./LLD-sim-objects.md). For the instantiate flow, see [`LLD-instantiate.md`](./LLD-instantiate.md). For tests, see [`TEST.md`](./TEST.md).*

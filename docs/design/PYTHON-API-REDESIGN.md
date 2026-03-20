# Python API Redesign: First-Class SimObject Hierarchy

*Design document for helm-ng Python API v2 — inspired by gem5 and Simics*

---

## 1. Problems with the Current API

### Problem 1: Disconnected Python Config Classes

The Python classes (`Platform`, `Core`, `Cache`, `MemorySystem`) exist in `python/helm/` but produce dicts via `to_dict()` that **nobody consumes**. They aren't wired to Rust. The actual simulator is built by `build_simulation()` which takes flat string/int arguments:

```python
# Current: flat factory, config classes are dead code
sim = helm.build_simulation(isa="aarch64", mode="fs", timing="virtual", mem_mib=1024)
sim.load_kernel("Image", "virt.dtb", "initrd")
```

The `Platform(cores=[Core(...)], memory=MemorySystem(...))` hierarchy is never consumed by Rust.

### Problem 2: Monolithic Simulation Object

Everything — CPU state, memory, devices, plugins, symbols — lives on a single `Simulation` PyO3 class with 30+ methods. There's no way to:
- Access a device directly from Python (e.g. read UART registers)
- Create or configure devices individually
- Wire devices together
- Inspect the memory map

### Problem 3: Hardcoded Platform Wiring

The ARM virt platform is hardcoded in Rust (`platform/arm_virt.rs`). Python can't:
- Define a custom memory map
- Add/remove devices
- Change interrupt routing
- Create a new platform without writing Rust

### Problem 4: No 1:1 Type Mapping

In gem5, every Python class (`TimingSimpleCPU`, `DDR3_1600`, `Pl011`) has a 1:1 C++ backing class. In helm-ng, Rust types like `FlatMem`, `Pl011`, `GicDistributor`, `Aarch64ArchState` have **no Python presence** — they're hidden inside the `Simulation` facade.

---

## 2. Lessons from gem5

### What to Adopt

| Pattern | gem5 | helm-ng Adaptation |
|---------|------|-------------------|
| **SimObject base class** | Python metaclass + C++ | `#[pyclass(subclass)]` `SimObject` in Rust, extended by all objects |
| **Typed Params** | `Param.Latency("1ns")`, `Param.Unsigned(64)` | Rust struct fields exposed as `#[pyo3(get, set)]` properties |
| **Port wiring** | `cpu.icache_port = bus.cpu_side` | Python assignment stores connection descriptors, resolved at instantiate() |
| **Two-phase construction** | Python tree → `m5.instantiate()` → C++ objects | Python tree → `system.instantiate()` → Rust objects |
| **Device isolation** | `Pl011(int_num=37)` but platform sets the address | Device declares params; platform maps them |
| **Object hierarchy** | `system.cpu`, `system.membus`, `system.mem_ctrl` | Same: `system.cpu[0]`, `system.gic`, `system.uart[0]` |

### What to Skip

| Pattern | Why Skip |
|---------|----------|
| **MetaSimObject metaclass** | PyO3 `#[pyclass]` is compiled Rust — can't use Python metaclass magic. Registration happens at compile time via proc macros, not runtime metaclass. |
| **SCons code generation** | gem5 generates `Params` structs from `.py` files. PyO3 derives directly from Rust structs. No code generation needed. |
| **50+ Param types** | gem5 has `Param.Latency`, `Param.Frequency`, `Param.MemorySize`, etc. Start with 5: `int`, `str`, `float`, `bool`, `SimObject`. Add domain types later. |
| **`allClasses` global registry** | gem5 maintains a global dict of all SimObject classes. PyO3 modules have explicit registration. Use Python's normal import system. |
| **Proxy objects (`Parent.any`)** | gem5 resolves `Parent.any` by walking the tree. Over-engineering for helm-ng's scale. Explicit references are clearer. |

---

## 3. Lessons from Simics

### What to Adopt

| Pattern | Simics | helm-ng Adaptation |
|---------|--------|-------------------|
| **Attribute flags** | `Sim_Attr_Required`, `Optional`, `Session`, `Pseudo` | Map to `AttrDescriptor` — Required (must-set), Optional (default), Session (non-checkpointed) |
| **`pre_conf_object` batch construction** | Create placeholders → `SIM_add_configuration()` → live objects | Python `SimObject()` creates descriptor → `instantiate()` → Rust objects |
| **Interface as trait** | C struct of function pointers registered on class | Rust `Device` trait exposed via `#[pymethods]` on each device's pyclass |
| **Bank / function numbers** | `memory_space.map` entries have `function` field selecting device register bank | `MemoryMap.add(base, device, bank=0)` — GICv2 uses bank 0 (dist) and bank 1 (cpuif) |
| **Connect = typed outgoing reference** | DML `connect irq { interface signal; }` | Rust `Connect<dyn Signal>` field, set from Python as `device.irq = gic.input[33]` |

### What to Skip

| Pattern | Why Skip |
|---------|----------|
| **DML** | Simics needs DML because writing devices in C is tedious. Rust's type system + derive macros provide equivalent ergonomics. |
| **No elaboration phase** | Simics allows hot-add at any time. helm-ng's `init→elaborate→startup` lifecycle is better for Rust's borrow checker and determinism. |
| **Flat conf namespace** | `conf.board.mb.phys_mem` with string-based lookup. Use typed Python objects with compile-time-checked Rust backing. |
| **C function pointer interfaces** | Rust traits are strictly better: type-safe, lifetime-aware, zero-cost. |

---

## 4. Proposed API Design

### 4.1 SimObject Base Class

Every simulatable component inherits from `SimObject`. This is a `#[pyclass(subclass)]` in Rust:

```rust
// In helm-python/src/simobject.rs
#[pyclass(subclass)]
pub struct SimObject {
    name: String,
    children: HashMap<String, PyObject>,  // child SimObjects
    state: SimObjectState,                // Pending | Instantiated
}

#[pymethods]
impl SimObject {
    #[new]
    fn new(name: &str) -> Self { ... }

    #[getter]
    fn name(&self) -> &str { &self.name }

    fn add_child(&mut self, name: &str, child: PyObject) { ... }
}
```

### 4.2 Concrete PyO3 Classes (1:1 with Rust)

Each major Rust type gets its own `#[pyclass(extends=SimObject)]`:

```
Python Class           Rust Backing Type           Crate
─────────────────────  ──────────────────────────  ──────────────
SimObject              (base)                      helm-python
System                 HelmEngine<T> (via HelmSim) helm-engine
Cpu                    Aarch64ArchState            helm-arch
MemorySpace            SystemMem / FlatMem         helm-engine
Ram                    FlatMem                     helm-engine
MemoryMap              AddressMap                  helm-engine
GicV2                  GicDistributor + GicCpuIf   helm-hw-intc
Pl011                  Pl011                       helm-hw-char
Sp804                  Sp804Timer                  helm-hw-timer
GenericTimer           (arch timer in ArchState)   helm-arch
Cache                  (descriptor only, no Rust)  helm-python
Board                  (composition, no Rust)      helm-python
```

### 4.3 Typed Parameters

Instead of gem5's 50+ Param types, use Rust struct fields exposed as Python properties. Type checking happens in Rust:

```rust
#[pyclass(extends=SimObject)]
pub struct Cpu {
    #[pyo3(get, set)]
    isa: String,            // "aarch64", "riscv64"
    #[pyo3(get, set)]
    model: String,          // "cortex-a55", "generic"
    #[pyo3(get, set)]
    width: u32,             // issue width
    #[pyo3(get, set)]
    rob_size: u32,          // reorder buffer entries
    #[pyo3(get, set)]
    iq_size: u32,           // instruction queue
    #[pyo3(get, set)]
    lq_size: u32,           // load queue
    #[pyo3(get, set)]
    sq_size: u32,           // store queue
}
```

Python usage — looks like gem5:

```python
cpu = helm.Cpu("cpu0", isa="aarch64", model="cortex-a55")
cpu.width = 4
cpu.rob_size = 128
```

### 4.4 Port-Based Wiring

Inspired by gem5's port assignments and Simics's connects. Connections are descriptors stored during configuration, resolved during `instantiate()`:

```rust
// In Rust: a connection descriptor (not yet resolved)
pub struct PortRef {
    target_name: String,     // target SimObject name
    port_name: String,       // port on target (e.g., "input[33]")
}

#[pyclass(extends=SimObject)]
pub struct Pl011 {
    #[pyo3(get, set)]
    irq: Option<PortRef>,    // connects to GIC input port
    // ... other fields
}
```

Python usage:

```python
uart = helm.Pl011("uart0")
gic  = helm.GicV2("gic0", num_irqs=96)

# Port wiring — stores descriptor, resolved at instantiate()
uart.irq = gic.spi(33)      # SPI #33 (INTID 33)
```

### 4.5 Memory Map

Inspired by Simics's `memory_space.map` attribute:

```python
mem = helm.MemorySpace("phys_mem")
ram = helm.Ram("ram0", size="1GiB")

# Each entry: (base_addr, object, size)
# Device knows no base address — the memory map assigns it
mem.add_map(0x4000_0000, ram,  "1GiB")
mem.add_map(0x0800_0000, gic,  0x1_0000, bank=0)   # distributor
mem.add_map(0x0801_0000, gic,  0x1_0000, bank=1)   # CPU interface
mem.add_map(0x0900_0000, uart, 0x1000)
```

### 4.6 System — Top-Level Container

```rust
#[pyclass(extends=SimObject)]
pub struct System {
    #[pyo3(get, set)]
    timing: String,          // "virtual", "interval", "accurate"
    #[pyo3(get, set)]
    mem_mode: String,        // "se", "fe", "fs"
    // ... internal: holds HelmSim after instantiate()
    sim: Option<HelmSim>,
}
```

### 4.7 Two-Phase Construction

Like gem5's `m5.instantiate()`:

```python
import helm

# ── Phase 1: Describe (Python objects, no Rust simulation yet) ──

system = helm.System("virt", timing="virtual", mode="fs")

# Create components
system.cpu  = helm.Cpu("cpu0", isa="aarch64", model="cortex-a55")
system.gic  = helm.GicV2("gic0", num_irqs=96)
system.uart = helm.Pl011("uart0")
system.ram  = helm.Ram("ram0", size="1GiB")
system.mem  = helm.MemorySpace("phys_mem")

# Wire memory map
system.mem.add_map(0x4000_0000, system.ram,  "1GiB")
system.mem.add_map(0x0800_0000, system.gic,  0x1_0000, bank=0)
system.mem.add_map(0x0801_0000, system.gic,  0x1_0000, bank=1)
system.mem.add_map(0x0900_0000, system.uart,  0x1000)

# Wire interrupts
system.uart.irq = system.gic.spi(33)

# ── Phase 2: Instantiate (creates Rust objects, config frozen) ──

system.instantiate()

# ── Phase 3: Load and Run ──

system.load_kernel("Image", dtb="virt.dtb", initrd="initrd.cpio")

while True:
    result = system.run(10_000_000)
    if result != "quantum":
        break
```

### 4.8 SE Mode (Simpler)

```python
import helm

system = helm.System("se", timing="virtual", mode="se")
system.cpu = helm.Cpu("cpu0", isa="aarch64")
system.ram = helm.Ram("ram0", size="512MiB")

system.instantiate()
system.load_elf("./hello", argv=["hello"])

while not system.has_exited:
    system.run(10_000_000)

print(f"exit {system.exit_code} after {system.insn_count:,} insns")
```

---

## 5. Complete Before/After Comparison

### BEFORE: Current API (FS mode)

```python
import _helm_ng

sim = _helm_ng.build_simulation(
    isa="aarch64", mode="fs", timing="virtual", mem_mib=1024
)
sim.set_cpu_model("cortex-a55")
sim.load_kernel("Image", "virt.dtb", "initrd.cpio", "console=ttyAMA0")

# Can't see devices, can't access GIC, can't change memory map
# Everything is hidden inside `sim`

while remaining > 0:
    result = sim.run(10_000_000)
    remaining -= 10_000_000
```

Problems:
- No visibility into the system composition
- Can't create custom platforms without writing Rust
- Devices hardcoded in `build_arm_virt()`
- `sim` is a god object

### AFTER: Redesigned API (FS mode)

```python
import helm

# Every component is a first-class Python object
system = helm.System("virt", timing="virtual", mode="fs")

# CPU — maps 1:1 to Aarch64ArchState + ArmCoreModel
cpu = helm.Cpu("cpu0", isa="aarch64", model="cortex-a55")
cpu.width = 4
cpu.rob_size = 128

# Memory — maps 1:1 to FlatMem / SystemMem
ram = helm.Ram("ram0", size="1GiB")

# Devices — each maps 1:1 to its Rust type
gic  = helm.GicV2("gic0", num_irqs=96)
uart = helm.Pl011("uart0")

# Memory map — maps 1:1 to AddressMap
mem = helm.MemorySpace("phys_mem")
mem.add_map(0x4000_0000, ram,   "1GiB")
mem.add_map(0x0800_0000, gic,   0x1_0000, bank=0)  # distributor
mem.add_map(0x0801_0000, gic,   0x1_0000, bank=1)  # cpu interface
mem.add_map(0x0900_0000, uart,  0x1000)

# Interrupt wiring — device knows no IRQ number
uart.irq = gic.spi(33)

# Compose system
system.cpu  = cpu
system.mem  = mem

# Freeze config, create Rust objects
system.instantiate()

# Load and run
system.load_kernel("Image", dtb="virt.dtb", initrd="initrd.cpio",
                   append="console=ttyAMA0")

while True:
    result = system.run(10_000_000)
    if result != "quantum":
        break

# Post-run: access components directly
print(f"PC = {system.cpu.pc:#x}")
print(f"UART TX bytes = {system.uart.tx_count}")
print(f"GIC pending = {system.gic.pending_mask:#x}")
```

Benefits:
- Every component visible and inspectable from Python
- Custom platforms by composing objects — no Rust changes needed
- Memory map declared in Python — devices are address-agnostic
- Interrupt routing declared in Python
- Post-simulation introspection on individual devices

---

## 6. Python-to-Rust Type Mapping Table

| Python Class | Rust Backing Type | PyO3 Strategy | Crate |
|---|---|---|---|
| `SimObject` | (base, no Rust type) | `#[pyclass(subclass)]` | helm-python |
| `System` | `HelmSim` (enum over `HelmEngine<T>`) | `#[pyclass(extends=SimObject)]` | helm-python |
| `Cpu` | `Aarch64ArchState` / `RiscvArchState` | `#[pyclass(extends=SimObject)]` | helm-python |
| `Ram` | `FlatMem` | `#[pyclass(extends=SimObject)]` | helm-python |
| `MemorySpace` | `SystemMem` (FlatMem + AddressMap + devices) | `#[pyclass(extends=SimObject)]` | helm-python |
| `GicV2` | `GicDistributor` + `GicCpuInterface` + `GicState` | `#[pyclass(extends=SimObject)]` | helm-python |
| `Pl011` | `Pl011` | `#[pyclass(extends=SimObject)]` | helm-python |
| `Sp804` | `Sp804Timer` | `#[pyclass(extends=SimObject)]` | helm-python |
| `Pl031` | `Pl031Rtc` | `#[pyclass(extends=SimObject)]` | helm-python |
| `Cache` | (descriptor only until timing model consumes it) | `#[pyclass(extends=SimObject)]` | helm-python |
| `Board` | (pure composition — sugar for System + devices) | Pure Python class | python/helm/ |

### How `instantiate()` Works

```
Python Phase                              Rust Phase
────────────────────                      ──────────────────────
System("virt")                            (nothing yet)
  .cpu = Cpu("cpu0")                      (nothing yet)
  .mem = MemorySpace("phys_mem")          (nothing yet)
    .add_map(0x4000_0000, ram, "1GiB")    (stores descriptor)
    .add_map(0x0900_0000, uart, 0x1000)   (stores descriptor)
  uart.irq = gic.spi(33)                 (stores PortRef)

system.instantiate()          ─────────>  1. Create HelmEngine<T> based on timing
                                          2. Create FlatMem(base, size) for Ram
                                          3. Create AddressMap from map descriptors
                                          4. Create device instances (Pl011, GicDistributor...)
                                          5. Build SystemMem from FlatMem + AddressMap + devices
                                          6. Resolve PortRefs → wire Arc<GicState> into Pl011.irq_out
                                          7. Call init() → elaborate(system) → startup()
                                          8. Freeze config — system.instantiated = true
```

After `instantiate()`:
- Python objects become **handles** to live Rust objects
- Property getters/setters on `Cpu`, `GicV2`, etc. read/write the Rust state directly
- `system.run()` delegates to `HelmSim::run()`
- Adding new children or changing the memory map raises an error

### Preserving Timing Monomorphization

The `System` pyclass internally holds a `HelmSim` enum, exactly as today:

```rust
#[pyclass(extends=SimObject)]
pub struct System {
    timing: String,
    mode: String,
    // After instantiate():
    sim: Option<HelmSim>,  // HelmSim::Virtual | Interval | Accurate
}
```

`instantiate()` selects the timing variant and constructs `HelmEngine<Virtual>`, `HelmEngine<Interval>`, or `HelmEngine<Accurate>`. The monomorphization is preserved — timing is inlined in the hot path, not vtable-dispatched.

---

## 7. Pre-Built Boards (High-Level API)

For users who don't want to wire every device, provide pre-built boards — like gem5's stdlib:

```python
import helm
from helm.boards import ArmVirt

# One line: creates System + Cpu + GIC + UART + RAM + MemorySpace, all wired
system = ArmVirt(mem="1GiB", cpu_model="cortex-a55")
system.instantiate()
system.load_kernel("Image", dtb="virt.dtb")
system.run(10_000_000_000)
```

`ArmVirt` is a **pure Python** class in `python/helm/boards/arm_virt.py` that creates and wires all the components. It replaces the hardcoded Rust `build_arm_virt()`. This is analogous to gem5's `SimpleBoard` and Simics's component system.

```python
# python/helm/boards/arm_virt.py
class ArmVirt:
    """QEMU-compatible ARM virt platform."""

    GIC_DIST = 0x0800_0000
    GIC_CPUIF = 0x0801_0000
    UART0 = 0x0900_0000
    RAM_BASE = 0x4000_0000

    def __init__(self, mem="512MiB", cpu_model="cortex-a55", timing="virtual"):
        self.system = helm.System("virt", timing=timing, mode="fs")
        self.system.cpu = helm.Cpu("cpu0", isa="aarch64", model=cpu_model)
        self.system.gic = helm.GicV2("gic0", num_irqs=96)
        self.system.uart = helm.Pl011("uart0")
        self.system.ram = helm.Ram("ram0", size=mem)

        self.system.mem = helm.MemorySpace("phys_mem")
        self.system.mem.add_map(self.RAM_BASE,  self.system.ram,  mem)
        self.system.mem.add_map(self.GIC_DIST,  self.system.gic,  0x1_0000, bank=0)
        self.system.mem.add_map(self.GIC_CPUIF, self.system.gic,  0x1_0000, bank=1)
        self.system.mem.add_map(self.UART0,     self.system.uart, 0x1000)

        self.system.uart.irq = self.system.gic.spi(33)

    def instantiate(self):
        self.system.instantiate()

    def load_kernel(self, kernel, **kwargs):
        self.system.load_kernel(kernel, **kwargs)

    def run(self, n):
        return self.system.run(n)
```

---

## 8. Device Introspection After Instantiation

After `instantiate()`, Python objects become live handles. Properties read/write through to Rust:

```python
system.instantiate()

# CPU state — reads Aarch64ArchState directly
print(f"PC:   {system.cpu.pc:#x}")
print(f"SP:   {system.cpu.sp:#x}")
print(f"X0:   {system.cpu.xn(0):#x}")
print(f"NZCV: {system.cpu.nzcv:#010b}")

# GIC state — reads GicState directly
print(f"GICD_CTLR:   {system.gic.ctlr:#x}")
print(f"Pending:     {system.gic.pending_mask:#x}")
print(f"Enabled:     {system.gic.enabled_mask:#x}")

# UART state — reads Pl011 directly
print(f"UART TXFE:   {system.uart.tx_fifo_empty}")
print(f"UART RXFF:   {system.uart.rx_fifo_full}")
```

---

## 9. Summary of Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Base class mechanism | `#[pyclass(subclass)]` SimObject | PyO3 native, no metaclass magic needed |
| Param types | Rust struct fields as `#[pyo3(get, set)]` | Type-checked at compile time in Rust, clean Python properties |
| Port wiring | `device.irq = gic.spi(N)` stores PortRef | Resolved at `instantiate()` into `Arc` refs. Keeps config phase borrow-free |
| Memory map | `MemorySpace.add_map(base, obj, size, bank)` | Device stays address-agnostic. Maps to AddressMap entries |
| Two-phase | `instantiate()` freezes config | Mirrors gem5's `m5.instantiate()`. Config mutations after freeze → error |
| Pre-built boards | Pure Python classes in `python/helm/boards/` | Replaces hardcoded Rust platform code. Users can subclass/customize |
| Timing | `System(timing="virtual")` selects `HelmEngine<T>` variant | Monomorphization preserved exactly as today |
| Device hierarchy | Flat on System: `system.uart`, `system.gic` | Simpler than gem5's deep nesting. Can add hierarchy later |
| Post-instantiate access | Properties delegate to Rust objects | Inspection/debugging without breaking the hot path |
| Backward compat | Keep `build_simulation()` as sugar | Internally creates System + components + calls instantiate() |

---

## 10. Migration Path

### Phase A: SimObject Infrastructure
- Add `SimObject` base pyclass to `helm-python`
- Add `System`, `Cpu`, `Ram`, `MemorySpace` pyclasses
- `instantiate()` delegates to existing `build_simulator()`

### Phase B: Device Pyclasses
- Add `GicV2`, `Pl011` pyclasses wrapping existing Rust device structs
- Add `MemorySpace.add_map()` to replace hardcoded address map
- Add port wiring (`device.irq = gic.spi(N)`)

### Phase C: Platform in Python
- Move `build_arm_virt()` logic to `python/helm/boards/arm_virt.py`
- Rust `build_arm_virt()` becomes internal helper or is removed
- Python fully controls platform composition

### Phase D: Post-Instantiate Introspection
- Device properties read live Rust state
- CPU register access through `system.cpu.xn(N)` etc.
- GIC/UART state inspection

### Keep Forever
- `build_simulation()` remains as a convenience wrapper
- `sim.run()`, `sim.load_elf()`, `sim.load_kernel()` remain
- All existing Python scripts continue to work

# QEMU QOM & QMP: Deep Technical Reference for Helm Design

> Research into QEMU's object model (QOM) and machine protocol (QMP).
> Each section ends with explicit Helm design choices derived from the findings.

---

## Part 1: QOM — QEMU Object Model

### 1.1 Type System

#### `TypeInfo` Struct — All Fields

```c
struct TypeInfo {
    const char        *name;
    const char        *parent;
    size_t             instance_size;       // sizeof(MyState)
    size_t             instance_align;
    void (*instance_init)(Object *obj);     // infallible constructor
    void (*instance_post_init)(Object *obj);
    void (*instance_finalize)(Object *obj); // destructor
    bool               abstract;            // true = cannot instantiate directly
    size_t             class_size;          // sizeof(MyClass)
    void (*class_init)(ObjectClass *klass, const void *data);  // vtable setup
    void (*class_base_init)(ObjectClass *klass, const void *data); // before class_init
    const void        *class_data;
    const InterfaceInfo *interfaces;        // NULL-terminated
};
```

`class_base_init` runs after parent class is `memcpy`'d into child class — correct place to null-out inherited pointers the subtype doesn't want.

#### Self-Registration via `type_init`

```c
static void my_device_register(void) { type_register_static(&my_device_info); }
type_init(my_device_register)
// expands to __attribute__((constructor)) — runs before main()
```

#### Object Lifecycle

```
object_new()         → alloc + instance_init; refcount=1
object_ref()         → refcount++
object_unref()       → refcount--; calls instance_finalize at 0
```

Lazy class initialization: `type_initialize()` called on first `object_new()` of a type.

#### Class Hierarchy

```
ObjectClass         ← TYPE_OBJECT root
  └─ DeviceClass    ← .realize, .unrealize, .reset, .props
       └─ SysBusDeviceClass
            └─ MyDeviceClass

Object
  └─ DeviceState    ← .realized, .parent_bus, ResettableState
       └─ SysBusDevice  ← .mmio[], .irq[]
            └─ MyDevice
```

**First field must be parent type** — C guarantees first member at offset 0, enabling safe casts.

---

### 1.2 Properties

#### Key `DEFINE_PROP_*` Macros

```c
DEFINE_PROP_UINT32  ("fifo-depth", MyState, fifo_depth, 16)
DEFINE_PROP_UINT64  ("dma-mask",   MyState, dma_mask,   0xFFFFFFFF)
DEFINE_PROP_BOOL    ("big-endian", MyState, big_endian, false)
DEFINE_PROP_STRING  ("label",      MyState, label)
DEFINE_PROP_LINK    ("memory",     MyState, mr, TYPE_MEMORY_REGION, MemoryRegion *)
DEFINE_PROP_CHR     ("chardev",    MyState, chr)
DEFINE_PROP_NETDEV  ("netdev",     MyState, netdev)
DEFINE_PROP_DRIVE   ("drive",      MyState, blk)
DEFINE_PROP_ENUM    ("tx-mode",    MyState, tx_mode, DEFAULT, EnumLookup)
DEFINE_PROP_END_OF_LIST()
```

#### How Properties Work

`device_class_set_props(dc, my_device_props)` registers each prop as a QOM object property during `instance_init`. Properties are set from CLI (`-device my-dev,fifo-depth=32`) before `realize()`. After `realize()` succeeds, `DEFINE_PROP_*` properties become read-only.

#### `DEFINE_PROP_LINK` — Typed Object References

Type-checked at set time via `object_dynamic_cast(target, TYPE_MEMORY_REGION)`. Calls `object_ref()` on set, `object_unref()` on release. The Rust equivalent: `Arc<dyn MemoryRegion>` with trait bound enforcing the protocol.

---

### 1.3 Device Lifecycle: realize / unrealize

#### Why `realize()` Exists vs. `instance_init()`

| Phase | `instance_init()` | `realize()` |
|-------|-------------------|-------------|
| Callsite | On `object_new()` AND during introspection queries | Only when device is actually used |
| Failures | Must not fail | Receives `Error **errp`, may fail |
| Resources | Must not acquire teardown-requiring resources | Correct place for IRQ alloc, MMIO map, backend open |
| Purpose | Register properties for introspection | Bring device into service |

QEMU creates temporary unregistered instances for `-device my-dev,help` introspection. A `realize()` that opens files or registers IRQs would break this.

#### Canonical Instantiation Pattern (modern)

```c
DeviceState *dev = qdev_new(TYPE_MY_DEVICE);
qdev_prop_set_uint32(dev, "fifo-depth", 64);
sysbus_realize_and_unref(SYS_BUS_DEVICE(dev), &error_fatal);
```

#### Reset Protocol — Resettable Interface

Three-phase reset (not a direct `DeviceClass::reset` call):

```c
resettable_assert_reset(obj, RESET_TYPE_COLD);  // freeze I/O, set reset values
resettable_release_reset(obj, RESET_TYPE_COLD); // restart clocks, start DMA
// Or atomically:
resettable_reset(obj, RESET_TYPE_COLD);
```

`BusClass.child_foreach` cascades reset through the qbus tree. Devices not on the bus tree (e.g. some CPUs) are NOT auto-reset.

---

### 1.4 Casting and Type Safety

```c
// Modern (preferred)
OBJECT_DECLARE_SIMPLE_TYPE(MyDevice, MY_DEVICE)
// → generates: MY_DEVICE(obj), MY_DEVICE_GET_CLASS(obj), MY_DEVICE_CLASS(oc)

// Runtime dynamic cast
Object *result = object_dynamic_cast(obj, TYPE_SYS_BUS_DEVICE);
// Returns NULL if: obj is-not-a typename, or interface ambiguous

// TYPE_* string convention: kebab-case, globally unique
#define TYPE_MY_DEVICE "my-device"
```

---

### 1.5 QOM Interfaces

Stateless multiple inheritance via `InterfaceClass`. An interface carries only virtual method function pointers, never instance state.

```c
static const TypeInfo my_device_info = {
    .name      = TYPE_MY_DEVICE,
    .parent    = TYPE_DEVICE,
    .interfaces = (InterfaceInfo[]) {
        { TYPE_RESETTABLE_INTERFACE },
        { }  // sentinel
    },
};
```

`object_dynamic_cast` handles interface resolution — walks class hierarchy.

---

### 1.6 QOM Design Rationale

**Why C, not C++:**
1. Toolchain universality — C compilers stable everywhere QEMU targets
2. OOP achievable without C++ (QOM proves it)
3. Explicit memory layout control — no hidden vtable pointers
4. Codebase inertia — millions of lines, C++ ABI variance is a real risk

**Two-level class/instance split:**
- `FooClass` initialized once per type (shared by all instances)
- `Foo` instance allocated per object
- Virtual methods are explicit C function pointer fields, not compiler-managed
- `memcpy`-then-override inheritance is simple and debuggable

**Bus system:**
- `BusState` is itself a QOM object
- Devices have parent bus; buses have list of child devices
- `BusClass.child_foreach` drives recursive tree operations (reset, migration, etc.)

---

## Part 2: QMP — QEMU Machine Protocol

### 2.1 Architecture

JSON protocol (JSON-RPC 2.0-like) over Unix socket or TCP. Three phases:

```
1. Capabilities Negotiation
   S: { "QMP": { "version": {...}, "capabilities": ["oob"] } }
   C: { "execute": "qmp_capabilities", "arguments": { "enable": ["oob"] } }
   S: { "return": {} }

2. Command Mode — all commands available
   C: { "execute": "query-status", "id": "req-1" }
   S: { "return": { "status": "running", ... }, "id": "req-1" }

3. Async Events (unsolicited)
   S: { "event": "SHUTDOWN", "data": {...}, "timestamp": {...} }
```

Out-of-band requests use `"exec-oob"` instead of `"execute"` — jumps the command queue.

### 2.2 QAPI Schema

Schema-driven code generation. Schema is the single source of truth for wire protocol + C types.

```python
# Struct
{ 'struct': 'StatusInfo',
  'data': { 'running': 'bool', 'status': 'RunState' } }

# Enum
{ 'enum': 'RunState',
  'data': ['running', 'paused', 'shutdown', 'debug', ...] }

# Discriminated union
{ 'union': 'BlockdevOptions',
  'base': { 'driver': 'BlockdevDriver' },
  'discriminator': 'driver',
  'data': { 'file': 'BlockdevOptionsFile', 'qcow2': '...' } }

# Command
{ 'command': 'query-status', 'returns': 'StatusInfo' }

# Event
{ 'event': 'SHUTDOWN', 'data': { 'guest': 'bool', 'reason': 'ShutdownCause' } }
```

**Generated artifacts:**
- `qapi-types.h/c` — C typedefs for all schema types
- `qapi-commands.h/c` — marshalling shims calling `qmp_*()` functions
- `qapi-events.h/c` — `qapi_event_send_*()` emit functions
- `qapi-introspect.c` — data for `query-qmp-schema`

**Developer implements:** `qmp_query_status()` by hand; framework generates everything else.

### 2.3 Key QMP Commands

```json
{ "execute": "stop" }                    // pause vCPUs
{ "execute": "cont" }                    // resume
{ "execute": "system_reset" }
{ "execute": "query-status" }
{ "execute": "query-cpus-fast" }
{ "execute": "device-add", "arguments": { "driver": "...", "id": "...", ... } }
{ "execute": "device-del", "arguments": { "id": "net1" } }
{ "execute": "human-monitor-command", "arguments": { "command-line": "info pci" } }
{ "execute": "query-qmp-schema" }        // introspect full API
```

### 2.4 Async Events

| Event | Trigger |
|-------|---------|
| `SHUTDOWN` | VM halted |
| `STOP` | vCPUs paused |
| `RESUME` | vCPUs unpaused |
| `RESET` | machine reset |
| `DEVICE_DELETED` | hotunplug complete |
| `MIGRATION` | migration state change |
| `BLOCK_IO_ERROR` | disk I/O failure |
| `GUEST_PANICKED` | kernel panic |

Rate-limited: max 1 event/second for high-frequency events; excess events dropped except last.

### 2.5 QMP vs. HMP

| Dimension | QMP | HMP |
|-----------|-----|-----|
| Format | Structured JSON | Human-readable text |
| Stability | Stable, QAPI-versioned | Best-effort |
| Async events | Yes, delivered automatically | No |
| Direction | Canonical; all new commands here | Legacy; wraps QMP calls |
| Programmatic use | Yes | Via `human-monitor-command` escape |

---

## Part 3: QOM vs. SIMICS — Comparison

### What QOM Does Better

| QOM Strength | vs. SIMICS |
|-------------|------------|
| Fully open source | DML compiler open-sourced recently but ecosystem smaller |
| Properties writable after realize | SIMICS attributes have strict config vs. operational semantics |
| QOM tree introspection built-in | SIMICS introspection less composable |
| `HotplugHandler` integrated into bus system | SIMICS hotplug via attribute patterns |
| Device models portable (plain C) | DML compiles to C but is SIMICS-specific |

### QOM Weaknesses vs. SIMICS

1. **No register/bank abstraction** — QOM has zero built-in concepts for registers, banks, or bitfields. Every device manually implements MMIO dispatch (giant switch statements). SIMICS DML gives you `bank`, `register`, `field` as first-class constructs.

2. **State serialization is decoupled** — VMState (migration/snapshots) is a separate system that developers must manually keep in sync with `DeviceState`. In SIMICS, attributes are inherently serializable — checkpointing works by default.

3. **Realize-or-init ambiguity** — A recurring source of bugs (code in wrong phase). SIMICS has cleaner separation.

4. **No reverse execution** — SIMICS checkpointing + determinism enables this natively. QEMU has no first-class support.

5. **Boilerplate load** — Even with modern macros, a minimal device is ~100 lines of scaffolding. SIMICS DML requires a fraction.

6. **BQL (Big QEMU Lock)** — Most QOM operations require the BQL. Modern QEMU is incrementally removing it but it remains a fundamental constraint. SIMICS has native multithreading.

---

## Part 4: Helm Design Choices Derived from QOM/QMP

### From QOM

#### ✅ Two-Phase Lifecycle: `DeviceConfig` → `Device::realize()`

```rust
// Phase 1: infallible property setting (like QOM instance_init)
// Can be used for introspection / schema queries without side effects
#[derive(Default, Clone)]
pub struct UartConfig {
    pub clock_hz: u32,   // default: 1_843_200
    pub fifo_depth: u8,  // default: 16
}

// Phase 2: fallible realization (like QOM realize)
impl Device for Uart16550 {
    fn realize(config: &DeviceParams, world: &mut World) -> Result<Self, DeviceError>;
    fn unrealize(&mut self, world: &mut World);
}
```

#### ✅ Self-Registration via `inventory` Crate

```rust
// Device module registers itself — no central dispatch table needed
use inventory;
inventory::submit! {
    DeviceDescriptor {
        name: "uart16550",
        factory: |params| Box::new(Uart16550::from_params(params)?),
        param_schema: Uart16550::param_schema,
        python_class: include_str!("uart16550.py"),
    }
}
// Equivalent to QEMU's type_init(__attribute__((constructor)))
```

#### ✅ `register_bank!` Macro (QOM Weakness → Helm Strength)

QOM has no register abstraction. Helm makes it first-class:

```rust
register_bank! {
    UartRegs for Uart16550 at offset 0x0 {
        reg RHR_THR @ 0x00 size 1 {
            field DATA [7:0]
        }
        reg LSR @ 0x14 size 1 is read_only {
            field THRE [5]   // TX holding register empty
            field DR   [0]   // data ready
        }
        reg LCR @ 0x0C size 1 { ... }
    }
}
// Generates: MmioHandler impl, AttrDescriptors for checkpoint, trace points
```

#### ✅ Three-Phase Reset (`Resettable` Trait)

```rust
pub enum ResetType { Cold, Warm, Bus }

pub trait Resettable {
    fn assert_reset(&mut self, kind: ResetType);   // freeze I/O, set reset values
    fn release_reset(&mut self, kind: ResetType);  // restart, begin operation
}

// World drives cascade: bus.assert_reset() → walks all children
```

#### ✅ Typed Property Links

```rust
pub struct Connect<T: Interface> {
    target: Option<(HelmObjectId, String)>,   // (obj, port_name)
    cached: Option<Arc<T>>,
}
// Stronger than DEFINE_PROP_LINK: Rust trait bound enforced at compile time
```

### From QMP → `HelmProtocol`

QMP's typed command/event protocol maps to Helm's control API:

```rust
// Typed command enum (replaces QMP's string "execute" field)
pub enum HelmCommand {
    Stop,
    Continue,
    Reset { cold: bool },
    QueryStatus,
    QueryCpus,
    ReadRegister { cpu: u32, reg: String },
    WriteRegister { cpu: u32, reg: String, val: u64 },
    ReadMemory { addr: u64, len: usize },
    WriteMemory { addr: u64, data: Vec<u8> },
    ListDevices,
    DeviceProperties { name: String },
    Checkpoint { path: String },
    Restore { path: String },
}

pub enum HelmResponse {
    Ok,
    Status { running: bool, mode: ExecMode, cycle: u64 },
    Registers { regs: Vec<(String, u64)> },
    Memory { data: Vec<u8> },
    Devices { list: Vec<DeviceInfo> },
    Error { class: String, message: String },
}

// Async events (like QMP events)
pub enum HelmEvent {
    SimulationStopped { reason: StopReason },
    SimulationStarted,
    BreakpointHit { addr: u64 },
    CheckpointSaved { path: String },
    DeviceIrq { device: String, asserted: bool },
}

// Server: Unix socket or TCP, JSON wire format
pub struct HelmServer { ... }
impl HelmServer {
    pub fn bind(path: &Path) -> io::Result<Self>;
    pub fn handle_command(&mut self, cmd: HelmCommand) -> HelmResponse;
    pub fn emit_event(&self, event: HelmEvent);
}
```

**Schema introspection:** `HelmCommand::ListDevices` → returns device names + param schemas → equivalent to QMP's `query-qmp-schema`. Python tooling can discover what devices are available and what params they accept.

### Key Design Differences: Helm vs. QEMU

| Concern | QEMU/QOM | Helm |
|---------|----------|------|
| Register modeling | Manual switch/offset dispatch | `register_bank!` macro |
| State serialization | Separate VMState (manual sync) | `#[attr(Required)]` = auto-checkpoint |
| Thread safety | BQL (global lock) | Temporal decoupling + per-hart state |
| Introspection | QOM tree + `device-list-properties` | `DeviceDescriptor::param_schema` |
| Reset | `Resettable` interface, 3-phase | Same pattern, Rust trait |
| Self-registration | `type_init` + constructor attr | `inventory::submit!` |
| Lifecycle | `instance_init` / `realize` split | `DeviceConfig` / `Device::realize()` |
| Control protocol | QMP (JSON over socket) | `HelmProtocol` (typed enum + JSON) |

---

## Sources

- [The QEMU Object Model (QOM)](https://qemu-project.gitlab.io/qemu/devel/qom.html)
- [QEMU QOM API Reference](https://www.qemu.org/docs/master/devel/qom-api.html)
- [QEMU instance_init vs. realize](https://people.redhat.com/~thuth/blog/qemu/2018/09/10/instance-init-realize.html)
- [QEMU Machine Protocol Specification](https://www.qemu.org/docs/master/interop/qmp-spec.html)
- [QMP Reference Manual](https://qemu-project.gitlab.io/qemu/interop/qemu-qmp-ref.html)
- [QAPI Code Generator](https://www.qemu.org/docs/master/devel/qapi-code-gen.html)
- [Reset in QEMU: Resettable Interface](https://www.qemu.org/docs/master/devel/reset.html)
- [QOM Conventions — QEMU Wiki](https://wiki.qemu.org/Documentation/QOMConventions)
- [qdev-properties.h — QEMU source](https://github.com/qemu/qemu/blob/master/include/hw/qdev-properties.h)

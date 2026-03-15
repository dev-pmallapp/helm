# SIMICS Research: Object Model, Interfaces, Attributes, Configuration

> Source: Deep research into Intel SIMICS API. Used to inform helm-ng design decisions.

---

## 1. SIMICS Object Model (`conf_object_t` / `conf_class_t`)

### How SIMICS Does It

Every simulated object — CPU, device, memory, bus — is a `conf_object_t`. Classes are registered once at module load time. All device-specific state lives in a module-private struct accessed via `object_data`.

```c
typedef struct conf_object {
    conf_class_t   *class_data;   // pointer to class
    const char     *name;         // full dot-path name: "board.cpu0"
    struct conf_object *queue;    // simulation clock
    lang_void      *object_data;  // device state (opaque)
} conf_object_t;

// Modern registration (Simics 6+)
typedef struct {
    conf_object_t *(*alloc)(conf_class_t *cls);
    lang_void     *(*init)(conf_object_t *obj);
    void           (*finalize)(conf_object_t *obj);
    void           (*objects_finalized)(conf_object_t *obj);
    void           (*deinit)(conf_object_t *obj);
    void           (*dealloc)(conf_object_t *obj);
    const char     *description;
    class_kind_t    kind;         // Vanilla (saved) | Session | Pseudo (never saved)
} class_info_t;
```

**Lifecycle**: `alloc → init → [attributes set] → finalize → objects_finalized → (simulation) → deinit → dealloc`

Critical invariant: during `init()`, cross-object calls are forbidden (peer attributes not yet set). Cross-object calls are only safe from `finalize()` onward. `objects_finalized()` guarantees all peers in the config batch have completed their own `finalize()`.

**Class Kinds:**
- `Vanilla` — hardware model, saved to checkpoint (default)
- `Session` — transient, not saved
- `Pseudo` — tool/instrumentation objects, never saved

**Object naming**: dot-path hierarchy — `board0.cpu0.core[0][0]`. `SIM_get_object("board.cpu0")` for lookup.

### ✅ Helm Design Choice: `HelmObject` + Class Registry

```rust
/// The universal identity handle — every simulated entity is a HelmObject.
/// All state is exposed via the Attribute system (no dark state).
pub struct HelmObject {
    pub class:  &'static ClassDescriptor,
    pub name:   String,       // dot-path: "board.cpu0.icache"
    pub parent: Option<HelmObjectId>,
    attrs:      AttrStore,    // all registered attributes live here
    data:       Box<dyn Any + Send>, // device-specific state (opaque)
}

pub type HelmObjectId = u32;  // stable ID, not a pointer

/// Registered once per type, at module/crate init time.
pub struct ClassDescriptor {
    pub name:        &'static str,
    pub kind:        ObjectKind,   // Persistent | Session | Pseudo
    pub alloc:       fn() -> Box<dyn Any + Send>,
    pub init:        fn(&mut HelmObject),
    pub finalize:    fn(&mut HelmObject, &mut World),
    pub all_finalized: fn(&mut HelmObject, &World),
    pub deinit:      fn(&mut HelmObject),
}

pub enum ObjectKind {
    Persistent,  // saved to checkpoint (most hardware models)
    Session,     // not saved (debug tools, stats)
    Pseudo,      // never saved, recomputed (read-only views)
}
```

**Key differences from current helm-ng design:**
- `SimObject` trait → replaced by `ClassDescriptor` struct + `HelmObject` handle
- `elaborate()` → split into `finalize()` + `objects_finalized()` (matches SIMICS lifecycle)
- No "dark state" — every persistent field registered as an attribute

---

## 2. SIMICS Interface System

### How SIMICS Does It

Interfaces are C structs of function pointers — **not C++ virtual functions**. Named, registered per class, retrieved at runtime. A single static instance per class (not per object).

```c
// Define an interface
typedef struct {
    void (*signal_raise)(conf_object_t *obj);
    void (*signal_lower)(conf_object_t *obj);
} signal_interface_t;

// Register: this class implements "signal"
SIM_register_interface(cls, "signal", &my_signal_impl);

// Retrieve and call at runtime
const signal_interface_t *sig = SIM_get_interface(target, "signal");
sig->signal_raise(target);
```

**Why not C++ vtables?** Four concrete reasons:
1. Multi-language modules (DML, Python, C, C++) must all interop — C fn-ptr structs are FFI-accessible from any language
2. ABI stability — C++ vtables break on field insertion; named C struct fields don't
3. Cross-compiler interoperability — vtable layout is implementation-defined
4. Runtime binding — objects connect at config time, not link time; mirrors COM `QueryInterface`

**Port interfaces** — multiple instances of the same interface type on one object via named ports:
```c
// Device exposes two interrupt output ports
SIM_register_port_interface(cls, "signal", &irq_out_0_impl, "IRQ[0]", "...");
SIM_register_port_interface(cls, "signal", &irq_out_1_impl, "IRQ[1]", "...");
// Appear as child objects: dev.port.IRQ[0], dev.port.IRQ[1]
```

**Interface versioning**: when `foo` must change incompatibly, register `foo_v2`. Keep `foo` for old modules. Simulator checks newest version first.

### ✅ Helm Design Choice: Named Interface Registry

```rust
/// An interface is a named, type-erased vtable stored per class.
/// Retrieved at runtime by name — enables runtime composition and plugin devices.
pub struct Interface {
    pub name:    &'static str,
    pub version: u32,
    vtable:      Box<dyn Any + Send + Sync>,  // the actual fn-ptr struct, type-erased
}

pub struct InterfaceRegistry {
    // class_name → interface_name → Interface
    map: HashMap<&'static str, HashMap<&'static str, Interface>>,
}

impl InterfaceRegistry {
    /// Register: this class implements `iface_name`
    pub fn register<T: 'static + Send + Sync>(
        &mut self, class: &'static str, iface_name: &'static str, vtable: T);

    /// Retrieve: get typed interface for an object (returns None if not implemented)
    pub fn get<T: 'static>(&self, obj: &HelmObject, iface_name: &str) -> Option<&T>;

    /// Port variant: retrieve named port's interface
    pub fn get_port<T: 'static>(
        &self, obj: &HelmObject, iface_name: &str, port: &str) -> Option<&T>;
}

// The signal interface — standard interrupt pin protocol
pub struct SignalInterface {
    pub raise: fn(obj: &mut HelmObject),
    pub lower: fn(obj: &mut HelmObject),
}

// Port objects — child HelmObjects representing named interface instances
// "board.uart.port.irq_out" → HelmObject implementing SignalInterface
```

**Named interfaces** replace our current ad-hoc `InterruptPin` / `MmioHandler` traits with a unified discovery mechanism. Any code can ask "does this object implement 'signal'?" at runtime — crucial for plugin devices.

---

## 3. SIMICS Attribute System

### How SIMICS Does It

`attr_value_t` is a tagged union — the universal value for all attribute traffic. **All state flows through attributes.** This single invariant enables: checkpoint/restore, scripting access, CLI introspection, and remote debugging.

```c
typedef enum {
    Sim_Attr_Required   = 0,   // must be set; saved in checkpoint
    Sim_Attr_Optional   = 1,   // may be absent; saved if set
    Sim_Attr_Session    = 3,   // not saved (perf counters, debug state)
    Sim_Attr_Pseudo     = 4,   // computed; not saved (read-only views, triggers)
} attr_attr_t;

// Type descriptor string — validated before calling setter
// "i" int | "s" string | "o" obj | "b" bool | "[i*]" list of ints | "o|n" obj-or-nil
void SIM_register_typed_attribute(
    conf_class_t *cls, const char *name,
    getter_fn, setter_fn,
    attr_attr_t kind, const char *type_string, const char *desc);
```

**Checkpoint = attribute system.** On save: call every `Required`/`Optional` getter, serialize to file. On restore: call every setter listed in file, then `finalize()`. There is no separate serialization code — attributes ARE persistence.

### ✅ Helm Design Choice: `HelmAttr` System

```rust
/// Attribute kind — controls persistence and semantics
pub enum AttrKind {
    Required,   // must be set; saved to checkpoint
    Optional,   // may be absent; saved if set; has default
    Session,    // not saved (PerfCounters, debug flags, tracing state)
    Pseudo,     // computed; not saved (read-only views, write-only triggers)
}

/// Typed attribute value — the universal currency of the attribute system
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum AttrValue {
    Integer(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Object(HelmObjectId),           // reference to another object
    Port(HelmObjectId, String),     // object + port name (for port interfaces)
    List(Vec<AttrValue>),
    Dict(Vec<(String, AttrValue)>),
    Nil,
}

/// Attribute descriptor — registered once per (class, attr_name)
pub struct AttrDescriptor {
    pub name:    &'static str,
    pub kind:    AttrKind,
    pub type_:   AttrType,          // runtime type constraint
    pub default: Option<AttrValue>, // for Optional attrs
    pub get: fn(&HelmObject) -> AttrValue,
    pub set: fn(&mut HelmObject, AttrValue) -> Result<(), AttrError>,
    pub desc:    &'static str,
}

// Checkpoint = serialize all Required + Optional attrs to JSON/CBOR.
// No separate serialize() method needed. Attribute system IS persistence.
```

**The invariant for helm-ng:** every field of a `HelmObject` that must survive checkpoint/restore MUST be registered as a `Required` or `Optional` attribute. Fields registered as `Session` are cleared on restore. Un-registered fields are "dark state" — forbidden.

---

## 4. SIMICS Configuration / Component System

### How SIMICS Does It

The `pre_conf_object_t` pattern: describe the full topology first (no side effects), instantiate everything at once.

```python
# Python — build topology description (no C objects created yet)
cpu      = pre_conf_object('board.cpu',  'riscv-hart')
mem      = pre_conf_object('board.mem',  'memory-space')
uart     = pre_conf_object('board.uart', 'uart16550')

cpu.physical_memory = mem       # object reference — resolved later
uart.clock_hz       = 1_843_200

# Instantiate: alloc → set attrs → finalize → objects_finalized
SIM_add_configuration([cpu, mem, uart], None)
```

The component system provides a factory abstraction for hierarchical boards:
```python
class VirtRiscvBoard(component_object):
    def add_objects(self):       # create all pre_conf_objects
    def add_connector_info(self): # publish connectors to peers
    def connect(self, conn, info): # wire inter-component connections
```

### ✅ Helm Design Choice: `HelmConfig` + `World::instantiate()`

```rust
/// Pending object — describes what to create (no side effects until instantiate())
pub struct PendingObject {
    pub class: &'static str,
    pub name:  String,
    pub attrs: Vec<(String, AttrValue)>,  // deferred attribute assignments
}

impl PendingObject {
    pub fn set(&mut self, attr: &str, val: AttrValue) -> &mut Self;
    pub fn set_obj(&mut self, attr: &str, name: &str) -> &mut Self; // forward ref by name
}

/// World owns all instantiated HelmObjects
pub struct World {
    objects:   HashMap<HelmObjectId, HelmObject>,
    by_name:   HashMap<String, HelmObjectId>,
    registry:  InterfaceRegistry,
    attr_registry: AttrRegistry,
    event_bus: Arc<HelmEventBus>,
    event_queue: EventQueue,
}

impl World {
    /// Instantiate a batch of PendingObjects:
    /// 1. alloc() all objects
    /// 2. set all attrs in dependency order (resolve forward name refs)
    /// 3. finalize() all objects
    /// 4. all_finalized() all objects
    pub fn instantiate(&mut self, config: Vec<PendingObject>) -> Result<(), ConfigError>;

    /// Get a typed interface for an object by name
    pub fn get_interface<T: 'static>(
        &self, obj_name: &str, iface: &str) -> Option<&T>;

    /// Get a typed port interface
    pub fn get_port_interface<T: 'static>(
        &self, obj_name: &str, iface: &str, port: &str) -> Option<&T>;
}

/// Python-facing: helm_ng.PendingObject / World exposed via PyO3
```

**The `pre_conf_object` insight applied to helm-ng:**
- Python config builds `PendingObject` instances with no Rust side effects
- `World::instantiate()` runs the full lifecycle atomically
- Forward references (object A's attr references object B by name) resolved in dependency order
- Platform files describe full topology in Python, then call `sim.instantiate()`

---

## Cross-Cutting Insight: What SIMICS Gets Fundamentally Right

The single deepest insight from SIMICS API design:

> **The attribute system is the single source of truth for all object state. There is no separate serialization, introspection, or scripting API — they all go through attributes.**

This means:
- Checkpoint/restore is free (serialize all `Required`/`Optional` attrs)
- CLI introspection is free (`obj.attr` in Python = `SIM_get_attribute()`)
- Remote debugging state access is free (GDB stub reads attrs)
- Python scripting access is free (same attrs)
- No impedance mismatch between "the object's real state" and "what the tools see"

**helm-ng must adopt this invariant.** The current design has `checkpoint_save() -> Vec<u8>` (manual, fragile), separate Python inspection API, and separate GDB register access. These should all collapse into one: the attribute system.

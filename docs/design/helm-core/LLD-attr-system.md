# helm-engine — LLD: Attribute System

> Complete Rust API specification for `AttrValue`, `AttrKind`, `AttrDescriptor`, and `AttrStore`.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-object-model.md`](./LLD-object-model.md)

---

## Table of Contents

1. [AttrValue — Typed Union](#1-attrvalue--typed-union)
2. [AttrKind — Persistence Classification](#2-attrkind--persistence-classification)
3. [AttrDescriptor — Per-Attribute Metadata](#3-attrdescriptor--per-attribute-metadata)
4. [AttrStore — Per-Object Storage](#4-attrstore--per-object-storage)
5. [The No-Dark-State Invariant](#5-the-no-dark-state-invariant)
6. [Checkpoint and Restore Protocol](#6-checkpoint-and-restore-protocol)
7. [AttrError Enum](#7-attrerror-enum)
8. [Mapping to SIMICS SIM_register_typed_attribute](#8-mapping-to-simics-sim_register_typed_attribute)
9. [Python Attribute Access](#9-python-attribute-access)
10. [AttrDescriptor Registration Pattern](#10-attrdescriptor-registration-pattern)
11. [Full Class Example](#11-full-class-example)

---

## 1. AttrValue — Typed Union

```rust
/// The universal value type for attribute storage, transfer, and serialization.
///
/// Serializable via serde (CBOR for checkpoints, JSON for Python introspection).
/// Clone is cheap for scalar variants; List/Dict/Str clone their heap data.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AttrValue {
    /// Signed 64-bit integer. Covers all Rust integer types (u8..u64, i8..i64)
    /// after widening conversion.
    Integer(i64),

    /// IEEE 754 double. Covers all Rust float types.
    Float(f64),

    /// Boolean.
    Bool(bool),

    /// UTF-8 string. Used for names, paths, and enum-as-string config fields.
    Str(String),

    /// Reference to another HelmObject by id.
    ///
    /// SIMICS equivalent: attr_value_t with kind = Sim_Val_Object.
    /// On checkpoint save: serialized as the object's dot-path name string.
    /// On restore: resolved back to HelmObjectId by World.
    Object(HelmObjectId),

    /// Reference to a named port on another HelmObject.
    ///
    /// (object_id, port_name).
    /// Used for interrupt wiring: AttrValue::Port(plic_id, "input_10".into()).
    Port(HelmObjectId, String),

    /// Ordered list of AttrValues. Elements may be of mixed types.
    List(Vec<AttrValue>),

    /// Key-value map. Keys are strings; values are AttrValues.
    ///
    /// Ordered (Vec not HashMap) for deterministic checkpoint serialization.
    Dict(Vec<(String, AttrValue)>),

    /// Absent / unset. Used for Optional attrs that have no value and no default.
    Nil,
}

impl AttrValue {
    /// Unwrap as i64. Panics with a clear message if not Integer.
    pub fn as_integer(&self) -> i64 {
        match self {
            AttrValue::Integer(v) => *v,
            _ => panic!("expected AttrValue::Integer, got {:?}", self),
        }
    }

    /// Unwrap as bool. Panics if not Bool.
    pub fn as_bool(&self) -> bool {
        match self {
            AttrValue::Bool(v) => *v,
            _ => panic!("expected AttrValue::Bool, got {:?}", self),
        }
    }

    /// Unwrap as &str. Panics if not Str.
    pub fn as_str(&self) -> &str {
        match self {
            AttrValue::Str(s) => s.as_str(),
            _ => panic!("expected AttrValue::Str, got {:?}", self),
        }
    }

    /// Unwrap as HelmObjectId. Panics if not Object.
    pub fn as_object_id(&self) -> HelmObjectId {
        match self {
            AttrValue::Object(id) => *id,
            _ => panic!("expected AttrValue::Object, got {:?}", self),
        }
    }

    /// Return true if this value is Nil.
    pub fn is_nil(&self) -> bool {
        matches!(self, AttrValue::Nil)
    }
}

// Convenience conversions from Rust primitive types
impl From<i64>   for AttrValue { fn from(v: i64)   -> Self { AttrValue::Integer(v) } }
impl From<i32>   for AttrValue { fn from(v: i32)   -> Self { AttrValue::Integer(v as i64) } }
impl From<u64>   for AttrValue { fn from(v: u64)   -> Self { AttrValue::Integer(v as i64) } }
impl From<u32>   for AttrValue { fn from(v: u32)   -> Self { AttrValue::Integer(v as i64) } }
impl From<u8>    for AttrValue { fn from(v: u8)    -> Self { AttrValue::Integer(v as i64) } }
impl From<bool>  for AttrValue { fn from(v: bool)  -> Self { AttrValue::Bool(v) } }
impl From<f64>   for AttrValue { fn from(v: f64)   -> Self { AttrValue::Float(v) } }
impl From<String> for AttrValue { fn from(v: String) -> Self { AttrValue::Str(v) } }
impl From<&str>  for AttrValue { fn from(v: &str)  -> Self { AttrValue::Str(v.to_string()) } }
impl From<HelmObjectId> for AttrValue { fn from(id: HelmObjectId) -> Self { AttrValue::Object(id) } }
```

---

## 2. AttrKind — Persistence Classification

```rust
/// Controls whether an attribute participates in checkpoint/restore.
///
/// SIMICS equivalent: Sim_Attr_Required, Sim_Attr_Optional, Sim_Attr_Session, Sim_Attr_Pseudo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrKind {
    /// Saved in checkpoint. Must be present on restore.
    ///
    /// Use for: CPU register file, device register banks, DMA descriptor state,
    /// interrupt controller pending bits, FIFO contents, timer reload values.
    ///
    /// Absent from checkpoint file → ConfigError::MissingRequiredAttr on restore.
    Required,

    /// Saved in checkpoint if present. Absent on restore → use default.
    ///
    /// Use for: optional config fields that have reasonable defaults.
    /// Example: "loopback" mode on a UART (defaults to false).
    Optional,

    /// Not saved in checkpoint. Reset to default on restore.
    ///
    /// Use for: runtime caches, TLB contents, prefetch buffers, host OS handles.
    /// The field is correct during simulation but does not need to persist.
    Session,

    /// Not saved. No backing storage. Value computed on every read from device state.
    ///
    /// Use for: statistics (tx_bytes_count), debugging (current_state_string),
    /// computed fields (baud_rate = clock_hz / divisor).
    Pseudo,
}
```

### AttrKind and Checkpoint Behavior

| Kind | serialize_persistent() | restore_from() behavior |
|---|---|---|
| `Required` | Included | Must be present → error if absent |
| `Optional` | Included if not Nil | Absent → use `AttrDescriptor::default` |
| `Session` | Excluded | Restored to `AttrDescriptor::default` (or Nil) |
| `Pseudo` | Excluded | Not written on restore; value is always computed |

---

## 3. AttrDescriptor — Per-Attribute Metadata

```rust
/// Metadata for one named attribute on a class.
///
/// Registered as part of a ClassDescriptor's attr list.
/// SIMICS equivalent: the arguments to SIM_register_typed_attribute().
pub struct AttrDescriptor {
    /// Attribute name. Must be unique within a class.
    pub name: &'static str,

    /// Persistence classification.
    pub kind: AttrKind,

    /// Default value, if any.
    ///
    /// Required attrs with no default must be set explicitly in PendingObject.
    /// Optional/Session attrs use this if not overridden.
    pub default: Option<AttrValue>,

    /// Get the attribute value from the object.
    ///
    /// For Required/Optional/Session: typically reads from AttrStore::values.
    /// For Pseudo: computes the value from device state directly.
    ///
    /// Signature matches SIMICS `get_attr_t`: fn(obj) -> AttrValue.
    pub get: fn(&HelmObject) -> AttrValue,

    /// Set the attribute value on the object.
    ///
    /// For Required/Optional/Session: validates and stores in AttrStore::values.
    /// For Pseudo: may update computed state (e.g. setting a read-write computed reg).
    ///             May also be a no-op if the attr is read-only.
    ///
    /// Returns Err(AttrError) if the value is invalid.
    pub set: fn(&mut HelmObject, AttrValue) -> Result<(), AttrError>,

    /// Human-readable documentation string.
    ///
    /// Shown in `world.describe_attr("board.uart0", "clock_hz")` and Python help().
    pub desc: &'static str,
}
```

### Canonical get/set Implementations

For `Required`/`Optional`/`Session` attributes, `get` and `set` are boilerplate that delegates to `AttrStore::values`:

```rust
// Standard get: read from AttrStore's value HashMap
fn uart_get_clock_hz(obj: &HelmObject) -> AttrValue {
    obj.attrs.get("clock_hz")
        .cloned()
        .unwrap_or(AttrValue::Integer(1_843_200))
}

// Standard set: validate type, then store
fn uart_set_clock_hz(obj: &mut HelmObject, val: AttrValue) -> Result<(), AttrError> {
    match &val {
        AttrValue::Integer(hz) if *hz > 0 => {
            obj.data_mut::<Uart16550State>().clock_hz = *hz as u64;
            obj.attrs.values.insert("clock_hz", val);
            Ok(())
        }
        AttrValue::Integer(_) => Err(AttrError::InvalidValue {
            attr: "clock_hz",
            msg: "must be positive".to_string(),
        }),
        _ => Err(AttrError::TypeMismatch {
            attr: "clock_hz",
            expected: "Integer",
            got: val.type_name(),
        }),
    }
}

// Pseudo get: compute from device state
fn uart_get_baud_rate(obj: &HelmObject) -> AttrValue {
    let state = obj.data::<Uart16550State>();
    let divisor = state.divisor_latch.max(1) as u64;
    AttrValue::Integer((state.clock_hz / (16 * divisor)) as i64)
}

// Pseudo set: no-op (read-only computed attr)
fn uart_set_baud_rate(_obj: &mut HelmObject, _val: AttrValue) -> Result<(), AttrError> {
    Err(AttrError::ReadOnly { attr: "baud_rate" })
}
```

---

## 4. AttrStore — Per-Object Storage

```rust
/// Per-object attribute storage.
///
/// Owns the AttrDescriptor table (shared per class, static lifetime) and
/// per-object value overrides.
pub struct AttrStore {
    /// Descriptor table for this object's class.
    /// Static: shared across all instances of the same class.
    descriptors: &'static [AttrDescriptor],

    /// Per-object stored values. Only Required/Optional/Session attrs appear here.
    /// Pseudo attrs are never stored; they are always computed via AttrDescriptor::get.
    values: HashMap<&'static str, AttrValue>,
}

impl AttrStore {
    /// Create an AttrStore from a ClassDescriptor's attr table.
    ///
    /// Populates values with defaults for Optional and Session attrs.
    /// Required attrs are left absent until set explicitly.
    pub fn new(desc: &'static ClassDescriptor) -> Self {
        let mut values = HashMap::new();
        for attr in desc.attrs {
            if attr.kind != AttrKind::Pseudo {
                if let Some(default) = attr.default.clone() {
                    values.insert(attr.name, default);
                }
            }
        }
        AttrStore { descriptors: desc.attrs, values }
    }

    /// Get an attribute value by name.
    ///
    /// For Pseudo attrs: calls AttrDescriptor::get() (computed, no HelmObject context
    /// needed for the store itself — callers must use HelmObject::get_attr for Pseudo).
    /// For others: reads from values HashMap.
    ///
    /// Returns None if the attr is not registered or has no value.
    pub fn get(&self, name: &str) -> Option<&AttrValue> {
        // AttrStore::get is for non-Pseudo only (Pseudo requires HelmObject context).
        // For Pseudo, callers use HelmObject::get_attr which routes through the descriptor.
        self.values.get(name)
    }

    /// Set an attribute value by name.
    ///
    /// Validates that `name` is a registered attr for this class.
    /// Calls AttrDescriptor::set for type checking and side-effect application.
    /// Stores the validated value in the values HashMap.
    ///
    /// Callers must use HelmObject::set_attr, which provides the &mut HelmObject context.
    /// This method is for internal use during deserialization.
    pub fn set(&mut self, name: &str, val: AttrValue) -> Result<(), AttrError> {
        let desc = self.descriptors.iter().find(|d| d.name == name)
            .ok_or(AttrError::Unknown { attr: name.to_string() })?;

        if desc.kind == AttrKind::Pseudo {
            // Pseudo attrs are writable only through HelmObject::set_attr (needs &mut HelmObject)
            return Err(AttrError::NeedsObjectContext { attr: name.to_string() });
        }

        // Store the value. Type validation is performed by AttrDescriptor::set
        // when called via HelmObject::set_attr. Here we store the pre-validated value.
        self.values.insert(desc.name, val);
        Ok(())
    }

    /// Serialize all persistent attributes (Required and Optional) to a vector.
    ///
    /// Used by the checkpoint protocol. Session and Pseudo attrs are excluded.
    /// Returns all (name, value) pairs where kind is Required or Optional.
    pub fn serialize_persistent(&self) -> Vec<(String, AttrValue)> {
        let mut result = Vec::new();
        for desc in self.descriptors {
            match desc.kind {
                AttrKind::Required | AttrKind::Optional => {
                    if let Some(val) = self.values.get(desc.name) {
                        result.push((desc.name.to_string(), val.clone()));
                    }
                }
                AttrKind::Session | AttrKind::Pseudo => {
                    // Not saved
                }
            }
        }
        result
    }

    /// Restore attribute values from a saved checkpoint.
    ///
    /// For each (name, value) pair:
    ///   - Required: must be present. Absent → AttrError::MissingRequired.
    ///   - Optional: applied if present. Absent → use default.
    ///   - Session: not present in saved; reset to default.
    ///   - Pseudo: skipped.
    ///
    /// After restore_from(), the caller must drive ClassDescriptor::finalize()
    /// and then ClassDescriptor::all_finalized() to re-establish cross-object wiring.
    pub fn restore_from(&mut self, saved: Vec<(String, AttrValue)>) -> Result<(), AttrError> {
        // Build lookup from saved pairs
        let mut saved_map: HashMap<String, AttrValue> =
            saved.into_iter().collect();

        for desc in self.descriptors {
            match desc.kind {
                AttrKind::Required => {
                    let val = saved_map.remove(desc.name)
                        .ok_or(AttrError::MissingRequired { attr: desc.name.to_string() })?;
                    self.values.insert(desc.name, val);
                }
                AttrKind::Optional => {
                    if let Some(val) = saved_map.remove(desc.name) {
                        self.values.insert(desc.name, val);
                    }
                    // else: keep current default
                }
                AttrKind::Session => {
                    // Reset to default (or remove if no default)
                    match &desc.default {
                        Some(d) => { self.values.insert(desc.name, d.clone()); }
                        None    => { self.values.remove(desc.name); }
                    }
                }
                AttrKind::Pseudo => {
                    // Never stored, never restored
                }
            }
        }

        Ok(())
    }

    /// Return a descriptor for a named attribute, if registered.
    pub fn descriptor(&self, name: &str) -> Option<&'static AttrDescriptor> {
        self.descriptors.iter().find(|d| d.name == name)
    }

    /// Return the full descriptor list for this class.
    pub fn all_descriptors(&self) -> &'static [AttrDescriptor] {
        self.descriptors
    }
}
```

---

## 5. The No-Dark-State Invariant

**Definition:** Dark state is any correctness-critical simulation state that is not reachable via `AttrStore::serialize_persistent()`. Dark state is invisible to checkpointing, silent on restore, and creates impossible-to-debug divergences between a running simulation and its checkpoint.

**Examples of dark state (bugs):**

```rust
// BUG: tx_count is a Rust field, never registered as an attr.
// After restore, tx_count = 0 even if the pre-checkpoint sim had tx_count = 1000.
pub struct UartState {
    pub divisor_latch: u16,
    pub tx_count: u64,   // ← dark state — never serialized
}
```

```rust
// BUG: dma_desc_base is set during init and never exposed as an attr.
// A DMA engine that pauses mid-transfer will restart from address 0 after restore.
pub struct DmaState {
    pub dma_desc_base: u64,  // ← dark state
    pub current_desc:  u32,  // ← dark state
}
```

**The enforcement mechanism:**

The design does not (and cannot) automatically detect dark state at compile time. Enforcement is by convention and code review:

1. Every field in a device's `*State` struct is either:
   - Registered as a `Required` or `Optional` `AttrDescriptor`, OR
   - Explicitly documented as `Session` (transient, safe to lose on restore), OR
   - A pure cache/performance counter (use `Pseudo` kind).

2. The `serialize_persistent()` + `restore_from()` round-trip is tested for every device class.

3. A device that loses state after `serialize_persistent()` → `restore_from()` → `finalize()` fails the checkpoint test.

**Performance counters are NOT dark state:**

Counters (`tx_bytes_count`, `rx_overrun_count`) are not architectural state. They never affect correctness. They are registered as `Pseudo` attrs (readable but not saved). This is correct by definition — performance counters exist outside the correctness boundary.

---

## 6. Checkpoint and Restore Protocol

### Checkpoint Save

```rust
impl World {
    /// Save a full checkpoint of all Persistent objects to a CBOR blob.
    ///
    /// Collects serialize_persistent() for every Persistent HelmObject.
    /// Session and Pseudo objects are not included.
    pub fn checkpoint_save(&self) -> Result<Vec<u8>, CheckpointError> {
        let mut snapshot: Vec<ObjectSnapshot> = Vec::new();

        // Deterministic order: sort by HelmObjectId
        let mut ids: Vec<HelmObjectId> = self.objects.keys().copied().collect();
        ids.sort();

        for id in ids {
            let obj = &self.objects[&id];
            if obj.class.kind != ObjectKind::Persistent {
                continue;
            }
            snapshot.push(ObjectSnapshot {
                name:  obj.name.clone(),
                class: obj.class.name.to_string(),
                attrs: obj.attrs.serialize_persistent(),
            });
        }

        let bytes = ciborium::ser::into_writer_vec(&snapshot)
            .map_err(|e| CheckpointError::Serialize(e.to_string()))?;
        Ok(bytes)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ObjectSnapshot {
    name:  String,
    class: String,
    attrs: Vec<(String, AttrValue)>,
}
```

### Checkpoint Restore

```rust
impl World {
    /// Restore a World from a CBOR checkpoint blob.
    ///
    /// Protocol:
    ///   1. Deserialize ObjectSnapshot list from blob.
    ///   2. Re-instantiate all objects (alloc + init, same as normal instantiate).
    ///   3. Call restore_from() for each object with its saved attrs.
    ///   4. Call finalize() for all objects (re-establish cross-object wiring).
    ///   5. Call all_finalized() for all objects (re-validate).
    ///
    /// Note: Step 4-5 are required because finalize() establishes runtime state
    /// (EventQueue refs, MMIO registration, interrupt wiring) that is not checkpointed.
    pub fn checkpoint_restore(
        &mut self,
        blob: &[u8],
    ) -> Result<(), CheckpointError> {
        let snapshots: Vec<ObjectSnapshot> = ciborium::de::from_reader(blob)
            .map_err(|e| CheckpointError::Deserialize(e.to_string()))?;

        let registry = ClassRegistry::global();

        // Phase 1: Alloc + init
        for snap in &snapshots {
            let desc = registry.get(&snap.class)
                .ok_or_else(|| CheckpointError::UnknownClass(snap.class.clone()))?;

            let id = self.next_id;
            self.next_id += 1;

            let mut obj = HelmObject {
                id,
                class: desc,
                name:  snap.name.clone(),
                parent: None,
                children: Vec::new(),
                attrs: AttrStore::new(desc),
                data: (desc.alloc)(),
            };
            (desc.init)(&mut obj);
            self.objects.insert(id, obj);
            self.by_name.insert(snap.name.clone(), id);
        }

        // Wire hierarchy from names
        self.wire_hierarchy_from_names()?;

        // Phase 2: restore_from() for each object
        for snap in &snapshots {
            let id = *self.by_name.get(&snap.name).unwrap();
            let obj = self.objects.get_mut(&id).unwrap();
            obj.attrs.restore_from(snap.attrs.clone())
                .map_err(|e| CheckpointError::AttrError {
                    object: snap.name.clone(),
                    source: e,
                })?;
        }

        // Phase 3: finalize (re-establish wiring)
        let ids: Vec<HelmObjectId> = self.objects.keys().copied().collect();
        for &id in &ids {
            let desc = self.objects[&id].class;
            let mut obj = self.objects.remove(&id).unwrap();
            (desc.finalize)(&mut obj, self);
            self.objects.insert(id, obj);
        }

        // Phase 4: all_finalized
        for &id in &ids {
            let desc = self.objects[&id].class;
            let obj = self.objects.get_mut(&id).unwrap();
            (desc.all_finalized)(obj, self);
        }

        self.elaborated = true;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("serialize failed: {0}")]
    Serialize(String),
    #[error("deserialize failed: {0}")]
    Deserialize(String),
    #[error("unknown class '{0}' in checkpoint")]
    UnknownClass(String),
    #[error("object '{object}' attr restore failed: {source}")]
    AttrError { object: String, #[source] source: AttrError },
}
```

---

## 7. AttrError Enum

```rust
#[derive(Debug, thiserror::Error)]
pub enum AttrError {
    #[error("attribute '{attr}' is not registered for this class")]
    Unknown { attr: String },

    #[error("attribute '{attr}': type mismatch — expected {expected}, got {got}")]
    TypeMismatch { attr: &'static str, expected: &'static str, got: &'static str },

    #[error("attribute '{attr}': invalid value — {msg}")]
    InvalidValue { attr: &'static str, msg: String },

    #[error("attribute '{attr}' is read-only")]
    ReadOnly { attr: &'static str },

    #[error("attribute '{attr}' is required but absent from checkpoint")]
    MissingRequired { attr: String },

    #[error("attribute '{attr}' requires HelmObject context — use HelmObject::set_attr")]
    NeedsObjectContext { attr: String },

    #[error("attribute '{attr}': value out of range {min}..{max}, got {got}")]
    OutOfRange {
        attr: &'static str,
        min:  i64,
        max:  i64,
        got:  i64,
    },
}

impl AttrValue {
    /// Return a short type name for error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            AttrValue::Integer(_) => "Integer",
            AttrValue::Float(_)   => "Float",
            AttrValue::Bool(_)    => "Bool",
            AttrValue::Str(_)     => "Str",
            AttrValue::Object(_)  => "Object",
            AttrValue::Port(_, _) => "Port",
            AttrValue::List(_)    => "List",
            AttrValue::Dict(_)    => "Dict",
            AttrValue::Nil        => "Nil",
        }
    }
}
```

---

## 8. Mapping to SIMICS SIM_register_typed_attribute

| SIMICS concept | helm-engine equivalent |
|---|---|
| `SIM_register_typed_attribute(cls, name, get, set, Sim_Attr_Required, type, desc)` | `AttrDescriptor { name, kind: AttrKind::Required, get, set, desc, .. }` in ClassDescriptor's attr list |
| `Sim_Attr_Required` | `AttrKind::Required` |
| `Sim_Attr_Optional` | `AttrKind::Optional` |
| `Sim_Attr_Session` | `AttrKind::Session` |
| `Sim_Attr_Pseudo` | `AttrKind::Pseudo` |
| `get_attr_t` function pointer | `AttrDescriptor::get: fn(&HelmObject) -> AttrValue` |
| `set_attr_t` function pointer | `AttrDescriptor::set: fn(&mut HelmObject, AttrValue) -> Result<(), AttrError>` |
| `SIM_get_attribute(obj, name)` | `HelmObject::get_attr(name) -> Option<&AttrValue>` |
| `SIM_set_attribute(obj, name, val)` | `HelmObject::set_attr(name, val) -> Result<(), AttrError>` |
| `VT_save_object_state()` | `AttrStore::serialize_persistent()` |
| `VT_restore_object_state()` | `AttrStore::restore_from()` |
| SIMICS checkpoint format (text, property list) | CBOR blob via `ciborium` |

The key divergence from SIMICS: `AttrDescriptor` in helm-engine is a static struct in the ClassDescriptor's attr list, not a runtime call to a registration function. This avoids the global mutable state required by `SIM_register_typed_attribute` while preserving the same semantic contract.

---

## 9. Python Attribute Access

Python accesses attributes via dot-path strings on the live World after elaboration.

### Python-Side API

```python
# Get attribute value
clock_hz = world.get("board.uart0.clock_hz")   # returns int
loopback  = world.get("board.uart0.loopback")   # returns bool

# Set attribute value (calls AttrStore::set after AttrDescriptor::set validation)
world.set("board.uart0.loopback", True)
world.set("board.uart0.clock_hz", 3_686_400)

# Describe an attribute (returns AttrDescriptor metadata as dict)
info = world.describe_attr("board.uart0", "clock_hz")
# → {"name": "clock_hz", "kind": "Required", "desc": "Reference clock frequency in Hz."}
```

### Rust-Side PyO3 Binding

```rust
// In helm-python

#[pymethods]
impl PyWorld {
    /// Get an attribute by "object_path.attr_name" dotted path.
    fn get(&self, py: Python<'_>, path: &str) -> PyResult<PyObject> {
        let (obj_path, attr_name) = split_attr_path(path)
            .ok_or_else(|| PyErr::new::<PyValueError, _>(
                format!("invalid attr path: '{path}'")
            ))?;

        let world = self.world.borrow();
        let obj = world.get_object(obj_path)
            .ok_or_else(|| PyErr::new::<PyKeyError, _>(
                format!("no object named '{obj_path}'")
            ))?;

        // Route through AttrDescriptor::get for Pseudo attrs
        let desc = obj.attrs.descriptor(attr_name);
        let val = match desc {
            Some(d) if d.kind == AttrKind::Pseudo => (d.get)(obj),
            _ => obj.get_attr(attr_name)
                     .cloned()
                     .unwrap_or(AttrValue::Nil),
        };

        attr_value_to_python(py, &val)
    }

    /// Set an attribute by "object_path.attr_name" dotted path.
    fn set(&self, py: Python<'_>, path: &str, value: PyObject) -> PyResult<()> {
        let (obj_path, attr_name) = split_attr_path(path)
            .ok_or_else(|| PyErr::new::<PyValueError, _>(
                format!("invalid attr path: '{path}'")
            ))?;

        let attr_val = python_to_attr_value(py, &value)?;

        let mut world = self.world.borrow_mut();
        // Must borrow-split: get obj, then call AttrDescriptor::set (needs &mut HelmObject)
        let id = world.by_name_id(obj_path)
            .ok_or_else(|| PyErr::new::<PyKeyError, _>(
                format!("no object named '{obj_path}'")
            ))?;

        let obj = world.get_object_mut(id).unwrap();
        obj.set_attr(attr_name, attr_val).map_err(|e| {
            PyErr::new::<PyValueError, _>(e.to_string())
        })
    }
}

/// Split "board.uart0.clock_hz" → ("board.uart0", "clock_hz")
fn split_attr_path(path: &str) -> Option<(&str, &str)> {
    let dot = path.rfind('.')?;
    Some((&path[..dot], &path[dot+1..]))
}
```

### AttrValue ↔ Python Type Mapping

| AttrValue | Python type |
|---|---|
| `Integer(i64)` | `int` |
| `Float(f64)` | `float` |
| `Bool(bool)` | `bool` |
| `Str(String)` | `str` |
| `Object(id)` | `str` (dot-path name of the object) |
| `Port(id, port)` | `tuple[str, str]` (object name, port name) |
| `List(items)` | `list` (recursive) |
| `Dict(pairs)` | `dict` (recursive) |
| `Nil` | `None` |

---

## 10. AttrDescriptor Registration Pattern

`AttrDescriptor`s are listed inline in the `ClassDescriptor` struct, registered via `inventory::submit!`. The full class + attr registration for a minimal device:

```rust
// Static attr table for uart16550 — shared across all instances
static UART16550_ATTRS: &[AttrDescriptor] = &[
    AttrDescriptor {
        name:    "clock_hz",
        kind:    AttrKind::Required,
        default: Some(AttrValue::Integer(1_843_200)),
        get:     uart_get_clock_hz,
        set:     uart_set_clock_hz,
        desc:    "Reference clock frequency in Hz. Determines baud rate divisor range.",
    },
    AttrDescriptor {
        name:    "loopback",
        kind:    AttrKind::Optional,
        default: Some(AttrValue::Bool(false)),
        get:     uart_get_loopback,
        set:     uart_set_loopback,
        desc:    "Enable internal loopback (TX connected to RX). Default: false.",
    },
    AttrDescriptor {
        name:    "divisor_latch",
        kind:    AttrKind::Required,
        default: Some(AttrValue::Integer(12)),   // 1.8432 MHz / (16 * 12) = 9600 baud
        get:     uart_get_divisor_latch,
        set:     uart_set_divisor_latch,
        desc:    "Baud rate divisor latch register. Persisted across checkpoints.",
    },
    AttrDescriptor {
        name:    "rx_fifo",
        kind:    AttrKind::Required,
        default: Some(AttrValue::List(vec![])),
        get:     uart_get_rx_fifo,
        set:     uart_set_rx_fifo,
        desc:    "RX FIFO contents as List of Integer byte values. Required for checkpoint.",
    },
    AttrDescriptor {
        name:    "tx_fifo",
        kind:    AttrKind::Required,
        default: Some(AttrValue::List(vec![])),
        get:     uart_get_tx_fifo,
        set:     uart_set_tx_fifo,
        desc:    "TX FIFO contents as List of Integer byte values. Required for checkpoint.",
    },
    AttrDescriptor {
        name:    "irq_out",
        kind:    AttrKind::Optional,
        default: None,
        get:     uart_get_irq_out,
        set:     uart_set_irq_out,
        desc:    "Port connection to interrupt controller input. \
                  Set to a Port value: (plic_object_id, 'input_N').",
    },
    AttrDescriptor {
        name:    "baud_rate",   // Pseudo: computed from clock_hz and divisor_latch
        kind:    AttrKind::Pseudo,
        default: None,
        get:     uart_get_baud_rate,
        set:     uart_set_baud_rate,  // returns AttrError::ReadOnly
        desc:    "Current baud rate (read-only). Computed as clock_hz / (16 * divisor_latch).",
    },
];
```

The `ClassDescriptor` then references this table:

```rust
pub struct ClassDescriptor {
    pub name:          &'static str,
    pub kind:          ObjectKind,
    pub attrs:         &'static [AttrDescriptor],   // ← added field
    pub alloc:         fn() -> Box<dyn Any + Send>,
    pub init:          fn(&mut HelmObject),
    pub finalize:      fn(&mut HelmObject, &mut World),
    pub all_finalized: fn(&mut HelmObject, &World),
    pub deinit:        fn(&mut HelmObject),
}

inventory::submit! {
    ClassDescriptor {
        name:          "uart16550",
        kind:          ObjectKind::Persistent,
        attrs:         UART16550_ATTRS,
        alloc:         uart_alloc,
        init:          uart_init,
        finalize:      uart_finalize,
        all_finalized: uart_all_finalized,
        deinit:        uart_deinit,
    }
}
```

---

## 11. Full Class Example

Complete attribute system implementation for a minimal 16550 UART, showing all `AttrKind` variants:

```rust
// helm-devices/src/uart16550/attrs.rs

use helm_world::{HelmObject, AttrValue, AttrError};
use super::Uart16550State;

// ── Required attr: clock_hz ──────────────────────────────────────────────────

pub fn get_clock_hz(obj: &HelmObject) -> AttrValue {
    obj.attrs.get("clock_hz").cloned().unwrap_or(AttrValue::Integer(1_843_200))
}

pub fn set_clock_hz(obj: &mut HelmObject, val: AttrValue) -> Result<(), AttrError> {
    let AttrValue::Integer(hz) = val else {
        return Err(AttrError::TypeMismatch {
            attr: "clock_hz", expected: "Integer", got: val.type_name(),
        });
    };
    if hz <= 0 {
        return Err(AttrError::InvalidValue {
            attr: "clock_hz", msg: "must be positive".into(),
        });
    }
    obj.data_mut::<Uart16550State>().clock_hz = hz as u64;
    obj.attrs.values.insert("clock_hz", AttrValue::Integer(hz));
    Ok(())
}

// ── Required attr: rx_fifo ────────────────────────────────────────────────────

pub fn get_rx_fifo(obj: &HelmObject) -> AttrValue {
    let state = obj.data::<Uart16550State>();
    AttrValue::List(
        state.rx_fifo.iter().map(|&b| AttrValue::Integer(b as i64)).collect()
    )
}

pub fn set_rx_fifo(obj: &mut HelmObject, val: AttrValue) -> Result<(), AttrError> {
    let AttrValue::List(items) = val else {
        return Err(AttrError::TypeMismatch {
            attr: "rx_fifo", expected: "List", got: val.type_name(),
        });
    };
    let mut fifo = std::collections::VecDeque::new();
    for item in &items {
        match item {
            AttrValue::Integer(b) if *b >= 0 && *b <= 255 => fifo.push_back(*b as u8),
            AttrValue::Integer(b) => return Err(AttrError::OutOfRange {
                attr: "rx_fifo", min: 0, max: 255, got: *b,
            }),
            _ => return Err(AttrError::TypeMismatch {
                attr: "rx_fifo", expected: "Integer", got: item.type_name(),
            }),
        }
    }
    obj.data_mut::<Uart16550State>().rx_fifo = fifo;
    obj.attrs.values.insert("rx_fifo", AttrValue::List(items));
    Ok(())
}

// ── Session attr: host_fd ─────────────────────────────────────────────────────
// A host file descriptor for PTY-based UART output. Not checkpointed.
// On restore it's left as -1 (no host I/O) and the caller re-opens the PTY.

pub fn get_host_fd(obj: &HelmObject) -> AttrValue {
    obj.attrs.get("host_fd").cloned().unwrap_or(AttrValue::Integer(-1))
}

pub fn set_host_fd(obj: &mut HelmObject, val: AttrValue) -> Result<(), AttrError> {
    let AttrValue::Integer(fd) = val else {
        return Err(AttrError::TypeMismatch {
            attr: "host_fd", expected: "Integer", got: val.type_name(),
        });
    };
    obj.data_mut::<Uart16550State>().host_fd = fd as i32;
    obj.attrs.values.insert("host_fd", AttrValue::Integer(fd));
    Ok(())
}

// ── Pseudo attr: baud_rate ────────────────────────────────────────────────────
// Computed from clock_hz and divisor_latch. Read-only.

pub fn get_baud_rate(obj: &HelmObject) -> AttrValue {
    let state = obj.data::<Uart16550State>();
    let divisor = state.divisor_latch.max(1) as u64;
    AttrValue::Integer((state.clock_hz / (16 * divisor)) as i64)
}

pub fn set_baud_rate(_obj: &mut HelmObject, _val: AttrValue) -> Result<(), AttrError> {
    Err(AttrError::ReadOnly { attr: "baud_rate" })
}

// ── Checkpoint round-trip test ─────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use helm_world::{World, PendingObject, AttrValue, AttrKind};

    #[test]
    fn test_uart_checkpoint_round_trip() {
        // Setup
        let mut world = World::new();
        world.instantiate(vec![
            PendingObject::new("board.uart0", "uart16550")
                .set("clock_hz",       1_843_200i64)
                .set("divisor_latch",  12i64),
        ]).unwrap();

        // Write some bytes into the RX FIFO to create non-default Required state
        {
            let obj = world.get_object_mut(
                *world.by_name_id("board.uart0").unwrap()
            ).unwrap();
            let state = obj.data_mut::<Uart16550State>();
            state.rx_fifo.push_back(0x41); // 'A'
            state.rx_fifo.push_back(0x42); // 'B'
        }

        // Checkpoint save
        let blob = world.checkpoint_save().expect("save failed");

        // Restore into a fresh World
        let mut world2 = World::new();
        world2.checkpoint_restore(&blob).expect("restore failed");

        // Verify Required attrs survived
        let obj = world2.get_object("board.uart0").unwrap();
        assert_eq!(obj.get_attr("clock_hz"), Some(&AttrValue::Integer(1_843_200)));
        assert_eq!(obj.get_attr("divisor_latch"), Some(&AttrValue::Integer(12)));

        // Verify RX FIFO survived (Required attr)
        let rx_fifo = obj.get_attr("rx_fifo").unwrap();
        assert_eq!(
            rx_fifo,
            &AttrValue::List(vec![
                AttrValue::Integer(0x41),
                AttrValue::Integer(0x42),
            ])
        );

        // Verify Session attr (host_fd) was reset to default (-1)
        let host_fd = obj.get_attr("host_fd").unwrap();
        assert_eq!(*host_fd, AttrValue::Integer(-1), "host_fd should be reset on restore");

        // Verify Pseudo attr is computable (not stored, computed on read)
        let uart_id = *world2.by_name_id("board.uart0").unwrap();
        let obj2    = world2.get_object_mut(uart_id).unwrap();
        let desc    = obj2.attrs.descriptor("baud_rate").unwrap();
        assert_eq!(desc.kind, AttrKind::Pseudo);
        let baud = (desc.get)(obj2);
        // 1_843_200 / (16 * 12) = 9600
        assert_eq!(baud, AttrValue::Integer(9600));
    }
}
```

---

*For the object model, see [`LLD-object-model.md`](./LLD-object-model.md). For the HLD, see [`HLD.md`](./HLD.md).*

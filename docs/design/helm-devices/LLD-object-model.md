# helm-engine — LLD: Object Model

> Complete Rust API specification for `World`, `HelmObject`, `ClassDescriptor`, `InterfaceRegistry`, and `PendingObject`.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-attr-system.md`](./LLD-attr-system.md)

---

## Table of Contents

1. [Core Types](#1-core-types)
2. [ClassDescriptor and ClassRegistry](#2-classdescriptor-and-classregistry)
3. [HelmObject](#3-helmobject)
4. [InterfaceRegistry](#4-interfaceregistry)
5. [World Struct](#5-world-struct)
6. [PendingObject](#6-pendingobject)
7. [World::instantiate() — Four-Phase Lifecycle](#7-worldinstantiate--four-phase-lifecycle)
8. [Forward Reference Resolution](#8-forward-reference-resolution)
9. [World::run() — CPU-Aware Dispatch](#9-worldrun--cpu-aware-dispatch)
10. [Self-Registration Pattern](#10-self-registration-pattern)
11. [Error Types](#11-error-types)
12. [Full Usage Example](#12-full-usage-example)

---

## 1. Core Types

```rust
/// Stable index for a HelmObject within a World.
///
/// u32, assigned at alloc time, never reused for the lifetime of the World.
/// Suitable as a map key and as an AttrValue::Object payload.
pub type HelmObjectId = u32;

/// Reason returned by World::run().
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// Requested number of instructions executed (CPU mode only).
    InstructionLimit,
    /// EventQueue drained with no pending events (device-only mode).
    EventQueueEmpty,
    /// A breakpoint or watchpoint was hit.
    Breakpoint { pc: u64 },
    /// A halt / WFI instruction was executed.
    Halted,
    /// An unhandled exception was raised.
    Exception { vector: u32, pc: u64 },
    /// Simulation stopped by user request (e.g. Python Ctrl-C, until callback).
    UserStop,
}

/// ObjectKind controls checkpoint participation and reset behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    /// Saved in checkpoint. Normal device/CPU objects.
    Persistent,
    /// Not saved in checkpoint. Created fresh on restore. E.g., OS handles, host resources.
    Session,
    /// Not saved. No persistent backing. Exists only at runtime.
    Pseudo,
}
```

---

## 2. ClassDescriptor and ClassRegistry

### ClassDescriptor

```rust
/// Registered once per Rust type. The runtime "vtable" for a simulated class.
///
/// SIMICS equivalent: class_info_t + SIM_register_class().
/// Registered globally at program startup via inventory::submit!.
pub struct ClassDescriptor {
    /// Unique class name. Used as the key in PendingObject and in ClassRegistry.
    pub name: &'static str,

    /// Object kind — controls checkpoint behavior (see ObjectKind).
    pub kind: ObjectKind,

    /// Allocate and return device-specific state.
    ///
    /// Called once per PendingObject during World::instantiate() phase 1 (alloc).
    /// Returns a Box<dyn Any + Send> stored as HelmObject::data.
    pub alloc: fn() -> Box<dyn Any + Send>,

    /// One-time initialization of a newly-allocated HelmObject.
    ///
    /// Called immediately after alloc(). The object's AttrStore is populated
    /// with defaults. No cross-object references are valid yet.
    pub init: fn(&mut HelmObject),

    /// Cross-object wiring. Called during phase 3 (finalize) after all objects are allocated.
    ///
    /// Devices acquire Arc<EventQueue>, Arc<HelmEventBus>, and InterfaceRegistry refs here.
    /// MMIO regions are registered in World's MemoryMap here.
    /// Interrupt wires are connected here.
    pub finalize: fn(&mut HelmObject, &mut World),

    /// Post-finalize validation. Called during phase 4 (all_finalized) after all finalize() calls.
    ///
    /// Objects verify that cross-object dependencies are satisfied.
    /// World reference is immutable — no further wiring allowed.
    pub all_finalized: fn(&mut HelmObject, &World),

    /// Cleanup. Called when World is dropped, in reverse registration order.
    pub deinit: fn(&mut HelmObject),
}
```

### ClassRegistry

```rust
/// Global registry of all ClassDescriptors.
///
/// Populated at startup via inventory::collect! + inventory::submit!.
/// Queried by World::instantiate() to find the descriptor for each PendingObject's class name.
pub struct ClassRegistry {
    map: HashMap<&'static str, &'static ClassDescriptor>,
}

impl ClassRegistry {
    /// Return the global singleton, populated by inventory at startup.
    pub fn global() -> &'static ClassRegistry {
        static INSTANCE: OnceLock<ClassRegistry> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            let mut map = HashMap::new();
            for desc in inventory::iter::<ClassDescriptor> {
                assert!(
                    map.insert(desc.name, desc).is_none(),
                    "duplicate class registration: '{}'", desc.name
                );
            }
            ClassRegistry { map }
        })
    }

    pub fn get(&self, name: &str) -> Option<&'static ClassDescriptor> {
        self.map.get(name).copied()
    }
}

// Make ClassDescriptor registerable via inventory
inventory::collect!(ClassDescriptor);
```

---

## 3. HelmObject

```rust
/// Universal handle for every simulated entity in a World.
///
/// Owns device-specific state via Box<dyn Any + Send>.
/// All persistent state is exposed through AttrStore — no dark state.
pub struct HelmObject {
    /// Stable index, assigned at alloc time. Never 0 (0 is reserved as Null).
    pub id: HelmObjectId,

    /// The registered class descriptor for this object's type.
    pub class: &'static ClassDescriptor,

    /// Dot-path name within the simulation hierarchy.
    ///
    /// Examples: "board", "board.uart0", "board.cpu0", "board.plic"
    /// Unique within a World.
    pub name: String,

    /// Parent object in the hierarchy, if any.
    ///
    /// "board.uart0" has parent Some(id_of_board).
    /// Top-level objects (e.g. "board") have parent None.
    pub parent: Option<HelmObjectId>,

    /// Child objects in the hierarchy.
    ///
    /// Populated by World::instantiate() from the name dot-paths.
    pub children: Vec<HelmObjectId>,

    /// Per-object attribute storage. All persistent state lives here.
    pub(crate) attrs: AttrStore,

    /// Device-specific state, opaque to World.
    ///
    /// Downcast via HelmObject::data::<T>() / data_mut::<T>().
    pub(crate) data: Box<dyn Any + Send>,
}

impl HelmObject {
    /// Access device-specific state as type T.
    ///
    /// Panics if the stored type is not T.
    pub fn data<T: 'static>(&self) -> &T {
        self.data.downcast_ref::<T>()
            .unwrap_or_else(|| panic!(
                "HelmObject '{}': data type mismatch — expected {}",
                self.name, std::any::type_name::<T>()
            ))
    }

    /// Mutable access to device-specific state as type T.
    pub fn data_mut<T: 'static>(&mut self) -> &mut T {
        let name = self.name.clone();
        self.data.downcast_mut::<T>()
            .unwrap_or_else(|| panic!(
                "HelmObject '{}': data type mismatch — expected {}",
                name, std::any::type_name::<T>()
            ))
    }

    /// Get an attribute value by name.
    pub fn get_attr(&self, name: &str) -> Option<&AttrValue> {
        self.attrs.get(name)
    }

    /// Set an attribute value by name.
    pub fn set_attr(&mut self, name: &str, val: AttrValue) -> Result<(), AttrError> {
        self.attrs.set(name, val)
    }

    /// Return true if this object's ClassDescriptor name matches `class_name`.
    pub fn is_class(&self, class_name: &str) -> bool {
        self.class.name == class_name
    }
}
```

---

## 4. InterfaceRegistry

```rust
/// Runtime-discoverable typed interfaces between objects.
///
/// SIMICS equivalent: SIM_get_interface() / SIM_register_interface().
/// Indexed by (class_name, interface_name, optional_port_name).
pub struct InterfaceRegistry {
    // Key: (class_name, interface_name, port_name)
    // port_name = None for singleton interfaces, Some("input_33") for port-indexed ones.
    map: HashMap<(&'static str, &'static str, Option<String>), Arc<dyn Any + Send + Sync>>,
}

impl InterfaceRegistry {
    pub fn new() -> Self {
        InterfaceRegistry { map: HashMap::new() }
    }

    /// Register a singleton interface vtable for a class.
    ///
    /// Example: register::<SignalInterface>("plic", "signal", plic_signal_vtable)
    ///
    /// Panics if the (class, iface) pair is already registered.
    pub fn register<T: Send + Sync + 'static>(
        &mut self,
        class: &'static str,
        iface: &'static str,
        vtable: T,
    ) {
        let key = (class, iface, None);
        let prev = self.map.insert(key, Arc::new(vtable));
        assert!(
            prev.is_none(),
            "InterfaceRegistry: duplicate registration for ({class}, {iface})"
        );
    }

    /// Register a port-indexed interface vtable.
    ///
    /// Example: register_port::<SignalInterface>("plic", "signal", "input_33", vtable)
    ///
    /// Panics if the (class, iface, port) triple is already registered.
    pub fn register_port<T: Send + Sync + 'static>(
        &mut self,
        class: &'static str,
        iface: &'static str,
        port: &str,
        vtable: T,
    ) {
        let key = (class, iface, Some(port.to_string()));
        let prev = self.map.insert(key, Arc::new(vtable));
        assert!(
            prev.is_none(),
            "InterfaceRegistry: duplicate registration for ({class}, {iface}, port={port})"
        );
    }

    /// Look up a singleton interface for an object.
    ///
    /// Returns None if not registered or if the stored type does not match T.
    pub fn get<T: 'static>(&self, obj: &HelmObject, iface: &str) -> Option<Arc<T>> {
        let key = (obj.class.name, iface, None);
        self.map.get(&key)?.clone().downcast::<T>().ok()
    }

    /// Look up a port-indexed interface for an object.
    pub fn get_port<T: 'static>(
        &self,
        obj: &HelmObject,
        iface: &str,
        port: &str,
    ) -> Option<Arc<T>> {
        let key = (obj.class.name, iface, Some(port.to_string()));
        self.map.get(&key)?.clone().downcast::<T>().ok()
    }
}
```

---

## 5. World Struct

```rust
// crates/helm-engine/src/world.rs

use slotmap::{SlotMap, new_key_type};
use std::collections::HashMap;
use std::sync::Arc;
use helm_memory::MemoryMap;
use helm_event::EventQueue;
use helm_devices::bus::event_bus::HelmEventBus;

// SlotMap key type for HelmObjects.
// We expose HelmObjectId as u32 externally; SlotMap uses its own DefaultKey internally.
// The by_id map provides HelmObjectId -> SlotMap key translation.

/// The simulation world. Owns all HelmObjects and all simulation infrastructure.
///
/// A World with HelmEngine objects runs CPU simulation.
/// A World with only device objects runs device-only simulation.
/// The API is identical in both cases.
pub struct World {
    /// All objects, keyed by HelmObjectId.
    objects: HashMap<HelmObjectId, HelmObject>,

    /// Fast lookup by dot-path name.
    by_name: HashMap<String, HelmObjectId>,

    /// Typed interface registry.
    interfaces: InterfaceRegistry,

    /// Unified MMIO address space.
    memory: MemoryMap,

    /// Time-ordered device timer callbacks.
    events: Arc<EventQueue>,

    /// Synchronous observable event bus.
    event_bus: Arc<HelmEventBus>,

    /// Monotonic virtual clock (tick = one reference clock cycle).
    clock: VirtualClock,

    /// Present only if HelmEngine objects were registered during instantiate().
    /// Set to Some(Scheduler::new(...)) during finalize of the HelmEngine class.
    scheduler: Option<Box<dyn SchedulerTrait>>,

    /// Monotonic ID counter. Starts at 1 (0 is reserved as the null id).
    next_id: HelmObjectId,

    /// True after instantiate() completes successfully.
    elaborated: bool,
}

impl World {
    /// Create an empty World.
    pub fn new() -> Self {
        World {
            objects:    HashMap::new(),
            by_name:    HashMap::new(),
            interfaces: InterfaceRegistry::new(),
            memory:     MemoryMap::new(),
            events:     Arc::new(EventQueue::new()),
            event_bus:  Arc::new(HelmEventBus::new()),
            clock:      VirtualClock::new(),
            scheduler:  None,
            next_id:    1,
            elaborated: false,
        }
    }

    /// Instantiate a list of pending objects. Drives the four-phase lifecycle.
    ///
    /// See Section 7 for the full protocol.
    ///
    /// Returns Err(ConfigError) if any phase fails for any object.
    /// On error, the World is in an invalid state and should be dropped.
    pub fn instantiate(&mut self, objects: Vec<PendingObject>) -> Result<(), ConfigError> {
        self.instantiate_inner(objects)
    }

    /// Run the simulation.
    ///
    /// If HelmEngine objects are present: runs the Scheduler quantum loop.
    /// If no HelmEngine: drains EventQueue until empty or a stop condition fires.
    ///
    /// Panics if called before instantiate().
    pub fn run(&mut self) -> StopReason {
        assert!(self.elaborated, "World::run() called before instantiate()");
        if let Some(ref mut sched) = self.scheduler {
            sched.run(&mut self.objects, &self.events, &self.event_bus, &mut self.clock)
        } else {
            self.drain_events()
        }
    }

    /// Advance the virtual clock by `cycles` ticks, draining EventQueue events along the way.
    ///
    /// Not available when HelmEngine objects are present (use run() instead).
    ///
    /// Panics if called before instantiate() or if HelmEngine is registered.
    pub fn advance(&mut self, cycles: u64) {
        assert!(self.elaborated, "World::advance() called before instantiate()");
        assert!(
            self.scheduler.is_none(),
            "World::advance() is not valid when HelmEngine objects are present — use run()"
        );
        let target = self.clock.current_tick() + cycles;
        while let Some(tick) = self.events.peek_tick() {
            if tick > target { break; }
            let event = self.events.pop().unwrap();
            self.clock.set(event.tick);
            (event.callback)();
        }
        self.clock.set(target);
    }

    /// Look up an object by dot-path name.
    pub fn get_object(&self, name: &str) -> Option<&HelmObject> {
        let id = self.by_name.get(name)?;
        self.objects.get(id)
    }

    /// Look up an object by id.
    pub fn get_object_mut(&mut self, id: HelmObjectId) -> Option<&mut HelmObject> {
        self.objects.get_mut(&id)
    }

    /// Perform an MMIO write.
    ///
    /// Dispatches to the device mapped at `addr` via MemoryMap.
    /// Fires HelmEvent::MemWrite on the event bus.
    ///
    /// Returns Err(MemFault) if no device is mapped at `addr`.
    pub fn mmio_write(&mut self, addr: u64, size: usize, val: u64) -> Result<(), MemFault> {
        assert!(self.elaborated, "World::mmio_write() called before instantiate()");
        let (id, offset) = self.memory.lookup(addr)
            .ok_or(MemFault::NoDevice { addr })?;
        let obj = self.objects.get_mut(&id)
            .ok_or(MemFault::NoDevice { addr })?;
        // Dispatch through ClassDescriptor-registered MMIO handler
        let handler = self.interfaces
            .get::<MmioInterface>(obj, "mmio")
            .ok_or(MemFault::NoDevice { addr })?;
        handler.write(obj, offset, size, val);
        self.event_bus.fire(HelmEvent::MemWrite {
            addr,
            size,
            val,
            cycle: self.clock.current_tick(),
        });
        Ok(())
    }

    /// Perform an MMIO read.
    ///
    /// Returns Err(MemFault) if no device is mapped at `addr`.
    pub fn mmio_read(&self, addr: u64, size: usize) -> Result<u64, MemFault> {
        assert!(self.elaborated, "World::mmio_read() called before instantiate()");
        let (id, offset) = self.memory.lookup(addr)
            .ok_or(MemFault::NoDevice { addr })?;
        let obj = self.objects.get(&id)
            .ok_or(MemFault::NoDevice { addr })?;
        let handler = self.interfaces
            .get::<MmioInterface>(obj, "mmio")
            .ok_or(MemFault::NoDevice { addr })?;
        let val = handler.read(obj, offset, size);
        Ok(val)
    }

    /// Map a HelmObject into the MMIO address space at `base`.
    ///
    /// The object must have registered a "mmio" MmioInterface.
    /// Called from ClassDescriptor::finalize() implementations.
    pub fn map_object(&mut self, id: HelmObjectId, base: u64) {
        let obj = self.objects.get(&id)
            .unwrap_or_else(|| panic!("map_object: unknown id {id}"));
        let size = self.interfaces
            .get::<MmioInterface>(obj, "mmio")
            .unwrap_or_else(|| panic!(
                "map_object: object '{}' has no 'mmio' interface", obj.name
            ))
            .region_size(obj);
        self.memory.register_mmio(base, size, id);
    }

    /// Connect two objects' interrupt interface.
    ///
    /// `from` must have a "signal_out" SignalInterface.
    /// `to` must have a "signal_in" (or port-indexed signal_in) SignalInterface.
    ///
    /// Called from ClassDescriptor::finalize() implementations.
    pub fn wire_interrupt(&mut self, from: &str, to: &str) -> Result<(), ConfigError> {
        let _ = (from, to); // resolved via InterfaceRegistry in finalize()
        // Full impl: look up both objects, connect their signal interfaces
        todo!("wire_interrupt: resolved via InterfaceRegistry in finalize()")
    }

    /// Look up a typed interface on a named object.
    pub fn get_interface<T: 'static>(
        &self,
        name: &str,
        iface: &str,
    ) -> Option<Arc<T>> {
        let obj = self.get_object(name)?;
        self.interfaces.get::<T>(obj, iface)
    }

    /// Return the current virtual clock tick.
    pub fn current_tick(&self) -> u64 {
        self.clock.current_tick()
    }

    /// Return a reference to the event bus.
    pub fn event_bus(&self) -> &Arc<HelmEventBus> {
        &self.event_bus
    }

    /// Return a reference to the event queue (for use by devices in finalize()).
    pub fn event_queue(&self) -> &Arc<EventQueue> {
        &self.events
    }

    /// Return a reference to the interface registry (for use by devices in finalize()).
    pub fn interfaces_mut(&mut self) -> &mut InterfaceRegistry {
        &mut self.interfaces
    }

    // Drain EventQueue until empty (device-only run()).
    fn drain_events(&mut self) -> StopReason {
        loop {
            match self.events.pop() {
                None => return StopReason::EventQueueEmpty,
                Some(event) => {
                    self.clock.set(event.tick);
                    (event.callback)();
                }
            }
        }
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}
```

---

## 6. PendingObject

```rust
/// A description of one object to be created by World::instantiate().
///
/// Carries the class name, the object's dot-path name in the hierarchy,
/// and a list of attribute assignments (possibly including forward references).
///
/// No side effects. No allocation. Pure data.
pub struct PendingObject {
    /// Registered class name. Must exist in ClassRegistry.
    pub class: &'static str,

    /// Dot-path name in the object hierarchy. Must be unique within the World.
    pub name: String,

    /// Attribute assignments. Applied during phase 2 (set attrs).
    attrs: Vec<(String, PendingAttrValue)>,
}

/// Attribute value in a PendingObject — may be a forward reference to another object.
pub(crate) enum PendingAttrValue {
    /// A fully-resolved value (no object references).
    Resolved(AttrValue),
    /// A forward reference to another object by dot-path name.
    /// Resolved to AttrValue::Object(id) after all objects are allocated.
    ObjectRef(String),
    /// A forward reference to a port on another object.
    /// Resolved to AttrValue::Port(id, port_name) after all objects are allocated.
    PortRef(String, String),
}

impl PendingObject {
    pub fn new(name: &str, class: &'static str) -> Self {
        PendingObject {
            class,
            name: name.to_string(),
            attrs: Vec::new(),
        }
    }

    /// Set a resolved attribute value (builder pattern).
    pub fn set(mut self, attr: &str, val: impl Into<AttrValue>) -> Self {
        self.attrs.push((attr.to_string(), PendingAttrValue::Resolved(val.into())));
        self
    }

    /// Set a forward-reference attribute (resolved after all objects are allocated).
    ///
    /// Use when attribute value is the identity of another object.
    /// Example: .set_obj("irq_target", "board.plic")
    pub fn set_obj(mut self, attr: &str, name: &str) -> Self {
        self.attrs.push((attr.to_string(), PendingAttrValue::ObjectRef(name.to_string())));
        self
    }

    /// Set a forward-reference to a port on another object.
    ///
    /// Example: .set_port("irq_out", "board.plic", "input_10")
    pub fn set_port(mut self, attr: &str, obj_name: &str, port: &str) -> Self {
        self.attrs.push((attr.to_string(), PendingAttrValue::PortRef(
            obj_name.to_string(),
            port.to_string(),
        )));
        self
    }
}
```

---

## 7. World::instantiate() — Four-Phase Lifecycle

```rust
impl World {
    fn instantiate_inner(&mut self, pending: Vec<PendingObject>) -> Result<(), ConfigError> {
        let registry = ClassRegistry::global();

        // ── Phase 1: Alloc ──────────────────────────────────────────────────
        // Allocate all objects before any finalize() is called.
        // This guarantees that by_name is fully populated before phase 2.

        for p in &pending {
            let desc = registry.get(p.class)
                .ok_or_else(|| ConfigError::UnknownClass(p.class.to_string()))?;

            if self.by_name.contains_key(&p.name) {
                return Err(ConfigError::DuplicateName(p.name.clone()));
            }

            let id = self.next_id;
            self.next_id += 1;

            let data = (desc.alloc)();

            let obj = HelmObject {
                id,
                class: desc,
                name: p.name.clone(),
                parent: None,      // wired below from dot-path
                children: Vec::new(),
                attrs: AttrStore::new(desc),
                data,
            };

            self.objects.insert(id, obj);
            self.by_name.insert(p.name.clone(), id);
        }

        // Wire parent/child from dot-paths
        self.wire_hierarchy(&pending)?;

        // ── Phase 2: Init + Set Attrs ────────────────────────────────────────
        // init() populates default attr values.
        // Then apply the PendingObject's attribute overrides.

        for p in &pending {
            let id = *self.by_name.get(&p.name).unwrap();

            // init() — must not borrow World mutably through HelmObject
            let desc = self.objects[&id].class;
            {
                let obj = self.objects.get_mut(&id).unwrap();
                (desc.init)(obj);
            }

            // Set attrs — resolve forward refs first
            for (attr_name, pending_val) in &p.attrs {
                let resolved = self.resolve_pending_value(pending_val)?;
                let obj = self.objects.get_mut(&id).unwrap();
                obj.attrs.set(attr_name, resolved)
                    .map_err(|e| ConfigError::AttrError {
                        object: obj.name.clone(),
                        attr: attr_name.clone(),
                        source: e,
                    })?;
            }
        }

        // ── Phase 3: Finalize ────────────────────────────────────────────────
        // Cross-object wiring. Devices connect to World infrastructure.
        // Performed in registration order (insertion order of pending list).

        let ids: Vec<HelmObjectId> = pending.iter()
            .map(|p| *self.by_name.get(&p.name).unwrap())
            .collect();

        for &id in &ids {
            let desc = self.objects[&id].class;
            // finalize receives (&mut HelmObject, &mut World) — split borrow
            // We use a two-step pattern: remove, call, re-insert.
            let mut obj = self.objects.remove(&id).unwrap();
            (desc.finalize)(&mut obj, self);
            self.objects.insert(id, obj);
        }

        // ── Phase 4: All Finalized ────────────────────────────────────────────
        // Post-wiring validation. World is immutable.

        for &id in &ids {
            let desc = self.objects[&id].class;
            let obj = self.objects.get_mut(&id).unwrap();
            (desc.all_finalized)(obj, self);
        }

        self.elaborated = true;
        Ok(())
    }

    /// Build the parent/child hierarchy from dot-path names.
    fn wire_hierarchy(&mut self, pending: &[PendingObject]) -> Result<(), ConfigError> {
        for p in pending {
            let id = *self.by_name.get(&p.name).unwrap();
            if let Some(dot) = p.name.rfind('.') {
                let parent_name = &p.name[..dot];
                let parent_id = *self.by_name.get(parent_name)
                    .ok_or_else(|| ConfigError::MissingParent {
                        object: p.name.clone(),
                        parent: parent_name.to_string(),
                    })?;
                self.objects.get_mut(&id).unwrap().parent = Some(parent_id);
                self.objects.get_mut(&parent_id).unwrap().children.push(id);
            }
        }
        Ok(())
    }

    /// Resolve a PendingAttrValue to a concrete AttrValue using the by_name map.
    fn resolve_pending_value(
        &self,
        v: &PendingAttrValue,
    ) -> Result<AttrValue, ConfigError> {
        match v {
            PendingAttrValue::Resolved(val) => Ok(val.clone()),
            PendingAttrValue::ObjectRef(name) => {
                let id = *self.by_name.get(name)
                    .ok_or_else(|| ConfigError::UnknownObject(name.clone()))?;
                Ok(AttrValue::Object(id))
            }
            PendingAttrValue::PortRef(obj_name, port) => {
                let id = *self.by_name.get(obj_name)
                    .ok_or_else(|| ConfigError::UnknownObject(obj_name.clone()))?;
                Ok(AttrValue::Port(id, port.clone()))
            }
        }
    }
}
```

---

## 8. Forward Reference Resolution

Forward references allow a `PendingObject` to reference another object by name without knowing its `HelmObjectId` at description time.

```rust
// Rust usage — describing topology without caring about id assignment order:
let objects = vec![
    PendingObject::new("board.plic", "plic")
        .set("n_sources", 32i64),

    PendingObject::new("board.uart0", "uart16550")
        .set("clock_hz", 1_843_200i64)
        .set_port("irq_out", "board.plic", "input_10"),  // forward ref resolved in phase 2

    PendingObject::new("board.uart1", "uart16550")
        .set("clock_hz", 1_843_200i64)
        .set_port("irq_out", "board.plic", "input_11"),
];

world.instantiate(objects)?;
```

The resolution contract:

1. After phase 1 (alloc), `by_name` maps every object name to its `HelmObjectId`.
2. During phase 2 (set attrs), `resolve_pending_value()` looks up `ObjectRef` and `PortRef` values in `by_name` and converts them to `AttrValue::Object(id)` / `AttrValue::Port(id, port)`.
3. `set_obj()` / `set_port()` on `PendingObject` store `ObjectRef` / `PortRef` variants, not raw ids.

This means the order of objects in the `Vec<PendingObject>` does not affect correctness of cross-object references. Circular references (A refs B refs A) are valid in this model — both are allocated before either attr is set.

---

## 9. World::run() — CPU-Aware Dispatch

```rust
impl World {
    // Called by run() when no Scheduler is present.
    fn drain_events(&mut self) -> StopReason {
        loop {
            match self.events.pop() {
                None => return StopReason::EventQueueEmpty,
                Some(event) => {
                    self.clock.set(event.tick);
                    (event.callback)();
                    // Callbacks may schedule new events — loop continues
                }
            }
        }
    }
}
```

The `SchedulerTrait` object method `run()` is implemented in `helm-engine`:

```rust
// In helm-engine::world
pub trait SchedulerTrait: Send {
    fn run(
        &mut self,
        objects:    &mut HashMap<HelmObjectId, HelmObject>,
        events:     &Arc<EventQueue>,
        event_bus:  &Arc<HelmEventBus>,
        clock:      &mut VirtualClock,
    ) -> StopReason;
}
```

`helm-engine` (World) / `helm-devices` (object model) / `helm-core` (AttrValue) holds `Option<Box<dyn SchedulerTrait>>`. `helm-engine`'s `ClassDescriptor::finalize` implementation constructs a `Scheduler` and sets `world.scheduler = Some(Box::new(scheduler))`. This is the only coupling point between the CPU simulation and the World: a trait object stored in an Option field.

When `World::run()` is called:

```rust
pub fn run(&mut self) -> StopReason {
    assert!(self.elaborated, "World::run() called before instantiate()");
    if let Some(ref mut sched) = self.scheduler {
        // CPU present: Scheduler interleaves hart quanta + EventQueue draining
        sched.run(&mut self.objects, &self.events, &self.event_bus, &mut self.clock)
    } else {
        // Device-only: drain EventQueue until empty
        self.drain_events()
    }
}
```

---

## 10. Self-Registration Pattern

Every class registers its `ClassDescriptor` at startup using the `inventory` crate. No central registration function needs to be called. Linking a crate that contains an `inventory::submit!` automatically registers its descriptors.

```rust
// In helm-devices/src/uart16550.rs

use helm_world::{ClassDescriptor, ObjectKind, HelmObject, World};

pub struct Uart16550State {
    pub clock_hz: u64,
    pub rx_fifo:  VecDeque<u8>,
    pub tx_fifo:  VecDeque<u8>,
    // ... register file
}

fn uart_alloc() -> Box<dyn Any + Send> {
    Box::new(Uart16550State {
        clock_hz: 1_843_200,
        rx_fifo:  VecDeque::new(),
        tx_fifo:  VecDeque::new(),
    })
}

fn uart_init(obj: &mut HelmObject) {
    // Set attribute defaults in AttrStore
    obj.set_attr("clock_hz", AttrValue::Integer(1_843_200)).unwrap();
    obj.set_attr("loopback", AttrValue::Bool(false)).unwrap();
}

fn uart_finalize(obj: &mut HelmObject, world: &mut World) {
    // Acquire EventQueue ref and store in device state
    let state = obj.data_mut::<Uart16550State>();
    let event_queue = world.event_queue().clone();
    // Register MMIO interface
    let clock_hz = match obj.get_attr("clock_hz") {
        Some(AttrValue::Integer(hz)) => *hz as u64,
        _ => 1_843_200,
    };
    state.clock_hz = clock_hz;
    // Register MMIO handler in InterfaceRegistry
    world.interfaces_mut().register::<MmioInterface>(
        "uart16550", "mmio", Uart16550MmioHandler { event_queue },
    );
}

fn uart_all_finalized(obj: &mut HelmObject, _world: &World) {
    // Validate: irq_out port must be connected if irq_enable attr is set
    if let Some(AttrValue::Bool(true)) = obj.get_attr("irq_enable") {
        assert!(
            obj.get_attr("irq_out").is_some(),
            "uart16550 '{}': irq_enable=true but irq_out not connected",
            obj.name
        );
    }
}

fn uart_deinit(_obj: &mut HelmObject) {}

// Self-registration: executed at program startup when this crate is linked.
inventory::submit! {
    ClassDescriptor {
        name:          "uart16550",
        kind:          ObjectKind::Persistent,
        alloc:         uart_alloc,
        init:          uart_init,
        finalize:      uart_finalize,
        all_finalized: uart_all_finalized,
        deinit:        uart_deinit,
    }
}
```

The global `ClassRegistry` is populated at the first call to `ClassRegistry::global()` by iterating `inventory::iter::<ClassDescriptor>`. Any crate that defines and submits a `ClassDescriptor` becomes a first-class participant in the World with no central registration call.

---

## 11. Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("unknown class '{0}'")]
    UnknownClass(String),

    #[error("duplicate object name '{0}'")]
    DuplicateName(String),

    #[error("object '{object}' references unknown parent '{parent}'")]
    MissingParent { object: String, parent: String },

    #[error("object '{object}' attr '{attr}': {source}")]
    AttrError {
        object: String,
        attr:   String,
        #[source]
        source: AttrError,
    },

    #[error("forward reference to unknown object '{0}'")]
    UnknownObject(String),

    #[error("object '{object}' finalize failed: {msg}")]
    FinalizeFailed { object: String, msg: String },

    #[error("validation failed: {0}")]
    Validation(String),
}

#[derive(Debug, thiserror::Error)]
pub enum MemFault {
    #[error("no device mapped at address {addr:#x}")]
    NoDevice { addr: u64 },

    #[error("access size {size} not supported at address {addr:#x}")]
    BadSize { addr: u64, size: usize },

    #[error("address {addr:#x} is not aligned to size {size}")]
    Misaligned { addr: u64, size: usize },
}
```

---

## 12. Full Usage Example

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use helm_world::{World, PendingObject, AttrValue};

    #[test]
    fn test_uart_mmio_write_read() {
        let mut world = World::new();

        let objects = vec![
            PendingObject::new("board.uart0", "uart16550")
                .set("clock_hz",  1_843_200i64)
                .set("base_addr", 0x1000_0000i64),
        ];

        world.instantiate(objects).expect("instantiate failed");

        const UART_BASE: u64 = 0x1000_0000;

        // Write to LCR (Line Control Register, offset 3)
        world.mmio_write(UART_BASE + 3, 1, 0x03).expect("mmio_write LCR");

        // Read LCR back
        let lcr = world.mmio_read(UART_BASE + 3, 1).expect("mmio_read LCR");
        assert_eq!(lcr & 0x03, 0x03, "LCR data bits should be 8");
    }

    #[test]
    fn test_forward_reference_resolution() {
        let mut world = World::new();

        // uart references plic, but uart is listed FIRST — forward ref
        let objects = vec![
            PendingObject::new("board.uart0", "uart16550")
                .set("clock_hz", 1_843_200i64)
                .set_port("irq_out", "board.plic", "input_10"),

            PendingObject::new("board.plic", "plic")
                .set("n_sources", 32i64)
                .set("base_addr", 0x0c00_0000i64),
        ];

        // Instantiate resolves the forward ref — ordering does not matter
        world.instantiate(objects).expect("forward ref must resolve");

        // Verify the attr was resolved to a Port value
        let uart = world.get_object("board.uart0").unwrap();
        let irq_attr = uart.get_attr("irq_out").unwrap();
        let plic_id  = *world.by_name_id("board.plic").unwrap();
        assert_eq!(
            *irq_attr,
            AttrValue::Port(plic_id, "input_10".to_string()),
            "irq_out should resolve to Port(plic_id, input_10)"
        );
    }

    #[test]
    fn test_device_only_advance() {
        let mut world = World::new();
        let objects = vec![
            PendingObject::new("board.uart0", "uart16550")
                .set("clock_hz",  1_843_200i64)
                .set("base_addr", 0x1000_0000i64),
        ];
        world.instantiate(objects).unwrap();

        assert_eq!(world.current_tick(), 0);
        world.advance(500);
        assert_eq!(world.current_tick(), 500);
    }

    #[test]
    fn test_run_device_only_stops_when_queue_empty() {
        let mut world = World::new();
        world.instantiate(vec![
            PendingObject::new("board.uart0", "uart16550")
                .set("clock_hz",  1_843_200i64)
                .set("base_addr", 0x1000_0000i64),
        ]).unwrap();

        let reason = world.run();
        assert_eq!(reason, StopReason::EventQueueEmpty);
    }
}
```

---

*For the attribute system, see [`LLD-attr-system.md`](./LLD-attr-system.md). For the HLD, see [`HLD.md`](./HLD.md).*

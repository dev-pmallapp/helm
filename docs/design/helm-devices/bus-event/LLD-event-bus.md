# helm-devices/bus — LLD: HelmEventBus

## `HelmEvent` Enum — All 15 Variants

```rust
use helm_core::{Cycles, HelmObjectId};

/// The complete set of observable simulation events.
/// Fired via HelmEventBus::fire().
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum HelmEvent {
    // ── Simulation Lifecycle ─────────────────────────────────────────────
    /// Simulation run started (after elaboration, before first instruction).
    SimStart,

    /// Simulation run stopped (all harts reached halt, or Python called stop()).
    /// `reason` is a human-readable string.
    SimStop { reason: String },

    // ── Hart Lifecycle ───────────────────────────────────────────────────
    /// A hart has been reset (hardware reset, not simulation init).
    HartReset { hart_id: u32 },

    /// A hart has halted (WFI, EBREAK, or unrecoverable fault).
    HartHalt { hart_id: u32, pc: u64 },

    // ── Exceptions and Interrupts ────────────────────────────────────────
    /// An exception was taken by a hart.
    Exception {
        hart_id: u32,
        cause: ExceptionCause,
        pc: u64,    // PC of the faulting instruction.
        tval: u64,  // Trap value (faulting address or instruction bits).
    },

    /// An interrupt was delivered to a hart.
    Interrupt {
        hart_id: u32,
        irq: u32,       // Interrupt number.
        pc: u64,        // PC at time of delivery.
    },

    // ── Memory Events ────────────────────────────────────────────────────
    /// A memory read was performed (functional mode only; not fired in Accurate).
    MemRead {
        object_id: HelmObjectId,
        addr: u64,
        size: u8,
        value: u64,
        pc: u64,
    },

    /// A memory write was performed.
    MemWrite {
        object_id: HelmObjectId,
        addr: u64,
        size: u8,
        value: u64,
        pc: u64,
    },

    // ── Breakpoints and Watchpoints ──────────────────────────────────────
    /// A software breakpoint was hit (EBREAK or GDB-set breakpoint).
    Breakpoint {
        hart_id: u32,
        pc: u64,
        breakpoint_id: u64,  // 0 = EBREAK, >0 = GDB-assigned.
    },

    /// A watchpoint was triggered.
    Watchpoint {
        hart_id: u32,
        addr: u64,
        size: u8,
        is_write: bool,
        watchpoint_id: u64,
    },

    // ── Region of Interest ───────────────────────────────────────────────
    /// A magic instruction signaling ROI entry was executed.
    RoiBegin { hart_id: u32, pc: u64 },

    /// A magic instruction signaling ROI exit was executed.
    RoiEnd { hart_id: u32, pc: u64 },

    // ── Checkpointing ────────────────────────────────────────────────────
    /// Checkpoint save is about to begin.
    CheckpointSave { path: std::path::PathBuf },

    /// Checkpoint restore has completed.
    CheckpointRestore { path: std::path::PathBuf },

    // ── Custom / User-Defined ────────────────────────────────────────────
    /// Escape hatch for Python scripts and plugins to fire custom events.
    Custom {
        name: String,
        data: serde_json::Value,
    },
}
```

---

## `HelmEventKind` Enum

Used to route subscribers to the correct event bucket without pattern-matching all variants.

```rust
/// The discriminant of HelmEvent, used as the subscription key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HelmEventKind {
    SimStart,
    SimStop,
    HartReset,
    HartHalt,
    Exception,
    Interrupt,
    MemRead,
    MemWrite,
    Breakpoint,
    Watchpoint,
    RoiBegin,
    RoiEnd,
    CheckpointSave,
    CheckpointRestore,
    Custom,
}

impl HelmEvent {
    pub fn kind(&self) -> HelmEventKind {
        match self {
            HelmEvent::SimStart              => HelmEventKind::SimStart,
            HelmEvent::SimStop { .. }        => HelmEventKind::SimStop,
            HelmEvent::HartReset { .. }      => HelmEventKind::HartReset,
            HelmEvent::HartHalt { .. }       => HelmEventKind::HartHalt,
            HelmEvent::Exception { .. }      => HelmEventKind::Exception,
            HelmEvent::Interrupt { .. }      => HelmEventKind::Interrupt,
            HelmEvent::MemRead { .. }        => HelmEventKind::MemRead,
            HelmEvent::MemWrite { .. }       => HelmEventKind::MemWrite,
            HelmEvent::Breakpoint { .. }     => HelmEventKind::Breakpoint,
            HelmEvent::Watchpoint { .. }     => HelmEventKind::Watchpoint,
            HelmEvent::RoiBegin { .. }       => HelmEventKind::RoiBegin,
            HelmEvent::RoiEnd { .. }         => HelmEventKind::RoiEnd,
            HelmEvent::CheckpointSave { .. } => HelmEventKind::CheckpointSave,
            HelmEvent::CheckpointRestore {..}=> HelmEventKind::CheckpointRestore,
            HelmEvent::Custom { .. }         => HelmEventKind::Custom,
        }
    }
}
```

---

## `ExceptionCause` Enum

```rust
/// RISC-V / AArch64 exception causes, unified for both ISAs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExceptionCause {
    // RISC-V standard causes
    InstructionAddressMisaligned,
    InstructionAccessFault,
    IllegalInstruction,
    Breakpoint,
    LoadAddressMisaligned,
    LoadAccessFault,
    StoreAmoAddressMisaligned,
    StoreAmoAccessFault,
    EcallUMode,
    EcallSMode,
    EcallMMode,
    InstructionPageFault,
    LoadPageFault,
    StoreAmoPageFault,
    // AArch64 causes
    SvcAarch64,
    DataAbort,
    InstructionAbort,
    // Generic
    Unknown(u64),
}
```

---

## `EventHandle` — Opaque Subscription Handle

`EventHandle` is returned by `subscribe` and `subscribe_filtered`. Dropping it unsubscribes. Optionally, the caller can call `handle.cancel()` explicitly.

```rust
/// Opaque handle to a subscription. Dropping this value unsubscribes.
pub struct EventHandle {
    id: SubscriberId,
    bus: std::sync::Weak<HelmEventBusInner>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SubscriberId(u64);

impl EventHandle {
    /// Explicitly cancel the subscription. Same as dropping.
    pub fn cancel(self) {
        // `drop(self)` triggers the Drop impl which does the unsubscribe.
    }

    /// Detach the handle: the subscription persists even if the handle is dropped.
    /// The subscription can only be cancelled by subscribing again and dropping
    /// the new handle, or by bus teardown.
    pub fn detach(mut self) {
        self.bus = std::sync::Weak::new();
        std::mem::forget(self);
    }
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        if let Some(bus) = self.bus.upgrade() {
            bus.unsubscribe(self.id);
        }
    }
}
```

---

## `HelmEventBus` — Internal Structure

### Subscriber Type

```rust
type SubscriberFn = Box<dyn Fn(&HelmEvent) + Send + Sync + 'static>;

struct Subscriber {
    id: SubscriberId,
    kind: HelmEventKind,
    /// Optional predicate for object-scoped or filtered subscriptions.
    /// If None, matches all events of the given kind.
    predicate: Option<Box<dyn Fn(&HelmEvent) -> bool + Send + Sync + 'static>>,
    callback: SubscriberFn,
}
```

### Inner State

```rust
use std::sync::{Arc, RwLock};
use std::cell::Cell;
use std::collections::HashMap;

struct HelmEventBusInner {
    /// Subscribers grouped by event kind for O(1) lookup on fire.
    subscribers: RwLock<HashMap<HelmEventKind, Vec<Subscriber>>>,

    /// Monotonically increasing subscriber ID counter.
    next_id: std::sync::atomic::AtomicU64,

    /// Current fire recursion depth. Checked and incremented/decremented around fire().
    /// Uses a thread-local because HelmEventBus may be used from multiple threads.
    // Note: Cell is not Send. We use AtomicU32 here.
    firing_depth: std::sync::atomic::AtomicU32,
}

impl HelmEventBusInner {
    fn unsubscribe(&self, id: SubscriberId) {
        let mut subs = self.subscribers.write().unwrap();
        for bucket in subs.values_mut() {
            bucket.retain(|s| s.id != id);
        }
    }
}
```

### Public Struct

```rust
/// The simulation event bus. Thread-safe. Clone to share.
#[derive(Clone)]
pub struct HelmEventBus {
    inner: Arc<HelmEventBusInner>,
}

impl HelmEventBus {
    pub fn new() -> Self {
        HelmEventBus {
            inner: Arc::new(HelmEventBusInner {
                subscribers: RwLock::new(HashMap::new()),
                next_id: std::sync::atomic::AtomicU64::new(1),
                firing_depth: std::sync::atomic::AtomicU32::new(0),
            }),
        }
    }
}
```

---

## `subscribe` — Basic Subscription

```rust
impl HelmEventBus {
    /// Subscribe to all events of a given kind.
    /// Returns an EventHandle; dropping it unsubscribes.
    pub fn subscribe<F>(&self, kind: HelmEventKind, callback: F) -> EventHandle
    where
        F: Fn(&HelmEvent) + Send + Sync + 'static,
    {
        let id = SubscriberId(
            self.inner.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let sub = Subscriber {
            id,
            kind,
            predicate: None,
            callback: Box::new(callback),
        };

        let mut subs = self.inner.subscribers.write().unwrap();
        subs.entry(kind).or_default().push(sub);

        EventHandle {
            id,
            bus: Arc::downgrade(&self.inner),
        }
    }
}
```

---

## `subscribe_filtered` — Predicate / Object-Scoped Variant

This implements Q58: `subscribe_obj(kind, object_ref, f)`. The object-scoped variant is expressed as a predicate closure.

```rust
impl HelmEventBus {
    /// Subscribe to events of a given kind, but only when the predicate returns true.
    ///
    /// Object-scoped example:
    /// ```rust
    /// let handle = bus.subscribe_filtered(
    ///     HelmEventKind::MemWrite,
    ///     move |ev| matches!(ev, HelmEvent::MemWrite { object_id, .. } if *object_id == uart_id),
    ///     move |ev| { /* only called for uart_id MemWrite events */ },
    /// );
    /// ```
    pub fn subscribe_filtered<P, F>(
        &self,
        kind: HelmEventKind,
        predicate: P,
        callback: F,
    ) -> EventHandle
    where
        P: Fn(&HelmEvent) -> bool + Send + Sync + 'static,
        F: Fn(&HelmEvent) + Send + Sync + 'static,
    {
        let id = SubscriberId(
            self.inner.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let sub = Subscriber {
            id,
            kind,
            predicate: Some(Box::new(predicate)),
            callback: Box::new(callback),
        };

        let mut subs = self.inner.subscribers.write().unwrap();
        subs.entry(kind).or_default().push(sub);

        EventHandle {
            id,
            bus: Arc::downgrade(&self.inner),
        }
    }
}
```

---

## `fire()` — Synchronous Dispatch

This is the hot path for every significant simulation event. It must be fast when there are no subscribers.

```rust
impl HelmEventBus {
    /// Fire an event synchronously.
    /// All subscribers for this event kind are called before fire() returns.
    /// Panics in subscribers are caught (catch_unwind) and logged; they do not
    /// propagate to the caller.
    /// Recursive calls from within a callback are allowed but logged as warnings.
    pub fn fire(&self, event: &HelmEvent) {
        let kind = event.kind();

        // Depth tracking for recursive-fire detection (Q59).
        let depth = self.inner.firing_depth
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if depth > 0 {
            tracing::warn!(
                depth = depth + 1,
                ?kind,
                "HelmEventBus::fire called recursively; proceeding"
            );
        }

        // Collect subscriber IDs + callbacks under a read lock.
        // We snapshot the subscriber list to allow callbacks to subscribe/unsubscribe.
        let snapshot: Vec<(Option<Box<dyn Fn(&HelmEvent) -> bool + Send + Sync>>, SubscriberFn)> = {
            let subs = self.inner.subscribers.read().unwrap();
            // We cannot easily clone Box<dyn Fn>. Instead, we hold the read lock
            // for the duration of dispatch (no subscribe/unsubscribe during fire).
            // This is acceptable for Phase 0; deferred snapshot refactoring for Phase 2.
            //
            // Phase 0 simplification: hold read lock during all callbacks.
            // This means subscribe() from within a callback will deadlock.
            // Documented limitation; subscribe from callback is not supported.
            let bucket = match subs.get(&kind) {
                Some(b) if !b.is_empty() => b,
                _ => {
                    self.inner.firing_depth
                        .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
            };

            // Early exit if no subscribers.
            drop(subs);
            // Re-acquire: needed for lifetime to hold across callbacks.
            // Phase 0: we re-read inside the impl below. See note.
            vec![]  // Placeholder; see actual impl below.
        };

        // Actual Phase 0 implementation: read-lock held during all callbacks.
        {
            let subs = self.inner.subscribers.read().unwrap();
            if let Some(bucket) = subs.get(&kind) {
                for sub in bucket {
                    // Apply predicate (object-scoped filter).
                    if let Some(pred) = &sub.predicate {
                        if !pred(event) {
                            continue;
                        }
                    }

                    // Invoke callback under catch_unwind (Q56).
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        (sub.callback)(event);
                    }));

                    if let Err(panic_val) = result {
                        let msg = panic_val
                            .downcast_ref::<String>()
                            .map(|s| s.as_str())
                            .or_else(|| panic_val.downcast_ref::<&str>().copied())
                            .unwrap_or("<non-string panic>");
                        tracing::error!(
                            subscriber_id = sub.id.0,
                            ?kind,
                            panic = msg,
                            "Subscriber panicked; continuing with remaining subscribers"
                        );
                    }
                }
            }
        }

        self.inner.firing_depth
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}
```

### Phase 0 Constraint Note

Holding the read lock during all callbacks means that:
- `subscribe()` called from within a callback will deadlock (write lock contended).
- `unsubscribe()` (via `EventHandle::drop`) called from within a callback will deadlock.

This is a documented Phase 0 limitation. The fix in Phase 2 is to snapshot the subscriber list into a `SmallVec` before releasing the lock, then iterate the snapshot. This requires `Arc`-wrapping each subscriber callback.

---

## Python Callbacks (PyO3 Integration)

Python subscribers are registered via `helm-python` with a wrapper that acquires the GIL per call (Q57).

```rust
// In helm-python (PyO3 crate):
#[cfg(feature = "pyo3")]
use pyo3::prelude::*;

#[cfg(feature = "pyo3")]
pub fn subscribe_python(
    bus: &HelmEventBus,
    kind: HelmEventKind,
    py_callable: PyObject,
) -> EventHandle {
    bus.subscribe(kind, move |event| {
        Python::with_gil(|py| {
            // Convert HelmEvent to a Python-friendly dict or named tuple.
            let py_event = helm_event_to_pyobject(py, event);
            if let Err(e) = py_callable.call1(py, (py_event,)) {
                // Python exceptions become tracing errors; they don't propagate.
                tracing::error!("Python event subscriber raised: {}", e);
            }
        });
    })
}

#[cfg(feature = "pyo3")]
fn helm_event_to_pyobject(py: Python, event: &HelmEvent) -> PyObject {
    // Convert each variant to a Python dict with "kind" key + variant fields.
    // Example: HelmEvent::Exception { hart_id, cause, pc, tval }
    //   → {"kind": "Exception", "hart_id": 0, "cause": "LoadFault", "pc": 0x1234, "tval": 0xBEEF}
    let dict = pyo3::types::PyDict::new(py);
    dict.set_item("kind", format!("{:?}", event.kind())).unwrap();
    match event {
        HelmEvent::Exception { hart_id, cause, pc, tval } => {
            dict.set_item("hart_id", hart_id).unwrap();
            dict.set_item("cause", format!("{:?}", cause)).unwrap();
            dict.set_item("pc", pc).unwrap();
            dict.set_item("tval", tval).unwrap();
        }
        HelmEvent::MemWrite { object_id, addr, size, value, pc } => {
            dict.set_item("object_id", object_id).unwrap();
            dict.set_item("addr", addr).unwrap();
            dict.set_item("size", size).unwrap();
            dict.set_item("value", value).unwrap();
            dict.set_item("pc", pc).unwrap();
        }
        _ => { /* fill remaining variants */ }
    }
    dict.into()
}
```

---

## Integration with `TraceLogger`

`helm-debug`'s `TraceLogger` subscribes to relevant event kinds as a standard subscriber.

```rust
// In helm-debug:
impl TraceLogger {
    pub fn attach_to_bus(&self, bus: &HelmEventBus) -> Vec<EventHandle> {
        let mut handles = Vec::new();

        // Subscribe to exceptions.
        let logger = self.clone();
        handles.push(bus.subscribe(HelmEventKind::Exception, move |ev| {
            if let HelmEvent::Exception { hart_id, cause, pc, tval } = ev {
                logger.log(TraceEvent::Exception {
                    hart_id: *hart_id,
                    cause: format!("{:?}", cause),
                    pc: *pc,
                    tval: *tval,
                });
            }
        }));

        // Subscribe to memory writes (if trace_mem_write feature is enabled).
        let logger2 = self.clone();
        handles.push(bus.subscribe(HelmEventKind::MemWrite, move |ev| {
            if let HelmEvent::MemWrite { addr, value, size, pc, .. } = ev {
                logger2.log(TraceEvent::MemWrite {
                    addr: *addr, value: *value, size: *size, pc: *pc,
                });
            }
        }));

        // ... subscribe to other events as needed ...

        handles
        // Caller must keep handles alive for the lifetime of the logger.
    }
}
```

---

## Complete Usage Example

```rust
fn main() {
    let bus = HelmEventBus::new();

    // Subscribe to all exceptions.
    let exc_handle = bus.subscribe(HelmEventKind::Exception, |ev| {
        if let HelmEvent::Exception { hart_id, cause, pc, .. } = ev {
            println!("[hart {}] Exception {:?} at PC={:#x}", hart_id, cause, pc);
        }
    });

    // Subscribe only to MemWrite events from a specific UART device.
    let uart_id: HelmObjectId = 5;
    let uart_handle = bus.subscribe_filtered(
        HelmEventKind::MemWrite,
        move |ev| matches!(ev, HelmEvent::MemWrite { object_id, .. } if *object_id == uart_id),
        |ev| {
            if let HelmEvent::MemWrite { addr, value, .. } = ev {
                println!("UART write: addr={:#x} value={:#x}", addr, value);
            }
        },
    );

    // Fire events from the simulation engine.
    bus.fire(&HelmEvent::SimStart);
    bus.fire(&HelmEvent::Exception {
        hart_id: 0,
        cause: ExceptionCause::LoadPageFault,
        pc: 0x8000_4000,
        tval: 0x0000_0008,
    });
    bus.fire(&HelmEvent::MemWrite {
        object_id: uart_id,
        addr: 0x1000_0000,
        size: 1,
        value: b'H' as u64,
        pc: 0x8000_4010,
    });
    bus.fire(&HelmEvent::SimStop {
        reason: "All harts halted".to_string(),
    });

    // Handles dropped here → automatic unsubscribe.
    drop(exc_handle);
    drop(uart_handle);
}
```

---

## Thread Safety Notes

`HelmEventBus` is `Send + Sync` and can be cloned cheaply (it is an `Arc` wrapper). The `subscribers` map is protected by `RwLock`. In Phase 0, `fire()` holds the read lock for the duration of all callbacks. In a multi-threaded scenario (e.g., GDB server thread firing a Breakpoint event while the simulation thread fires an Exception), both `fire()` calls can proceed concurrently (RwLock allows multiple readers). Deadlock is only possible if a callback calls `subscribe()` or drops an `EventHandle` (both need the write lock). This is documented as a Phase 0 limitation.

---

## Unsubscribe — Internal Implementation

```rust
impl HelmEventBusInner {
    fn unsubscribe(&self, id: SubscriberId) {
        let mut subs = self.subscribers.write().unwrap();
        for bucket in subs.values_mut() {
            bucket.retain(|s| s.id != id);
        }
        // Buckets are not removed when empty; the overhead is negligible
        // (15 fixed HelmEventKind variants).
    }
}
```

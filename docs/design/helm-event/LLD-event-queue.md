# helm-event — LLD: Event Queue

## Type Definitions

```rust
use std::collections::{BinaryHeap, HashSet, HashMap};
use std::cmp::Reverse;

/// Simulated cycle count. Re-exported from helm-core.
pub type Cycles = u64;

/// Opaque identifier for a simulator object. Re-exported from helm-core.
pub type HelmObjectId = u32;

/// Opaque event ID returned by post_cycles/post_at. Used for cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventId(pub(crate) u64);

/// Identifies a registered event class within a single EventQueue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventClassId(pub(crate) u32);
```

---

## `EventData` Trait

Callbacks receive a type-erased `Box<dyn EventData>`. Devices downcast to their concrete type using `Any`.

```rust
use std::any::Any;

/// Type-erased event payload. Devices downcast with `data.downcast_ref::<MyData>()`.
pub trait EventData: Any + Send + 'static {
    fn as_any(&self) -> &dyn Any;

    /// Optional: serialize this data for checkpoint.
    /// Returns `None` if the event type is not checkpointable.
    fn serialize_cbor(&self) -> Option<Vec<u8>> { None }
}

impl dyn EventData {
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.as_any().downcast_ref::<T>()
    }
}

/// Convenience: unit event data (no payload).
pub struct NoData;
impl EventData for NoData {
    fn as_any(&self) -> &dyn Any { self }
}
```

---

## `EventClass` Struct

`EventClass` is registered once per event type and describes how to handle it.

```rust
/// Callback type: receives the event data and a mutable reference to the queue
/// (so devices can re-post recurring events from within the callback).
pub type EventCallback = Box<dyn Fn(Box<dyn EventData>, &mut EventQueue) + Send + Sync + 'static>;

/// Optional serialization hook for checkpoint support.
pub type SerializeFn  = Box<dyn Fn(&dyn EventData) -> Option<Vec<u8>> + Send + Sync + 'static>;

/// Optional deserialization hook. Returns a Box<dyn EventData> from CBOR bytes.
pub type DeserializeFn = Box<dyn Fn(&[u8]) -> Box<dyn EventData> + Send + Sync + 'static>;

/// Registered event type. Created once and registered with EventQueue.
pub struct EventClass {
    /// Unique name for this event class (for debugging and checkpoint keys).
    pub name: &'static str,

    /// Callback invoked when the event fires. May call eq.post_cycles() to re-post.
    pub callback: EventCallback,

    /// If Some, this event class can be checkpointed.
    pub serialize: Option<SerializeFn>,

    /// Required if serialize is Some. Reconstructs data from CBOR on restore.
    pub deserialize: Option<DeserializeFn>,
}

impl EventClass {
    /// Create a non-checkpointable event class.
    pub fn new(
        name: &'static str,
        callback: EventCallback,
    ) -> Self {
        EventClass { name, callback, serialize: None, deserialize: None }
    }

    /// Create a checkpointable event class.
    pub fn new_checkpointable(
        name: &'static str,
        callback: EventCallback,
        serialize: SerializeFn,
        deserialize: DeserializeFn,
    ) -> Self {
        EventClass { name, callback, serialize: Some(serialize), deserialize: Some(deserialize) }
    }

    /// Whether this event class survives a checkpoint.
    pub fn is_checkpointable(&self) -> bool {
        self.serialize.is_some()
    }
}
```

---

## `PendingEvent` Struct

The internal heap entry. Implements `Ord` so that `BinaryHeap<Reverse<PendingEvent>>` is a min-heap ordered by `fire_at`, broken by `seq` (FIFO within same cycle).

```rust
/// One pending event in the heap.
pub struct PendingEvent {
    /// Absolute cycle at which this event should fire.
    pub fire_at: Cycles,

    /// Monotonically increasing sequence number (FIFO tie-breaking within same cycle).
    pub seq: u64,

    /// Registered class ID.
    pub class_id: EventClassId,

    /// The simulator object that owns this event (for debugging and object-scope cancel).
    pub owner: HelmObjectId,

    /// Type-erased payload.
    pub data: Box<dyn EventData>,

    /// Unique cancellation ID.
    pub id: EventId,
}

impl PartialEq for PendingEvent {
    fn eq(&self, other: &Self) -> bool {
        self.fire_at == other.fire_at && self.seq == other.seq
    }
}
impl Eq for PendingEvent {}

impl PartialOrd for PendingEvent {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PendingEvent {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Sorted first by fire_at (ascending), then by seq (ascending = FIFO).
        (self.fire_at, self.seq).cmp(&(other.fire_at, other.seq))
    }
}
```

---

## `EventQueue` Struct

```rust
pub struct EventQueue {
    /// Min-heap. `Reverse` wraps PendingEvent so BinaryHeap is a min-heap.
    heap: BinaryHeap<Reverse<PendingEvent>>,

    /// Set of cancelled EventIds. Events are skipped on drain, not removed from heap.
    cancelled: HashSet<EventId>,

    /// Registered event classes, indexed by EventClassId.
    classes: HashMap<EventClassId, EventClass>,

    /// Next EventClassId to assign.
    next_class_id: u32,

    /// Next EventId sequence number. Monotonically increasing; never reused.
    next_event_id: u64,

    /// Next sequence number for tie-breaking within the same cycle.
    next_seq: u64,

    /// Current simulated time (advanced by drain_until).
    current_time: Cycles,
}
```

### Constructor

```rust
impl EventQueue {
    pub fn new() -> Self {
        EventQueue::with_capacity(1024)
    }

    pub fn with_capacity(cap: usize) -> Self {
        EventQueue {
            heap: BinaryHeap::with_capacity(cap),
            cancelled: HashSet::new(),
            classes: HashMap::new(),
            next_class_id: 0,
            next_event_id: 1,  // 0 is reserved as "null" EventId.
            next_seq: 0,
            current_time: 0,
        }
    }
}
```

---

## `register_class`

```rust
impl EventQueue {
    /// Register an event class. Returns its EventClassId.
    /// Must be called before any events of this class are posted.
    /// Panics if the same name is registered twice (programming error).
    pub fn register_class(&mut self, class: EventClass) -> EventClassId {
        let id = EventClassId(self.next_class_id);
        self.next_class_id += 1;

        if self.classes.values().any(|c| c.name == class.name) {
            panic!("EventClass '{}' registered twice", class.name);
        }
        self.classes.insert(id, class);
        id
    }
}
```

---

## `post_cycles` and `post_at`

```rust
impl EventQueue {
    /// Post an event to fire `delay` cycles after the current simulated time.
    /// Returns an EventId for potential cancellation.
    pub fn post_cycles(
        &mut self,
        delay: Cycles,
        class_id: EventClassId,
        owner: HelmObjectId,
        data: Box<dyn EventData>,
    ) -> EventId {
        assert!(self.classes.contains_key(&class_id),
            "EventClassId {:?} not registered", class_id);
        let fire_at = self.current_time + delay;
        self.post_at_internal(fire_at, class_id, owner, data)
    }

    /// Post an event to fire at an absolute cycle count.
    /// `fire_at` must be >= current_time.
    pub fn post_at(
        &mut self,
        fire_at: Cycles,
        class_id: EventClassId,
        owner: HelmObjectId,
        data: Box<dyn EventData>,
    ) -> EventId {
        assert!(fire_at >= self.current_time,
            "post_at: fire_at {} < current_time {}; events cannot fire in the past",
            fire_at, self.current_time);
        self.post_at_internal(fire_at, class_id, owner, data)
    }

    fn post_at_internal(
        &mut self,
        fire_at: Cycles,
        class_id: EventClassId,
        owner: HelmObjectId,
        data: Box<dyn EventData>,
    ) -> EventId {
        let id = EventId(self.next_event_id);
        self.next_event_id += 1;
        let seq = self.next_seq;
        self.next_seq += 1;

        self.heap.push(Reverse(PendingEvent {
            fire_at,
            seq,
            class_id,
            owner,
            data,
            id,
        }));
        id
    }
}
```

---

## `cancel`

```rust
impl EventQueue {
    /// Cancel an event by its EventId.
    /// Returns true if the id was found and marked cancelled.
    /// Returns false if already fired, already cancelled, or never existed.
    /// Cancelled events remain in the heap but are silently skipped during drain.
    pub fn cancel(&mut self, id: EventId) -> bool {
        // We cannot cheaply remove from a BinaryHeap.
        // Lazy cancellation: insert into cancelled set; drain skips it.
        self.cancelled.insert(id)
        // Note: if the id was already in the set, insert returns false (already cancelled).
    }
}
```

---

## `drain_until`

This is the hot path called by timing models at every interval boundary.

```rust
impl EventQueue {
    /// Fire all pending events with fire_at <= until_cycle, in temporal order.
    /// Advances current_time to until_cycle.
    ///
    /// Callbacks may call post_cycles/post_at to schedule new events.
    /// Those new events will not be fired in this drain call unless they
    /// fire_at <= until_cycle AND are pushed before the heap top is re-peeked.
    /// (New events posted at fire_at > until_cycle are deferred to a future drain.)
    pub fn drain_until(&mut self, until_cycle: Cycles) {
        // Advance current time first so callbacks calling post_cycles see the
        // correct current_time.
        if until_cycle > self.current_time {
            self.current_time = until_cycle;
        }

        // Fire events in order of fire_at, then seq.
        loop {
            // Peek at the minimum without removing.
            match self.heap.peek() {
                None => break,
                Some(Reverse(ev)) if ev.fire_at > until_cycle => break,
                _ => {}
            }

            // Pop the minimum event.
            let Reverse(ev) = self.heap.pop().unwrap();

            // Skip cancelled events.
            if self.cancelled.remove(&ev.id) {
                continue;
            }

            // Look up the class and invoke its callback.
            // SAFETY: we hold &mut self so the class map is stable.
            // We temporarily remove the class to allow the callback to call
            // post_cycles on self without borrow conflicts.
            if let Some(class) = self.classes.remove(&ev.class_id) {
                (class.callback)(ev.data, self);
                // Re-insert the class after the callback completes.
                self.classes.insert(ev.class_id, class);
            }
            // If the class was unregistered during the callback (unusual), the event
            // is silently dropped.
        }

        // Trim the cancelled set: remove IDs that couldn't be in the heap
        // (IDs less than the minimum fire_at, which have already fired or been drained).
        // This is a periodic GC step; we keep the cancelled set bounded.
        if self.cancelled.len() > 512 {
            self.cancelled.retain(|id| id.0 >= self.next_event_id.saturating_sub(512));
        }
    }
}
```

---

## `peek_next_tick`

```rust
impl EventQueue {
    /// Returns the fire_at cycle of the earliest pending event, or None if empty.
    /// Used by the timing model to decide whether to drain early (before a boundary).
    pub fn peek_next_tick(&self) -> Option<Cycles> {
        self.heap.peek().map(|Reverse(ev)| ev.fire_at)
    }

    /// Current simulated time.
    pub fn current_time(&self) -> Cycles {
        self.current_time
    }

    /// Number of pending (not cancelled) events. O(n) — for debugging only.
    pub fn len(&self) -> usize {
        self.heap.len() - self.cancelled.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
            || self.heap.iter().all(|Reverse(e)| self.cancelled.contains(&e.id))
    }
}
```

---

## Per-Hart vs. Global Queue Design

### Ownership Model

```
World
 ├── global_event_queue: EventQueue       ← shared devices (PLIC, RTC, disk DMA)
 ├── harts: Vec<Hart>
 │    └── hart.event_queue: EventQueue    ← per-hart timer events (local CLINT mtimecmp)
 └── scheduler: Scheduler
```

### Temporal Decoupling

Each hart owns its `EventQueue`. During a quantum of `Q` instructions, the hart:
1. Runs `Q` instructions through `HelmEngine`.
2. At each interval boundary, calls `timing.on_interval_boundary(&mut self.event_queue)`.
3. At end of quantum, calls `timing.on_interval_boundary(&mut self.event_queue)` once more.

The scheduler then:
4. Computes `max_hart_cycles = max(hart.timing.current_cycles() for hart in harts)`.
5. Drains `global_event_queue.drain_until(max_hart_cycles)`.
6. Delivers any resulting interrupts to harts via `HelmEventBus`.

This allows harts to race ahead within a quantum without synchronization, while global events (shared timer, PLIC) are resolved at quantum boundaries.

### Per-Hart Queue Registration

```rust
impl Hart {
    pub fn event_queue(&mut self) -> &mut EventQueue {
        &mut self.event_queue
    }

    /// Called by CLINT device to schedule a timer interrupt for this hart.
    pub fn schedule_timer_interrupt(&mut self, at_cycle: Cycles) -> EventId {
        self.event_queue.post_at(
            at_cycle,
            self.timer_event_class_id,
            self.object_id,
            Box::new(NoData),
        )
    }
}
```

---

## Checkpoint Behavior

On checkpoint save:
1. Iterate all events in the heap (by draining into a temp vec and rebuilding).
2. For each event, check `class.is_checkpointable()`.
   - If yes: call `class.serialize(event.data)` → CBOR bytes → write to checkpoint.
   - If no: discard the event. It will not be restored.
3. Write `current_time` to checkpoint.

On checkpoint restore:
1. Restore `current_time`.
2. Re-create a fresh `EventQueue`.
3. For each saved event record: find its class by name, call `class.deserialize(bytes)` → `Box<dyn EventData>`, then `post_at(fire_at, ...)`.
4. Devices that post non-checkpointable events (e.g., DMA completion) must re-post them in their `checkpoint_restore()` method if the event is still relevant.

```rust
impl EventQueue {
    /// Serialize all checkpointable pending events to CBOR.
    /// Non-checkpointable events are dropped without error.
    pub fn checkpoint_save(&self) -> CheckpointData {
        let events: Vec<SavedEvent> = self.heap.iter()
            .filter_map(|Reverse(ev)| {
                if self.cancelled.contains(&ev.id) { return None; }
                let class = self.classes.get(&ev.class_id)?;
                let data_cbor = class.serialize.as_ref()
                    .and_then(|f| f(ev.data.as_ref()));
                Some(SavedEvent {
                    fire_at: ev.fire_at,
                    class_name: class.name,
                    owner: ev.owner,
                    data_cbor,
                })
            })
            .filter(|e| e.data_cbor.is_some())
            .collect();
        CheckpointData { current_time: self.current_time, events }
    }

    /// Restore from checkpoint. Classes must already be registered.
    pub fn checkpoint_restore(&mut self, data: CheckpointData) {
        self.current_time = data.current_time;
        self.heap.clear();
        self.cancelled.clear();

        // Build name → class_id reverse map.
        let name_map: HashMap<&str, EventClassId> = self.classes.iter()
            .map(|(id, c)| (c.name, *id))
            .collect();

        for saved in data.events {
            if let (Some(class_id), Some(bytes)) = (
                name_map.get(saved.class_name),
                saved.data_cbor,
            ) {
                let class = &self.classes[class_id];
                if let Some(deser) = &class.deserialize {
                    let data = deser(&bytes);
                    self.post_at(saved.fire_at, *class_id, saved.owner, data);
                }
            }
        }
    }
}

pub struct SavedEvent {
    pub fire_at: Cycles,
    pub class_name: &'static str,
    pub owner: HelmObjectId,
    pub data_cbor: Option<Vec<u8>>,
}

pub struct CheckpointData {
    pub current_time: Cycles,
    pub events: Vec<SavedEvent>,
}
```

---

## Usage Example — UART Baud-Rate Timer

```rust
// Device init: register the timer event class.
let class = EventClass::new(
    "uart16550.baud_tick",
    Box::new(|_data, eq| {
        // Fire one character transmission.
        uart.transmit_byte();
        // Recurring: re-post for the next baud period.
        eq.post_cycles(BAUD_PERIOD_CYCLES, uart.timer_class_id, uart.id, Box::new(NoData));
    }),
);
let class_id = eq.register_class(class);

// Start the timer.
let _timer_id = eq.post_cycles(BAUD_PERIOD_CYCLES, class_id, uart_id, Box::new(NoData));

// ... simulation runs ...
// At each interval boundary, timing model calls:
eq.drain_until(current_cycles);
// UART baud tick fires every BAUD_PERIOD_CYCLES simulated cycles.
```

---

## Usage Example — One-Shot DMA Completion

```rust
// When DMA transfer starts:
let dma_id = eq.post_cycles(
    dma_transfer_cycles,
    dma_complete_class_id,
    dma_controller.id,
    Box::new(DmaCompletionData { transfer_id: 42, bytes: 4096 }),
);

// If the driver cancels the transfer before it completes:
let was_pending = eq.cancel(dma_id);
assert!(was_pending, "DMA cancel should succeed while still pending");
```

---

## Design Decisions from Q&A

### Design Decision: BinaryHeap with initial capacity 1024 (Q51)

`EventQueue::new()` calls `BinaryHeap::with_capacity(1024)` (as implemented above). This avoids reallocation for typical device workloads (< 100 events at peak). At the expected device count for Helm-ng v1 (< 20 devices, each posting at most a few events), peak pending events will be well under 200 — far below the threshold where O(log n) is a concern. `EventQueue::with_capacity(n)` is available for workloads known to have more events.

### Design Decision: Re-post in callback, no post_recurring() API (Q52)

Devices re-post recurring events in their callback by calling `eq.post_cycles(interval, class, owner, data)`. There is no `post_recurring()` API. This allows devices to read their current divider register value and post the new interval dynamically, supporting reprogrammable timers without special-casing in the event queue. The callback signature receives `&mut EventQueue` so re-posting is always available from within a callback.

### Design Decision: Cancellation by EventId (Q53)

Cancellation is by `EventId` (exact, opaque `u64`). `cancel(id: EventId) -> bool` marks the event as cancelled in a `HashSet<EventId>`; the event remains in the heap but is skipped when drained. IDs are monotonically increasing and never reused within a simulation run. For the common "cancel all events for this device on reset" pattern, devices maintain their own list of `EventId`s and call `cancel(id)` for each.

### Design Decision: Per-hart EventQueue + global EventQueue for shared devices (Q54)

The topology is: **per-hart `EventQueue`** (owned by each `HelmEngine<T>`) + **one global `EventQueue`** for shared devices (owned by `World`/`Scheduler`). Each hart holds its own `EventQueue` and the timing model drains it at interval boundaries. Shared devices (PLIC, GIC, platform timer) post to the global `EventQueue` owned by `World`. The scheduler drains the global queue between hart quanta. This implements SIMICS-style temporal decoupling: harts run entirely independently within a quantum, with no shared locking on the hot path.

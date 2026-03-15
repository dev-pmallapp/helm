# helm-event — High-Level Design

## Purpose

`helm-event` provides the discrete event queue (DEQ) for Helm-ng simulated time. It is the mechanism by which device timers, interrupt controllers, periodic hardware events, and scheduler quanta are coordinated with the simulated clock. The DEQ operates entirely on simulated cycle counts — real wall-clock time has no role.

The design follows standard discrete-event simulation (DES) practice: events are posted with a future `fire_at` cycle, stored in a min-heap, and drained in temporal order. The timing models in `helm-timing` call `EventQueue::drain_until(current_cycles)` at interval boundaries to fire all events whose `fire_at` has been reached.

---

## Crate Position in the DAG

```
helm-core ──► helm-event
                   ▲
            helm-timing (consumer)
            helm-devices (producer)
            helm-engine (owner)
```

`helm-event` depends only on `helm-core` (for the `Cycles` type alias). It has no dependency on `helm-timing`, `helm-memory`, or `helm-arch`.

---

## Scope

- Discrete event queue with min-heap storage.
- Event registration (`EventClass`) with optional serialize/deserialize callbacks.
- Event posting, cancellation (by `EventId`), and draining.
- Per-hart queue (temporal decoupling) + one global queue for shared devices.
- Checkpoint support: events with no serialize callback are dropped on checkpoint; those with callbacks are serialized and restored.

---

## API Overview

### Core Types

| Type | Purpose |
|------|---------|
| `EventClass` | Registered event type with callback and optional serde hooks |
| `EventId` | Opaque `u64` handle for cancellation |
| `PendingEvent` | Internal heap entry: fire_at, seq, class, owner, data |
| `EventQueue` | BinaryHeap min-heap + class registry + id counter |

### Key Methods

```rust
impl EventQueue {
    pub fn new() -> Self;
    pub fn register_class(&mut self, class: EventClass) -> EventClassId;
    pub fn post_cycles(&mut self, delay: Cycles, class: EventClassId,
                       owner: HelmObjectId, data: Box<dyn EventData>) -> EventId;
    pub fn post_at(&mut self, fire_at: Cycles, class: EventClassId,
                   owner: HelmObjectId, data: Box<dyn EventData>) -> EventId;
    pub fn cancel(&mut self, id: EventId) -> bool;
    pub fn drain_until(&mut self, until_cycle: Cycles);
    pub fn peek_next_tick(&self) -> Option<Cycles>;
    pub fn current_time(&self) -> Cycles;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
```

---

## Design Decisions Answered

**Q51 — BinaryHeap initial capacity = 1024.**
Devices in a typical simulated system (UART, timer, PLIC, disk DMA) generate fewer than 100 simultaneously pending events. 1024 is chosen to avoid reallocations during simulation warmup. The capacity can be overridden via `EventQueue::with_capacity(n)`.

**Q52 — Recurring events: device re-posts in callback.**
There is no `post_recurring` API. Devices call `eq.post_cycles(interval, ...)` from within their callback. This is simpler to reason about, avoids a special case in the heap, and allows devices to vary the interval dynamically (e.g., a baud-rate timer that can be reprogrammed).

**Q53 — Cancel by EventId (exact, opaque u64).**
`EventId` is a monotonically increasing `u64` sequence number assigned at post time. `cancel(id)` marks the event as cancelled in a `HashSet<EventId>`; the event remains in the heap but is skipped when drained. This avoids O(n) heap rebuilding. IDs are never reused within a simulation run.

**Q54 — Per-hart queue + one global queue for shared devices.**
Each hart holds its own `EventQueue`. Shared devices (e.g., a timer that generates interrupts to all harts) use a separate global `EventQueue` owned by `World`. The timing model on each hart drains its own queue; the scheduler drains the global queue. This implements temporal decoupling: harts can race ahead of each other by up to one quantum without synchronization.

---

## Non-Goals for Phase 0

- Priority-based event pre-emption (all events at the same cycle are processed in FIFO order by sequence number).
- Async/await event callbacks.
- Event dependency chains (events triggering other events at the same cycle without re-entering drain are allowed; cyclic dependencies are the device's responsibility to avoid).
- Distributed simulation (no remote event queues).

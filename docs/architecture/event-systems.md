# Event Systems

helm-ng has two distinct event systems that serve fundamentally
different purposes. Understanding when to use each is critical for
device authors and engine contributors.

## Why Two Systems?

| Property | EventQueue | HelmEventBus |
|----------|-----------|--------------|
| **Crate** | `helm-event` | `helm-devices::bus::event_bus` |
| **Delivery** | Asynchronous (deferred) | Synchronous (inline) |
| **Timing** | Future tick T | Immediate |
| **Checkpointed** | Yes | No |
| **Use case** | Schedule future work | Observe current events |

These are not interchangeable. Using the wrong one creates subtle bugs:
- Scheduling an observation via `EventQueue` delays it, missing the
  event you wanted to observe.
- Deferring timer expiry via `HelmEventBus` fires it immediately
  instead of at the correct future tick.

## EventQueue (helm-event)

A discrete-event scheduler built on a `BinaryHeap` (min-heap). Events
are callbacks scheduled to fire at a future simulation tick.

### Types

| Type | Description |
|------|-------------|
| `Tick` | `u64` — simulation time unit |
| `EventId` | `u64` — unique event identifier |
| `EventQueue` | Min-heap scheduler |

### API

```rust
// Schedule an event `delay` ticks from now
queue.post_after(delay, callback) -> EventId

// Schedule an event at absolute tick T
queue.post_at(tick, callback) -> EventId

// Advance time to `until`, firing all events with tick <= until
queue.drain_until(until, handler)

// Fast-forward without processing events
queue.advance_to(tick)

// Inspect
queue.current_tick() -> Tick
queue.peek_next_tick() -> Option<Tick>
queue.is_empty() -> bool
queue.len() -> usize
```

### Usage Pattern

Typical use in a device timer:

```text
1. Timer device schedules expiry: queue.post_at(cval, fire_irq)
2. CPU runs instructions, incrementing ticks
3. Engine calls queue.drain_until(current_tick, handler)
4. Handler fires: assert interrupt pin, update device state
```

### Interaction with Timing Models

The `EventQueue` runs in simulation ticks. `VirtualTiming` increments
the tick by 1 per instruction. `IntervalTiming` increments by the
instruction's latency. `AccurateTiming` increments by pipeline cycles.
`set_tick_scale()` on `HelmEngine` converts ticks to nanoseconds for
device timers.

## HelmEventBus (helm-devices)

A synchronous, named event bus for observable notifications. When an
event fires, all registered subscribers are called immediately, inline
with the firing code.

### Key Properties

- **Synchronous** — subscribers execute in the caller's context, before
  the firing function returns.
- **Not checkpointed** — subscribers must re-register after checkpoint
  restore.
- **Named events** — events are identified by string names.
- **Observer pattern** — devices and plugins subscribe to events they
  care about without the emitter knowing who is listening.

### Usage Pattern

```text
1. Tool subscribes: token = bus.subscribe("cpu.exception", callback)
2. CPU raises exception, fires: bus.fire("cpu.exception", value)
3. All subscribers are called synchronously with the event payload
4. Emitter continues after all subscribers return
5. Tool unsubscribes explicitly with the returned token
```

### Typical Events

| Event Name | Emitter | Data |
|------------|---------|------|
| `cpu.exception` | Exception handler | Exception type, syndrome |
| `cpu.sysreg_write` | Sysreg handler | Register name, value |
| `device.mmio` | MMIO dispatcher | Address, size, direction |
| `gic.irq_assert` | GIC | IRQ number |

## Design Decision

This separation follows the principle that **scheduling** and
**observation** are orthogonal concerns:

- `EventQueue` answers: "what should happen at time T?"
- `HelmEventBus` answers: "who wants to know that X just happened?"

gem5 conflates both into its event system. QEMU has `timer_mod` for
scheduling and ad-hoc callbacks for observation. Simics separates them
cleanly into "events" (scheduled) and "haps" (observable) — helm-ng
follows the Simics model.

## Comparison

| Aspect | QEMU | gem5 | Simics | helm-ng |
|--------|------|------|--------|---------|
| Scheduled events | `timer_mod()` | `Event` class | `SIM_event_post()` | `EventQueue::post_at()` |
| Observable events | Ad-hoc callbacks | Same `Event` class | HAP system | `HelmEventBus::fire()` |
| Separation | Partial | No | Yes | Yes |
| Checkpoint | Timer state saved | Event queue saved | Events saved, HAPs re-register | EventQueue saved, EventBus re-registers |

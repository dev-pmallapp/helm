# helm-devices/bus — High-Level Design

## Purpose

`helm-devices/src/bus/event_bus` provides the typed publish-subscribe event bus for Helm-ng. It is the simulation-wide notification channel for high-level simulator events: exceptions, resets, memory accesses, breakpoints, simulation start/stop, and Python scripting hooks. It is conceptually equivalent to SIMICS HAPs (Hardware Abstraction Points).

Unlike `helm-event` (which is a discrete-event queue for scheduled simulated-time events), `helm-devices/src/bus/event_bus` is synchronous, fire-and-forget, and not tied to simulated time. Subscribers are notified immediately when an event fires, before the firing call returns.

---

## Crate Position in the DAG

```
helm-core ──► helm-devices/bus
                    ▲
          helm-engine (fires events)
          helm-debug (TraceLogger subscribes)
          helm-python (Python subscribes)
```

`helm-devices/src/bus/event_bus` depends only on `helm-core`. It has no dependency on `helm-timing`, `helm-event`, or `helm-arch`.

---

## Scope

- 15 typed `HelmEvent` variants covering the core simulation lifecycle.
- Synchronous dispatch: all subscribers are called before `fire()` returns.
- Panic isolation: a panicking subscriber does not kill others or the simulation.
- Python support: Python callables can subscribe; GIL is acquired per callback.
- Object-scoped filtering: subscribe to events from a specific simulator object.
- Recursive fire from callback: allowed, logged as warning.
- NOT checkpointed: subscribers re-register in their `init()` method on restore.

---

## API Overview

```rust
// Instantiation
let bus = HelmEventBus::new();

// Subscription (returns an EventHandle for lifetime management)
let handle = bus.subscribe(HelmEventKind::Exception, move |ev| {
    if let HelmEvent::Exception { hart_id, cause, .. } = ev {
        println!("Exception on hart {}: {:?}", hart_id, cause);
    }
});

// Object-scoped subscription
let handle2 = bus.subscribe_filtered(
    HelmEventKind::MemWrite,
    move |ev| matches!(ev, HelmEvent::MemWrite { object_id, .. } if *object_id == uart_id),
    move |ev| { /* handle */ },
);

// Firing
bus.fire(&HelmEvent::Exception {
    hart_id: 0,
    cause: ExceptionCause::LoadFault,
    pc: 0x8000_1234,
    tval: 0xDEAD_BEEF,
});

// Unsubscribe (drop the handle, or call explicitly)
drop(handle);
```

---

## Design Decisions Answered

**Q55 — Synchronous-only callbacks.**
Phase 0 uses synchronous `fn` callbacks only. `async fn` support requires either a runtime (Tokio) or a custom executor, both of which are out of scope for Phase 0. The API is designed so that adding an async variant later does not break existing synchronous subscribers.

**Q56 — `catch_unwind` per subscriber.**
`fire()` wraps each subscriber call in `std::panic::catch_unwind`. A panicking subscriber logs the panic message and continues to the next subscriber. This prevents one bad script or plugin from crashing the entire simulation. Panics in subscribers are treated as non-fatal simulation errors.

**Q57 — Python callbacks acquire the GIL per call.**
When a Python callable is registered (via `helm-python`), the Rust callback wrapper calls `Python::with_gil(|py| callable.call1(py, (event_repr,)))`. The GIL is held only for the duration of the Python callback, then released. This is safe because the simulation runs single-threaded per hart in Phase 0. Multi-threaded Python integration is deferred.

**Q58 — Object-scoped filtering via `subscribe_filtered`.**
`subscribe_filtered(kind, predicate, callback)` stores a `(predicate, callback)` pair. During `fire()`, the predicate is evaluated first; if false, the callback is skipped. This is equivalent to `SIM_hap_add_callback_obj`. The predicate is a plain `Fn(&HelmEvent) -> bool`.

**Q59 — Recursive fire is allowed, logged as warning.**
`HelmEventBus` tracks a `firing_depth: Cell<u32>`. If a callback calls `bus.fire()` while `firing_depth > 0`, the depth is incremented and the event is dispatched normally, but a warning is logged via `tracing::warn!`. This avoids deadlock (no locks are held across callbacks). Infinite recursion is the caller's responsibility.

---

## Non-Goals for Phase 0

- Async/await subscriber callbacks.
- Event history replay.
- Cross-process event bus (no IPC).
- Priority-ordered subscriber dispatch (subscribers are called in registration order).
- Subscriber call count limits (no rate limiting).

# helm-event — Test Plan

## Test Hierarchy

| Layer | Tool | Coverage Target |
|-------|------|----------------|
| Unit | `#[test]` | All EventQueue methods, ordering, cancellation |
| Property | `proptest` | Event ordering invariants, no double-fire |
| Integration | `tests/` | Device re-post pattern, checkpoint/restore |

---

## Unit Tests — `EventClass` Registration

### `register_class_assigns_sequential_ids`

```rust
#[test]
fn register_class_assigns_sequential_ids() {
    let mut eq = EventQueue::new();
    let c0 = EventClass::new("a", Box::new(|_, _| {}));
    let c1 = EventClass::new("b", Box::new(|_, _| {}));
    let id0 = eq.register_class(c0);
    let id1 = eq.register_class(c1);
    assert_ne!(id0, id1);
}
```

### `register_duplicate_class_panics`

```rust
#[test]
#[should_panic(expected = "registered twice")]
fn register_duplicate_class_panics() {
    let mut eq = EventQueue::new();
    let c0 = EventClass::new("same_name", Box::new(|_, _| {}));
    let c1 = EventClass::new("same_name", Box::new(|_, _| {}));
    eq.register_class(c0);
    eq.register_class(c1);  // Must panic.
}
```

---

## Unit Tests — `post_cycles` / `post_at`

### `post_cycles_fires_at_correct_absolute_time`

```rust
#[test]
fn post_cycles_fires_at_correct_absolute_time() {
    let mut eq = EventQueue::new();
    let fired_at: Arc<Mutex<Option<Cycles>>> = Arc::new(Mutex::new(None));
    let fired_at_c = Arc::clone(&fired_at);

    let class = EventClass::new("t", Box::new(move |_, eq| {
        *fired_at_c.lock().unwrap() = Some(eq.current_time());
    }));
    let cid = eq.register_class(class);

    // Advance time to 100, then post for 50 cycles in the future.
    eq.drain_until(100);
    eq.post_cycles(50, cid, 0, Box::new(NoData));

    eq.drain_until(149);
    assert!(fired_at.lock().unwrap().is_none(), "Must not fire before cycle 150");

    eq.drain_until(150);
    assert_eq!(*fired_at.lock().unwrap(), Some(150u64));
}
```

### `post_at_rejects_past_time`

```rust
#[test]
#[should_panic(expected = "fire_at")]
fn post_at_rejects_past_time() {
    let mut eq = EventQueue::new();
    let cid = eq.register_class(EventClass::new("t", Box::new(|_, _| {})));
    eq.drain_until(1000);
    eq.post_at(999, cid, 0, Box::new(NoData));  // Must panic.
}
```

### `post_unregistered_class_panics`

```rust
#[test]
#[should_panic(expected = "not registered")]
fn post_unregistered_class_panics() {
    let mut eq = EventQueue::new();
    let fake_id = EventClassId(99);
    eq.post_cycles(10, fake_id, 0, Box::new(NoData));
}
```

---

## Unit Tests — `drain_until`

### `drain_until_fires_events_in_temporal_order`

```rust
#[test]
fn drain_until_fires_events_in_temporal_order() {
    let mut eq = EventQueue::new();
    let order: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));

    // Register a class that records fire_at.
    let order_c = Arc::clone(&order);
    let class = EventClass::new("ord", Box::new(move |data, eq| {
        let data = data.downcast_ref::<u64>().unwrap();
        order_c.lock().unwrap().push(eq.current_time());
    }));

    // Post events out of temporal order.
    let cid = eq.register_class(class);
    eq.post_at(30, cid, 0, Box::new(30u64));
    eq.post_at(10, cid, 0, Box::new(10u64));
    eq.post_at(20, cid, 0, Box::new(20u64));

    eq.drain_until(30);

    let fired = order.lock().unwrap();
    assert_eq!(*fired, vec![10u64, 20, 30]);
}
```

### `drain_until_does_not_fire_future_events`

```rust
#[test]
fn drain_until_does_not_fire_future_events() {
    let mut eq = EventQueue::new();
    let fired = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let fired_c = Arc::clone(&fired);

    let class = EventClass::new("f", Box::new(move |_, _| {
        fired_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    let cid = eq.register_class(class);

    eq.post_at(100, cid, 0, Box::new(NoData));
    eq.post_at(200, cid, 0, Box::new(NoData));

    eq.drain_until(100);
    assert_eq!(fired.load(std::sync::atomic::Ordering::SeqCst), 1,
        "Only the cycle-100 event should fire");

    eq.drain_until(199);
    assert_eq!(fired.load(std::sync::atomic::Ordering::SeqCst), 1,
        "Cycle-200 event must not fire before cycle 200");

    eq.drain_until(200);
    assert_eq!(fired.load(std::sync::atomic::Ordering::SeqCst), 2);
}
```

### `drain_until_fifo_within_same_cycle`

Multiple events posted for the same cycle must fire in posting order (FIFO by sequence number).

```rust
#[test]
fn drain_until_fifo_within_same_cycle() {
    let mut eq = EventQueue::new();
    let log: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));

    let log_c = Arc::clone(&log);
    let class = EventClass::new("fifo", Box::new(move |data, _| {
        let n = *data.downcast_ref::<u32>().unwrap();
        log_c.lock().unwrap().push(n);
    }));
    let cid = eq.register_class(class);

    eq.post_at(50, cid, 0, Box::new(1u32));
    eq.post_at(50, cid, 0, Box::new(2u32));
    eq.post_at(50, cid, 0, Box::new(3u32));

    eq.drain_until(50);
    assert_eq!(*log.lock().unwrap(), vec![1u32, 2, 3],
        "Same-cycle events must fire in FIFO (posting) order");
}
```

### `drain_until_advances_current_time`

```rust
#[test]
fn drain_until_advances_current_time() {
    let mut eq = EventQueue::new();
    assert_eq!(eq.current_time(), 0);
    eq.drain_until(500);
    assert_eq!(eq.current_time(), 500);
    eq.drain_until(1000);
    assert_eq!(eq.current_time(), 1000);
}
```

---

## Unit Tests — `cancel`

### `cancel_prevents_event_from_firing`

```rust
#[test]
fn cancel_prevents_event_from_firing() {
    let mut eq = EventQueue::new();
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fired_c = Arc::clone(&fired);

    let class = EventClass::new("c", Box::new(move |_, _| {
        fired_c.store(true, std::sync::atomic::Ordering::SeqCst);
    }));
    let cid = eq.register_class(class);

    let eid = eq.post_at(100, cid, 0, Box::new(NoData));
    let cancelled = eq.cancel(eid);

    assert!(cancelled, "cancel must return true for a pending event");
    eq.drain_until(200);
    assert!(!fired.load(std::sync::atomic::Ordering::SeqCst),
        "Cancelled event must not fire");
}
```

### `cancel_returns_false_for_unknown_id`

```rust
#[test]
fn cancel_returns_false_for_unknown_id() {
    let mut eq = EventQueue::new();
    // Never posted.
    let fake_id = EventId(9999);
    assert!(!eq.cancel(fake_id));
}
```

### `cancel_returns_false_if_already_cancelled`

```rust
#[test]
fn cancel_returns_false_if_already_cancelled() {
    let mut eq = EventQueue::new();
    let class = EventClass::new("x", Box::new(|_, _| {}));
    let cid = eq.register_class(class);
    let eid = eq.post_at(100, cid, 0, Box::new(NoData));

    assert!(eq.cancel(eid));
    assert!(!eq.cancel(eid), "Second cancel must return false");
}
```

### `cancel_after_fire_returns_false`

```rust
#[test]
fn cancel_after_fire_returns_false() {
    let mut eq = EventQueue::new();
    let class = EventClass::new("af", Box::new(|_, _| {}));
    let cid = eq.register_class(class);
    let eid = eq.post_at(10, cid, 0, Box::new(NoData));

    eq.drain_until(10);  // Event fires.

    // Attempting to cancel a fired event: the ID is not in the heap and
    // not in the cancelled set, so it was never inserted → returns false.
    assert!(!eq.cancel(eid), "Cannot cancel an already-fired event");
}
```

---

## Unit Tests — `peek_next_tick`

### `peek_next_tick_returns_earliest_pending`

```rust
#[test]
fn peek_next_tick_returns_earliest_pending() {
    let mut eq = EventQueue::new();
    let cid = eq.register_class(EventClass::new("p", Box::new(|_, _| {})));

    assert_eq!(eq.peek_next_tick(), None);

    eq.post_at(50, cid, 0, Box::new(NoData));
    eq.post_at(30, cid, 0, Box::new(NoData));
    eq.post_at(70, cid, 0, Box::new(NoData));

    assert_eq!(eq.peek_next_tick(), Some(30));
}
```

---

## Unit Tests — Recurring Events (Device Re-post Pattern)

### `recurring_event_fires_multiple_times`

```rust
#[test]
fn recurring_event_fires_multiple_times() {
    let mut eq = EventQueue::new();
    let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_c = Arc::clone(&count);

    // The class re-posts itself every 100 cycles.
    // We need the class_id to re-post, so we use a placeholder and patch after registration.
    let class_id_holder: Arc<Mutex<Option<EventClassId>>> = Arc::new(Mutex::new(None));
    let holder_c = Arc::clone(&class_id_holder);

    let class = EventClass::new("tick", Box::new(move |_, eq| {
        count_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(cid) = *holder_c.lock().unwrap() {
            eq.post_cycles(100, cid, 0, Box::new(NoData));
        }
    }));
    let cid = eq.register_class(class);
    *class_id_holder.lock().unwrap() = Some(cid);

    eq.post_at(100, cid, 0, Box::new(NoData));

    eq.drain_until(100);
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);

    eq.drain_until(200);
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);

    eq.drain_until(500);
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 5);
}
```

---

## Unit Tests — Checkpoint

### `checkpoint_roundtrip_checkpointable_events`

```rust
#[test]
fn checkpoint_roundtrip_checkpointable_events() {
    let mut eq = EventQueue::new();
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fired_c = Arc::clone(&fired);

    let class = EventClass::new_checkpointable(
        "ckpt_timer",
        Box::new(move |_, _| {
            fired_c.store(true, std::sync::atomic::Ordering::SeqCst);
        }),
        Box::new(|data| {
            // Serialize: our data is NoData, nothing to write.
            Some(vec![])
        }),
        Box::new(|_bytes| {
            Box::new(NoData) as Box<dyn EventData>
        }),
    );
    let cid = eq.register_class(class);
    eq.post_at(500, cid, 0, Box::new(NoData));
    eq.drain_until(100);

    // Checkpoint at cycle 100.
    let ckpt = eq.checkpoint_save();
    assert_eq!(ckpt.current_time, 100);
    assert_eq!(ckpt.events.len(), 1);

    // Restore into a new queue.
    let mut eq2 = EventQueue::new();
    let class2 = EventClass::new_checkpointable(
        "ckpt_timer",
        Box::new(move |_, _| {
            // (separate arc for eq2 test — we just check the event is restored)
        }),
        Box::new(|_| Some(vec![])),
        Box::new(|_| Box::new(NoData) as Box<dyn EventData>),
    );
    eq2.register_class(class2);
    eq2.checkpoint_restore(ckpt);

    assert_eq!(eq2.current_time(), 100);
    assert_eq!(eq2.peek_next_tick(), Some(500));
}
```

### `checkpoint_drops_non_checkpointable_events`

```rust
#[test]
fn checkpoint_drops_non_checkpointable_events() {
    let mut eq = EventQueue::new();
    let cid = eq.register_class(EventClass::new("nc", Box::new(|_, _| {})));
    eq.post_at(100, cid, 0, Box::new(NoData));

    let ckpt = eq.checkpoint_save();
    assert_eq!(ckpt.events.len(), 0,
        "Non-checkpointable events must be dropped from checkpoint");
}
```

---

## Property Tests

### `prop_events_always_fire_in_order`

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_events_always_fire_in_order(
        fire_ats in prop::collection::vec(0u64..10_000, 1..100)
    ) {
        let mut eq = EventQueue::new();
        let order: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let order_c = Arc::clone(&order);

        let class = EventClass::new("ord", Box::new(move |_, eq| {
            order_c.lock().unwrap().push(eq.current_time());
        }));
        let cid = eq.register_class(class);

        for &fat in &fire_ats {
            eq.post_at(fat, cid, 0, Box::new(NoData));
        }

        let max_fire = *fire_ats.iter().max().unwrap();
        eq.drain_until(max_fire);

        let fired = order.lock().unwrap();
        prop_assert_eq!(fired.len(), fire_ats.len(), "All events must fire");
        for window in fired.windows(2) {
            prop_assert!(window[0] <= window[1], "Events must fire in non-decreasing cycle order");
        }
    }
}
```

### `prop_cancelled_events_never_fire`

```rust
proptest! {
    #[test]
    fn prop_cancelled_events_never_fire(
        fire_ats in prop::collection::vec(1u64..10_000, 1..50),
        cancel_indices in prop::collection::vec(any::<bool>(), 1..50),
    ) {
        let mut eq = EventQueue::new();
        let fire_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let fire_count_c = Arc::clone(&fire_count);

        let class = EventClass::new("c", Box::new(move |_, _| {
            fire_count_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        let cid = eq.register_class(class);

        let mut expected_fires = 0usize;
        let pairs: Vec<_> = fire_ats.iter().zip(
            cancel_indices.iter().chain(std::iter::repeat(&false))
        ).collect();

        for (&fat, &should_cancel) in &pairs {
            let eid = eq.post_at(fat, cid, 0, Box::new(NoData));
            if should_cancel {
                eq.cancel(eid);
            } else {
                expected_fires += 1;
            }
        }

        let max_fire = *fire_ats.iter().max().unwrap();
        eq.drain_until(max_fire);

        prop_assert_eq!(
            fire_count.load(std::sync::atomic::Ordering::SeqCst) as usize,
            expected_fires,
            "Fire count must match non-cancelled events"
        );
    }
}
```

---

## Integration Tests

### `integration_per_hart_queue_decoupling`

Two harts advance at different rates. Global queue drains at the max cycle of all harts. Verifies events fire once each, from the correct queue.

```rust
#[test]
fn integration_per_hart_queue_decoupling() {
    let mut hart0_eq = EventQueue::new();
    let mut hart1_eq = EventQueue::new();
    let mut global_eq = EventQueue::new();

    let global_fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let global_fired_c = Arc::clone(&global_fired);

    let hart0_class = EventClass::new("h0.timer", Box::new(|_, _| {}));
    let global_class = EventClass::new("global.irq", Box::new(move |_, _| {
        global_fired_c.store(true, std::sync::atomic::Ordering::SeqCst);
    }));

    let h0_cid = hart0_eq.register_class(hart0_class);
    let g_cid = global_eq.register_class(global_class);

    hart0_eq.post_at(200, h0_cid, 1, Box::new(NoData));
    global_eq.post_at(300, g_cid, 0, Box::new(NoData));

    // Hart 0 advances to cycle 200.
    hart0_eq.drain_until(200);
    // Hart 1 advances to cycle 150.
    hart1_eq.drain_until(150);

    // Scheduler drains global queue at max(200, 150) = 200.
    global_eq.drain_until(200);
    assert!(!global_fired.load(std::sync::atomic::Ordering::SeqCst),
        "Global event at 300 must not fire when max hart cycle is 200");

    // Next quantum: hart1 catches up.
    hart1_eq.drain_until(350);
    hart0_eq.drain_until(350);
    global_eq.drain_until(350);
    assert!(global_fired.load(std::sync::atomic::Ordering::SeqCst),
        "Global event at 300 must fire when max hart cycle reaches 350");
}
```

---

## CI Requirements

- All unit tests: `cargo test -p helm-event`
- Property tests: `cargo test -p helm-event --features proptest` (200 iterations)
- Miri pass on all unit tests (no unsafe in this crate): `cargo +nightly miri test -p helm-event`

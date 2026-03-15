# helm-devices/bus — Test Plan

## Test Hierarchy

| Layer | Tool | Coverage Target |
|-------|------|----------------|
| Unit | `#[test]` | All 15 variants, subscribe/unsubscribe, filtering, catch_unwind |
| Property | `proptest` | Subscriber isolation, count correctness |
| Integration | `tests/` | TraceLogger subscriber pattern, recursive fire |

---

## Unit Tests — `HelmEvent::kind()`

### `event_kind_matches_variant`

```rust
#[test]
fn event_kind_matches_variant() {
    let cases: &[(HelmEvent, HelmEventKind)] = &[
        (HelmEvent::SimStart,                                       HelmEventKind::SimStart),
        (HelmEvent::SimStop { reason: "test".into() },              HelmEventKind::SimStop),
        (HelmEvent::HartReset { hart_id: 0 },                       HelmEventKind::HartReset),
        (HelmEvent::HartHalt { hart_id: 0, pc: 0 },                 HelmEventKind::HartHalt),
        (HelmEvent::Exception { hart_id:0, cause: ExceptionCause::Breakpoint, pc:0, tval:0 },
            HelmEventKind::Exception),
        (HelmEvent::Interrupt { hart_id:0, irq:1, pc:0 },           HelmEventKind::Interrupt),
        (HelmEvent::MemRead { object_id:0, addr:0, size:4, value:0, pc:0 },
            HelmEventKind::MemRead),
        (HelmEvent::MemWrite { object_id:0, addr:0, size:4, value:0, pc:0 },
            HelmEventKind::MemWrite),
        (HelmEvent::Breakpoint { hart_id:0, pc:0, breakpoint_id:0 },
            HelmEventKind::Breakpoint),
        (HelmEvent::Watchpoint { hart_id:0, addr:0, size:4, is_write:false, watchpoint_id:0 },
            HelmEventKind::Watchpoint),
        (HelmEvent::RoiBegin { hart_id:0, pc:0 },                   HelmEventKind::RoiBegin),
        (HelmEvent::RoiEnd { hart_id:0, pc:0 },                     HelmEventKind::RoiEnd),
        (HelmEvent::CheckpointSave { path: "/tmp/ck".into() },      HelmEventKind::CheckpointSave),
        (HelmEvent::CheckpointRestore { path: "/tmp/ck".into() },   HelmEventKind::CheckpointRestore),
        (HelmEvent::Custom { name: "x".into(), data: serde_json::Value::Null },
            HelmEventKind::Custom),
    ];

    for (ev, expected_kind) in cases {
        assert_eq!(ev.kind(), *expected_kind,
            "Event {:?} should have kind {:?}", ev, expected_kind);
    }
    // Ensure all 15 variants are covered.
    assert_eq!(cases.len(), 15, "All 15 HelmEvent variants must be tested");
}
```

---

## Unit Tests — Subscribe / Unsubscribe

### `subscribe_callback_is_called_on_fire`

```rust
#[test]
fn subscribe_callback_is_called_on_fire() {
    let bus = HelmEventBus::new();
    let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_c = Arc::clone(&count);

    let _handle = bus.subscribe(HelmEventKind::SimStart, move |_| {
        count_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });

    bus.fire(&HelmEvent::SimStart);
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);

    bus.fire(&HelmEvent::SimStart);
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
}
```

### `drop_handle_unsubscribes`

```rust
#[test]
fn drop_handle_unsubscribes() {
    let bus = HelmEventBus::new();
    let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_c = Arc::clone(&count);

    {
        let _handle = bus.subscribe(HelmEventKind::SimStart, move |_| {
            count_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        bus.fire(&HelmEvent::SimStart);
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
        // handle dropped here.
    }

    bus.fire(&HelmEvent::SimStart);
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1,
        "Callback must not be called after handle is dropped");
}
```

### `detached_handle_persists_after_drop`

```rust
#[test]
fn detached_handle_persists_after_drop() {
    let bus = HelmEventBus::new();
    let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_c = Arc::clone(&count);

    {
        let handle = bus.subscribe(HelmEventKind::SimStop, move |_| {
            count_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        handle.detach();
        // Handle dropped here but subscription was detached.
    }

    bus.fire(&HelmEvent::SimStop { reason: "test".into() });
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1,
        "Detached subscription must persist after handle drop");
}
```

### `multiple_subscribers_all_called`

```rust
#[test]
fn multiple_subscribers_all_called() {
    let bus = HelmEventBus::new();
    let sum = Arc::new(std::sync::atomic::AtomicU32::new(0));

    let handles: Vec<_> = (1u32..=5).map(|i| {
        let sum_c = Arc::clone(&sum);
        bus.subscribe(HelmEventKind::HartReset, move |_| {
            sum_c.fetch_add(i, std::sync::atomic::Ordering::SeqCst);
        })
    }).collect();

    bus.fire(&HelmEvent::HartReset { hart_id: 0 });
    // 1+2+3+4+5 = 15
    assert_eq!(sum.load(std::sync::atomic::Ordering::SeqCst), 15);

    drop(handles);
}
```

### `subscription_only_fires_for_matching_kind`

```rust
#[test]
fn subscription_only_fires_for_matching_kind() {
    let bus = HelmEventBus::new();
    let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_c = Arc::clone(&count);

    let _handle = bus.subscribe(HelmEventKind::Exception, move |_| {
        count_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });

    // Fire events of a different kind — must not call our callback.
    bus.fire(&HelmEvent::SimStart);
    bus.fire(&HelmEvent::HartReset { hart_id: 0 });
    bus.fire(&HelmEvent::MemRead { object_id: 0, addr: 0, size: 4, value: 0, pc: 0 });

    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0,
        "Exception subscriber must not fire on non-Exception events");

    bus.fire(&HelmEvent::Exception {
        hart_id: 0, cause: ExceptionCause::Breakpoint, pc: 0x1000, tval: 0,
    });
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
}
```

---

## Unit Tests — `subscribe_filtered`

### `filtered_subscription_only_fires_when_predicate_true`

```rust
#[test]
fn filtered_subscription_only_fires_when_predicate_true() {
    let bus = HelmEventBus::new();
    let uart_id: HelmObjectId = 42;
    let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_c = Arc::clone(&count);

    let _handle = bus.subscribe_filtered(
        HelmEventKind::MemWrite,
        move |ev| matches!(ev, HelmEvent::MemWrite { object_id, .. } if *object_id == uart_id),
        move |_| { count_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst); },
    );

    // Write from a different object: must not fire.
    bus.fire(&HelmEvent::MemWrite {
        object_id: 99, addr: 0, size: 1, value: 0, pc: 0
    });
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);

    // Write from uart_id: must fire.
    bus.fire(&HelmEvent::MemWrite {
        object_id: uart_id, addr: 0x1000_0000, size: 1, value: b'H' as u64, pc: 0x4000
    });
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
}
```

### `filtered_and_unfiltered_subscribers_coexist`

```rust
#[test]
fn filtered_and_unfiltered_subscribers_coexist() {
    let bus = HelmEventBus::new();
    let all_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let uart_count = Arc::new(std::sync::atomic::AtomicU32::new(0));

    let all_c = Arc::clone(&all_count);
    let uart_c = Arc::clone(&uart_count);
    let uart_id: HelmObjectId = 5;

    let _h1 = bus.subscribe(HelmEventKind::MemWrite, move |_| {
        all_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    let _h2 = bus.subscribe_filtered(
        HelmEventKind::MemWrite,
        move |ev| matches!(ev, HelmEvent::MemWrite { object_id, .. } if *object_id == uart_id),
        move |_| { uart_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst); },
    );

    bus.fire(&HelmEvent::MemWrite { object_id: 99, addr: 0, size: 1, value: 0, pc: 0 });
    assert_eq!(all_count.load(std::sync::atomic::Ordering::SeqCst), 1, "Unfiltered must fire");
    assert_eq!(uart_count.load(std::sync::atomic::Ordering::SeqCst), 0, "Filtered must not fire");

    bus.fire(&HelmEvent::MemWrite { object_id: uart_id, addr: 0, size: 1, value: 0, pc: 0 });
    assert_eq!(all_count.load(std::sync::atomic::Ordering::SeqCst), 2, "Unfiltered must fire again");
    assert_eq!(uart_count.load(std::sync::atomic::Ordering::SeqCst), 1, "Filtered must fire for uart");
}
```

---

## Unit Tests — `catch_unwind` Subscriber Isolation (Q56)

### `panicking_subscriber_does_not_kill_others`

```rust
#[test]
fn panicking_subscriber_does_not_kill_others() {
    let bus = HelmEventBus::new();
    let after_panic_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let after_c = Arc::clone(&after_panic_count);

    // Subscriber 1: panics.
    let _h1 = bus.subscribe(HelmEventKind::SimStart, |_| {
        panic!("intentional panic in subscriber");
    });

    // Subscriber 2: must still be called after subscriber 1 panics.
    let _h2 = bus.subscribe(HelmEventKind::SimStart, move |_| {
        after_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });

    // Must not propagate the panic.
    bus.fire(&HelmEvent::SimStart);

    assert_eq!(after_panic_count.load(std::sync::atomic::Ordering::SeqCst), 1,
        "Subscriber after the panicking one must still be called");
}
```

### `multiple_panicking_subscribers_all_isolated`

```rust
#[test]
fn multiple_panicking_subscribers_all_isolated() {
    let bus = HelmEventBus::new();
    let count = Arc::new(std::sync::atomic::AtomicU32::new(0));

    let n = 5usize;
    let mut handles = Vec::new();
    for i in 0..n {
        let count_c = Arc::clone(&count);
        if i % 2 == 0 {
            handles.push(bus.subscribe(HelmEventKind::HartHalt, move |_| {
                panic!("panic from subscriber {}", i);
            }));
        } else {
            handles.push(bus.subscribe(HelmEventKind::HartHalt, move |_| {
                count_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }));
        }
    }

    bus.fire(&HelmEvent::HartHalt { hart_id: 0, pc: 0 });

    // 5 subscribers: indices 0,2,4 panic; 1,3 succeed.
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2,
        "Non-panicking subscribers must all fire despite others panicking");
}
```

---

## Unit Tests — Recursive Fire (Q59)

### `recursive_fire_from_callback_completes`

```rust
#[test]
fn recursive_fire_from_callback_completes() {
    let bus = HelmEventBus::new();
    let bus_clone = bus.clone();
    let outer_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let inner_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let outer_c = Arc::clone(&outer_count);
    let inner_c = Arc::clone(&inner_count);

    // SimStart subscriber fires a SimStop from within.
    let _h1 = bus.subscribe(HelmEventKind::SimStart, move |_| {
        outer_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        bus_clone.fire(&HelmEvent::SimStop { reason: "from_callback".into() });
    });

    let _h2 = bus.subscribe(HelmEventKind::SimStop, move |_| {
        inner_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });

    bus.fire(&HelmEvent::SimStart);

    assert_eq!(outer_count.load(std::sync::atomic::Ordering::SeqCst), 1,
        "Outer (SimStart) subscriber must fire once");
    assert_eq!(inner_count.load(std::sync::atomic::Ordering::SeqCst), 1,
        "Inner (SimStop from recursive fire) subscriber must fire once");
}
```

---

## Unit Tests — `no_subscribers_is_a_noop`

```rust
#[test]
fn no_subscribers_is_a_noop() {
    let bus = HelmEventBus::new();
    // Must not panic even when there are no subscribers.
    bus.fire(&HelmEvent::SimStart);
    bus.fire(&HelmEvent::Exception {
        hart_id: 0, cause: ExceptionCause::InstructionAccessFault, pc: 0, tval: 0,
    });
}
```

---

## Unit Tests — Clone Shares Subscribers

```rust
#[test]
fn cloned_bus_shares_subscribers() {
    let bus1 = HelmEventBus::new();
    let bus2 = bus1.clone();

    let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_c = Arc::clone(&count);

    // Subscribe on bus1.
    let _handle = bus1.subscribe(HelmEventKind::RoiBegin, move |_| {
        count_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });

    // Fire on bus2: must reach the subscriber registered on bus1.
    bus2.fire(&HelmEvent::RoiBegin { hart_id: 0, pc: 0x8000 });
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1,
        "Clone must share the subscriber map via Arc");
}
```

---

## Property Tests

### `prop_fire_count_matches_subscriber_count`

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_fire_count_matches_subscriber_count(n_subscribers in 0usize..20) {
        let bus = HelmEventBus::new();
        let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let mut handles = Vec::new();

        for _ in 0..n_subscribers {
            let count_c = Arc::clone(&count);
            handles.push(bus.subscribe(HelmEventKind::SimStart, move |_| {
                count_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }));
        }

        bus.fire(&HelmEvent::SimStart);
        prop_assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst) as usize,
            n_subscribers,
            "Fire count must equal subscriber count"
        );
    }
}
```

### `prop_unsubscribe_reduces_fire_count`

```rust
proptest! {
    #[test]
    fn prop_unsubscribe_reduces_fire_count(
        n_subscribers in 1usize..20,
        n_drop in 0usize..20,
    ) {
        let n_drop = n_drop.min(n_subscribers);
        let bus = HelmEventBus::new();
        let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let mut handles = Vec::new();

        for _ in 0..n_subscribers {
            let count_c = Arc::clone(&count);
            handles.push(bus.subscribe(HelmEventKind::RoiEnd, move |_| {
                count_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }));
        }

        // Drop some handles.
        handles.truncate(n_subscribers - n_drop);

        bus.fire(&HelmEvent::RoiEnd { hart_id: 0, pc: 0 });
        prop_assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst) as usize,
            n_subscribers - n_drop,
            "Fire count must equal remaining (non-dropped) subscribers"
        );
    }
}
```

---

## Integration Tests

### `integration_trace_logger_subscriber_pattern`

Verifies that a `TraceLogger`-style object can subscribe to multiple event kinds and collect them correctly.

```rust
#[test]
fn integration_trace_logger_subscriber_pattern() {
    use std::sync::Mutex;

    let bus = HelmEventBus::new();
    let log: Arc<Mutex<Vec<HelmEventKind>>> = Arc::new(Mutex::new(Vec::new()));

    let mut handles = Vec::new();
    for kind in &[
        HelmEventKind::Exception,
        HelmEventKind::MemWrite,
        HelmEventKind::RoiBegin,
        HelmEventKind::RoiEnd,
    ] {
        let log_c = Arc::clone(&log);
        let k = *kind;
        handles.push(bus.subscribe(*kind, move |ev| {
            log_c.lock().unwrap().push(ev.kind());
        }));
    }

    bus.fire(&HelmEvent::SimStart);  // Not subscribed; must be ignored.
    bus.fire(&HelmEvent::Exception { hart_id: 0, cause: ExceptionCause::EcallUMode, pc: 0, tval: 0 });
    bus.fire(&HelmEvent::MemWrite { object_id: 1, addr: 0x1000, size: 4, value: 42, pc: 0 });
    bus.fire(&HelmEvent::RoiBegin { hart_id: 0, pc: 0x4000 });
    bus.fire(&HelmEvent::RoiEnd { hart_id: 0, pc: 0x8000 });

    let fired = log.lock().unwrap();
    assert_eq!(*fired, vec![
        HelmEventKind::Exception,
        HelmEventKind::MemWrite,
        HelmEventKind::RoiBegin,
        HelmEventKind::RoiEnd,
    ]);
}
```

### `integration_all_15_variants_can_be_fired`

Ensures that every `HelmEvent` variant can be constructed, fired, and received by a catch-all subscriber without panicking.

```rust
#[test]
fn integration_all_15_variants_can_be_fired() {
    let bus = HelmEventBus::new();
    let received_kinds: Arc<Mutex<Vec<HelmEventKind>>> = Arc::new(Mutex::new(Vec::new()));
    let rk_c = Arc::clone(&received_kinds);

    // Subscribe to all 15 kinds.
    let mut handles = Vec::new();
    let all_kinds = [
        HelmEventKind::SimStart, HelmEventKind::SimStop,
        HelmEventKind::HartReset, HelmEventKind::HartHalt,
        HelmEventKind::Exception, HelmEventKind::Interrupt,
        HelmEventKind::MemRead, HelmEventKind::MemWrite,
        HelmEventKind::Breakpoint, HelmEventKind::Watchpoint,
        HelmEventKind::RoiBegin, HelmEventKind::RoiEnd,
        HelmEventKind::CheckpointSave, HelmEventKind::CheckpointRestore,
        HelmEventKind::Custom,
    ];
    for kind in all_kinds {
        let rk = Arc::clone(&rk_c);
        handles.push(bus.subscribe(kind, move |ev| {
            rk.lock().unwrap().push(ev.kind());
        }));
    }

    let events = vec![
        HelmEvent::SimStart,
        HelmEvent::SimStop { reason: "done".into() },
        HelmEvent::HartReset { hart_id: 0 },
        HelmEvent::HartHalt { hart_id: 0, pc: 0 },
        HelmEvent::Exception { hart_id: 0, cause: ExceptionCause::Breakpoint, pc: 0, tval: 0 },
        HelmEvent::Interrupt { hart_id: 0, irq: 1, pc: 0 },
        HelmEvent::MemRead { object_id: 0, addr: 0, size: 4, value: 0, pc: 0 },
        HelmEvent::MemWrite { object_id: 0, addr: 0, size: 4, value: 0, pc: 0 },
        HelmEvent::Breakpoint { hart_id: 0, pc: 0, breakpoint_id: 0 },
        HelmEvent::Watchpoint { hart_id: 0, addr: 0, size: 4, is_write: false, watchpoint_id: 0 },
        HelmEvent::RoiBegin { hart_id: 0, pc: 0 },
        HelmEvent::RoiEnd { hart_id: 0, pc: 0 },
        HelmEvent::CheckpointSave { path: "/tmp/ck".into() },
        HelmEvent::CheckpointRestore { path: "/tmp/ck".into() },
        HelmEvent::Custom { name: "custom".into(), data: serde_json::Value::Null },
    ];

    for ev in &events {
        bus.fire(ev);
    }

    let received = received_kinds.lock().unwrap();
    assert_eq!(received.len(), 15, "All 15 variants must be received");

    // Verify order matches fire order.
    for (i, (received_kind, fired_event)) in received.iter().zip(events.iter()).enumerate() {
        assert_eq!(*received_kind, fired_event.kind(),
            "Event {} kind mismatch: got {:?}, expected {:?}", i, received_kind, fired_event.kind());
    }
}
```

---

## CI Requirements

- All unit tests: `cargo test -p helm-devices/bus`
- Property tests: `cargo test -p helm-devices/bus --features proptest` (200 iterations)
- `cargo test -p helm-devices/bus` must complete without any `tracing::error!` output in CI (panic log lines are acceptable only in the `catch_unwind` tests).
- Feature flag `pyo3`: `cargo test -p helm-devices/bus --features pyo3` (requires Python 3.x dev headers in CI).

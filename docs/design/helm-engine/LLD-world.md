# helm-engine — LLD: World API

> Complete Rust API specification for the `World` struct.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-bus-framework.md`](./LLD-bus-framework.md) · [`TEST.md`](./TEST.md)

---

## Table of Contents

1. [World Struct](#1-deviceworld-struct)
2. [Construction and Registration](#2-construction-and-registration)
3. [Lifecycle — elaborate()](#3-lifecycle--elaborate)
4. [MMIO Operations](#4-mmio-operations)
5. [Signal Interface](#5-signal-interface)
6. [Time Advancement](#6-time-advancement)
7. [Observability](#7-observability)
8. [Interrupt Interface](#8-interrupt-interface)
9. [VirtualClock](#9-virtualclock)
10. [WorldInterruptSink](#10-deviceworldinterruptsink)
11. [Full Implementation Sketch](#11-full-implementation-sketch)

---

## 1. World Struct

```rust
// crates/helm-engine/src/world.rs

use std::collections::HashMap;
use std::sync::Arc;
use helm_devices::{Device, InterruptPin, InterruptSink};
use helm_event::EventQueue;
use helm_devices::bus::event_bus::{HelmEventBus, HelmEvent, HelmEventKind};
use helm_memory::MemoryMap;
use helm_stats::StatsRegistry;
use crate::clock::VirtualClock;
use crate::interrupt_sink::WorldInterruptSink;

/// A stable opaque identifier for a device registered in a World.
///
/// Assigned by add_device(), valid for the lifetime of the World.
/// Stable across elaborate() calls on the same world instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HelmObjectId(pub(crate) u64);

/// A registered device and its metadata.
struct RegisteredDevice {
    name:     String,
    device:   Box<dyn Device>,
    base:     Option<u64>,           // None until map_device() is called
    irq_pins: Vec<(String, InterruptPin)>,  // (pin_name, pin) pairs
}

/// Drop guard for an event bus subscription.
/// Dropping this value unsubscribes the callback.
pub struct EventHandle {
    id:  helm_devices::bus::event_bus::SubscriberId,
    bus: Arc<HelmEventBus>,
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        self.bus.unsubscribe(self.id);
    }
}

/// Headless device simulation environment.
///
/// Provides the minimum substrate required to exercise device models:
///   - MMIO dispatch (via MemoryMap)
///   - Device timer callbacks (via EventQueue)
///   - Observable events (via HelmEventBus)
///   - A virtual clock (via VirtualClock)
///   - Interrupt observation (via WorldInterruptSink)
///
/// No CPU, no ISA, no ArchState. No dependency on helm-core, helm-arch, or helm-engine.
pub struct World {
    /// Registered devices, keyed by HelmObjectId.
    objects:   HashMap<HelmObjectId, RegisteredDevice>,

    /// Unified MMIO address space.
    memory:    MemoryMap,

    /// Time-ordered callback queue for device timer events.
    event_queue: EventQueue,

    /// Synchronous observable event bus.
    event_bus:   Arc<HelmEventBus>,

    /// Monotonic virtual clock.
    clock:       VirtualClock,

    /// Performance counter registry.
    stats:       StatsRegistry,

    /// Built-in interrupt sink — records assert/deassert per (device, pin).
    irq_sink:    Arc<WorldInterruptSink>,

    /// Monotonic ID counter for add_device().
    next_id:     u64,

    /// True after elaborate() has been called. Prevents double-elaborate.
    elaborated:  bool,
}
```

---

## 2. Construction and Registration

### World::new()

```rust
impl World {
    /// Create an empty World.
    ///
    /// No devices, no MMIO mappings, VirtualClock at tick 0.
    /// Memory: initially empty MemoryMap with no regions mapped.
    pub fn new() -> Self {
        let irq_sink = Arc::new(WorldInterruptSink::new());
        World {
            objects:     HashMap::new(),
            memory:      MemoryMap::new(),
            event_queue: EventQueue::new(),
            event_bus:   Arc::new(HelmEventBus::new()),
            clock:       VirtualClock::new(),
            stats:        StatsRegistry::new(),
            irq_sink,
            next_id:     1,
            elaborated:  false,
        }
    }
```

### World::add_device()

```rust
    /// Register a device with the given name.
    ///
    /// Returns a stable HelmObjectId. The device is not mapped to any
    /// MMIO address yet; call map_device() to place it in the address space.
    ///
    /// Calls device.init() immediately (before elaborate()).
    ///
    /// Panics:
    ///   - if elaborate() has already been called on this world.
    ///   - if `name` is empty.
    pub fn add_device(&mut self, name: &str, mut device: Box<dyn Device>) -> HelmObjectId {
        assert!(!self.elaborated, "add_device() called after elaborate()");
        assert!(!name.is_empty(), "device name must not be empty");

        device.init();

        let id = HelmObjectId(self.next_id);
        self.next_id += 1;

        self.objects.insert(id, RegisteredDevice {
            name:     name.to_string(),
            device,
            base:     None,
            irq_pins: Vec::new(),
        });

        id
    }
```

### World::map_device()

```rust
    /// Map a registered device into the MMIO address space at `base`.
    ///
    /// The mapping covers [base, base + device.region_size()).
    ///
    /// Panics:
    ///   - if `id` is not a registered device.
    ///   - if the device is already mapped.
    ///   - if the address range overlaps an existing mapping.
    ///   - if elaborate() has already been called.
    pub fn map_device(&mut self, id: HelmObjectId, base: u64) {
        assert!(!self.elaborated, "map_device() called after elaborate()");

        let reg = self.objects.get_mut(&id)
            .unwrap_or_else(|| panic!("map_device: unknown HelmObjectId {:?}", id));

        assert!(reg.base.is_none(), "device {:?} is already mapped", reg.name);

        let size = reg.device.region_size();
        // MemoryMap::register_mmio() panics on overlap — satisfies the contract
        self.memory.register_mmio(base, size, id);

        reg.base = Some(base);
    }
```

### World::wire_interrupt()

```rust
    /// Connect a device's interrupt output pin to an interrupt sink.
    ///
    /// In most tests, `sink` is `self.irq_sink()` — the built-in observer.
    /// For routing tests, `sink` is a PLIC device's input sink.
    ///
    /// The `pin` must be an InterruptPin owned by the device. The device
    /// provides named pins via `Device::irq_pin(name) -> &mut InterruptPin`.
    ///
    /// Panics:
    ///   - if elaborate() has already been called.
    pub fn wire_interrupt(
        &mut self,
        from_device: HelmObjectId,
        pin_name:    &str,
        to_sink:     Arc<dyn InterruptSink>,
    ) {
        assert!(!self.elaborated, "wire_interrupt() called after elaborate()");

        let reg = self.objects.get_mut(&from_device)
            .unwrap_or_else(|| panic!("wire_interrupt: unknown HelmObjectId {:?}", from_device));

        let pin = reg.device.irq_pin_mut(pin_name)
            .unwrap_or_else(|| panic!(
                "device '{}' has no interrupt pin named '{}'",
                reg.name, pin_name
            ));

        pin.connect(InterruptWire::new(to_sink, WireId::new(from_device.0, pin_name)));
    }

    /// Return a reference to the built-in interrupt sink.
    ///
    /// Use this as the `to_sink` argument for wire_interrupt() when you want
    /// the World's pending_interrupts() query to reflect the interrupt state.
    pub fn irq_sink(&self) -> Arc<dyn InterruptSink> {
        Arc::clone(&self.irq_sink) as Arc<dyn InterruptSink>
    }
```

---

## 3. Lifecycle — elaborate()

```rust
    /// Finalize the component graph.
    ///
    /// Drives the SimObject lifecycle for all registered devices:
    ///   1. elaborate() — devices acquire EventQueue, HelmEventBus, StatsRegistry refs
    ///   2. startup() — devices schedule initial events, assert initial signals
    ///
    /// Must be called exactly once, after all add_device() / map_device() /
    /// wire_interrupt() calls and before any mmio_write / advance / etc.
    ///
    /// Panics:
    ///   - if called more than once.
    ///   - if any device's elaborate() panics.
    pub fn elaborate(&mut self) {
        assert!(!self.elaborated, "elaborate() called more than once");

        // Build the WorldSystem context — passed to device.elaborate()
        let ctx = WorldContext {
            memory:      &mut self.memory,
            event_queue: &self.event_queue,
            event_bus:   &self.event_bus,
            stats:       &mut self.stats,
        };

        // elaborate() in add_device order (insertion order via HashMap iteration
        // is NOT guaranteed — use a sorted list of ids for determinism)
        let mut ids: Vec<HelmObjectId> = self.objects.keys().copied().collect();
        ids.sort_by_key(|id| id.0);

        for id in &ids {
            let reg = self.objects.get_mut(id).unwrap();
            reg.device.elaborate_in_world(&ctx);
        }

        // startup() in the same order
        for id in &ids {
            let reg = self.objects.get_mut(id).unwrap();
            reg.device.startup();
        }

        self.elaborated = true;
    }
```

---

## 4. MMIO Operations

```rust
    /// Perform a MMIO write of `size` bytes at absolute `addr`.
    ///
    /// Dispatches to the device mapped at `addr` via MemoryMap FlatView lookup.
    /// The device receives the offset (addr - base) and the value.
    ///
    /// Fires a HelmEvent::MemWrite on the event bus.
    ///
    /// Panics:
    ///   - if no device is mapped at `addr`.
    ///   - if `size` is not 1, 2, 4, or 8.
    ///   - if elaborate() has not been called.
    pub fn mmio_write(&mut self, addr: u64, size: usize, val: u64) {
        self.require_elaborated();
        assert!(
            matches!(size, 1 | 2 | 4 | 8),
            "mmio_write: invalid size {size}, must be 1, 2, 4, or 8"
        );

        let (id, offset) = self.memory.lookup(addr)
            .unwrap_or_else(|| panic!("mmio_write: no device mapped at {addr:#x}"));

        let reg = self.objects.get_mut(&id).unwrap();
        reg.device.write(offset, size, val);

        self.event_bus.fire(HelmEvent::MemWrite {
            addr,
            size,
            val,
            cycle: self.clock.current_tick(),
        });
    }

    /// Perform a MMIO read of `size` bytes from absolute `addr`.
    ///
    /// Returns the device's response. Value is zero-extended to u64.
    ///
    /// Fires a HelmEvent::MemRead on the event bus.
    ///
    /// Panics:
    ///   - if no device is mapped at `addr`.
    ///   - if `size` is not 1, 2, 4, or 8.
    ///   - if elaborate() has not been called.
    pub fn mmio_read(&self, addr: u64, size: usize) -> u64 {
        self.require_elaborated();
        assert!(
            matches!(size, 1 | 2 | 4 | 8),
            "mmio_read: invalid size {size}, must be 1, 2, 4, or 8"
        );

        let (id, offset) = self.memory.lookup(addr)
            .unwrap_or_else(|| panic!("mmio_read: no device mapped at {addr:#x}"));

        let reg = self.objects.get(&id).unwrap();
        let val = reg.device.read(offset, size);

        self.event_bus.fire(HelmEvent::MemRead {
            addr,
            size,
            val,
            cycle: self.clock.current_tick(),
        });

        val
    }
```

---

## 5. Signal Interface

```rust
    /// Assert a named signal on a device (active-high).
    ///
    /// Calls device.signal(port, 1).
    /// Use for: "reset", "clock_enable", "dma_ack", etc.
    ///
    /// Panics: if `id` is not registered or `port` is empty.
    pub fn signal_raise(&mut self, id: HelmObjectId, port: &str) {
        self.require_elaborated();
        let reg = self.objects.get_mut(&id)
            .unwrap_or_else(|| panic!("signal_raise: unknown id {:?}", id));
        reg.device.signal(port, 1);
        self.event_bus.fire(HelmEvent::DeviceSignal {
            device: reg.name.as_str(),
            port,
            val: 1,
        });
    }

    /// Deassert a named signal on a device (active-high → 0).
    ///
    /// Calls device.signal(port, 0).
    ///
    /// Panics: if `id` is not registered or `port` is empty.
    pub fn signal_lower(&mut self, id: HelmObjectId, port: &str) {
        self.require_elaborated();
        let reg = self.objects.get_mut(&id)
            .unwrap_or_else(|| panic!("signal_lower: unknown id {:?}", id));
        reg.device.signal(port, 0);
        self.event_bus.fire(HelmEvent::DeviceSignal {
            device: reg.name.as_str(),
            port,
            val: 0,
        });
    }
```

---

## 6. Time Advancement

```rust
    /// Advance the virtual clock by `cycles` ticks.
    ///
    /// Drains all events scheduled at or before `clock.current_tick() + cycles`.
    /// Events are processed in tick order. Callbacks may schedule new events;
    /// those are drained if they fall within the advance window.
    ///
    /// After draining, advances VirtualClock by `cycles` regardless of event count.
    ///
    /// Panics: if elaborate() has not been called.
    pub fn advance(&mut self, cycles: u64) {
        self.require_elaborated();

        let target = self.clock.current_tick() + cycles;

        // Drain events up to target tick
        while let Some(tick) = self.event_queue.peek_tick() {
            if tick > target { break; }
            // Safety: we just peeked, so pop will succeed
            let event = self.event_queue.pop().unwrap();
            // Set clock to event tick before calling callback
            self.clock.set(event.tick);
            (event.callback)();
        }

        // Advance clock to target (may already be there if events consumed it)
        self.clock.set(target);
    }

    /// Advance the clock to the next scheduled event and process it.
    ///
    /// If no events are pending, returns immediately without advancing.
    ///
    /// Panics: if elaborate() has not been called.
    pub fn run_to_next_event(&mut self) {
        self.require_elaborated();
        if let Some(event) = self.event_queue.pop() {
            self.clock.set(event.tick);
            (event.callback)();
        }
    }

    /// Return the current virtual clock tick.
    pub fn current_tick(&self) -> u64 {
        self.clock.current_tick()
    }
```

---

## 7. Observability

```rust
    /// Subscribe to a HelmEvent kind. Returns an EventHandle drop guard.
    ///
    /// The callback `f` is called synchronously when the event fires.
    /// Drop the returned EventHandle to unsubscribe.
    ///
    /// Thread safety: `f` must be `Send + 'static`.
    pub fn on_event<F>(&self, kind: HelmEventKind, f: F) -> EventHandle
    where
        F: Fn(&HelmEvent) + Send + 'static,
    {
        let id = self.event_bus.subscribe(kind, f);
        EventHandle { id, bus: Arc::clone(&self.event_bus) }
    }

    /// Return a reference to the HelmEventBus for direct subscription.
    pub fn event_bus(&self) -> &Arc<HelmEventBus> {
        &self.event_bus
    }

    /// Return a reference to the StatsRegistry for reading performance counters.
    pub fn stats(&self) -> &StatsRegistry {
        &self.stats
    }

    /// Return the name of a registered device.
    ///
    /// Returns None if `id` is not registered.
    pub fn device_name(&self, id: HelmObjectId) -> Option<&str> {
        self.objects.get(&id).map(|r| r.name.as_str())
    }

    /// Return all currently-asserted interrupt pins.
    ///
    /// Each entry is (HelmObjectId, pin_name: String).
    /// Uses the built-in WorldInterruptSink.
    pub fn pending_interrupts(&self) -> Vec<(HelmObjectId, String)> {
        self.irq_sink.pending()
    }

    // Internal helper
    fn require_elaborated(&self) {
        assert!(
            self.elaborated,
            "World operation called before elaborate()"
        );
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}
```

---

## 8. Interrupt Interface

Devices expose named interrupt output pins via `Device::irq_pin_mut(name)`. This method returns `None` if the device has no pin with that name.

The `Device` trait extension for World:

```rust
// In helm-devices/src/device.rs

pub trait Device: SimObject {
    fn read(&self, offset: u64, size: usize) -> u64;
    fn write(&mut self, offset: u64, size: usize, val: u64);
    fn region_size(&self) -> u64;
    fn signal(&mut self, name: &str, val: u64);

    /// Return a mutable reference to a named interrupt output pin.
    ///
    /// Returns None if this device has no pin with the given name.
    /// Devices declare their pins as fields and return references to them here.
    fn irq_pin_mut(&mut self, name: &str) -> Option<&mut InterruptPin> {
        let _ = name;
        None   // default: no interrupt pins
    }
}
```

A UART with one interrupt output declares it as:

```rust
pub struct Uart16550 {
    // ... register state ...
    pub irq_out: InterruptPin,
}

impl Device for Uart16550 {
    fn irq_pin_mut(&mut self, name: &str) -> Option<&mut InterruptPin> {
        match name {
            "irq_out" => Some(&mut self.irq_out),
            _         => None,
        }
    }
    // ...
}
```

---

## 9. VirtualClock

```rust
// crates/helm-engine/src/clock.rs

/// A monotonically-increasing virtual clock.
///
/// Tick 0 = reset state. All World operations happen relative to
/// the current tick. Devices schedule events at absolute tick values.
pub struct VirtualClock {
    tick: u64,
}

impl VirtualClock {
    pub fn new() -> Self {
        VirtualClock { tick: 0 }
    }

    /// Return the current tick.
    #[inline]
    pub fn current_tick(&self) -> u64 {
        self.tick
    }

    /// Set the current tick. Used by World::advance() after draining events.
    ///
    /// Panics if `new_tick < self.tick` (clock must not go backward).
    #[inline]
    pub fn set(&mut self, new_tick: u64) {
        assert!(
            new_tick >= self.tick,
            "VirtualClock: attempted to set tick backward from {} to {}",
            self.tick, new_tick
        );
        self.tick = new_tick;
    }

    /// Advance the clock by `delta` ticks.
    #[inline]
    pub fn advance(&mut self, delta: u64) {
        self.tick = self.tick.checked_add(delta)
            .expect("VirtualClock: tick overflow (u64)");
    }
}
```

---

## 10. WorldInterruptSink

```rust
// crates/helm-engine/src/interrupt_sink.rs

use std::collections::HashMap;
use std::sync::Mutex;
use helm_devices::{InterruptSink, WireId};
use crate::world::HelmObjectId;

/// Built-in interrupt observer for World.
///
/// Records all assert/deassert calls into an internal map.
/// World::pending_interrupts() queries this map.
pub struct WorldInterruptSink {
    /// Maps (device_id_u64, pin_name) → currently_asserted
    state: Mutex<HashMap<(u64, String), bool>>,
}

impl WorldInterruptSink {
    pub fn new() -> Self {
        WorldInterruptSink {
            state: Mutex::new(HashMap::new()),
        }
    }

    /// Return all (HelmObjectId, pin_name) pairs that are currently asserted.
    pub fn pending(&self) -> Vec<(HelmObjectId, String)> {
        let state = self.state.lock().unwrap();
        state.iter()
            .filter(|(_, &asserted)| asserted)
            .map(|((id, name), _)| (HelmObjectId(*id), name.clone()))
            .collect()
    }
}

impl InterruptSink for WorldInterruptSink {
    fn on_assert(&self, wire_id: WireId) {
        let mut state = self.state.lock().unwrap();
        state.insert((wire_id.device_id, wire_id.pin_name.to_string()), true);
    }

    fn on_deassert(&self, wire_id: WireId) {
        let mut state = self.state.lock().unwrap();
        state.insert((wire_id.device_id, wire_id.pin_name.to_string()), false);
    }
}
```

---

## 11. Full Implementation Sketch

Complete usage example from Rust tests:

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;
    use helm_devices::uart::Uart16550;
    use helm_devices::bus::event_bus::HelmEventKind;
    use std::sync::{Arc, Mutex};

    const UART_BASE: u64 = 0x10000000;

    fn uart_world() -> (World, HelmObjectId) {
        let mut world = World::new();

        // Register device
        let uart_id = world.add_device("uart", Box::new(Uart16550::new(1_843_200)));

        // Map into MMIO space
        world.map_device(uart_id, UART_BASE);

        // Wire irq_out to built-in sink (so pending_interrupts() works)
        world.wire_interrupt(uart_id, "irq_out", world.irq_sink());

        // Finalize
        world.elaborate();

        (world, uart_id)
    }

    #[test]
    fn test_uart_tx_basic() {
        let (mut world, uart_id) = uart_world();

        // Enable TX interrupt (IER bit 1 = THRE enable)
        world.mmio_write(UART_BASE + 1, 1, 0x02);

        // Write 'A' to THR (TX Holding Register, offset 0 when DLAB=0)
        world.mmio_write(UART_BASE, 1, b'A' as u64);

        // Advance one baud period: 1.8432 MHz at 9600 baud
        // cycles_per_bit = 1_843_200 / (16 * 9600) = 12 ticks per baud clock tick
        // 10 bits * 12 = 120 ticks minimum; advance 200 for margin
        world.advance(200);

        // THRE interrupt should now be asserted
        let irqs = world.pending_interrupts();
        assert!(
            irqs.iter().any(|(_, pin)| pin == "irq_out"),
            "THRE interrupt not asserted after TX: irqs = {:?}", irqs
        );

        println!("tick after tx: {}", world.current_tick());
    }

    #[test]
    fn test_event_bus_observes_mmio_writes() {
        let (mut world, _) = uart_world();

        let observed: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let obs_clone = Arc::clone(&observed);

        let _handle = world.on_event(HelmEventKind::MemWrite, move |event| {
            if let HelmEvent::MemWrite { addr, .. } = event {
                obs_clone.lock().unwrap().push(*addr);
            }
        });

        world.mmio_write(UART_BASE, 1, 0x42);
        world.mmio_write(UART_BASE + 3, 1, 0x03);

        let writes = observed.lock().unwrap();
        assert!(writes.contains(&UART_BASE), "TX write not observed");
        assert!(writes.contains(&(UART_BASE + 3)), "LCR write not observed");
    }

    #[test]
    fn test_current_tick_advances() {
        let mut world = World::new();
        world.elaborate();

        assert_eq!(world.current_tick(), 0);
        world.advance(1000);
        assert_eq!(world.current_tick(), 1000);
        world.advance(500);
        assert_eq!(world.current_tick(), 1500);
    }

    #[test]
    fn test_reset_via_re_instantiation() {
        // For fuzzing: create a fresh World per iteration
        for iteration in 0..10 {
            let (mut world, uart_id) = uart_world();

            // Each iteration starts at tick 0 with no pending IRQs
            assert_eq!(world.current_tick(), 0, "iteration {iteration}");
            assert!(world.pending_interrupts().is_empty(), "iteration {iteration}");

            // Do some work
            world.mmio_write(UART_BASE, 1, iteration as u64);
            world.advance(100);

            // Drop world — implicit cleanup
        }
    }
}
```

---

## Design Decisions from Q&A

### Design Decision: Single World type for device-only and full simulation (Q101)

`World` is the same type regardless of whether a CPU (`HelmEngine`) is registered. Device-only mode = `World` without any `HelmEngine` registered. Full simulation mode = `World` with one or more `HelmEngine` instances registered and driven by the `Scheduler`. There is no separate `DeviceWorld` type. The `World` already provides everything needed for device-only use — MMIO dispatch, timer callbacks, observability, stats, and interrupt wiring. Not adding a CPU is device-only mode.

### Design Decision: Monolithic struct with WorldContext at elaborate (Q107)

`World` is a monolithic struct with all fields as direct members (as shown in §1). The `elaborate()` method creates a temporary `WorldContext` (a struct of shared references) passed to each device's `elaborate_in_world()` — this avoids borrow conflicts during elaboration without splitting the struct permanently. Borrow checker conflicts (cannot borrow `objects` and `memory` simultaneously) are handled by looking up the device ID from `memory` first, then looking up the device in `objects` — two separate borrows.

### Design Decision: HelmObjectId is a monotonic u64 wrapper (Q108)

`HelmObjectId` is `struct HelmObjectId(pub(crate) u64)`. Assigned by `add_device()`, incremented from `next_id: u64` in `World`. Never reused. Stored in `HashMap<HelmObjectId, RegisteredDevice>` for O(1) lookup. `next_id` starts at 1; ID 0 is reserved as a sentinel value. Since `World` never removes a device after `elaborate()`, use-after-free is impossible — the simplicity of a monotonic u64 is sufficient without the overhead of generational indices.

### Design Decision: Rust edition 2021, MSRV 1.70 (Q109)

The workspace uses Rust edition 2021 with MSRV 1.70 (`workspace.package.rust-version = "1.70"`). This covers all required features: GATs (1.65), let-else (1.65), PyO3 0.20 requirements (1.63), `deku` 0.16 (1.60). MSRV is enforced in CI by `cargo +1.70 check` as a separate job. Adding a dependency with MSRV > 1.70 requires a workspace-wide MSRV bump.

---

*For the bus framework (PCI, I2C, SPI), see [`LLD-bus-framework.md`](./LLD-bus-framework.md). For tests, see [`TEST.md`](./TEST.md).*

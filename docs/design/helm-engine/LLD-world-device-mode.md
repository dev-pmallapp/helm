# helm-engine — High-Level Design

> High-level design for `helm-engine`: headless device simulation without a CPU, ISA, or `ArchState`.
> Cross-references: [`docs/design/HLD.md`](../HLD.md) · [`LLD-world.md`](./LLD-world.md) · [`LLD-bus-framework.md`](./LLD-bus-framework.md) · [`TEST.md`](./TEST.md) · [`docs/research/device-world.md`](../../research/device-world.md)

---

## Table of Contents

1. [Purpose and Use Cases](#1-purpose-and-use-cases)
2. [Position in the System Stack](#2-position-in-the-system-stack)
3. [Design Principles](#3-design-principles)
4. [Crate Structure and Dependencies](#4-crate-structure-and-dependencies)
5. [Component Model in World](#5-component-model-in-deviceworld)
6. [Time Model](#6-time-model)
7. [Interrupt Model](#7-interrupt-model)
8. [Observability](#8-observability)
9. [Fuzzing Support](#9-fuzzing-support)
10. [Key Design Decisions](#10-key-design-decisions)

---

## 1. Purpose and Use Cases

`World` provides a simulation environment for device models that requires no CPU, no ISA, and no `ArchState`. The user instantiates devices, maps them into a virtual address space, advances a virtual clock, and drives MMIO transactions directly — the same transactions that firmware would generate, but without firmware.

### Why This Exists

Full-system simulation requires a CPU, a kernel, and usually firmware to exercise any device model. This creates a fundamental testing problem:

- A UART bug that manifests at Linux boot requires running all of: bootloader, kernel initialization, interrupt controllers, driver probe, UART config, device write. Isolating the hardware bug requires hours of debugging, not minutes.
- Protocol edge cases and error paths are never exercised by real firmware, which follows the happy path.
- A new device model cannot be tested at all until a complete platform exists.
- Fuzzing a device state machine through a full-system simulator is 1000x too slow to be useful.

`World` solves all four problems by inverting the dependency: the CPU is the optional component, not the required one.

### Use Case Taxonomy

| Use case | World role |
|---|---|
| Device unit test (register-level) | Drive MMIO reads/writes, advance clock, assert interrupt state |
| Protocol fuzzing | Feed libFuzzer byte stream as MMIO sequences; no process overhead |
| Bus protocol simulation | Drive PCI config reads, I2C start/data/stop, SPI chip-select, without CPU |
| Interrupt routing verification | Wire UART → PLIC, fire THRE interrupt, verify PLIC pending bit |
| Device state reset between tests | Call `elaborate()` again from a fresh `World` |
| RTL co-simulation | Drive TlmBridge device; compare output with RTL simulator |
| SoC bring-up without CPU RTL | Drive bus infrastructure; CPU RTL not yet functional |

---

## 2. Position in the System Stack

helm-ng's execution environments form a spectrum from no-CPU to full-system:

```
World (no HelmEngine)  ←── World ─── no CPU, no ISA, no ArchState
                                       user drives MMIO directly
                                       virtual clock = user-controlled

ExecMode::Functional  ←── HelmEngine<Virtual> + ExecMode::Functional
                            correct instruction execution, no timing, no OS

ExecMode::Syscall  ←── HelmEngine<T> + LinuxSyscallHandler
                         userspace binaries without kernel boot

ExecMode::System  ←── HelmEngine<T> + full device tree + kernel
                        boots Linux, complete platform
```

`World (no HelmEngine)` is not a mode of `HelmEngine`. `World` is a separate struct in a separate crate (`helm-engine`) that replaces `HelmEngine` entirely. It links only the device, memory, event, and eventbus infrastructure — no CPU simulation code.

### Shared Infrastructure

`World` uses the same crates as a full simulation for everything below the CPU abstraction boundary:

| Crate | Used by World | Notes |
|---|---|---|
| `helm-core` | No | CPU-only: ArchState, ExecContext |
| `helm-arch` | No | CPU-only: ISA decode + execute |
| `helm-engine` | No | CPU-only: HelmEngine<T> |
| `helm-memory` | Yes | MemoryMap, FlatView, MmioHandler |
| `helm-devices` | Yes | Device trait, InterruptPin, InterruptSink, Bus |
| `helm-event` | Yes | EventQueue — device timer callbacks |
| `helm-devices/src/bus/event_bus` | Yes | HelmEventBus — observable events |
| `helm-stats` | Yes | PerfCounter — devices register counters normally |
| `helm-debug` | Partial | Checkpoint/restore: yes. GDB: no (no registers). TraceLogger: yes. |

A device that passes all World tests is promoted to a full system by changing one Python line:

```python
# World test
world = helm_ng.World()
uart = world.add_device(helm_ng.Uart16550(clock_hz=1_843_200), name="uart")

# Full system (identical device, different environment)
system = helm_ng.System(isa=helm_ng.Isa.RiscV, exec_mode=helm_ng.ExecMode.System)
uart = system.add_device(helm_ng.Uart16550(clock_hz=1_843_200), name="uart")
```

No changes to the `Uart16550` Rust implementation.

---

## 3. Design Principles

### Same Device Trait, Same Lifecycle

A device that works in `World` works identically in a full `HelmEngine<T>` simulation. The `Device` trait, `SimObject` lifecycle (`init → elaborate → startup`), `InterruptPin` model, and `EventQueue` usage are identical. `World` is not a simplified simulation — it is the same simulation infrastructure minus the CPU.

### No CPU Intrusion

`World` has zero imports from `helm-core`, `helm-arch`, or `helm-engine`. The Cargo dependency tree enforces this: `helm-engine` does not depend on these crates. Any code that would require CPU-level types is a design error in `World`.

### Deterministic by Default

The virtual clock advances only when `World::advance(cycles)` or `World::run_to_next_event()` is called explicitly. There are no threads, no wall-clock timers, no background callbacks. Two identical sequences of `add_device`, `map_device`, `elaborate`, and `mmio_write`/`advance` calls produce identical results, always.

### Zero Setup Cost for Tests

A `World` is constructed in microseconds. The `new()` constructor allocates nothing on the heap beyond the struct itself. `add_device` + `map_device` + `elaborate` for a single UART runs in well under 1 ms. This makes `World` the right primitive for a `cargo test` test helper that is called thousands of times in a fuzz corpus.

---

## 4. Crate Structure and Dependencies

```
crates/helm-engine/
├── Cargo.toml
│     [dependencies]
│       helm-devices  = { path = "../helm-devices" }
│       helm-memory   = { path = "../helm-memory" }
│       helm-event    = { path = "../helm-event" }
│       helm-devices/bus = { path = "../helm-devices/bus" }
│       helm-stats    = { path = "../helm-stats" }
└── src/
    ├── lib.rs              # pub use World, HelmObjectId, VirtualClock, EventHandle
    ├── world.rs            # World struct + all methods
    ├── clock.rs            # VirtualClock — monotonic tick counter
    ├── interrupt_sink.rs   # WorldInterruptSink — built-in IRQ observer
    └── bus_support.rs      # attach_to_bus(), get_bus() helpers
```

### Cargo.toml Constraint

`helm-engine` must never add `helm-core`, `helm-arch`, or `helm-engine` as dependencies, directly or transitively. This is enforced by CI with `cargo deny check bans`.

---

## 5. Component Model in World

The `SimObject` lifecycle applies in `World` with a reduced set of calls:

```
World::new()
  → allocates: MemoryMap, EventQueue, HelmEventBus, VirtualClock, WorldInterruptSink

World::add_device(name, Box<dyn Device>) -> HelmObjectId
  → stores device in objects map under generated id
  → calls device.init()   (init is called here, not in elaborate)

World::map_device(id, base)
  → inserts MemoryRegion::Mmio into MemoryMap at [base, base + device.region_size())
  → panics if device is already mapped or range overlaps

World::wire_interrupt(from_pin, to_sink)
  → connects InterruptPin → InterruptWire → InterruptSink
  → if to_sink is None, uses built-in WorldInterruptSink

World::elaborate()
  → calls device.elaborate() on all devices (in add_device order)
    — devices acquire their EventQueue and HelmEventBus references here
  → calls device.startup() on all devices
  → sets elaborated = true; panics if called again

World::advance(cycles)
  → drains EventQueue up to clock.current_tick + cycles
  → advances VirtualClock by cycles after draining

World::drop()
  → calls device.reset() on all devices (cleanup, not power-on reset)
```

`elaborate()` must be called exactly once before any `mmio_write`, `mmio_read`, `signal_raise`, or `advance`. The `elaborated` bool flag enforces this.

---

## 6. Time Model

`World` uses a `VirtualClock` — a monotonically increasing `u64` tick counter with no relationship to wall-clock time.

### Tick Definition

One tick = one clock cycle of the device's reference clock. There is no global frequency; each device interprets a tick count relative to its own clock parameter. A UART at 1.8432 MHz with a 9600 baud rate needs `1_843_200 / (16 * 9600)` = 12 ticks per baud clock period.

### Advance Semantics

`World::advance(cycles)`:

1. Sets `target_tick = clock.current_tick + cycles`.
2. Drains `EventQueue`: while the earliest event is at `tick ≤ target_tick`, dequeues and calls its callback.
3. Callbacks may schedule new events (re-entry into EventQueue is safe).
4. After the queue is drained up to `target_tick`, sets `clock.current_tick = target_tick`.

Events scheduled at exactly `target_tick` are processed in the same `advance` call. Events scheduled past `target_tick` are deferred to a future `advance`.

### run_to_next_event()

`World::run_to_next_event()` advances the clock to the next pending event and processes it. If the queue is empty, it returns immediately. Useful for event-driven test loops:

```rust
loop {
    world.run_to_next_event();
    if world.pending_interrupts().len() > 0 { break; }
    if world.current_tick() > 1_000_000 { panic!("timeout"); }
}
```

---

## 7. Interrupt Model

`World` uses the same `InterruptPin → InterruptWire → InterruptSink` model as a full system. Devices assert and deassert `InterruptPin` outputs; the wire propagates the signal to the connected `InterruptSink`.

### Built-in Interrupt Sink

`World` includes a `WorldInterruptSink` that records all `on_assert`/`on_deassert` calls into an internal map keyed by `(HelmObjectId, pin_name)`. `World::pending_interrupts()` queries this map and returns all currently-asserted `(id, pin_name)` pairs.

Tests use this to assert:

```rust
assert!(
    world.pending_interrupts().contains(&(uart_id, "irq_out".into())),
    "THRE interrupt not raised"
);
```

### Wiring a PLIC

For tests that need to validate interrupt routing through a PLIC model (not just raw assertion), a PLIC device can be added to the `World`:

```rust
let uart = world.add_device("uart", Box::new(Uart16550::new(1_843_200)));
let plic = world.add_device("plic", Box::new(Plic::new(32)));
world.map_device(uart, 0x10000000);
world.map_device(plic, 0x0c000000);
world.wire_interrupt(uart.irq_pin("irq_out"), plic.input_sink(10));
world.elaborate();
```

The PLIC's `InterruptSink` on input 10 sets the PLIC's internal pending bit when the UART asserts. The test then reads the PLIC's pending register via `mmio_read` to verify routing:

```rust
let pending = world.mmio_read(0x0c001000, 4);  // PLIC pending register
assert_ne!(pending & (1 << 10), 0, "UART IRQ not in PLIC pending");
```

---

## 8. Observability

`World` supports the same observability mechanisms as a full simulation:

### HelmEventBus

`World` owns an `Arc<HelmEventBus>`. Devices fire events the same way they do in a full simulation. Tests can subscribe:

```rust
let _handle = world.on_event(HelmEventKind::MemWrite, |event| {
    if let HelmEvent::MemWrite { addr, val, .. } = event {
        println!("MMIO write: {addr:#x} = {val:#x}");
    }
});
```

`on_event` returns an `EventHandle` (a drop guard). Dropping it unsubscribes.

### Stats

Devices register `PerfCounter` instances at elaborate time with `StatsRegistry`. `World` owns a `StatsRegistry`. Tests can read counters:

```rust
world.elaborate();
world.mmio_write(UART_BASE, 1, b'A' as u64);
let tx_count = world.stats().get("uart.tx_bytes").unwrap_or(0);
assert_eq!(tx_count, 1);
```

### Trace Logger

A `TraceLogger` can be attached to `World`'s `HelmEventBus` — the same way it attaches to a full simulation's bus. This produces a JSON Lines trace file of all device events for offline analysis.

---

## 9. Fuzzing Support

`World` is specifically designed to be a `cargo-fuzz` target primitive. Key properties:

- **Zero startup cost** — constructing a `World`, adding a device, and calling `elaborate()` takes microseconds. libFuzzer can create a fresh world for every fuzz iteration.
- **Reset via re-instantiation** — for fuzzing, the simplest reset strategy is to drop the old `World` and create a new one (Q105). Each fuzz iteration starts from a clean state.
- **No process overhead** — the fuzzer is in-process. No sockets, no subprocess launch, no IPC.
- **Rust memory safety** — Rust catches all buffer overflows, use-after-free, and integer overflows with ASAN/UBSan at zero extra code cost.

See [`TEST.md`](./TEST.md) for fuzz target structure.

---

## 10. Key Design Decisions

### Q101 — Shared Device Trait, Not CPU-Less Clone

`World` uses the same `Device` trait and `MemoryMap` as a full system. There is no "lite" Device trait or simplified memory system. This guarantees that a device tested in `World` is tested identically to how it will run in a full system — no integration gap.

### Q102 — User-Controlled Time

Simulated time advances only when the user calls `World::advance(cycles)`. No automatic advancement. This is essential for determinism in testing: a test advances time by exactly the number of cycles needed for an operation, asserts the result, and stops. There is no background thread that might advance time unexpectedly.

The only alternative would be wall-clock-driven advancement (advance by real elapsed time), which would make tests non-deterministic and non-reproducible. That is never acceptable for a test primitive.

### Q103 — Same Wire_interrupt Mechanism

`World::wire_interrupt(from_pin, to_sink)` uses the same `InterruptPin`/`InterruptWire`/`InterruptSink` types as `World::wire_interrupt()` in the full system. There is no World-specific interrupt API. This means an interrupt routing test in `World` validates the exact same wiring code that runs in production.

### Q104 — HelmEventBus for Observability

`World` owns an `Arc<HelmEventBus>` and uses it identically to the full system. Tests can subscribe to `MemWrite`, `DeviceSignal`, `Custom`, and other event kinds. This replaces the need for test-specific hooks or introspection APIs: anything observable via the event bus in production is observable in tests.

### Q105 — Reset via Re-Instantiation

For fuzzing, the reset strategy is to drop the `World` and create a new one (`elaborate()` again with the same config). For unit tests, `World` could also support a `reset()` method that calls `device.reset()` on all devices. Both strategies are valid; the LLD documents the re-instantiation path as primary because it is simpler, guaranteed to produce a clean state, and has negligible cost for small device worlds.

---

*For the complete `World` Rust API, see [`LLD-world.md`](./LLD-world.md). For the bus framework, see [`LLD-bus-framework.md`](./LLD-bus-framework.md). For tests, see [`TEST.md`](./TEST.md).*

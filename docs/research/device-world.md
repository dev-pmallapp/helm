# World — Headless Device and Bus Simulation

> Research and design document for `World (no HelmEngine)` / `World` in helm-ng.
> Cross-references: [`ARCHITECTURE.md`](../ARCHITECTURE.md) · [`object-model.md`](../object-model.md) · [`traits.md`](../traits.md)

---

## Table of Contents

1. [Motivation and Use Cases](#1-motivation-and-use-cases)
2. [World Design](#2-deviceworld-design)
3. [Python Config for World](#3-python-config-for-deviceworld)
4. [Bus Framework in World](#4-bus-framework-in-deviceworld)
5. [Testing Patterns in World](#5-testing-patterns-in-deviceworld)
6. [Fuzzing with World](#6-fuzzing-with-deviceworld)
7. [Co-simulation with RTL (SystemC TLM Bridge)](#7-co-simulation-with-rtl-systemc-tlm-bridge)
8. [World (no HelmEngine) in the Simulation Stack](#8-execmodedevice-in-the-simulation-stack)

---

## 1. Motivation and Use Cases

### Why Test Devices Without a CPU?

Every device model in helm-ng is ultimately a `Device` trait implementor — a Rust struct with `read()`, `write()`, `signal()`, and `InterruptPin` outputs. In the full `HelmEngine<T>` path, a simulated CPU drives MMIO accesses; the device model is exercised only as a side effect of running target code. This creates several problems:

**Test isolation is impossible.** A UART receive-FIFO overflow bug that manifests at Linux boot involves a complete software stack: bootloader, kernel, driver, interrupt handler, PIC routing, timer events. Isolating the hardware bug from the software bug requires days of single-stepping, not hours of device-level unit testing.

**Coverage is blind.** A CPU-driven simulation exercises only the paths that real firmware exercises. Error paths, protocol edge cases, and out-of-spec sequences are never hit. Fuzzing — the most effective way to find bugs in state machines — requires the ability to drive arbitrary sequences independent of software control.

**Turnaround time is too slow.** Booting Linux to exercise a VirtIO device takes minutes even in functional mode. A device unit test should take milliseconds. Fast iteration is the difference between a test suite that gets written and one that never does.

**Early bring-up before software is ready.** A new device model is often written before any firmware or driver exists for it. Protocol simulation, DMA address generation verification, and interrupt routing validation must be possible without a running OS.

### Use Cases by Category

**Unit testing — correctness at the register level.** Write to the TX holding register, advance a baud period, verify the TX shift register drains and the `THRE` interrupt fires. Test the RX FIFO overflow bit. Test baud rate divisor register effects on simulated clock cycles per bit. These are deterministic, repeatable, millisecond-scale tests that `cargo test` can run on every commit.

**Fuzzing — coverage-guided exploration of device state machines.** A device register file is a state machine. `libFuzzer` and `cargo-fuzz` can drive millions of randomized MMIO sequences per second against a `World` instance, finding panic paths, assertion failures, and undefined behavior that no human-authored test would reach.

**Protocol simulation without a CPU.** An I2C bus master doesn't need a CPU model to simulate a sensor read sequence. An SPI flash erase-program-verify cycle doesn't need a kernel scheduler. The bus transaction is the unit under test; the CPU would only add noise.

**Co-simulation with RTL.** An RTL implementation of a peripheral (Verilog/VHDL) can be driven by a `World` via a SystemC TLM bridge. The software model and the gate-level model are stimulated identically; divergences reveal RTL bugs before tape-out.

**SoC bring-up without a functional CPU.** Early in an SoC project, the CPU RTL may not be ready, or may not boot reliably. Platform bring-up — clocking up memory controllers, verifying PCIe enumeration, exercising the interrupt fabric — can proceed with `World` driving the bus directly.

### What Industry Does Today

**QEMU device unit tests.** QEMU has `tests/unit/` and `tests/qtest/` directories. `qtest` drives a QEMU instance externally via a socket protocol, sending MMIO reads/writes and reading the machine state back. It works but requires launching a full QEMU process, which still requires a machine model. Truly isolated device testing requires extracting the device struct and driving it directly — QEMU's object model makes this possible in principle but it is not an officially supported workflow.

**SIMICS standalone device bring-up.** SIMICS supports loading a single device model into a minimal `conf_object_t` universe without a full platform. Internal SIMICS projects use this for device validation. It is not documented in the public API, and the setup is manual. DML-generated devices require the SIMICS runtime to be present.

**SystemC TLM (Transaction-Level Modeling).** The SystemC TLM-2.0 standard defines a `tlm_initiator_socket` / `tlm_target_socket` pair with blocking and non-blocking transport interfaces. A TLM initiator (the testbench) can drive arbitrary transactions directly to a TLM target (the device model) without a processor model. This is the closest analog to `World` in industry practice. The drawback: SystemC/TLM is C++-only, verbose, and requires the SystemC kernel to be linked in.

**UVM (Universal Verification Methodology).** Hardware verification teams build UVM environments in SystemVerilog to test RTL blocks in isolation. UVM scoreboards, monitors, and sequence libraries provide the structure that `World` + `cargo test` provides on the software side.

### Why No Existing Simulator Makes This Easy

QEMU, gem5, and SIMICS all share the same architectural assumption: a device model exists to be driven by a CPU model. The component tree, memory map, interrupt routing, and simulation lifecycle are all designed around a central processor. Extracting a device and driving it standalone requires fighting the framework.

`World` inverts this. The CPU is the optional component; the device is the first-class citizen. The `World` struct provides the minimum substrate a device needs — a memory map for MMIO dispatch, an event queue for timer callbacks, an interrupt observation interface, and a virtual clock — without any CPU, MMU, register file, or ISA machinery.

---

## 2. World Design

### Design Principles

- **No CPU, no ISA, no ArchState.** `World` has zero dependencies on `helm-arch`, `helm-engine`, or `helm-core`. It links only `helm-devices`, `helm-memory`, `helm-event`, `helm-devices/src/bus/event_bus`, and the clock.
- **Same `Device` trait.** A device that works in `World` works identically in a full `HelmEngine<T>` simulation. Zero code changes when graduating from unit test to full system.
- **Same `SimObject` lifecycle.** `init → elaborate → startup → advance/run` applies. `World::elaborate()` drives the lifecycle, not a full `System`.
- **Deterministic by default.** The virtual clock advances only when `World::advance()` or `run_to_next_event()` is called. No threads, no wall-clock dependency, complete reproducibility.

### Crate Placement

`World` lives in a new crate: `crates/helm-engine/`. It is a standalone crate in the Cargo workspace, not part of `helm-engine`. The Python bindings in `helm-python` expose it as `helm_ng.World`.

### Complete Rust API

```rust
use helm_devices::{Device, InterruptPin, InterruptSink};
use helm_event::EventQueue;
use helm_devices::bus::event_bus::{HelmEventBus, HelmEvent, HelmEventKind};
use helm_memory::MemoryMap;
use std::collections::HashMap;
use std::sync::Arc;

/// A stable opaque identifier for an object registered in a World.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HelmObjectId(u64);

/// A registered simulation object — wraps the device and its metadata.
struct HelmObject {
    name: String,
    device: Box<dyn Device>,
    base_addr: Option<u64>,
}

/// A handle to a registered event callback. Drop to unsubscribe.
pub struct EventHandle(u64);

/// Headless device simulation environment — no CPU, no ISA, no ArchState.
///
/// Provides the minimum substrate required to exercise device models:
/// MMIO dispatch, interrupt observation, event scheduling, and a virtual clock.
pub struct World {
    objects:     HashMap<HelmObjectId, HelmObject>,
    memory:      MemoryMap,
    event_queue: EventQueue,
    event_bus:   Arc<HelmEventBus>,
    clock:       VirtualClock,
    next_id:     u64,
    elaborated:  bool,
}

impl World {
    /// Create an empty World. No devices, no mappings, clock at tick 0.
    pub fn new() -> Self;

    /// Register a device with the given name. Returns a stable HelmObjectId.
    ///
    /// The device is not mapped to any address yet; call `map_device()` to
    /// place it in the MMIO address space.
    pub fn add_device(&mut self, name: &str, device: Box<dyn Device>) -> HelmObjectId;

    /// Map a registered device into the MMIO address space at `base`.
    ///
    /// The device's `region_size()` determines the extent of the mapping.
    /// Panics if the device is already mapped, or if the range overlaps an
    /// existing mapping.
    pub fn map_device(&mut self, id: HelmObjectId, base: u64);

    /// Wire a device's interrupt output pin to an interrupt sink.
    ///
    /// In a full system this would be a PLIC or GIC; in a World it is
    /// typically `world.interrupt_sink()` — the built-in observer that records
    /// all asserted interrupts for inspection by tests.
    pub fn wire_interrupt(&mut self, from: &InterruptPin, to: &dyn InterruptSink);

    /// Finalize the component graph. Must be called before any simulation.
    ///
    /// Drives `init() → elaborate() → startup()` on all registered devices.
    /// Panics if called more than once.
    pub fn elaborate(&mut self);

    /// Perform a MMIO write of `size` bytes at `addr`.
    ///
    /// Dispatches to the device mapped at that address. Panics if no device
    /// covers `addr` or if `size` is not 1, 2, 4, or 8.
    pub fn mmio_write(&mut self, addr: u64, size: usize, val: u64);

    /// Perform a MMIO read of `size` bytes at `addr`.
    ///
    /// Returns the device's response. Panics if no device covers `addr`.
    pub fn mmio_read(&self, addr: u64, size: usize) -> u64;

    /// Assert a named signal on a device (e.g. "reset", "clock_enable").
    ///
    /// Calls `device.signal(port, 1)`. Use for active-high signal lines.
    pub fn signal_raise(&mut self, device: HelmObjectId, port: &str);

    /// Deassert a named signal on a device.
    ///
    /// Calls `device.signal(port, 0)`.
    pub fn signal_lower(&mut self, device: HelmObjectId, port: &str);

    /// Advance the virtual clock by `cycles` ticks.
    ///
    /// Drains all events scheduled at or before `clock.current_tick() + cycles`,
    /// calling their callbacks in tick order. Devices may schedule new events
    /// inside callbacks; those are drained too if they fall within the window.
    pub fn advance(&mut self, cycles: u64);

    /// Advance the clock to the next scheduled event and process it.
    ///
    /// If the event queue is empty, returns immediately without advancing.
    pub fn run_to_next_event(&mut self);

    /// Subscribe to a HelmEvent kind. Returns a handle; drop to unsubscribe.
    ///
    /// The callback `f` is called synchronously when the event fires.
    pub fn on_event<F: Fn(&HelmEvent) + Send + 'static>(
        &self,
        kind: HelmEventKind,
        f: F,
    ) -> EventHandle;

    /// Return all interrupt pins that are currently asserted.
    ///
    /// Each entry is `(device_id, pin_name)`. The built-in interrupt sink
    /// records all assert/deassert calls and this method queries its state.
    pub fn pending_interrupts(&self) -> Vec<(HelmObjectId, String)>;

    /// Return the current virtual clock tick.
    pub fn current_tick(&self) -> u64;

    /// Return a reference to the internal HelmEventBus for direct subscription.
    pub fn event_bus(&self) -> &Arc<HelmEventBus>;

    /// Return the name of a registered device by id.
    pub fn device_name(&self, id: HelmObjectId) -> Option<&str>;
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}
```

### VirtualClock

`World` owns a `VirtualClock` — a monotonically increasing `u64` tick counter. Devices that need timer callbacks use the `EventQueue` injected at `elaborate()` time (the same `helm-event` crate used by the full simulator). The clock has no relationship to wall-clock time; it advances strictly through explicit `advance()` calls.

```rust
/// Monotonic virtual clock — tick-accurate, wall-clock-independent.
pub struct VirtualClock {
    tick: u64,
}

impl VirtualClock {
    pub fn current_tick(&self) -> u64 { self.tick }
    pub fn advance(&mut self, delta: u64) { self.tick += delta; }
}
```

### Built-in Interrupt Sink

`World` includes a built-in `InterruptSink` implementation that records all asserted/deasserted signals into an internal map. `pending_interrupts()` queries this map. Tests that need to observe interrupt behavior do not need to write their own sink.

```rust
struct WorldInterruptSink {
    /// Maps (device_id, pin_name) → asserted?
    state: Arc<Mutex<HashMap<(HelmObjectId, String), bool>>>,
}

impl InterruptSink for WorldInterruptSink {
    fn on_assert(&self, wire_id: WireId) {
        let mut state = self.state.lock().unwrap();
        state.insert(wire_id.into_key(), true);
    }

    fn on_deassert(&self, wire_id: WireId) {
        let mut state = self.state.lock().unwrap();
        state.insert(wire_id.into_key(), false);
    }
}
```

---

## 3. Python Config for World

The `helm_ng.World` Python class mirrors the Rust API via PyO3 bindings in `helm-python`. The same device classes used in full-system config (`Uart16550`, `Plic`, `I2cBus`) work without modification.

### Basic Usage

```python
import helm_ng
from helm_ng import World, Uart16550, Plic, I2cBus

# Create a headless device world — no CPU, no ISA
world = World()

# Add and map a UART
uart = world.add_device(Uart16550(clock_hz=1_843_200), name="uart")
world.map_device(uart, base=0x10000000)

# Finalize the component graph
world.elaborate()

# Drive MMIO directly — write 'a' (0x61) to the UART TX holding register
world.mmio_write(0x10000000, 1, 0x61)

# Advance 10,000 clock ticks (covers several baud periods at 1.8432 MHz)
world.advance(cycles=10_000)

# Inspect interrupt state
irqs = world.pending_interrupts()
print(f"Pending IRQs: {irqs}")
```

### Multi-Device Platform Testbench

```python
import helm_ng
from helm_ng import World, Uart16550, Plic, I2cBus, Tmp102

world = World()

# Add all devices
uart  = world.add_device(Uart16550(clock_hz=1_843_200), name="uart")
plic  = world.add_device(Plic(num_sources=32), name="plic")
i2c   = world.add_device(I2cBus(), name="i2c0")
temp  = world.add_device(Tmp102(i2c_addr=0x48), name="temp_sensor")

# Map MMIO
world.map_device(uart,  base=0x10000000)
world.map_device(plic,  base=0x0c000000)
world.map_device(i2c,   base=0x10001000)

# Wire interrupts: uart.irq_out → plic input 10
world.wire_interrupt(uart.irq_out,  plic.input(10))
world.wire_interrupt(i2c.irq_out,   plic.input(11))

# Attach sensor to I2C bus (no MMIO mapping — it's on the bus)
i2c.attach(temp, address=0x48)

world.elaborate()

# Subscribe to events from Python
@world.on_event("MemWrite")
def on_mmio_write(event):
    print(f"MMIO write: addr={event.addr:#x} val={event.val:#x}")

# Exercise: trigger an I2C temperature read
world.mmio_write(0x10001000, 1, 0x48)  # set target address
world.mmio_write(0x10001001, 1, 0x01)  # start read
world.advance(cycles=100_000)          # allow transaction to complete

print(f"Tick: {world.current_tick()}")
```

### Parameterized Test Helper

```python
def make_uart_world(clock_hz: int = 1_843_200, base: int = 0x10000000) -> tuple:
    """Create a minimal world with one UART. Returns (world, uart_id)."""
    world = helm_ng.World()
    uart = world.add_device(helm_ng.Uart16550(clock_hz=clock_hz), name="uart")
    world.map_device(uart, base=base)
    world.elaborate()
    return world, uart

# Reuse in tests
world, uart = make_uart_world()
world.mmio_write(0x10000000, 1, ord('Z'))
world.advance(cycles=50_000)
assert (uart, "irq_out") in world.pending_interrupts()
```

---

## 4. Bus Framework in World

### The Bus Trait

Buses in helm-ng are `Device` subtypes — they sit in the MMIO address space and expose a multiplexed address space to the devices attached to them. The `Bus` trait extends `SimObject` with enumeration and device-addressed transaction methods.

```rust
/// A bus is a Device that multiplexes a set of attached child devices.
///
/// The bus sits in the MMIO address space of the parent world. Transactions
/// to the bus's mapped region are decoded by the bus and forwarded to the
/// appropriate child device.
pub trait Bus: SimObject {
    /// Return the HelmObjectIds of all devices attached to this bus.
    fn devices(&self) -> Vec<HelmObjectId>;

    /// Read `size` bytes from device `device_id` at bus-local `offset`.
    fn bus_read(&self, device_id: u8, offset: u16, size: usize) -> u64;

    /// Write `size` bytes to device `device_id` at bus-local `offset`.
    fn bus_write(&mut self, device_id: u8, offset: u16, size: usize, val: u64);

    /// Return a descriptor for every device visible on this bus.
    fn enumerate(&self) -> Vec<BusDevice>;
}

/// Descriptor returned by Bus::enumerate().
pub struct BusDevice {
    pub device_id: u8,
    pub vendor_id: u16,
    pub device_class: u16,
    pub name: &'static str,
}
```

### PCI Enumeration Without a CPU

In a full system, a CPU executes BIOS/firmware code that reads PCI config space (bus 0, device 0, function 0, offset 0) to discover attached devices. In `World`, the testbench calls `Bus::enumerate()` directly — the enumeration logic that the firmware would have executed is now a Rust method call.

```rust
#[test]
fn test_pci_bus_enumerate() {
    let mut world = World::new();

    // Add a minimal PCI bus with two devices attached
    let mut pci_bus = PciBus::new();
    pci_bus.attach(PciDevice::new(VENDOR_QEMU, DEVICE_VIRTIO_DISK));
    pci_bus.attach(PciDevice::new(VENDOR_QEMU, DEVICE_VIRTIO_NET));

    let bus_id = world.add_device("pci0", Box::new(pci_bus));
    world.map_device(bus_id, 0x30000000);
    world.elaborate();

    // Enumerate without a CPU — call the Bus trait method directly
    let bus = world.get_bus(bus_id).expect("bus registered");
    let devices = bus.enumerate();

    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].vendor_id, VENDOR_QEMU);
    assert_eq!(devices[1].device_class, CLASS_NETWORK);
}
```

For MMIO-driven config space reads (testing the firmware path), `World::mmio_read()` exercises the same dispatch path that a CPU would use, with no CPU present:

```rust
// PCI config space: bus 0, device 1, function 0, offset 0 → vendor/device id
let vendor_device = world.mmio_read(pci_cfg_addr(0, 1, 0, 0), 4);
assert_eq!(vendor_device & 0xFFFF, VENDOR_QEMU as u64);
```

### I2C Transactions Without a CPU

An I2C bus master (a software-model I2C controller) generates START, address, data, STOP sequences. In a full system, firmware writes to the I2C controller's MMIO registers to drive these sequences. In `World`, the testbench writes those same registers, exercising the controller's state machine directly:

```rust
#[test]
fn test_i2c_sensor_read() {
    let mut world = World::new();

    let i2c = world.add_device("i2c0", Box::new(I2cController::new()));
    world.map_device(i2c, 0x10001000);

    // Attach a temperature sensor to the bus (no MMIO — bus-connected)
    let sensor = world.add_device("tmp102", Box::new(Tmp102::new(0x48)));
    world.attach_to_bus(i2c, sensor);

    world.elaborate();

    // Drive the I2C controller the same way firmware would:
    // Write target address + READ bit to control register
    world.mmio_write(0x10001000, 1, (0x48 << 1) | 1);
    // Issue START
    world.mmio_write(0x10001001, 1, 0x01);
    // Advance enough cycles for START + address + ACK + 2 data bytes
    world.advance(100_000);
    // Read the received data register
    let temp_raw = world.mmio_read(0x10001002, 2);
    // TMP102 power-on default is 25°C = 0x0C80 >> 4
    assert_eq!(temp_raw >> 4, 0x0C8);
}
```

### SPI Without a CPU

SPI transactions follow the same pattern. The SPI controller sits in MMIO space; the SPI flash is bus-attached:

```rust
#[test]
fn test_spi_flash_read_id() {
    let mut world = World::new();

    let spi = world.add_device("spi0", Box::new(SpiController::new()));
    world.map_device(spi, 0x10002000);

    let flash = world.add_device("flash", Box::new(SpiNorFlash::new(8 * 1024 * 1024)));
    world.attach_to_bus(spi, flash);

    world.elaborate();

    // Assert CS, send JEDEC READ ID command (0x9F)
    world.mmio_write(0x10002004, 1, 0x00); // assert CS (active low)
    world.mmio_write(0x10002000, 1, 0x9F); // TX byte
    world.advance(1_000);
    let id_byte0 = world.mmio_read(0x10002001, 1); // RX byte
    world.mmio_write(0x10002004, 1, 0x01); // deassert CS

    assert_eq!(id_byte0, 0xEF); // Winbond manufacturer ID
}
```

---

## 5. Testing Patterns in World

### Pattern 1: Single-Register Correctness

The simplest test: write to one register, advance minimal time, check the effect.

```rust
#[cfg(test)]
mod uart_tests {
    use helm_world::World;
    use helm_devices::uart::Uart16550;

    const UART_BASE: u64 = 0x10000000;

    fn uart_world() -> (World, helm_world::HelmObjectId) {
        let mut world = World::new();
        let uart = world.add_device("uart", Box::new(Uart16550::new(1_843_200)));
        world.map_device(uart, UART_BASE);
        world.elaborate();
        (world, uart)
    }

    #[test]
    fn test_uart_tx() {
        let (mut world, uart) = uart_world();

        // Write 'A' to the TX Holding Register (offset 0, DLAB=0)
        world.mmio_write(UART_BASE, 1, b'A' as u64);

        // Advance one baud period: at 1.8432 MHz, 9600 baud → ~192 cycles/bit
        // 10 bits (start + 8 data + stop) = ~1920 cycles
        world.advance(2_000);

        // TX Holding Register Empty interrupt should be asserted
        assert!(
            world.pending_interrupts().contains(&(uart, "irq_out".into())),
            "THRE interrupt not raised after TX"
        );
    }

    #[test]
    fn test_uart_rx_loopback() {
        let (mut world, _uart) = uart_world();

        // Enable loopback mode: MCR bit 4
        world.mmio_write(UART_BASE + 4, 1, 0x10);

        // Write to TX — in loopback, this feeds directly to RX FIFO
        world.mmio_write(UART_BASE, 1, b'Z' as u64);
        world.advance(2_000);

        // LSR bit 0 (Data Ready) should be set
        let lsr = world.mmio_read(UART_BASE + 5, 1);
        assert_eq!(lsr & 0x01, 0x01, "DR bit not set after loopback");

        // Read the received byte
        let rx = world.mmio_read(UART_BASE, 1);
        assert_eq!(rx, b'Z' as u64, "Loopback byte mismatch");
    }

    #[test]
    fn test_uart_fifo_overflow() {
        let (mut world, _uart) = uart_world();

        // Enable FIFO (FCR bit 0), 16-byte depth
        world.mmio_write(UART_BASE + 2, 1, 0x01);

        // Fill the RX FIFO beyond capacity (17 writes in loopback)
        world.mmio_write(UART_BASE + 4, 1, 0x10); // loopback
        for byte in 0..17u64 {
            world.mmio_write(UART_BASE, 1, byte);
            world.advance(2_000);
        }

        // LSR bit 1 (Overrun Error) must be set
        let lsr = world.mmio_read(UART_BASE + 5, 1);
        assert_eq!(lsr & 0x02, 0x02, "OE bit not set on FIFO overflow");
    }

    #[test]
    fn test_uart_divisor_latch() {
        let (mut world, _uart) = uart_world();

        // Set DLAB=1 (LCR bit 7) to access divisor registers
        world.mmio_write(UART_BASE + 3, 1, 0x80);

        // Write divisor for 115200 baud: 1.8432 MHz / (16 * 115200) = 1
        world.mmio_write(UART_BASE,     1, 0x01); // DLL
        world.mmio_write(UART_BASE + 1, 1, 0x00); // DLM

        // Clear DLAB
        world.mmio_write(UART_BASE + 3, 1, 0x03); // 8N1, DLAB=0

        // Verify divisor readback
        world.mmio_write(UART_BASE + 3, 1, 0x80); // DLAB=1
        let dll = world.mmio_read(UART_BASE, 1);
        assert_eq!(dll, 0x01, "DLL readback mismatch");
    }
}
```

### Pattern 2: Event Observation

Subscribe to `HelmEventBus` events to verify device-internal behavior without polling registers.

```rust
#[test]
fn test_uart_tx_event_observed() {
    use std::sync::{Arc, Mutex};
    use helm_devices::bus::event_bus::HelmEventKind;

    let (mut world, _uart) = uart_world();

    // Collect all MemWrite events fired during the test
    let log: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let log_clone = Arc::clone(&log);

    let _handle = world.on_event(HelmEventKind::MemWrite, move |event| {
        if let helm_devices::bus::event_bus::HelmEvent::MemWrite { addr, val, .. } = event {
            log_clone.lock().unwrap().push(*val);
        }
    });

    world.mmio_write(UART_BASE, 1, 0xAB);
    world.advance(1_000);

    let writes = log.lock().unwrap();
    assert!(writes.contains(&0xAB), "TX write not observed on event bus");
}
```

### Pattern 3: Interrupt Routing Verification

Test that interrupt routing is wired correctly — the interrupt from the device reaches the controller.

```rust
#[test]
fn test_uart_irq_routes_to_plic() {
    let mut world = World::new();

    let uart = world.add_device("uart", Box::new(Uart16550::new(1_843_200)));
    let plic = world.add_device("plic", Box::new(Plic::new(32)));

    world.map_device(uart, 0x10000000);
    world.map_device(plic, 0x0c000000);

    // Wire uart.irq_out → plic input 10
    // In the Rust API, get_irq_pin is provided by the device
    // (In practice, devices expose named pins via a method)
    world.wire_interrupt(uart.irq_pin("irq_out"), plic.input_sink(10));

    world.elaborate();

    // Enable UART TX interrupt in IER (bit 1)
    world.mmio_write(0x10000001, 1, 0x02);

    // Write to TX register — THRE interrupt fires after drain
    world.mmio_write(0x10000000, 1, b'X' as u64);
    world.advance(2_000);

    // PLIC pending register (offset 0x1000, source 10): bit 10 should be set
    let pending = world.mmio_read(0x0c001000, 4);
    assert_ne!(pending & (1 << 10), 0, "UART IRQ not reflected in PLIC pending");
}
```

### Pattern 4: Checkpoint and Restore

Verify that a device's state can be saved and restored correctly.

```rust
#[test]
fn test_uart_checkpoint_restore() {
    let (mut world, uart) = uart_world();

    // Put the UART in a known state: 115200 baud, 8N1
    world.mmio_write(UART_BASE + 3, 1, 0x80); // DLAB=1
    world.mmio_write(UART_BASE,     1, 0x01); // DLL=1
    world.mmio_write(UART_BASE + 3, 1, 0x03); // 8N1, DLAB=0

    // Save checkpoint
    let checkpoint = world.checkpoint_save(uart);

    // Corrupt the state
    world.mmio_write(UART_BASE + 3, 1, 0x80);
    world.mmio_write(UART_BASE, 1, 0xFF);
    world.mmio_write(UART_BASE + 3, 1, 0x00);

    // Restore
    world.checkpoint_restore(uart, &checkpoint);

    // Verify restored state
    world.mmio_write(UART_BASE + 3, 1, 0x80);
    let dll = world.mmio_read(UART_BASE, 1);
    assert_eq!(dll, 0x01, "DLL not restored from checkpoint");
}
```

---

## 6. Fuzzing with World

### Why World is Ideal for Fuzzing

A device register file is a finite state machine driven by MMIO sequences. `World` makes the FSM directly accessible from `cargo-fuzz` / `libFuzzer` without any process startup overhead, without a CPU model, and with full Rust memory safety catching all out-of-bounds access, integer overflow, and use-after-free at test time.

The ergonomics are the same as any `cargo-fuzz` target: create a `fuzz_target!` closure, build a fresh `World` for every fuzzer iteration, and drive MMIO sequences from the fuzzer's byte stream. The fuzzer finds panic paths, assertion failures, and state corruption that human-authored tests would never reach.

### Fuzz Target Structure

```rust
// fuzz/fuzz_targets/uart_mmio.rs
#![no_main]

use libfuzzer_sys::fuzz_target;
use helm_world::World;
use helm_devices::uart::Uart16550;

fuzz_target!(|data: &[u8]| {
    let mut world = World::new();
    let device = world.add_device("uart", Box::new(Uart16550::new(1_843_200)));
    world.map_device(device, 0x1000);
    world.elaborate();

    // Drive arbitrary MMIO sequences.
    // Packet format: [offset: u16 LE][size_tag: u8][value: u32 LE]
    // Total: 7 bytes per packet.
    for chunk in data.chunks(7) {
        if chunk.len() < 7 { break; }

        let offset = u16::from_le_bytes(chunk[0..2].try_into().unwrap()) as u64;
        // Constrain offset to device region (UART has 8 registers)
        let offset = offset % 8;
        // size_tag 0→1, 1→2, 2→4, 3→1 (cycle through valid sizes)
        let size = match chunk[2] % 4 { 0 => 1, 1 => 2, 2 => 4, _ => 1 };
        let val = u32::from_le_bytes(chunk[3..7].try_into().unwrap()) as u64;

        // Mix reads and writes based on the MSB of size_tag
        if chunk[2] & 0x80 == 0 {
            world.mmio_write(0x1000 + offset, size, val);
        } else {
            let _ = world.mmio_read(0x1000 + offset, size);
        }

        // Advance a small number of ticks to let timer callbacks fire
        world.advance(10);
    }
    // Must not panic. Sanitizers catch UB, leaks, and overflows.
});
```

### Running the Fuzzer

```bash
# Initialize fuzz targets (once)
cargo fuzz init

# Run the UART fuzzer with AddressSanitizer
cargo fuzz run uart_mmio -- -max_len=1024 -timeout=10

# Reproduce a specific crash
cargo fuzz run uart_mmio fuzz/artifacts/uart_mmio/crash-<hash>

# Generate a corpus from known good sequences
cargo fuzz run uart_mmio -- -seed_corpus=fuzz/corpus/uart_mmio/
```

### Multi-Device Fuzz Target

For bus interaction bugs, fuzz two devices simultaneously:

```rust
fuzz_target!(|data: &[u8]| {
    let mut world = World::new();
    let uart  = world.add_device("uart",  Box::new(Uart16550::new(1_843_200)));
    let timer = world.add_device("timer", Box::new(RvClint::new()));

    world.map_device(uart,  0x10000000);
    world.map_device(timer, 0x02000000);
    world.elaborate();

    // Interleaved: each 8-byte chunk targets either uart or timer
    for chunk in data.chunks(8) {
        if chunk.len() < 8 { break; }
        let target_base = if chunk[0] & 1 == 0 { 0x10000000 } else { 0x02000000 };
        let offset = (chunk[1] as u64) % 16;
        let size   = match chunk[2] % 3 { 0 => 1, 1 => 2, _ => 4 };
        let val    = u32::from_le_bytes(chunk[4..8].try_into().unwrap()) as u64;

        world.mmio_write(target_base + offset, size, val);
        world.advance(5);
    }
});
```

### Sanitizer Recommendations

| Sanitizer | What It Finds | Enable With |
|-----------|--------------|-------------|
| AddressSanitizer (ASan) | Buffer overflow, use-after-free in `unsafe` blocks | `RUSTFLAGS="-Z sanitizer=address"` |
| UndefinedBehaviorSanitizer (UBSan) | Integer overflow, out-of-bounds shift | `RUSTFLAGS="-Z sanitizer=undefined"` |
| MemorySanitizer (MSan) | Uninitialized reads | `RUSTFLAGS="-Z sanitizer=memory"` |
| ThreadSanitizer (TSan) | Data races if `World` is used from threads | `RUSTFLAGS="-Z sanitizer=thread"` |

`cargo-fuzz` enables ASan by default. For the `World` fuzzer, UBSan is particularly valuable for catching arithmetic errors in baud-rate divisor math, FIFO pointer wraparound, and DMA address alignment.

---

## 7. Co-simulation with RTL (SystemC TLM Bridge)

### Motivation

A software device model is a behavioral abstraction. An RTL implementation (Verilog/VHDL) compiled to a gate-level simulation is the ground truth. Comparing them under identical stimulus finds model bugs before tape-out and RTL bugs before silicon. `World` provides the software side of this comparison; a TLM bridge connects it to the RTL side.

### Architecture

```
┌─────────────────────────────────────────┐
│            helm-ng World           │
│                                         │
│  mmio_write(addr, size, val)            │
│         │                               │
│         ▼                               │
│  TlmBridge (implements Device trait)    │
│         │                               │
└─────────│───────────────────────────────┘
          │ TLM-2.0 blocking_transport()
          │ (socket pair over shared memory
          │  or Unix socket)
          ▼
┌─────────────────────────────────────────┐
│     RTL Simulator (Verilator / VCS)     │
│     wrapped as SystemC TLM target       │
│                                         │
│  sc_in<sc_logic> addr[15:0]            │
│  sc_in<sc_logic> data[31:0]            │
│  sc_in<sc_logic> we                    │
│  sc_out<sc_logic> irq                  │
└─────────────────────────────────────────┘
```

### TlmBridge Device

The bridge is a `Device` implementation that translates `mmio_write`/`mmio_read` calls into TLM-2.0 generic payload objects and sends them across a socket to a SystemC kernel.

```rust
/// A Device that forwards all MMIO accesses to an RTL simulator via TLM-2.0.
///
/// The RTL simulator must expose a SystemC TLM-2.0 target socket bound to
/// a known Unix domain socket path. The bridge connects at elaborate() time.
pub struct TlmBridge {
    socket_path: PathBuf,
    conn: Option<UnixStream>,
    region_size: u64,
}

impl Device for TlmBridge {
    fn read(&self, offset: u64, size: usize) -> u64 {
        // Serialize a TLM read command, send to RTL sim, await response
        self.send_tlm_cmd(TlmCmd::Read { offset, size })
            .expect("TLM read failed")
    }

    fn write(&mut self, offset: u64, size: usize, val: u64) {
        // Serialize a TLM write command, send to RTL sim
        // RTL sim advances its clock, applies the write, returns ack
        self.send_tlm_cmd(TlmCmd::Write { offset, size, val })
            .expect("TLM write failed");
    }

    fn region_size(&self) -> u64 { self.region_size }

    fn signal(&mut self, name: &str, val: u64) {
        // Drive top-level RTL signals (reset, clock_enable, etc.)
        self.send_tlm_cmd(TlmCmd::Signal { name: name.to_string(), val })
            .expect("TLM signal failed");
    }
}

impl SimObject for TlmBridge {
    fn elaborate(&mut self, _system: &mut System) {
        self.conn = Some(
            UnixStream::connect(&self.socket_path)
                .expect("TLM bridge: RTL simulator not listening"),
        );
    }
}
```

### Scoreboard Pattern

With the bridge in place, the same stimulus drives both the software model and the RTL:

```rust
#[test]
fn test_uart_sw_rtl_correlation() {
    // Software model world
    let mut sw_world = World::new();
    let sw_uart = sw_world.add_device("uart_sw", Box::new(Uart16550::new(1_843_200)));
    sw_world.map_device(sw_uart, 0x10000000);
    sw_world.elaborate();

    // RTL model world (TlmBridge forwards to Verilator-compiled UART RTL)
    let mut rtl_world = World::new();
    let rtl_uart = rtl_world.add_device(
        "uart_rtl",
        Box::new(TlmBridge::new("/tmp/helm_uart_rtl.sock", 8)),
    );
    rtl_world.map_device(rtl_uart, 0x10000000);
    rtl_world.elaborate();

    // Apply identical stimulus to both
    let test_sequence: &[(u64, usize, u64)] = &[
        (0x10000003, 1, 0x03), // LCR: 8N1
        (0x10000001, 1, 0x02), // IER: enable THRE interrupt
        (0x10000000, 1, 0x41), // THR: transmit 'A'
    ];

    for &(addr, size, val) in test_sequence {
        sw_world.mmio_write(addr, size, val);
        rtl_world.mmio_write(addr, size, val);
    }

    sw_world.advance(5_000);
    rtl_world.advance(5_000);

    // Compare observable state: LSR register
    let sw_lsr  = sw_world.mmio_read(0x10000005, 1);
    let rtl_lsr = rtl_world.mmio_read(0x10000005, 1);

    assert_eq!(sw_lsr, rtl_lsr,
        "LSR divergence: sw={sw_lsr:#04x} rtl={rtl_lsr:#04x}");

    // Compare interrupt state
    let sw_irq  = sw_world.pending_interrupts().contains(&(sw_uart,  "irq_out".into()));
    let rtl_irq = rtl_world.pending_interrupts().contains(&(rtl_uart, "irq_out".into()));
    assert_eq!(sw_irq, rtl_irq, "IRQ state divergence");
}
```

### Clock Synchronization

RTL simulators advance in picosecond resolution. `World` advances in cycles. The `TlmBridge` maps cycles to picoseconds using a configurable ratio (e.g., at 50 MHz, 1 cycle = 20 ns = 20,000 ps). The RTL simulator's SystemC kernel is told to advance by the equivalent picosecond count on each `World::advance()` call. This ensures time-locked co-simulation without free-running clocks on either side.

---

## 8. World (no HelmEngine) in the Simulation Stack

### Position in the Stack

helm-ng's execution modes form a spectrum from no-CPU to full-system:

```
World (no HelmEngine)      ← World — no CPU, no ISA, no ArchState
ExecMode::Functional  ← HelmEngine<Virtual>, no timing, correct instruction execution
ExecMode::Syscall     ← HelmEngine<T> + LinuxSyscallHandler, no kernel boot
ExecMode::System      ← HelmEngine<T> + full platform, boots a real kernel
```

`World (no HelmEngine)` is not a mode of `HelmEngine`; it is the absence of `HelmEngine`. `World` replaces the engine entirely.

### Shared Infrastructure

Even without a CPU, `World` uses the same infrastructure crates as a full simulation:

| Crate | Used by World? | Notes |
|-------|---------------------|-------|
| `helm-core` | No | ArchState, ExecContext — CPU-only |
| `helm-arch` | No | ISA decode + execute — CPU-only |
| `helm-engine` | No | HelmEngine<T> — CPU-only |
| `helm-memory` | Yes | MemoryMap, MemoryRegion, FlatView, MmioHandler |
| `helm-devices` | Yes | Device trait, InterruptPin, InterruptSink, Bus trait |
| `helm-event` | Yes | EventQueue — devices schedule timer callbacks |
| `helm-devices/src/bus/event_bus` | Yes | HelmEventBus — observable events, same as full system |
| `helm-stats` | Yes | PerfCounter — devices register stats normally |
| `helm-debug` | Partial | Checkpoint/restore: yes. GDB stub: no (no registers). TraceLogger: yes. |

### Component Lifecycle

The `SimObject` lifecycle applies identically in `World`:

```
World::new()       → allocate world substrate (MemoryMap, EventQueue, HelmEventBus, VirtualClock)
World::add_device() → register device, assign HelmObjectId, call device.init()
World::map_device() → insert MMIO region into MemoryMap
World::elaborate()  → call device.elaborate() + device.startup() for all devices
                           (same order as System::elaborate() in a full simulation)
World::advance()    → drain EventQueue, call device callbacks, advance VirtualClock
World::drop()       → call device.deinit() for all devices (cleanup)
```

A device that passes all its `World` tests is promoted to a full-system test with a single Python config change:

```python
# World test (headless)
world = helm_ng.World()
uart = world.add_device(helm_ng.Uart16550(clock_hz=1_843_200), name="uart")
world.map_device(uart, base=0x10000000)
world.elaborate()

# Full system (identical device, different world)
system = helm_ng.System(isa=helm_ng.Isa.RiscV, exec_mode=helm_ng.ExecMode.System)
uart = system.add_device(helm_ng.Uart16550(clock_hz=1_843_200), name="uart")
system.map_device(uart, base=0x10000000)
system.wire_interrupt(uart.irq_out, system.plic.input(10))
system.elaborate()
system.run(n_instructions=1_000_000_000)
```

The `Uart16550` struct is identical in both cases. No code changes. The device is world-agnostic.

### When to Use World

| Situation | Use |
|-----------|-----|
| Writing a new device model | `World` — fast iteration, `cargo test`, no boot overhead |
| Unit testing a device register protocol | `World` — millisecond-scale tests |
| Fuzzing a device state machine | `World` + `cargo-fuzz` |
| Testing interrupt routing | `World` with PLIC + device wired together |
| Correlating with RTL | `World` + `TlmBridge` |
| Testing bus enumeration (PCI, I2C, SPI) | `World` with bus model, no CPU |
| SoC bring-up without working CPU | `World` driving all buses manually |
| Regression testing device behavior | `World` — deterministic, reproducible, CI-safe |
| Booting Linux | `ExecMode::System` with `HelmEngine<T>` — World not applicable |
| ISA validation | `ExecMode::Functional` — World not applicable |

### EventQueue and HelmEventBus in World

Both event systems operate identically to a full simulation:

**EventQueue** (`helm-event`): Devices schedule timer callbacks the same way — `event_queue.schedule(tick + delay, callback)`. `World::advance(cycles)` drains the queue. A UART transmit shift register that takes 10 bits × 192 ticks = 1920 ticks to drain schedules its THRE interrupt via the EventQueue, exactly as it would in a full system.

**HelmEventBus** (`helm-devices/src/bus/event_bus`): All device-fired events (`MemWrite`, `Custom`, etc.) are delivered to subscribers. A `TraceLogger` can subscribe to record all MMIO accesses during a test. A Python callback can subscribe to observe events in an interactive testbench session.

Neither system requires a CPU. They are CPU-independent infrastructure that `World` owns directly, the same way `System` owns them in a full simulation.

---

*This document describes the design of `World` / `World (no HelmEngine)` for helm-ng. It should be read alongside [`ARCHITECTURE.md`](../ARCHITECTURE.md) (full system context) and [`traits.md`](../traits.md) (Device, SimObject, InterruptSink trait contracts).*

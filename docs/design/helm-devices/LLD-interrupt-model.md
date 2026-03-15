# helm-devices — LLD: Interrupt Model

> Low-level design for `InterruptPin`, `InterruptWire`, `InterruptSink`, `WireId`, and the platform wiring protocol.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-device-trait.md`](./LLD-device-trait.md) · [`ARCHITECTURE.md`](../../ARCHITECTURE.md)

---

## Table of Contents

1. [Design Principles](#1-design-principles)
2. [Type Definitions](#2-type-definitions)
3. [InterruptPin](#3-interruptpin)
4. [InterruptWire](#4-interruptwire)
5. [InterruptSink Trait](#5-interruptsink-trait)
6. [WireId](#6-wireid)
7. [Platform Wiring Protocol](#7-platform-wiring-protocol)
8. [Interrupt Controller Implementation Pattern (PLIC)](#8-interrupt-controller-implementation-pattern-plic)
9. [Interrupt State Checkpointing](#9-interrupt-state-checkpointing)
10. [Python Wiring API](#10-python-wiring-api)
11. [Full Data Flow Example](#11-full-data-flow-example)

---

## 1. Design Principles

The interrupt model is built around three non-negotiable constraints:

**Devices have no knowledge of IRQ numbers, interrupt controllers, or routing.** A device asserts or deasserts its `InterruptPin`. That is the complete device-side API. The rest is platform configuration.

**Wiring is a platform/SoC concern expressed in Python configuration.** `World::wire_interrupt(pin, sink, wire_id)` is called during the `elaborate()` phase. After `startup()`, the wiring graph is frozen and interrupt propagation is purely synchronous: `pin.assert()` → `sink.on_assert(wire_id)`.

**One pin, one wire, one sink (Q70).** `InterruptPin` is not `Clone`. A device has exactly one wire per pin. Fan-out (one device interrupt driving multiple controller inputs) requires an explicit fan-out sink in the platform configuration.

These constraints mirror real hardware: an IP block has an `irq` output port. The SoC designer connects it to an interrupt controller input in the netlist. The IP block RTL has no `#define IRQ_LINE 33`.

---

## 2. Type Definitions

```
Device
  └── InterruptPin (owns)
        └── Option<Arc<InterruptWire>> (connected at elaborate time, None until then)

InterruptWire
  ├── sink: Arc<dyn InterruptSink>
  ├── wire_id: WireId
  └── asserted: AtomicBool           (canonical signal state)

dyn InterruptSink                    (implemented by PLIC, GIC, PIC, test sinks)
  ├── on_assert(WireId)
  └── on_deassert(WireId)

WireId                               (opaque u64 — chosen by platform, meaningful to sink)
```

---

## 3. InterruptPin

```rust
/// A device's interrupt output pin.
///
/// The device owns this struct. The device calls `assert()` or `deassert()`
/// to raise or lower the interrupt signal. The device has no knowledge of
/// where the signal goes, what interrupt number it represents, or which
/// controller receives it.
///
/// `InterruptPin` is NOT `Clone` (Q70). One pin = one wire = one sink.
/// Platform fan-out requires an explicit fan-out sink implementation.
///
/// Before `World::wire_interrupt()` is called, the pin is unconnected.
/// `assert()` on an unconnected pin is a no-op with a `log::warn!()` (Q71).
pub struct InterruptPin {
    /// The active wire, set by `World::wire_interrupt()` at elaborate time.
    /// `None` until wired. After `startup()`, this is immutable.
    wire: Option<Arc<InterruptWire>>,
}

impl InterruptPin {
    /// Create a new, unconnected pin.
    pub fn new() -> Self {
        Self { wire: None }
    }

    /// Assert the interrupt — propagate to the connected sink.
    ///
    /// If already asserted, this is a no-op (edge-triggered behavior:
    /// the sink is only called on level transitions, not on repeated
    /// assertions of the same level).
    ///
    /// If not connected: emits `log::warn!()`, returns without calling
    /// any sink. Does not panic (Q71).
    pub fn assert(&self) {
        match &self.wire {
            None => {
                log::warn!("InterruptPin::assert() called on unconnected pin — no-op");
            }
            Some(wire) => {
                // Only propagate on 0→1 transition
                let was_asserted = wire.asserted.swap(true, std::sync::atomic::Ordering::SeqCst);
                if !was_asserted {
                    wire.sink.on_assert(wire.wire_id);
                }
            }
        }
    }

    /// Deassert the interrupt — propagate to the connected sink.
    ///
    /// If already deasserted, this is a no-op.
    /// If not connected: emits `log::warn!()`, returns without panicking.
    pub fn deassert(&self) {
        match &self.wire {
            None => {
                log::warn!("InterruptPin::deassert() called on unconnected pin — no-op");
            }
            Some(wire) => {
                // Only propagate on 1→0 transition
                let was_asserted = wire.asserted.swap(false, std::sync::atomic::Ordering::SeqCst);
                if was_asserted {
                    wire.sink.on_deassert(wire.wire_id);
                }
            }
        }
    }

    /// Query current assertion state.
    ///
    /// Returns `false` if unconnected (no wire = no assertion state).
    pub fn is_asserted(&self) -> bool {
        self.wire.as_ref()
            .map(|w| w.asserted.load(std::sync::atomic::Ordering::SeqCst))
            .unwrap_or(false)
    }

    /// Internal: connect this pin to a wire. Called only by `World::wire_interrupt()`.
    ///
    /// Panics if the pin is already connected (double-wiring is a configuration error).
    pub(crate) fn connect(&mut self, wire: Arc<InterruptWire>) {
        assert!(
            self.wire.is_none(),
            "InterruptPin::connect() called on already-connected pin — \
             double-wiring is a configuration error"
        );
        self.wire = Some(wire);
    }

    /// Internal: called by checkpoint_restore() to reset pin state.
    pub(crate) fn set_asserted_state(&self, asserted: bool) {
        if let Some(wire) = &self.wire {
            wire.asserted.store(asserted, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

impl Default for InterruptPin {
    fn default() -> Self { Self::new() }
}

// NOT Clone, NOT Copy — enforces one-to-one wiring (Q70)
```

---

## 4. InterruptWire

`InterruptWire` is the internal connection object. It is created by `World::wire_interrupt()` and shared via `Arc` between the `InterruptPin` (held by the device) and the interrupt bookkeeping infrastructure.

```rust
/// Internal type representing a live connection between a pin and a sink.
///
/// Created by `World::wire_interrupt()`. Not part of the public API.
/// Shared via `Arc` between the owning `InterruptPin` and the `World`'s
/// wire registry (for checkpoint/restore access to assertion state).
pub(crate) struct InterruptWire {
    /// Opaque wire identifier — meaningful to the sink (e.g., PLIC source number).
    pub(crate) wire_id: WireId,

    /// The interrupt controller (or test sink) that receives level changes.
    pub(crate) sink: Arc<dyn InterruptSink>,

    /// Current assertion state. Canonical source of truth.
    /// `true` = asserted. Stored atomically to allow lock-free `is_asserted()`.
    pub(crate) asserted: std::sync::atomic::AtomicBool,
}

impl InterruptWire {
    pub(crate) fn new(wire_id: WireId, sink: Arc<dyn InterruptSink>) -> Arc<Self> {
        Arc::new(Self {
            wire_id,
            sink,
            asserted: std::sync::atomic::AtomicBool::new(false),
        })
    }
}
```

---

## 5. InterruptSink Trait

```rust
/// Implemented by interrupt controllers that receive interrupt signals.
///
/// Common implementors: PLIC, ARM GIC, Intel 8259 PIC, `World`'s
/// built-in test sink.
///
/// Both methods are called synchronously from `InterruptPin::assert()` /
/// `InterruptPin::deassert()`. They must not block, must not acquire
/// locks that the calling device might hold, and must not re-enter the
/// device that triggered the interrupt.
///
/// `InterruptSink` must be `Send + Sync` because it is stored in an `Arc`
/// and may be called from any thread that owns a device (in future
/// multi-threaded simulations).
pub trait InterruptSink: Send + Sync {
    /// Called when a wire transitions from deasserted to asserted (0→1).
    ///
    /// `wire_id` is the identifier chosen by the platform at wiring time.
    /// For a PLIC, `wire_id` carries the source number (e.g., 10 for UART).
    /// The sink uses `wire_id` to identify which of its inputs changed.
    fn on_assert(&self, wire_id: WireId);

    /// Called when a wire transitions from asserted to deasserted (1→0).
    ///
    /// Same `wire_id` semantics as `on_assert`.
    fn on_deassert(&self, wire_id: WireId);
}
```

---

## 6. WireId

```rust
/// Opaque wire identifier.
///
/// Chosen by the platform at `World::wire_interrupt()` time. The value is
/// meaningful to the sink; the pin and the wire infrastructure treat it
/// as an opaque u64.
///
/// For a PLIC, the platform passes `WireId::from(source_number)`.
/// For a GIC, the platform passes `WireId::from(spi_number)`.
/// For a test sink, any value works — the test checks it by value.
///
/// `WireId` is `Copy` because sinks store it in collections (e.g., a `HashMap`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WireId(u64);

impl WireId {
    pub fn new(val: u64) -> Self { Self(val) }
    pub fn as_u64(self) -> u64 { self.0 }
}

impl From<u64> for WireId { fn from(v: u64) -> Self { Self(v) } }
impl From<u32> for WireId { fn from(v: u32) -> Self { Self(v as u64) } }
impl From<usize> for WireId { fn from(v: usize) -> Self { Self(v as u64) } }
```

---

## 7. Platform Wiring Protocol

Wiring is performed during the `elaborate()` phase by the platform configuration layer (either `System::elaborate()` in a full simulation, or `World::elaborate()` in a headless context).

```rust
/// Wire a device interrupt pin to an interrupt sink.
///
/// Called by the platform at elaborate() time — never by the device itself.
/// After startup(), wiring is frozen; calling this after startup() panics.
///
/// # Arguments
///
/// * `pin`     — Mutable reference to the device's `InterruptPin` field.
///               The pin is modified in-place (connected to the wire).
/// * `sink`    — Arc to the interrupt controller implementing `InterruptSink`.
/// * `wire_id` — Opaque identifier the sink uses to distinguish its inputs.
///
/// # Panics
///
/// Panics if `pin` is already connected (duplicate wiring).
/// Panics if called after `startup()` (wiring graph is frozen).
pub fn wire_interrupt(
    &mut self,
    pin: &mut InterruptPin,
    sink: Arc<dyn InterruptSink>,
    wire_id: WireId,
) {
    assert!(
        self.phase < SimPhase::Startup,
        "wire_interrupt() called after startup() — wiring graph is frozen"
    );
    let wire = InterruptWire::new(wire_id, sink);
    // Register the wire in the world's wire registry (for checkpoint access)
    self.wires.push(Arc::clone(&wire));
    // Connect the pin to the wire
    pin.connect(wire);
}
```

**Typical wiring call site (Python-driven, resolved at elaborate time):**

```rust
// In System::elaborate() or World::elaborate(), after user config is applied:
let plic_sink: Arc<dyn InterruptSink> = system.get_arc::<Plic>("system.plic").unwrap();
let uart_id = system.get_mut::<Uart16550>("system.uart0").unwrap();

system.wire_interrupt(
    &mut uart_id.irq_out,
    Arc::clone(&plic_sink),
    WireId::from(10u32),  // UART → PLIC source 10
);
```

---

## 8. Interrupt Controller Implementation Pattern (PLIC)

A RISC-V PLIC (Platform-Level Interrupt Controller) is a canonical `InterruptSink` implementation. It stores pending interrupt state in a bitfield array and updates CPU external-interrupt lines when any source is pending and enabled.

```rust
/// RISC-V Platform-Level Interrupt Controller.
///
/// Implements `InterruptSink` to receive interrupt assertions from devices.
/// Implements `Device` for MMIO access to its claim/complete registers.
/// Implements `InterruptSink` again (via a different mechanism) to drive
/// CPU external interrupt inputs.
pub struct Plic {
    num_sources: u32,
    /// Pending bits: one bit per source (source 0 unused per RISC-V spec)
    pending: Vec<u32>,              // indexed by source / 32
    /// Enable bits: one bit per source per context
    enable: Vec<Vec<u32>>,
    /// Priority per source (1–7; 0 = disabled)
    priority: Vec<u32>,
    /// Priority threshold per context
    threshold: Vec<u32>,
    /// CPU external-interrupt output pins (one per hart/context)
    cpu_irq_out: Vec<InterruptPin>,
    irq_out: InterruptPin,          // combined output (for simpler single-hart setups)
}

impl InterruptSink for Plic {
    fn on_assert(&self, wire_id: WireId) {
        let source = wire_id.as_u64() as u32;
        if source == 0 || source >= self.num_sources {
            log::warn!("PLIC: on_assert() for out-of-range source {}", source);
            return;
        }
        // Set pending bit (source N → word N/32, bit N%32)
        // SAFETY: pending is indexed by source/32; bounds checked above.
        // Use interior mutability — PLIC pending is updated from interrupt
        // callback context, which takes &self (the sink).
        // In practice: use Mutex<Vec<u32>> or AtomicU32 array.
        let word = (source / 32) as usize;
        let bit  = source % 32;
        self.pending_atomic[word].fetch_or(1 << bit, std::sync::atomic::Ordering::SeqCst);
        self.evaluate_and_drive_cpu();
    }

    fn on_deassert(&self, wire_id: WireId) {
        let source = wire_id.as_u64() as u32;
        let word = (source / 32) as usize;
        let bit  = source % 32;
        self.pending_atomic[word].fetch_and(!(1 << bit), std::sync::atomic::Ordering::SeqCst);
        self.evaluate_and_drive_cpu();
    }
}

impl Plic {
    /// Re-evaluate all pending+enabled sources and drive CPU interrupt inputs.
    ///
    /// Called after every pending-bit change. Finds the highest-priority
    /// pending+enabled source above the threshold, and asserts or deasserts
    /// the CPU external interrupt pin accordingly.
    fn evaluate_and_drive_cpu(&self) {
        // For each context (hart):
        for (ctx_idx, cpu_pin) in self.cpu_irq_out.iter().enumerate() {
            let threshold = self.threshold[ctx_idx];
            let best_priority = self.find_best_for_context(ctx_idx, threshold);
            if best_priority > 0 {
                cpu_pin.assert();
            } else {
                cpu_pin.deassert();
            }
        }
    }

    fn find_best_for_context(&self, ctx: usize, threshold: u32) -> u32 {
        let mut best = 0u32;
        for source in 1..self.num_sources as usize {
            let word = source / 32;
            let bit  = source % 32;
            let pending = (self.pending_atomic[word].load(
                std::sync::atomic::Ordering::SeqCst) >> bit) & 1 != 0;
            let enabled = (self.enable[ctx][word] >> bit) & 1 != 0;
            let priority = self.priority[source];
            if pending && enabled && priority > threshold && priority > best {
                best = priority;
            }
        }
        best
    }
}
```

The PLIC's own `cpu_irq_out` pins are wired to CPU external-interrupt inputs by the platform at elaborate time:

```python
# platforms/virt_riscv.py
system.wire_interrupt(plic.cpu_irq_out[0], cpu.external_irq_sink, WireId(0))
```

---

## 9. Interrupt State Checkpointing

Interrupt assertion state must be checkpointed. This is marked `HelmAttr::Required` in the checkpoint protocol — failure to restore interrupt state causes incorrect device behavior after checkpoint/restore.

**What must be saved:**

1. **Per-device:** `InterruptPin::is_asserted()` — whether each device's IRQ output is currently raised.
2. **Per-controller (PLIC/GIC):** All pending bits, enable bits, priority registers, threshold registers, and claim/complete state.

**The wire assertion state** (`InterruptWire::asserted`) is the canonical source of truth for (1). The `World` checkpoint machinery iterates its wire registry and saves the `asserted` field for each wire.

**Device checkpoint** includes pin state:

```rust
// In Device::checkpoint_save() / SimObject::checkpoint_save():
fn checkpoint_save(&self) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&CKPT_VERSION.to_le_bytes());
    // ... register state ...
    // Interrupt pin assertion state — Required
    buf.push(self.irq_out.is_asserted() as u8);
    buf
}

fn checkpoint_restore(&mut self, data: &[u8]) {
    // ... restore register state ...
    let irq_state = data[irq_offset] != 0;
    // Restore assertion state without re-triggering the sink
    // (the sink will be restored separately via PLIC checkpoint_restore)
    self.irq_out.set_asserted_state(irq_state);
    // Do NOT call irq_out.assert() here — that would call on_assert()
    // on the sink, which may not yet be restored.
}
```

**Restore order matters.** When restoring from checkpoint:

1. All device registers are restored (including interrupt enable bits).
2. All interrupt controller state is restored (pending bits, etc.).
3. All assertion states are restored (`set_asserted_state()` — no sink callbacks).
4. `World::validate_interrupt_state()` is called to verify consistency.

Step 3 uses `set_asserted_state()` — an internal method that sets the wire's `AtomicBool` directly without calling `on_assert()`. This avoids double-triggering the sink during restore.

---

## 10. Python Wiring API

From the Python configuration layer, interrupt wiring is expressed declaratively:

```python
import helm_ng

# Create devices — no IRQ knowledge required
uart  = helm_ng.Uart16550(clock_hz=1_843_200)
plic  = helm_ng.Plic(num_sources=64, num_contexts=2)
cpu   = helm_ng.RiscVHart(isa=helm_ng.Isa.RV64GC)

# Map devices into address space
system.map_device(uart,  base=0x10000000)
system.map_device(plic,  base=0x0c000000)

# Wire interrupts — platform integration concern, not device concern
# uart.irq_out → plic input 10 (UART doesn't know the number 10)
system.wire_interrupt(uart.irq_out, plic.input(10))

# plic CPU output → CPU external interrupt input
system.wire_interrupt(plic.cpu_out(context=0), cpu.external_irq)
```

The Python `plic.input(N)` method creates a `(sink, wire_id)` pair:

```python
class Plic:
    def input(self, source_number: int) -> InterruptSinkBinding:
        """Return a (sink, wire_id) binding for the given PLIC source number."""
        return InterruptSinkBinding(sink=self._rust_sink, wire_id=source_number)
```

On the Rust side, `System::wire_interrupt(pin, binding)` extracts the `Arc<dyn InterruptSink>` and `WireId` from the binding:

```rust
pub fn wire_interrupt(
    &mut self,
    pin_ref: &mut InterruptPin,
    binding: InterruptSinkBinding,
) {
    self.wire_interrupt_internal(pin_ref, binding.sink, binding.wire_id);
}
```

**Full RISC-V virt platform wiring example:**

```python
# platforms/virt_riscv.py
def wire_platform(system):
    plic  = system.get("plic")
    clint = system.get("clint")
    uart  = system.get("uart0")
    disk  = system.get("virtio_disk0")
    cpu   = system.get("cpu0")

    # Device → PLIC
    system.wire_interrupt(uart.irq_out,  plic.input(10))
    system.wire_interrupt(disk.irq_out,  plic.input(8))

    # PLIC → CPU external interrupt
    system.wire_interrupt(plic.cpu_out(context=0), cpu.external_irq)

    # CLINT → CPU timer and software interrupts
    system.wire_interrupt(clint.timer_out(hart=0),    cpu.timer_irq)
    system.wire_interrupt(clint.software_out(hart=0), cpu.software_irq)
```

---

## 11. Full Data Flow Example

This traces what happens when a UART RX FIFO fills and the UART asserts its interrupt, routed through the PLIC to the CPU.

```
1. CPU (simulated) writes a byte via DMA to UART RX FIFO.
   (In practice: the testbench calls world.mmio_write() for unit test;
    in FS mode, DMA hardware writes to UART FIFO via memory bus.)

2. Uart16550::write(offset=0x00, size=1, val=0x41) is called.
   → on_write_rbr_thr() fires (generated by register_bank!)
   → byte pushed onto rx_fifo
   → LSR.DR bit set to 1
   → update_interrupt() called

3. update_interrupt() sees IER.ERBFI=1, LSR.DR=1
   → calls self.irq_out.assert()

4. InterruptPin::assert()
   → wire.asserted was false → swap to true → transition detected
   → calls wire.sink.on_assert(WireId(10))       ← PLIC, source 10

5. Plic::on_assert(WireId(10))
   → sets pending[10/32] bit 10%32
   → calls evaluate_and_drive_cpu()

6. evaluate_and_drive_cpu()
   → finds source 10 pending, enabled for context 0, priority 1 > threshold 0
   → calls self.cpu_irq_out[0].assert()

7. CPU hart's external_irq InterruptPin::assert()
   → calls CpuHart's InterruptSink::on_assert(WireId(0))

8. CpuHart::on_assert(WireId(0))
   → sets external_interrupt_pending = true
   → CPU checks this flag at the next instruction boundary
   → if MIE.MEIE=1, takes M-mode external interrupt trap
```

Each step is a direct synchronous function call. There are no queues, no threads, and no locks in the critical path (except the `AtomicBool` in `InterruptWire` and the `AtomicU32` array in PLIC for thread safety).

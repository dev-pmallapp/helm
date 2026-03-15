# Device Author Guide

> For: engineers writing a new simulated device (UART, timer, VirtIO disk, custom IP block).
>
> Prerequisites: familiarity with Rust, basic understanding of MMIO-based hardware.
>
> Cross-references: [`docs/design/helm-devices/LLD-device-trait.md`](../design/helm-devices/LLD-device-trait.md) · [`docs/design/helm-devices/LLD-register-bank-macro.md`](../design/helm-devices/LLD-register-bank-macro.md) · [`docs/design/helm-devices/LLD-interrupt-model.md`](../design/helm-devices/LLD-interrupt-model.md) · [`docs/design/helm-devices/LLD-device-registry.md`](../design/helm-devices/LLD-device-registry.md)

---

## 1. What Is a Device in helm-ng?

A device is any Rust struct that implements the `Device` trait from `helm-devices`. It receives MMIO reads and writes at byte offsets within its mapped memory region, optionally asserts an interrupt signal, and optionally responds to named control signals (reset, clock enable). That is the complete contract.

**What a device does not know:**

- Its base address in the system address space. The `MemoryMap` strips the base before calling `read()` / `write()`. The device only sees the offset within its region.
- Its IRQ number. The device owns an `InterruptPin` and calls `pin.assert()`. The platform configuration (not the device) wires that pin to interrupt controller input N.
- Which CPU model, timing model, or execution mode is in use. The device API is identical in Virtual, Interval, and Accurate timing modes.

This separation is the fundamental SIMICS-inspired invariant: the device models the hardware behavior; the platform configures the system.

### Lifecycle

Device objects pass through these phases during a simulation:

```
alloc      — Device struct created (Rust constructor / factory closure)
init       — reset to power-on state; do NOT call other devices here
[attrs set] — platform sets parameters (clock_hz, etc.)
finalize   — may query other devices via InterfaceRegistry; all devices exist by now
all_finalized — all peers have finalized; safe to complete cross-device wiring
run        — simulation active; read()/write() calls arrive
deinit     — cleanup; drop handles
```

Cross-device calls are forbidden in `init()`. They are safe from `finalize()` onward, because all devices have been allocated and had their attrs set by that point. This two-phase config guarantees there is no ordering problem between device constructors.

---

## 2. Minimal Device Example

The simplest possible device: a 32-bit read/write counter register at offset 0, with no interrupts.

```rust
use helm_devices::{Device, register_bank};

register_bank! {
    pub struct CounterRegs {
        /// Counts writes; readable at any time.
        reg COUNT @ 0x00;
    }
    device = Counter;
}

pub struct Counter {
    regs: CounterRegs,
}

impl Counter {
    pub fn new() -> Self {
        Self { regs: CounterRegs::default() }
    }

    // Called by generated MmioHandler after every write to COUNT
    fn on_write_count(&mut self, _old: u32, new: u32) {
        // In this example: just let the write land; COUNT holds the value.
        // A real counter might increment instead:
        // self.regs.set_count(self.regs.count() + 1);
        let _ = new;
    }
}

impl Device for Counter {
    fn read(&self, offset: u64, size: usize) -> u64 {
        self.regs.mmio_read(offset, size)
    }

    fn write(&mut self, offset: u64, size: usize, val: u64) {
        self.regs.mmio_write(offset, size, val, self);
    }

    fn region_size(&self) -> u64 { 4 }  // one 32-bit register
}
```

This device can be mapped and exercised immediately in a headless `World` (see section 8).

---

## 3. Adding Interrupts

### 3.1 Declaring the Pin

Add an `InterruptPin` field to the device struct. Initialize it in the constructor with `InterruptPin::new()`.

```rust
use helm_devices::interrupt::InterruptPin;

pub struct MyTimer {
    pub irq_out: InterruptPin,   // pub so platform can pass it to wire_interrupt()
    regs: TimerRegs,
    // ... other fields
}

impl MyTimer {
    pub fn new() -> Self {
        Self {
            irq_out: InterruptPin::new(),
            regs: TimerRegs::default(),
        }
    }
}
```

Make the pin field `pub`. The platform configuration layer needs to pass `&mut timer.irq_out` to `World::wire_interrupt()` during `elaborate()`. The device itself never calls `wire_interrupt()`.

### 3.2 Asserting and Deasserting

Call `self.irq_out.assert()` when the interrupt condition becomes true. Call `self.irq_out.deassert()` when it clears. The pin handles level-transition detection: calling `assert()` twice in a row without an intervening `deassert()` is a no-op (the sink is only called on 0→1 transitions).

```rust
fn update_interrupt(&mut self) {
    let should_fire = self.regs.ctrl_ie() != 0   // interrupt enable
                   && self.regs.status_ready() != 0;  // condition met

    if should_fire {
        self.irq_out.assert();
    } else {
        self.irq_out.deassert();
    }
}
```

Call `update_interrupt()` at the end of every `on_write_*` hook that could change the interrupt condition, and whenever the device internally modifies status bits.

### 3.3 Wiring in Python Config

The device does not know about PLIC or IRQ numbers. That is the platform's job:

```python
timer = helm_ng.Timer(clock_hz=10_000_000)
plic  = helm_ng.Plic(num_sources=64)

sim.map_device(timer, base=0x2000000)
sim.map_device(plic,  base=0x0c000000)

# Wire timer IRQ output to PLIC source 7
# The timer has no idea the number 7 is involved
sim.wire_interrupt(timer.irq_out, plic.input(7))
```

### 3.4 Interrupt State in Checkpoints

The assertion state of `irq_out` must be saved and restored with the device. See section 5 for the exact checkpoint pattern.

---

## 4. MMIO Register Modeling with `register_bank!`

The `register_bank!` proc-macro is the primary device modeling primitive. It replaces manual MMIO switch statements and generates a complete register bank with dispatch, bitfield accessors, serde checkpoint, and Python introspection — at compile time, with no runtime overhead.

### 4.1 Basic Syntax

```rust
register_bank! {
    pub struct BankName {
        reg RegName @ offset_hex qualifier* {
            field FieldName [bit_range];
        }
    }
    device = DeviceType;
}
```

The `device = DeviceType` clause tells the macro which type implements the `on_write_*` / `on_read_*` side-effect hooks.

### 4.2 Access Qualifiers

| Qualifier | Read behavior | Write behavior | Hook generated |
|-----------|---------------|----------------|---------------|
| (none) | returns stored value | stores value, calls `on_write_*` | `on_write_*` |
| `read_only` | returns stored value | silently ignored | none |
| `write_only` | returns 0 | stores value, calls `on_write_*` | `on_write_*` |
| `clear_on_read` | returns value, then clears to 0 | silently ignored | `on_read_*` optional |
| `write_1_to_clear` | returns stored value | bits set in val are cleared | `on_write_*` |

### 4.3 Bit Ranges

```rust
field ENABLE [0]      // single bit 0
field MODE   [2:1]    // bits 2 and 1 (2-bit field, value right-shifted by 1)
field DATA   [7:0]    // bits 7:0 (full byte)
```

### 4.4 Side-Effect Hooks

Write hooks are called after the register value is stored. The signature is always:

```rust
fn on_write_<regname_lowercase>(&mut self, old: u32, new: u32)
```

`old` is the value before the write; `new` is the value now in the register. This lets you detect field transitions:

```rust
fn on_write_ctrl(&mut self, old: u32, new: u32) {
    let was_enabled = (old >> 0) & 1 != 0;
    let now_enabled = (new >> 0) & 1 != 0;
    if !was_enabled && now_enabled {
        // Device just enabled — schedule first event
        self.schedule_next_tick();
    } else if was_enabled && !now_enabled {
        // Device disabled — cancel pending events
        self.cancel_tick();
    }
}
```

For `clear_on_read` registers, the read hook is:

```rust
fn on_read_status(&mut self) -> Option<u32> {
    // Return Some(v) to override the register value seen by the caller.
    // Return None to use the stored value (macro will then auto-clear it).
    None
}
```

If the device type does not define a hook for a given register, the macro generates a no-op default via the generated `BankNameHooks` trait.

### 4.5 Split-Function Registers

Hardware often has read and write at the same offset mapped to different registers. Declare two `reg` entries at the same offset with complementary qualifiers:

```rust
register_bank! {
    pub struct UartRegs {
        reg RBR @ 0x00 is read_only;    // read: receive buffer
        reg THR @ 0x00 is write_only;   // write: transmit holding

        reg IIR @ 0x02 is read_only  { field IID [3:1]; field NO_INT [0]; }
        reg FCR @ 0x02 is write_only { field FIFO_EN [0]; field RX_RESET [1]; }
        // ...
    }
    device = Uart16550;
}
```

The generated dispatch routes reads to `RBR` and writes to `THR` with no ambiguity. The device author reconciles any shared state (e.g., DLAB muxing) in the hooks.

### 4.6 Complete UART 16550 Example

```rust
register_bank! {
    pub struct Uart16550Regs {
        /// Receive Buffer (DLAB=0, read) / Transmit Holding (DLAB=0, write)
        reg RBR @ 0x00 is read_only;
        reg THR @ 0x00 is write_only;

        reg IER @ 0x01 {
            field ERBFI [0];   // Enable Received Data Available Interrupt
            field ETBEI [1];   // Enable Transmitter Holding Register Empty Interrupt
        }

        /// Interrupt Identification Register (read) / FIFO Control (write)
        reg IIR @ 0x02 is read_only  { field NO_INT [0]; field IID [3:1]; }
        reg FCR @ 0x02 is write_only { field FIFO_EN [0]; field RX_RESET [1]; field TX_RESET [2]; }

        reg LCR @ 0x03 { field WLS [1:0]; field DLAB [7]; }
        reg MCR @ 0x04 { field DTR [0]; field RTS [1]; field LOOP [4]; }

        reg LSR @ 0x05 is read_only {
            field DR   [0];   // Data Ready
            field THRE [5];   // TX Holding Register Empty
            field TEMT [6];   // TX Empty
        }
        reg MSR @ 0x06 is read_only { field CTS [4]; field DSR [5]; }
        reg SCR @ 0x07;
    }
    device = Uart16550;
}
```

### 4.7 Generated Bitfield Accessors

For each field `THRE [5]` in register `LSR`, the macro generates:

```rust
// Getter (right-shifted to bit 0):
pub fn lsr_thre(&self) -> u32 { (self.lsr >> 5) & 0x1 }

// Setter (takes unshifted value):
pub fn set_lsr_thre(&mut self, val: u32) {
    self.lsr = (self.lsr & !(0x1 << 5)) | ((val & 0x1) << 5);
}
```

All accessors are `#[inline(always)]` and compile to 1–3 instructions.

---

## 5. Checkpoint Correctness

helm-ng checkpoints use the SIMICS invariant: **all state that must survive a checkpoint/restore cycle must be in `AttrStore` or in the device's explicit `checkpoint_save()` / `checkpoint_restore()` implementation.** Unregistered state is dark state and will be lost on restore.

### 5.1 What MUST Be Saved

| State | Why |
|-------|-----|
| All `register_bank!` register values | Architectural register state |
| All FIFO contents (rx_buf, tx_buf) | In-flight data |
| `irq_out.is_asserted()` | Interrupt state must be consistent after restore |
| Any internal state machine position | Prevents corrupted protocol state after restore |
| Timer countdown / event position | Deferred actions must fire at the right time |

The `register_bank!` macro generates `serde::Serialize` and `serde::Deserialize` on the register struct, so you can include it directly:

```rust
impl SimObject for MyDevice {
    fn checkpoint_save(&self) -> Vec<u8> {
        #[derive(serde::Serialize)]
        struct Ckpt<'a> {
            version: u32,
            regs: &'a MyDeviceRegs,          // macro-generated serde
            rx_buf: &'a VecDeque<u8>,
            irq_asserted: bool,
        }
        bincode::serialize(&Ckpt {
            version: CKPT_VERSION,
            regs: &self.regs,
            rx_buf: &self.rx_buf,
            irq_asserted: self.irq_out.is_asserted(),
        }).expect("checkpoint serialize")
    }

    fn checkpoint_restore(&mut self, data: &[u8]) {
        #[derive(serde::Deserialize)]
        struct Ckpt {
            version: u32,
            regs: MyDeviceRegs,
            rx_buf: VecDeque<u8>,
            irq_asserted: bool,
        }
        let ckpt: Ckpt = bincode::deserialize(data).expect("checkpoint deserialize");
        assert_eq!(ckpt.version, CKPT_VERSION, "checkpoint version mismatch");
        self.regs = ckpt.regs;
        self.rx_buf = ckpt.rx_buf;
        // Use set_asserted_state, NOT assert() — do not call on_assert() during restore
        self.irq_out.set_asserted_state(ckpt.irq_asserted);
    }
}
```

### 5.2 What Must NOT Be Dark

Any field that affects device behavior and is not saved is dark state. Common mistakes:

- **Timer tick counter** — if not saved, the timer fires at the wrong time after restore.
- **FIFO head/tail index** — if using a ring buffer, the indices must be saved.
- **Protocol state machine** (e.g., I2C START condition received) — must be saved.
- **DMA transfer in progress flag** — if not saved, DMA may restart or silently skip after restore.
- **`irq_out` assertion state** — this is the most common oversight. Always save it.

### 5.3 Restore Ordering

During restore, `set_asserted_state()` sets the `AtomicBool` in the wire directly without calling `on_assert()` on the sink. This avoids double-triggering the interrupt controller, which is also in the process of being restored. Do not call `irq_out.assert()` inside `checkpoint_restore()`.

---

## 6. Device-to-Device Communication

Devices communicate through named interfaces registered in `InterfaceRegistry`. This is the SIMICS-style interface model: a device declares that it implements a named interface, and peers look it up by device name and interface name.

### 6.1 Defining an Interface

An interface is a `'static` vtable (function pointers on a struct):

```rust
pub struct DmaInterface {
    /// Request a DMA transfer. Returns a transfer handle.
    pub start_transfer: fn(
        ctx: *mut std::ffi::c_void,  // opaque device pointer
        src: u64,
        dst: u64,
        len: u32,
    ) -> u64,
}
```

### 6.2 Registering

During `finalize()`, a device registers itself as implementing an interface:

```rust
impl SimObject for DmaController {
    fn finalize(&mut self, system: &mut System) {
        system.interfaces.register(
            &self.name,
            "dma",
            DmaInterface {
                start_transfer: |ctx, src, dst, len| {
                    let this = unsafe { &mut *(ctx as *mut DmaController) };
                    this.enqueue(src, dst, len)
                },
            },
            self as *mut _ as *mut std::ffi::c_void,
        );
    }
}
```

### 6.3 Looking Up and Calling

From another device's `finalize()` or later:

```rust
impl SimObject for NvmeDisk {
    fn finalize(&mut self, system: &mut System) {
        let dma = system.interfaces
            .get::<DmaInterface>("board.dma0", "dma")
            .expect("DMA controller not found");
        self.dma = Some(dma);
    }
}

impl NvmeDisk {
    fn do_io(&mut self, src: u64, dst: u64, len: u32) {
        if let Some(dma) = &self.dma {
            (dma.iface.start_transfer)(dma.ctx, src, dst, len);
        }
    }
}
```

---

## 7. Attribute Declaration

Device parameters that the Python config layer can set are declared via `AttrDescriptor`. This is how the two-phase config system works: Python sets attributes on `PendingObject` before `elaborate()`; the Rust device receives them through its `set_attrs()` lifecycle method.

```rust
use helm_devices::attrs::{AttrDescriptor, AttrKind, AttrValue};

impl MyDevice {
    pub fn attr_descriptors() -> &'static [AttrDescriptor] {
        &[
            AttrDescriptor {
                name: "clock_hz",
                kind: AttrKind::Required,
                description: "Input clock frequency in Hz",
            },
            AttrDescriptor {
                name: "fifo_depth",
                kind: AttrKind::Optional(AttrValue::Int(16)),
                description: "FIFO depth: 1, 16, 32, or 64",
            },
        ]
    }

    pub fn set_attr(&mut self, name: &str, val: AttrValue) -> Result<(), AttrError> {
        match name {
            "clock_hz" => {
                self.clock_hz = val.as_int()? as u32;
                Ok(())
            }
            "fifo_depth" => {
                let depth = val.as_int()? as usize;
                if !matches!(depth, 1 | 16 | 32 | 64) {
                    return Err(AttrError::invalid("fifo_depth", "must be 1, 16, 32, or 64"));
                }
                self.fifo_depth = depth;
                Ok(())
            }
            _ => Err(AttrError::unknown(name)),
        }
    }
}
```

`AttrKind::Required` means `elaborate()` will fail if the attribute is not set. `AttrKind::Optional(default)` provides a fallback value.

---

## 8. Testing a Device in Isolation

Use `World` (device-only mode) to exercise a device without any CPU, ISA, or timing model. `World` provides MMIO dispatch, an event queue for timer callbacks, and a built-in interrupt sink for assertion checking.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use helm_engine::world::World;

    #[test]
    fn uart_tx_asserts_irq() {
        let mut world = World::new();

        // Add and map device
        let uart_id = world.add_device("uart0", Box::new(Uart16550::new(1_843_200)));
        world.map_device(uart_id, 0x1000_0000).expect("map failed");

        // Wire interrupt to world's built-in test sink
        world.wire_irq_to_sink(uart_id, "irq_out").expect("wire failed");

        // elaborate() allocates FlatView and freezes wiring
        world.elaborate().expect("elaborate failed");

        // Enable TX empty interrupt: IER.ETBEI = 1
        world.mmio_write(0x1000_0001, 1, 0x02).expect("write IER");

        // TX FIFO starts empty, THRE=1, so IRQ should be asserted
        assert!(
            world.irq_asserted(uart_id, "irq_out"),
            "UART TX empty IRQ should be asserted after enabling IER.ETBEI"
        );

        // Disable interrupt; IRQ should deassert
        world.mmio_write(0x1000_0001, 1, 0x00).expect("write IER");
        assert!(!world.irq_asserted(uart_id, "irq_out"));
    }

    #[test]
    fn uart_read_returns_zero_at_undefined_offset() {
        let mut world = World::new();
        let uart_id = world.add_device("uart0", Box::new(Uart16550::new(1_843_200)));
        world.map_device(uart_id, 0x1000_0000).expect("map failed");
        world.elaborate().expect("elaborate failed");

        // Offset 0xFF is undefined — must return 0, never panic
        let val = world.mmio_read(0x1000_00FF, 4).expect("read should not fail");
        assert_eq!(val, 0);
    }
}
```

Key properties to always test:
- Undefined offsets return 0 on read, are silently ignored on write.
- Interrupt asserts and deasserts at the correct register state transitions.
- Checkpoint round-trip: save state, modify state, restore, verify original state.
- `signal("reset", 1)` returns all registers to power-on defaults.

---

## 9. Writing a Plugin Device (.so)

A plugin device is a `cdylib` Rust crate that exports two C-ABI symbols and registers one or more device types.

### 9.1 Cargo.toml

```toml
[package]
name = "helm-plugin-my-device"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
helm-devices = { path = "../../crates/helm-devices" }
log = "0.4"
```

### 9.2 Required Exports

```rust
use helm_devices::{DeviceDescriptor, DeviceParams, DeviceRegistry, PluginError, register_bank};
use helm_devices::interrupt::InterruptPin;

// ── ABI version — must equal helm_devices::HELM_DEVICES_ABI_VERSION ──────────
#[no_mangle]
pub static HELM_DEVICES_ABI_VERSION: u32 = helm_devices::HELM_DEVICES_ABI_VERSION;

// ── Entry point ───────────────────────────────────────────────────────────────
/// Called by DeviceRegistry::load_plugin() after ABI version check passes.
/// Register all device types this plugin provides. Must not panic.
#[no_mangle]
pub extern "C" fn helm_device_register(registry: *mut DeviceRegistry) {
    // Safety: caller guarantees valid non-null pointer to a live DeviceRegistry.
    let r = unsafe { &mut *registry };
    if let Err(e) = r.register(my_device_descriptor()) {
        log::error!("helm_device_register failed: {e}");
    }
}
```

### 9.3 Descriptor and Factory

```rust
fn my_device_descriptor() -> DeviceDescriptor {
    DeviceDescriptor {
        name: "my_counter",
        version: "0.1.0",
        description: "Simple hardware counter",
        factory: |params: DeviceParams| -> Result<Box<dyn helm_devices::Device>, PluginError> {
            let clock_hz = params.get_int("clock_hz")? as u32;
            Ok(Box::new(Counter::new(clock_hz)))
        },
        param_schema: || {
            helm_devices::params::ParamSchema::new()
                .int("clock_hz", "Input clock frequency in Hz")
        },
        // Python class injected into helm_ng namespace at plugin load time
        python_class: r#"
class MyCounter(Device):
    """Simple hardware counter."""
    clock_hz: Param.Int = 10_000_000
"#,
    }
}
```

### 9.4 ABI Versioning

The `HELM_DEVICES_ABI_VERSION` integer is bumped whenever `DeviceDescriptor`, `Device::read()` / `write()`, or `DeviceParams` have a breaking layout change. A plugin compiled against ABI version N will be rejected by a host at version N+1 with a clear error: `ABI version mismatch: host=2, plugin=1 — recompile plugin against helm-devices 2`.

To ensure the plugin always embeds the correct version from the crate it was compiled against:

```rust
// This fails to compile if helm-devices is not in [dependencies]
#[no_mangle]
pub static HELM_DEVICES_ABI_VERSION: u32 = helm_devices::HELM_DEVICES_ABI_VERSION;
```

### 9.5 Loading from Python

```python
import helm_ng

# Load the plugin — MyCounter class is now available in helm_ng namespace
helm_ng.load_plugin("/opt/mydevices/lib/libhelm_plugin_my_device.so")

counter = helm_ng.MyCounter(clock_hz=50_000_000)
sim.map_device(counter, base=0x2000_0000)
sim.elaborate()
```

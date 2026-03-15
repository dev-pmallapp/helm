# helm-devices — LLD: Device Trait

> Low-level design for the `Device` trait, `DeviceConfig`/`DeviceError` pattern, and interaction with `register_bank!`.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-interrupt-model.md`](./LLD-interrupt-model.md) · [`LLD-register-bank-macro.md`](./LLD-register-bank-macro.md)

---

## Table of Contents

1. [Device Trait Definition](#1-device-trait-definition)
2. [Method Contracts](#2-method-contracts)
3. [DeviceConfig and DeviceError Pattern](#3-deviceconfig-and-deviceerror-pattern)
4. [Interaction with register_bank!](#4-interaction-with-register_bank)
5. [Interaction with SimObject](#5-interaction-with-simobject)
6. [Full Implementation Example](#6-full-implementation-example)
7. [DeviceError Enum](#7-deviceerror-enum)

---

## 1. Device Trait Definition

```rust
/// Core device interface.
///
/// A `Device` receives MMIO reads and writes at byte offsets within its
/// mapped region, receives named signal assertions, and exposes an interrupt
/// output pin. It has no knowledge of its base address or of IRQ numbers —
/// those are platform configuration concerns.
///
/// Every device that participates in a `World` must also implement `SimObject`
/// (Q60 answer: `Device: SimObject`). This ensures uniform checkpoint, reset,
/// and component-tree participation. Headless test harnesses use a minimal
/// `MockSimObject` impl that satisfies the trait boundary without real behavior.
/// The `register_bank!` macro generates `checkpoint_save`/`checkpoint_restore`
/// delegation automatically for register-state devices.
pub trait Device: SimObject + Send {
    /// Handle a read of `size` bytes at `offset` within this device's region.
    ///
    /// `offset` is the byte offset from the start of the device's mapped
    /// region, NOT the absolute address in the system address space.
    /// `size` is 1, 2, 4, or 8 bytes.
    ///
    /// Returns the value as a `u64`. For sub-word sizes, only the low bits
    /// are meaningful. Reads to write-only registers return 0. Reads to
    /// undefined offsets return 0 (never panic).
    fn read(&self, offset: u64, size: usize) -> u64;

    /// Handle a write of `size` bytes of `val` at `offset` within this region.
    ///
    /// `offset` is relative to the device's mapped base, not absolute.
    /// `size` is 1, 2, 4, or 8. For sub-word sizes, only the low bits of
    /// `val` are significant. Writes to read-only or undefined registers are
    /// silently ignored.
    fn write(&mut self, offset: u64, size: usize, val: u64);

    /// Return the size in bytes of this device's MMIO region.
    ///
    /// This value is read by `MemoryMap` when the device is mapped, and
    /// again when `FlatView` is rebuilt. It must be constant for the lifetime
    /// of the device (Phase 0 constraint — see Q61). Returning a different
    /// value after mapping produces undefined behavior.
    fn region_size(&self) -> u64;

    /// Receive a named signal assertion.
    ///
    /// Signals are named strings: `"reset"`, `"clock_enable"`, `"dma_ack"`.
    /// `val` is the signal level: 1 = asserted, 0 = deasserted. Other values
    /// are permitted for multi-level signals but are device-defined.
    ///
    /// The default implementation is a no-op — devices that do not respond
    /// to any named signal do not need to override this method.
    fn signal(&mut self, _name: &str, _val: u64) {}
}
```

---

## 2. Method Contracts

### `read(offset, size) -> u64`

**Offset semantics.** The `MemoryMap` strips the base address before calling `read()`. If a device is mapped at `0x1000_0000` and the CPU reads address `0x1000_0004`, the device receives `offset = 4`. The device never sees the absolute address.

**Size.** Valid values: 1, 2, 4, 8. Devices may assert `debug_assert!(matches!(size, 1 | 2 | 4 | 8))` at the top of `read()`. In release builds, receiving an invalid size is a `MemoryMap` bug.

**Return value.** For a 1-byte read, only bits `[7:0]` of the returned `u64` are used by the caller. Devices should zero the upper bits to avoid confusion in traces and logs.

**Side effects.** Some registers have read-side effects (RHR clears the RX FIFO head; reading a clear-on-read status bit clears it). Read side effects are modeled by making `read()` take `&self` by convention but calling an internal `&mut self` method via interior mutability (`Cell`, `RefCell`, or `Mutex`), or by making the device's bank mut and having the proc-macro handle it.

**Practical note:** The `register_bank!` macro generates a `read()` dispatch that takes `&mut self` internally (through the generated `MmioHandler`). The `Device::read()` trait method signature is `&self` for external callers. The macro bridges the gap by generating a wrapper.

**Undefined offsets.** Return 0. Never panic. This is both a correctness requirement (buggy driver code must not crash the simulator) and a fuzzing requirement (arbitrary offsets must not cause panics).

### `write(offset, size, val)`

**Offset semantics.** Same as `read()` — relative to the mapped base, never absolute.

**Size and val.** The caller guarantees `size` ∈ {1, 2, 4, 8}. For a 1-byte write, only bits `[7:0]` of `val` are meaningful; the upper bits are zero. Devices should mask before storing: `self.reg = val as u8`.

**Undefined offsets.** Silently ignore. Never panic.

**Side effects.** A write to a register may trigger device-internal behavior: enqueue a TX byte, set a timer, DMA start, interrupt assertion. These side effects execute synchronously during `write()`. If a side effect needs to be deferred (e.g., "transmit the byte after N clock cycles"), the device schedules an event on the `EventQueue`.

### `region_size() -> u64`

**Invariant.** The value returned must be identical on every call for the lifetime of the device. The `MemoryMap` caches this value. Changing it after mapping is a logic error.

**Phase 0 constraint (Q61).** For Phase 0, all devices have a fixed region size set at construction time. PCIe BARs may require dynamic resizing in Phase 3; that will be handled by a `MemoryMap::resize_region()` notification, not by changing the `region_size()` return value dynamically.

**Typical values.** A 16550 UART with 8 byte-wide registers at 4-byte spacing occupies 32 bytes (`0x20`). A PLIC with full source/context arrays may need 64 MiB (`0x400_0000`). The value should be a power of two for alignment compatibility.

### `signal(name, val)`

**Purpose.** Allows the platform to assert named control signals on a device without a full MMIO write. Common signal names:

| Signal name | Semantics |
|-------------|-----------|
| `"reset"` | Hardware reset — device returns to power-on state |
| `"clock_enable"` | Gating the device's functional clock (val=1: running, val=0: halted) |
| `"dma_ack"` | DMA controller acknowledges a transfer completion |
| `"nmi"` | Non-maskable interrupt input (for interrupt controllers) |

**Default no-op.** The trait provides a default no-op implementation. Devices that do not respond to any signal do not need to override this method.

**Unknown signals.** Devices should silently ignore signal names they do not recognize. They may emit `log::debug!()` for unrecognized names. Panicking on unknown signals breaks forward compatibility.

---

## 3. DeviceConfig and DeviceError Pattern

Devices that have a non-trivial initialization path use the `DeviceConfig` builder pattern for separation between the infallible "set parameters" phase and the fallible "allocate / validate" phase.

```rust
/// Infallible device parameter holder.
///
/// A `DeviceConfig` carries all parameters needed to construct a device.
/// Construction of the config itself always succeeds (all fields have
/// defaults). Validation happens in `Device::realize()`.
///
/// This pattern lets Python set parameters one by one via attribute
/// assignment without triggering any allocation or validation until
/// `elaborate()` calls `realize()`.
pub struct Uart16550Config {
    pub clock_hz: u32,
    pub fifo_depth: usize,
}

impl Default for Uart16550Config {
    fn default() -> Self {
        Self {
            clock_hz: 1_843_200,
            fifo_depth: 16,
        }
    }
}

impl Uart16550Config {
    pub fn clock_hz(mut self, hz: u32) -> Self {
        self.clock_hz = hz;
        self
    }

    pub fn fifo_depth(mut self, depth: usize) -> Self {
        self.fifo_depth = depth;
        self
    }

    /// Validate configuration and construct the device.
    ///
    /// Returns `Err(DeviceError)` if parameters are invalid.
    /// This is the only fallible step in device construction.
    pub fn realize(self) -> Result<Uart16550, DeviceError> {
        if self.clock_hz == 0 {
            return Err(DeviceError::InvalidParam {
                param: "clock_hz",
                reason: "must be non-zero",
            });
        }
        if !matches!(self.fifo_depth, 1 | 16 | 32 | 64) {
            return Err(DeviceError::InvalidParam {
                param: "fifo_depth",
                reason: "must be 1, 16, 32, or 64",
            });
        }
        Ok(Uart16550 {
            clock_hz: self.clock_hz,
            fifo_depth: self.fifo_depth,
            // ... allocate FIFOs
            rx_fifo: VecDeque::with_capacity(self.fifo_depth),
            tx_fifo: VecDeque::with_capacity(self.fifo_depth),
            irq_out: InterruptPin::new(),
            // ... register state initialized to power-on defaults
        })
    }
}
```

**Usage at plug-in registration time:**

```rust
factory: |params| {
    let clock_hz = params.get_int("clock_hz")? as u32;
    let fifo_depth = params.get_int("fifo_depth").unwrap_or(16) as usize;
    Uart16550Config::default()
        .clock_hz(clock_hz)
        .fifo_depth(fifo_depth)
        .realize()
        .map(|d| Box::new(d) as Box<dyn Device>)
        .map_err(PluginError::from)
},
```

**Simpler devices** that have no meaningful failure cases can skip the config builder and construct directly:

```rust
impl Plic {
    pub fn new(num_sources: u32) -> Self {
        assert!(num_sources <= 1024, "PLIC: num_sources must be <= 1024");
        Self { num_sources, pending: vec![0u32; (num_sources as usize + 31) / 32], /* ... */ }
    }
}
```

---

## 4. Interaction with register_bank!

The `register_bank!` proc-macro generates a concrete struct (the register bank) and implements `MmioHandler` on it. The generated `MmioHandler` dispatches reads and writes by offset to individual register fields.

A device that uses `register_bank!` stores its register bank as a field and delegates `Device::read()` / `Device::write()` to the bank's generated methods:

```rust
// register_bank! generates: Uart16550Regs, impl MmioHandler for Uart16550Regs
// (see LLD-register-bank-macro.md for full macro syntax)

pub struct Uart16550 {
    pub irq_out: InterruptPin,
    clock_hz: u32,
    // The generated register bank holds all architectural register state
    regs: Uart16550Regs,
}

impl Device for Uart16550 {
    fn read(&self, offset: u64, size: usize) -> u64 {
        // Delegate to generated MmioHandler
        self.regs.mmio_read(offset, size)
    }

    fn write(&mut self, offset: u64, size: usize, val: u64) {
        // Delegate to generated MmioHandler; on_write hooks call back into self
        self.regs.mmio_write(offset, size, val, self)
    }

    fn region_size(&self) -> u64 {
        8 // 8 registers × 1 byte each for a basic 16550
    }
}
```

The `on_write_<reg>` hooks that the macro calls are methods on the device struct itself (via `&mut self` passed to `mmio_write`). See [`LLD-register-bank-macro.md`](./LLD-register-bank-macro.md) for the precise callback signature and macro expansion.

**The macro does not implement `Device` directly.** It implements `MmioHandler` on the generated register bank struct. The device author implements `Device` by delegating to the bank. This keeps the macro's scope minimal and keeps `Device` implementation explicit.

---

## 5. Interaction with SimObject

`Device` and `SimObject` are orthogonal traits. A device that needs the full system lifecycle implements both:

```rust
impl SimObject for Uart16550 {
    fn name(&self) -> &str { &self.name }

    fn init(&mut self) {
        // Reset all register state to power-on defaults
        self.regs = Uart16550Regs::default();
    }

    fn elaborate(&mut self, system: &mut System) {
        // Nothing specific — MemoryMap registration is done by the
        // platform layer via World::map_device(), not by the device itself.
        //
        // InterruptPin connections are wired by the platform:
        // World::wire_interrupt(&self.irq_out, plic.input_sink(N))
        // The device does NOT call wire_interrupt on itself.
        _ = system; // suppress unused warning
    }

    fn startup(&mut self) {
        // Schedule any initial events (e.g., a periodic baud rate clock tick)
        // Nothing for basic UART
    }

    fn reset(&mut self) {
        self.regs = Uart16550Regs::default();
        // Note: irq_out wiring is NOT reset — wiring survives reset
        // The pin state (asserted/deasserted) IS reset
        self.irq_out.deassert();
    }

    fn checkpoint_save(&self) -> Vec<u8> {
        // The register_bank! macro generates serde impls; use them here
        let state = CheckpointState {
            version: UART16550_CKPT_VERSION,
            regs: &self.regs,
            irq_asserted: self.irq_out.is_asserted(),
        };
        bincode::serialize(&state).expect("uart checkpoint serialize")
    }

    fn checkpoint_restore(&mut self, data: &[u8]) {
        let state: CheckpointState = bincode::deserialize(data)
            .expect("uart checkpoint deserialize");
        assert_eq!(state.version, UART16550_CKPT_VERSION,
            "UART checkpoint version mismatch");
        self.regs = state.regs;
        if state.irq_asserted {
            self.irq_out.assert();
        } else {
            self.irq_out.deassert();
        }
    }
}
```

**The `irq_out` assertion state must be checkpointed** (as `HelmAttr::Required`). If a device had an interrupt asserted when the checkpoint was taken and the checkpoint is restored, the interrupt controller must see the assertion again. Failing to restore interrupt state produces incorrect behavior after checkpoint/restore.

A device that does NOT implement `SimObject` is valid for use in `World` but cannot participate in:
- The `System` component tree (no name, no path resolution)
- Lifecycle-ordered elaborate/startup sequencing
- Checkpoint/restore through the `System` infrastructure

For the `DeviceRegistry` plugin path, devices must implement `SimObject` if they are intended for full-system use, and may omit it if they are headless-only.

---

## 6. Full Implementation Example

A minimal device with `register_bank!`, `InterruptPin`, and both `Device` + `SimObject`:

```rust
use helm_devices::{Device, InterruptPin, register_bank};
use helm_core::sim::SimObject;

// register_bank! generates Uart16550Regs and impl MmioHandler for Uart16550Regs
// (syntax defined in LLD-register-bank-macro.md)
register_bank! {
    pub struct Uart16550Regs {
        reg RBR_THR @ 0x00 {
            /// Receive Buffer / Transmit Holding Register
            /// Read-only as RBR; write-only as THR (split-function)
            /// Controlled by DLAB bit in LCR
        }
        reg IER @ 0x01 { field ERBFI [0]; field ETBEI [1]; field ELSI [2]; field EDSSI [3]; }
        reg IIR @ 0x02 is read_only { field IID [3:1]; field NO_INT [0]; }
        reg FCR @ 0x02 is write_only { field FIFO_EN [0]; field RX_RESET [1]; field TX_RESET [2]; }
        reg LCR @ 0x03 { field WLS [1:0]; field STB [2]; field PEN [3]; field DLAB [7]; }
        reg MCR @ 0x04 { field DTR [0]; field RTS [1]; field LOOP [4]; }
        reg LSR @ 0x05 is read_only { field DR [0]; field OE [1]; field THRE [5]; field TEMT [6]; }
        reg MSR @ 0x06 is read_only { field CTS [4]; field DSR [5]; field RI [6]; field DCD [7]; }
        reg SCR @ 0x07;
    }
    device = Uart16550;  // on_write_* and on_read_* hooks live on this type
}

pub struct Uart16550 {
    name: String,
    pub irq_out: InterruptPin,
    clock_hz: u32,
    regs: Uart16550Regs,
    rx_fifo: std::collections::VecDeque<u8>,
    tx_fifo: std::collections::VecDeque<u8>,
}

impl Uart16550 {
    pub fn new(name: impl Into<String>, clock_hz: u32) -> Self {
        Self {
            name: name.into(),
            irq_out: InterruptPin::new(),
            clock_hz,
            regs: Uart16550Regs::default(),
            rx_fifo: std::collections::VecDeque::with_capacity(16),
            tx_fifo: std::collections::VecDeque::with_capacity(16),
        }
    }

    // Called by generated MmioHandler when LSR is written
    // Signature generated by register_bank!: fn on_write_lsr(&mut self, old: u32, new: u32)
    fn on_write_lsr(&mut self, _old: u32, _new: u32) {
        // LSR is read-only; writes are silently ignored by hardware.
        // The generated dispatch will not call this for read-only regs,
        // but a device can implement it for documentation purposes.
    }

    // Called when THR receives a write (offset 0x00, DLAB=0)
    fn on_write_rbr_thr(&mut self, _old: u32, new: u32) {
        // Queue the byte for transmission
        if self.tx_fifo.len() < 16 {
            self.tx_fifo.push_back(new as u8);
        }
        // Clear THRE in LSR (TX holding register no longer empty)
        // The register_bank! macro provides field accessors:
        self.regs.lsr_mut().set_thre(0);
        self.update_interrupt();
    }

    fn update_interrupt(&mut self) {
        let ier = self.regs.ier();
        let thre_irq = ier.etbei() != 0 && self.regs.lsr().thre() != 0;
        let rda_irq  = ier.erbfi() != 0 && self.regs.lsr().dr() != 0;
        if thre_irq || rda_irq {
            self.irq_out.assert();
        } else {
            self.irq_out.deassert();
        }
    }
}

impl Device for Uart16550 {
    fn read(&self, offset: u64, size: usize) -> u64 {
        self.regs.mmio_read(offset, size)
    }

    fn write(&mut self, offset: u64, size: usize, val: u64) {
        self.regs.mmio_write(offset, size, val, self);
    }

    fn region_size(&self) -> u64 { 8 }

    fn signal(&mut self, name: &str, val: u64) {
        match name {
            "reset" if val != 0 => {
                self.regs = Uart16550Regs::default();
                self.rx_fifo.clear();
                self.tx_fifo.clear();
                self.irq_out.deassert();
            }
            _ => {
                log::debug!("Uart16550 {}: unknown signal '{}' val={}", self.name, name, val);
            }
        }
    }
}
```

---

## 7. DeviceError Enum

```rust
/// Errors that can occur during device construction or operation.
#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    /// A required parameter is missing from the provided `DeviceParams`.
    #[error("missing required parameter: {0}")]
    MissingParam(&'static str),

    /// A parameter has an invalid value.
    #[error("invalid parameter '{param}': {reason}")]
    InvalidParam {
        param: &'static str,
        reason: &'static str,
    },

    /// Device initialization failed for a reason beyond parameter validation
    /// (e.g., OS resource allocation failed).
    #[error("device initialization failed: {0}")]
    InitFailed(String),

    /// A write to a register caused a device-detected protocol error.
    /// Used for bus controllers that validate sequences (e.g., I2C START
    /// without STOP).
    #[error("protocol error in register write at offset {offset:#x}: {reason}")]
    ProtocolError { offset: u64, reason: String },
}

impl From<DeviceError> for crate::registry::PluginError {
    fn from(e: DeviceError) -> Self {
        crate::registry::PluginError::DeviceCreate(e.to_string())
    }
}
```

**Error handling philosophy:**

- `DeviceError` is returned from `DeviceConfig::realize()` and from factory closures in `DeviceDescriptor`.
- During normal simulation, device `read()` / `write()` methods must not return errors — they must handle all inputs gracefully (ignore writes to undefined registers, return 0 from reads to undefined registers).
- If a register write triggers a device-detected unrecoverable error (e.g., DMA transfer to a physically impossible address), the device may fire a `HelmEventBus::Custom` event rather than returning an error, because `write()` is infallible.

---

## Design Decisions from Q&A

### Design Decision: Device: SimObject (Q60)

`Device: SimObject` is required. Every device that can exist in a `World` must participate in the component lifecycle, including checkpoint. Headless test harnesses use a minimal `MockSimObject` impl that satisfies the trait boundary without real behavior. Rationale: if devices are orthogonal to the component tree, checkpoint becomes fragmented — some state is in `HelmAttr` attributes, some is not, leading to partial checkpoint bugs. Requiring `SimObject` means the checkpoint system can walk the component tree and collect all state uniformly. A `#[derive(SimObject)]` proc-macro provides a default impl for common cases.

### Design Decision: region_size() is fixed at construction (Q61)

`region_size()` returns a `u64` set in the constructor and never changes. If a device conceptually needs different sizes in different operating modes, it registers multiple fixed-size regions. Rationale: PCIe BARs, the only realistic case for "dynamic size", have their size fixed by hardware spec. The OS remaps the BAR to a different base address but does not change its size. Making size dynamic would require `MemoryMap` to subscribe to device change events and re-flatten on every notification.

### Design Decision: One-to-one InterruptPin (Q70)

`InterruptPin` is not `Clone`. One pin = one wire = one sink. Platform fan-out (one device interrupt driving multiple controller inputs) requires an explicit fan-out sink implementation in the platform configuration. This mirrors real hardware: an IP block has a single `irq` output port connected to exactly one interrupt controller input in the netlist.

### Design Decision: Warn on assert to unconnected pin (Q71)

`InterruptPin::assert()` on an unconnected pin emits a `WARN`-level trace event on the first occurrence (suppressed after first occurrence via `warn_once!`). No-op for subsequent calls. In `--strict` mode, pins marked `#[required]` are validated at `elaborate()` time and cause a `HelmConfigError` if unconnected. Silent no-op would hide misconfiguration; a one-time warning surfaces the issue without flooding the trace log.

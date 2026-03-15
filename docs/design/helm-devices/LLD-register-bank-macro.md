# helm-devices — LLD: register_bank! Macro

> Low-level design for the `register_bank!` proc-macro: grammar, generated code, side-effect hooks, split-function registers, bitfields, serde checkpoint, and Python introspection.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-device-trait.md`](./LLD-device-trait.md)

---

## Table of Contents

1. [Purpose and Scope](#1-purpose-and-scope)
2. [Macro Grammar (Full Syntax Reference)](#2-macro-grammar-full-syntax-reference)
3. [Generated Code Overview](#3-generated-code-overview)
4. [Register Dispatch Table](#4-register-dispatch-table)
5. [Side-Effect Hooks: on_write and on_read](#5-side-effect-hooks-on_write-and-on_read)
6. [Split-Function Registers](#6-split-function-registers)
7. [Bitfield Access](#7-bitfield-access)
8. [Serde Checkpoint Generation](#8-serde-checkpoint-generation)
9. [AttrDescriptor Generation for Python Introspection](#9-attrdescriptor-generation-for-python-introspection)
10. [Complete Macro Expansion Example](#10-complete-macro-expansion-example)
11. [Crate Structure](#11-crate-structure)

---

## 1. Purpose and Scope

The `register_bank!` proc-macro generates the boilerplate that every device register bank requires:

- An MMIO dispatch table (offset → read handler, offset → write handler)
- Typed register field accessors with bitfield masks
- `serde` serialization/deserialization for checkpoint
- `AttrDescriptor` array for Python debug introspection
- Hooks into device-defined `on_write_<reg>` / `on_read_<reg>` methods

The macro is **DML-inspired** (SIMICS Device Modeling Language) but implemented as a Rust proc-macro. It does not require a separate build step or external code generator. It runs at `cargo build` time via `helm-devices-macros` (a `proc-macro = true` crate).

**What the macro does NOT do:**
- Implement the `Device` trait (the device author does that, delegating to the bank)
- Implement `SimObject`
- Allocate any dynamic memory at macro-expansion time
- Generate any `unsafe` code

---

## 2. Macro Grammar (Full Syntax Reference)

```
register_bank! {
    // The generated struct name and visibility
    $vis struct $BankName {
        // Zero or more register declarations:
        reg $RegName @ $offset $( is $qualifier )? $( { $( $field_decl )* } )? ;

        // Optional: doc comment before reg
        // Optional: multiple field declarations in braces
    }

    // Required: which type owns the on_write_* / on_read_* methods
    device = $DeviceType;
}
```

### Register Declaration

```
reg $RegName @ $offset_literal
    $(is $qualifier)*
    $( { $( field $FieldName [$bit_range] $(;)? )* } )?
;
```

**`$offset_literal`**: A numeric literal (hex or decimal). E.g., `0x00`, `4`.

**`$qualifier`**: Optional access qualifier(s). Multiple qualifiers are comma-separated if combined in a future extension. Currently supported:

| Qualifier | Meaning |
|-----------|---------|
| `read_only` | Register is read-only. Writes are silently ignored. `on_write_*` hook is not generated. |
| `write_only` | Register is write-only. Reads return 0. `on_read_*` hook is not generated. |
| `read_write` | Default (no qualifier needed). Both read and write are enabled. |
| `clear_on_read` | Reading the register clears it to 0. An `on_read_*` hook is still generated. |
| `write_1_to_clear` | Writing 1 to a bit clears it. The generated write handler applies this logic. |

**`$bit_range`**: Either a single bit index `[N]` or a range `[high:low]`. Examples:
- `[0]` → bit 0 (single bit field)
- `[7:4]` → bits 7 through 4 (4-bit field, value right-shifted by 4)
- `[5]` → bit 5 (single bit, value right-shifted by 5)

### Full Grammar (EBNF)

```ebnf
bank        ::= vis "struct" ident "{" reg_decl* "}" "device" "=" ident ";"
reg_decl    ::= doc_comment? "reg" ident "@" expr qualifier* ("{" field_decl* "}")? ";"
qualifier   ::= "is" qual_kw ("," qual_kw)*
qual_kw     ::= "read_only" | "write_only" | "clear_on_read" | "write_1_to_clear"
field_decl  ::= doc_comment? "field" ident "[" bit_range "]" ";"?
bit_range   ::= expr ":" expr | expr
doc_comment ::= ("///" rest_of_line)+
vis         ::= "pub" | "pub(crate)" | ""
```

### Example: UART 16550 Register Bank

```rust
register_bank! {
    pub struct Uart16550Regs {
        /// Receive Buffer Register (DLAB=0, read) / Transmit Holding Register (DLAB=0, write)
        /// Also: Divisor Latch Low Byte (DLAB=1, read/write)
        /// Split-function handled by on_read/on_write hooks checking DLAB state
        reg RBR_THR @ 0x00;

        /// Interrupt Enable Register (DLAB=0) / Divisor Latch High Byte (DLAB=1)
        reg IER @ 0x01 {
            field ERBFI  [0];   /// Enable Received Data Available Interrupt
            field ETBEI  [1];   /// Enable Transmitter Holding Register Empty Interrupt
            field ELSI   [2];   /// Enable Receiver Line Status Interrupt
            field EDSSI  [3];   /// Enable Modem Status Interrupt
        }

        /// Interrupt Identification Register (read-only)
        reg IIR @ 0x02 is read_only {
            field NO_INT [0];   /// No interrupt pending (active low)
            field IID    [3:1]; /// Interrupt ID
            field FIFOEN [7:6]; /// FIFO enabled indicator
        }

        /// FIFO Control Register (write-only, same offset as IIR)
        reg FCR @ 0x02 is write_only {
            field FIFO_EN   [0];
            field RX_RESET  [1];
            field TX_RESET  [2];
            field DMA_MODE  [3];
            field RX_TRIG   [7:6];
        }

        /// Line Control Register
        reg LCR @ 0x03 {
            field WLS  [1:0]; /// Word Length Select
            field STB  [2];   /// Stop Bit Select
            field PEN  [3];   /// Parity Enable
            field EPS  [4];   /// Even Parity Select
            field SP   [5];   /// Stick Parity
            field SB   [6];   /// Set Break
            field DLAB [7];   /// Divisor Latch Access Bit
        }

        /// Modem Control Register
        reg MCR @ 0x04 {
            field DTR  [0];
            field RTS  [1];
            field OUT1 [2];
            field OUT2 [3];
            field LOOP [4];  /// Loopback mode
        }

        /// Line Status Register (read-only)
        reg LSR @ 0x05 is read_only {
            field DR   [0];   /// Data Ready
            field OE   [1];   /// Overrun Error
            field PE   [2];   /// Parity Error
            field FE   [3];   /// Framing Error
            field BI   [4];   /// Break Interrupt
            field THRE [5];   /// Transmitter Holding Register Empty
            field TEMT [6];   /// Transmitter Empty
            field FIFOE [7];  /// FIFO data error (FIFO mode)
        }

        /// Modem Status Register (read-only)
        reg MSR @ 0x06 is read_only {
            field DCTS [0]; field DDSR [1]; field TERI [2]; field DDCD [3];
            field CTS  [4]; field DSR  [5]; field RI   [6]; field DCD  [7];
        }

        /// Scratch Register
        reg SCR @ 0x07;
    }
    device = Uart16550;
}
```

---

## 3. Generated Code Overview

For the above `register_bank!` invocation, the macro generates:

1. `pub struct Uart16550Regs` — a flat struct of `u32` fields (one per register)
2. `impl Default for Uart16550Regs` — all zeros (power-on reset state)
3. `impl Uart16550Regs` — field accessors (getters/setters for each bitfield)
4. `impl MmioHandler for Uart16550Regs` — offset dispatch for read/write
5. `impl serde::Serialize for Uart16550Regs` — checkpoint serialization
6. `impl serde::Deserialize for Uart16550Regs` — checkpoint deserialization
7. `impl Uart16550Regs` — `attr_descriptors() -> &'static [AttrDescriptor]` for Python introspection

---

## 4. Register Dispatch Table

The `MmioHandler` implementation generates a match-based dispatch. Offsets are sorted at compile time (in the generated `match` arms) for branch predictor friendliness.

```rust
// Generated by register_bank! — do not edit manually

impl MmioHandler for Uart16550Regs {
    fn mmio_read(&self, offset: u64, size: usize) -> u64 {
        match offset {
            0x00 => {
                // RBR_THR: no qualifier on read side (not read_only, not write_only)
                // Device's on_read_rbr_thr hook is responsible for DLAB mux
                // The raw register value is returned; hook fires after
                // (returning u64 from hook would change signature — instead,
                //  the hook is called to trigger side effects; return value is reg state)
                self.rbr_thr as u64
            }
            0x01 => self.ier as u64,
            0x02 => self.iir as u64,    // IIR is read_only; FCR write_only at same offset
            0x03 => self.lcr as u64,
            0x04 => self.mcr as u64,
            0x05 => self.lsr as u64,
            0x06 => self.msr as u64,
            0x07 => self.scr as u64,
            _    => 0,  // undefined offset — always 0
        }
    }

    fn mmio_write(&mut self, offset: u64, size: usize, val: u64, device: &mut Uart16550) {
        let val32 = val as u32;
        match offset {
            0x00 => {
                let old = self.rbr_thr;
                self.rbr_thr = val32;
                device.on_write_rbr_thr(old, val32);
            }
            0x01 => {
                let old = self.ier;
                self.ier = val32 & 0x0F;  // only bits [3:0] are writable
                device.on_write_ier(old, self.ier);
            }
            0x02 => {
                // FCR is write_only at offset 0x02; IIR is read_only
                let old = self.fcr;
                self.fcr = val32;
                device.on_write_fcr(old, val32);
            }
            0x03 => {
                let old = self.lcr;
                self.lcr = val32;
                device.on_write_lcr(old, val32);
            }
            0x04 => {
                let old = self.mcr;
                self.mcr = val32 & 0x1F;
                device.on_write_mcr(old, self.mcr);
            }
            0x05 => { /* LSR is read_only: write silently ignored */ }
            0x06 => { /* MSR is read_only: write silently ignored */ }
            0x07 => {
                let old = self.scr;
                self.scr = val32;
                device.on_write_scr(old, val32);
            }
            _    => { /* undefined offset: silently ignored */ }
        }
    }
}
```

---

## 5. Side-Effect Hooks: on_write and on_read

For every register that is writable, the macro expects the `device` type to optionally define `fn on_write_<regname>(&mut self, old: u32, new: u32)`. For registers that are readable (and have side effects on read), it expects `fn on_read_<regname>(&mut self) -> Option<u32>`.

**Hook invocation rules:**

| Register qualifier | `on_write_*` generated? | `on_read_*` generated? |
|--------------------|------------------------|------------------------|
| (none — read/write) | Yes | No (hook optional) |
| `read_only` | No | No |
| `write_only` | Yes | No |
| `clear_on_read` | No | Yes (hook optional; if not provided, auto-clear) |

**Signature for write hook:**

```rust
/// Called after the register value is updated.
///
/// `old` is the register value before the write.
/// `new` is the register value after the write (already stored in `self.regs`).
///
/// The hook is optional: if the device type does not define this method,
/// the macro generates a no-op default.
fn on_write_lcr(&mut self, old: u32, new: u32) {
    // Example: DLAB change affects which register IER/DLL maps to
    let old_dlab = (old >> 7) & 1;
    let new_dlab = (new >> 7) & 1;
    if old_dlab != new_dlab {
        self.update_dlab_mux(new_dlab != 0);
    }
}
```

**Signature for read hook (clear-on-read):**

```rust
/// Called on every read of this register.
///
/// If `Some(v)` is returned, `v` is used as the read value instead of
/// the stored register value. If `None`, the stored register value is used.
///
/// After the hook returns, if the register has `clear_on_read`, the register
/// is automatically cleared to 0 (unless the hook returned Some).
fn on_read_lsr(&mut self) -> Option<u32> {
    // Standard LSR read has no special override; auto-clear handles W1C bits
    None
}
```

**Automatic default hooks.** If the device type does not define an `on_write_*` method for a particular register, the macro generates:

```rust
// Auto-generated no-op if not provided:
// fn on_write_scr(&mut self, _old: u32, _new: u32) {}
```

This is implemented via a blanket default method on a generated `Uart16550RegsHooks` trait that the device type must implement (either explicitly or via `#[derive]` if no hooks are needed). The `device = DeviceType` clause in the macro tells it which type to generate the trait for.

---

## 6. Split-Function Registers

Some hardware registers share a physical offset but have different read and write semantics. The canonical example is the 16550 UART where offset 0x00 is:

- **Read:** Receive Buffer Register (RBR) — reads from the RX FIFO
- **Write:** Transmit Holding Register (THR) — writes to the TX FIFO

And when DLAB=1, offset 0x00 becomes the Divisor Latch Low Byte (DLL) for both read and write.

The macro handles this via qualifiers applied to separate `reg` declarations at the same offset:

```rust
register_bank! {
    pub struct Uart16550Regs {
        // Two reg declarations at offset 0x00
        // The dispatch table generates separate read and write handlers
        reg RBR @ 0x00 is read_only;    // read path: pop from RX FIFO
        reg THR @ 0x00 is write_only;   // write path: push to TX FIFO
        // ...
    }
    device = Uart16550;
}
```

**Generated dispatch for split-function offset 0x00:**

```rust
fn mmio_read(&self, offset: u64, _size: usize) -> u64 {
    match offset {
        0x00 => self.rbr as u64,  // read path maps to RBR
        // ...
    }
}

fn mmio_write(&mut self, offset: u64, _size: usize, val: u64, device: &mut Uart16550) {
    match offset {
        0x00 => {
            // write path maps to THR
            let old = self.thr;
            self.thr = val as u32;
            device.on_write_thr(old, self.thr);
        }
        // ...
    }
}
```

The device author's `on_write_thr` and `on_read_rbr` hooks handle the DLAB muxing logic — the macro does not model DLAB. The macro generates separate storage fields `rbr: u32` and `thr: u32` for split-function registers, even if hardware shares physical state. The device author reconciles them in the hooks.

**Multiple split registers at the same offset (IIR/FCR at 0x02):**

```rust
reg IIR @ 0x02 is read_only  { field NO_INT [0]; field IID [3:1]; }
reg FCR @ 0x02 is write_only { field FIFO_EN [0]; field RX_RESET [1]; field TX_RESET [2]; }
```

The generated dispatch: reads from `self.iir`, writes to `self.fcr`. No ambiguity.

---

## 7. Bitfield Access

For each `field` declared in a register, the macro generates:

```rust
// For: reg LSR @ 0x05 { field DR [0]; field OE [1]; field THRE [5]; field TEMT [6]; }

impl Uart16550Regs {
    // Getter: returns the field value (right-shifted to bit 0)
    #[inline(always)]
    pub fn lsr_dr(&self) -> u32 { (self.lsr >> 0) & 0x1 }
    #[inline(always)]
    pub fn lsr_oe(&self) -> u32 { (self.lsr >> 1) & 0x1 }
    #[inline(always)]
    pub fn lsr_thre(&self) -> u32 { (self.lsr >> 5) & 0x1 }
    #[inline(always)]
    pub fn lsr_temt(&self) -> u32 { (self.lsr >> 6) & 0x1 }

    // Setter: takes the field value (unshifted) and writes it
    #[inline(always)]
    pub fn set_lsr_dr(&mut self, val: u32) {
        self.lsr = (self.lsr & !(0x1 << 0)) | ((val & 0x1) << 0);
    }
    #[inline(always)]
    pub fn set_lsr_thre(&mut self, val: u32) {
        self.lsr = (self.lsr & !(0x1 << 5)) | ((val & 0x1) << 5);
    }
    // ... and so on for each field
}
```

For a multi-bit range `[7:4]`:

```rust
// For: field RX_TRIG [7:6]  (in FCR)
#[inline(always)]
pub fn fcr_rx_trig(&self) -> u32 { (self.fcr >> 6) & 0x3 }

#[inline(always)]
pub fn set_fcr_rx_trig(&mut self, val: u32) {
    self.fcr = (self.fcr & !(0x3 << 6)) | ((val & 0x3) << 6);
}
```

All accessors are `#[inline(always)]`. The generated code contains no branches and compiles to 1–3 instructions.

---

## 8. Serde Checkpoint Generation

The macro generates `serde::Serialize` and `serde::Deserialize` for the generated struct automatically (Q64). The device author does not write serde impls.

```rust
// Generated automatically — not written by device author
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Uart16550Regs {
    rbr: u32,
    thr: u32,
    ier: u32,
    iir: u32,
    fcr: u32,
    lcr: u32,
    mcr: u32,
    lsr: u32,
    msr: u32,
    scr: u32,
}
```

In practice the macro `emit`s a `#[derive(serde::Serialize, serde::Deserialize)]` attribute on the generated struct. The serde crates handle the rest.

**Usage in device checkpoint:**

```rust
impl SimObject for Uart16550 {
    fn checkpoint_save(&self) -> Vec<u8> {
        #[derive(serde::Serialize)]
        struct Checkpoint<'a> {
            version: u32,
            regs: &'a Uart16550Regs,     // ← directly serializable
            irq_asserted: bool,
        }
        bincode::serialize(&Checkpoint {
            version: CKPT_VERSION,
            regs: &self.regs,
            irq_asserted: self.irq_out.is_asserted(),
        }).expect("checkpoint serialize")
    }

    fn checkpoint_restore(&mut self, data: &[u8]) {
        #[derive(serde::Deserialize)]
        struct Checkpoint {
            version: u32,
            regs: Uart16550Regs,         // ← directly deserializable
            irq_asserted: bool,
        }
        let ckpt: Checkpoint = bincode::deserialize(data).expect("checkpoint deserialize");
        assert_eq!(ckpt.version, CKPT_VERSION, "UART checkpoint version mismatch");
        self.regs = ckpt.regs;
        self.irq_out.set_asserted_state(ckpt.irq_asserted);
    }
}
```

---

## 9. AttrDescriptor Generation for Python Introspection

The macro generates a static array of `AttrDescriptor` records describing every register and its fields (Q66). This is used by the Python debug API to inspect and modify register state by name.

```rust
/// Descriptor for one device attribute (register or field) exposed to Python.
#[derive(Debug, Clone)]
pub struct AttrDescriptor {
    /// Dot-path name within the device: "lcr", "lcr.dlab", "ier.etbei"
    pub name: &'static str,
    /// Byte offset in the MMIO region
    pub offset: u64,
    /// If this is a field, its bit range within the register; else 31:0
    pub bit_high: u8,
    pub bit_low:  u8,
    /// Access: read-only, write-only, or read-write
    pub access: AttrAccess,
    /// Human-readable description (from /// doc comments in register_bank!)
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrAccess { ReadOnly, WriteOnly, ReadWrite, ClearOnRead }
```

Generated for the UART bank:

```rust
// Generated by register_bank! — placed in a static to avoid runtime allocation
impl Uart16550Regs {
    pub fn attr_descriptors() -> &'static [AttrDescriptor] {
        &[
            AttrDescriptor { name: "rbr",       offset: 0x00, bit_high: 31, bit_low: 0, access: AttrAccess::ReadOnly,  description: "Receive Buffer Register" },
            AttrDescriptor { name: "thr",       offset: 0x00, bit_high: 31, bit_low: 0, access: AttrAccess::WriteOnly, description: "Transmit Holding Register" },
            AttrDescriptor { name: "ier",       offset: 0x01, bit_high: 31, bit_low: 0, access: AttrAccess::ReadWrite, description: "Interrupt Enable Register" },
            AttrDescriptor { name: "ier.erbfi", offset: 0x01, bit_high: 0,  bit_low: 0, access: AttrAccess::ReadWrite, description: "Enable Received Data Available Interrupt" },
            AttrDescriptor { name: "ier.etbei", offset: 0x01, bit_high: 1,  bit_low: 1, access: AttrAccess::ReadWrite, description: "Enable Transmitter Holding Register Empty Interrupt" },
            // ... all registers and fields
            AttrDescriptor { name: "lsr",       offset: 0x05, bit_high: 31, bit_low: 0, access: AttrAccess::ReadOnly,  description: "Line Status Register" },
            AttrDescriptor { name: "lsr.dr",    offset: 0x05, bit_high: 0,  bit_low: 0, access: AttrAccess::ReadOnly,  description: "Data Ready" },
            AttrDescriptor { name: "lsr.thre",  offset: 0x05, bit_high: 5,  bit_low: 5, access: AttrAccess::ReadOnly,  description: "Transmitter Holding Register Empty" },
            // ...
        ]
    }
}
```

**Python usage:**

```python
# Introspect device registers by name
uart = system.get("uart0")
print(uart.attrs())           # list all AttrDescriptors
print(uart.get_attr("lsr"))   # read LSR register value
print(uart.get_attr("lsr.thre"))  # read THRE bit
uart.set_attr("lcr.dlab", 1)  # set DLAB bit (for testing)
```

---

## 10. Complete Macro Expansion Example

Input (simplified 3-register bank):

```rust
register_bank! {
    pub struct SimpleRegs {
        /// Control register
        reg CTRL @ 0x00 {
            field ENABLE [0];
            field MODE   [2:1];
        }
        /// Status register (read-only, cleared on read)
        reg STATUS @ 0x04 is read_only is clear_on_read {
            field READY [0];
            field ERR   [1];
        }
        /// Data register
        reg DATA @ 0x08 is write_only;
    }
    device = MyDevice;
}
```

Approximate expansion (formatted for readability; actual output is a token stream):

```rust
// ── Generated struct ─────────────────────────────────────────────────────────
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SimpleRegs {
    ctrl:   u32,
    status: u32,
    data:   u32,
}

// ── Bitfield accessors ───────────────────────────────────────────────────────
impl SimpleRegs {
    #[inline(always)]
    pub fn ctrl_enable(&self) -> u32 { (self.ctrl >> 0) & 0x1 }
    #[inline(always)]
    pub fn set_ctrl_enable(&mut self, v: u32) {
        self.ctrl = (self.ctrl & !0x1) | (v & 0x1);
    }
    #[inline(always)]
    pub fn ctrl_mode(&self) -> u32 { (self.ctrl >> 1) & 0x3 }
    #[inline(always)]
    pub fn set_ctrl_mode(&mut self, v: u32) {
        self.ctrl = (self.ctrl & !(0x3 << 1)) | ((v & 0x3) << 1);
    }

    #[inline(always)]
    pub fn status_ready(&self) -> u32 { (self.status >> 0) & 0x1 }
    #[inline(always)]
    pub fn status_err(&self) -> u32 { (self.status >> 1) & 0x1 }

    pub fn attr_descriptors() -> &'static [helm_devices::AttrDescriptor] {
        &[
            helm_devices::AttrDescriptor {
                name: "ctrl", offset: 0x00, bit_high: 31, bit_low: 0,
                access: helm_devices::AttrAccess::ReadWrite,
                description: "Control register",
            },
            helm_devices::AttrDescriptor {
                name: "ctrl.enable", offset: 0x00, bit_high: 0, bit_low: 0,
                access: helm_devices::AttrAccess::ReadWrite,
                description: "",
            },
            helm_devices::AttrDescriptor {
                name: "ctrl.mode", offset: 0x00, bit_high: 2, bit_low: 1,
                access: helm_devices::AttrAccess::ReadWrite,
                description: "",
            },
            helm_devices::AttrDescriptor {
                name: "status", offset: 0x04, bit_high: 31, bit_low: 0,
                access: helm_devices::AttrAccess::ClearOnRead,
                description: "Status register (read-only, cleared on read)",
            },
            helm_devices::AttrDescriptor {
                name: "status.ready", offset: 0x04, bit_high: 0, bit_low: 0,
                access: helm_devices::AttrAccess::ClearOnRead,
                description: "",
            },
            helm_devices::AttrDescriptor {
                name: "status.err", offset: 0x04, bit_high: 1, bit_low: 1,
                access: helm_devices::AttrAccess::ClearOnRead,
                description: "",
            },
            helm_devices::AttrDescriptor {
                name: "data", offset: 0x08, bit_high: 31, bit_low: 0,
                access: helm_devices::AttrAccess::WriteOnly,
                description: "Data register",
            },
        ]
    }
}

// ── Hook trait (device must implement) ───────────────────────────────────────
// The macro generates a trait; the device implements it (or gets no-op defaults)
pub trait SimpleRegsHooks {
    fn on_write_ctrl(&mut self, _old: u32, _new: u32) {}
    fn on_write_data(&mut self, _old: u32, _new: u32) {}
    // STATUS is read_only, clear_on_read: no on_write hook
    // on_read_status: default auto-clears the register
    fn on_read_status(&mut self) -> Option<u32> { None }
}

// The device type must impl the hook trait (compiler error if missing required hooks)
// For no-op: impl SimpleRegsHooks for MyDevice {}

// ── MmioHandler implementation ───────────────────────────────────────────────
impl SimpleRegs {
    pub fn mmio_read(&mut self, offset: u64, _size: usize, device: &mut MyDevice) -> u64 {
        match offset {
            0x00 => self.ctrl as u64,
            0x04 => {
                // clear_on_read: call hook, then auto-clear
                let hook_val = device.on_read_status();
                let ret = hook_val.unwrap_or(self.status) as u64;
                self.status = 0;  // auto-clear after read
                ret
            }
            0x08 => 0,  // write_only: reads return 0
            _    => 0,  // undefined: return 0
        }
    }

    pub fn mmio_write(&mut self, offset: u64, _size: usize, val: u64, device: &mut MyDevice) {
        let v = val as u32;
        match offset {
            0x00 => {
                let old = self.ctrl;
                self.ctrl = v;
                device.on_write_ctrl(old, v);
            }
            0x04 => { /* read_only: write silently ignored */ }
            0x08 => {
                let old = self.data;
                self.data = v;
                device.on_write_data(old, v);
            }
            _    => { /* undefined: silently ignored */ }
        }
    }
}
```

**Note on `read()` signature:** The generated `mmio_read` takes `&mut self` (to support `clear_on_read` side effects) and `device: &mut MyDevice` (for hooks). The outer `Device::read(&self, ...)` delegates via interior mutability (`RefCell<SimpleRegs>` or by having the device hold the bank as `&mut` through `UnsafeCell` if performance requires it). The most pragmatic approach for Phase 0: make `Device::read()` take `&mut self` (the trait allows this via interior mutability at the `Device` level).

---

## 11. Crate Structure

The macro lives in a separate `proc-macro = true` crate:

```
helm-devices-macros/
├── Cargo.toml            # proc-macro = true; deps: syn, quote, proc-macro2
└── src/
    ├── lib.rs            # #[proc_macro] register_bank entry point
    ├── parse.rs          # Parse register_bank! input (syn-based)
    ├── codegen.rs        # Emit struct, MmioHandler, serde, AttrDescriptor
    └── tests/            # Integration tests via trybuild
        ├── valid/        # .rs files that must compile successfully
        └── invalid/      # .rs files that must fail with specific errors
```

`helm-devices/Cargo.toml` adds:

```toml
[dependencies]
helm-devices-macros = { path = "../helm-devices-macros" }

[dev-dependencies]
trybuild = "1"  # for macro compilation tests
```

Users import the macro as:

```rust
use helm_devices::register_bank;
```

No separate import of `helm-devices-macros` is needed.

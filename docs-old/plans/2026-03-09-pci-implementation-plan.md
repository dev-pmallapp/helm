# PCI Subsystem Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a trait-based PCI subsystem to helm-device with ECAM host bridge, VirtIO PCI transport, LLVM-IR accelerator PCI device, and Python device creation.

**Architecture:** Four core traits (PciCapability, PciFunction, VirtioTransport, BarDecl) compose into a PciHostBridge that implements Device. VirtIO backends work unchanged through either MMIO or PCI transport. Accelerators wrap helm-llvm::Accelerator as PCI functions. Python creates all of this via the existing create_device() factory.

**Tech Stack:** Rust (helm-device crate), PyO3 (helm-python crate), LLVM IR (helm-llvm crate)

**Design doc:** `docs/plans/2026-03-09-pci-virtio-accel-design.md`

**Test command:** `cargo test -p helm-device`

**Full workspace check:** `cargo check --workspace`

---

### Task 1: PCI Traits & BarDecl

Foundation types. Everything else depends on these.

**Files:**
- Create: `crates/helm-device/src/pci/mod.rs`
- Create: `crates/helm-device/src/pci/traits.rs`
- Modify: `crates/helm-device/src/lib.rs` (add `pub mod pci;`)
- Test: `crates/helm-device/src/tests/pci/mod.rs`

**Step 1: Write the failing test**

Create `crates/helm-device/src/tests/pci/mod.rs`:

```rust
mod traits;
```

Create `crates/helm-device/src/tests/pci/traits.rs`:

```rust
use crate::pci::*;
use crate::device::DeviceEvent;

/// A minimal PCI function for testing.
struct DummyPciFunction {
    bars: [BarDecl; 6],
    caps: Vec<Box<dyn PciCapability>>,
    register: u64,
}

impl DummyPciFunction {
    fn new() -> Self {
        Self {
            bars: [
                BarDecl::Mmio32 { size: 0x1000 },
                BarDecl::Unused,
                BarDecl::Unused,
                BarDecl::Unused,
                BarDecl::Unused,
                BarDecl::Unused,
            ],
            caps: Vec::new(),
            register: 0,
        }
    }
}

impl PciFunction for DummyPciFunction {
    fn vendor_id(&self) -> u16 { 0x1DE5 }
    fn device_id(&self) -> u16 { 0x0001 }
    fn class_code(&self) -> u32 { 0x0200_00 }
    fn bars(&self) -> &[BarDecl; 6] { &self.bars }
    fn capabilities(&self) -> &[Box<dyn PciCapability>] { &self.caps }
    fn capabilities_mut(&mut self) -> &mut Vec<Box<dyn PciCapability>> { &mut self.caps }
    fn bar_read(&mut self, _bar: u8, _offset: u64, _size: usize) -> u64 { self.register }
    fn bar_write(&mut self, _bar: u8, _offset: u64, _size: usize, value: u64) { self.register = value; }
    fn reset(&mut self) { self.register = 0; }
    fn name(&self) -> &str { "dummy-pci" }
}

#[test]
fn bar_decl_size() {
    let bar = BarDecl::Mmio32 { size: 0x1000 };
    assert_eq!(bar.size(), 0x1000);
    assert!(!bar.is_unused());
    assert!(!bar.is_64bit());
}

#[test]
fn bar_decl_64bit() {
    let bar = BarDecl::Mmio64 { size: 0x10000 };
    assert!(bar.is_64bit());
    assert_eq!(bar.size(), 0x10000);
}

#[test]
fn bar_decl_unused() {
    let bar = BarDecl::Unused;
    assert!(bar.is_unused());
    assert_eq!(bar.size(), 0);
}

#[test]
fn pci_function_identity() {
    let f = DummyPciFunction::new();
    assert_eq!(f.vendor_id(), 0x1DE5);
    assert_eq!(f.device_id(), 0x0001);
    assert_eq!(f.class_code(), 0x0200_00);
}

#[test]
fn pci_function_bar_read_write() {
    let mut f = DummyPciFunction::new();
    f.bar_write(0, 0x00, 4, 0xDEAD_BEEF);
    assert_eq!(f.bar_read(0, 0x00, 4), 0xDEAD_BEEF);
}

#[test]
fn pci_function_reset() {
    let mut f = DummyPciFunction::new();
    f.bar_write(0, 0x00, 4, 42);
    f.reset();
    assert_eq!(f.bar_read(0, 0x00, 4), 0);
}
```

Add `mod pci;` to `crates/helm-device/src/tests/mod.rs`.

**Step 2: Run test to verify it fails**

Run: `cargo test -p helm-device pci::traits`
Expected: FAIL — `crate::pci` module does not exist

**Step 3: Write the traits and BarDecl**

Create `crates/helm-device/src/pci/mod.rs`:

```rust
//! PCI/PCIe subsystem — trait-based device model.
//!
//! # Architecture
//!
//! ```text
//! PciCapability (trait) ─── pluggable capability (MSI-X, AER, PM, ...)
//! PciFunction  (trait) ─── one device function on the bus
//! PciHostBridge         ─── ECAM host bridge, implements Device
//! PciBus                ─── bus topology, BDF routing
//! ```

mod traits;

pub use traits::*;
```

Create `crates/helm-device/src/pci/traits.rs`:

```rust
//! Core PCI traits and types.

use crate::device::DeviceEvent;

// ── BarDecl ──────────────────────────────────────────────────────────────────

/// BAR declaration from a PCI function.
#[derive(Debug, Clone)]
pub enum BarDecl {
    /// BAR slot not in use.
    Unused,
    /// 32-bit memory-mapped BAR.
    Mmio32 { size: u64 },
    /// 64-bit memory-mapped BAR (consumes this slot + next slot).
    Mmio64 { size: u64 },
    /// I/O-space BAR.
    Io { size: u32 },
}

impl BarDecl {
    /// Size in bytes (0 for Unused).
    pub fn size(&self) -> u64 {
        match self {
            BarDecl::Unused => 0,
            BarDecl::Mmio32 { size } => *size,
            BarDecl::Mmio64 { size } => *size,
            BarDecl::Io { size } => *size as u64,
        }
    }

    /// Is this BAR unused?
    pub fn is_unused(&self) -> bool {
        matches!(self, BarDecl::Unused)
    }

    /// Is this a 64-bit BAR?
    pub fn is_64bit(&self) -> bool {
        matches!(self, BarDecl::Mmio64 { .. })
    }

    /// Is this an I/O BAR?
    pub fn is_io(&self) -> bool {
        matches!(self, BarDecl::Io { .. })
    }
}

// ── PciCapability ────────────────────────────────────────────────────────────

/// A pluggable PCI/PCIe capability (MSI-X, AER, ACS, PM, PCIe).
///
/// Capabilities live in config space starting at their `offset()`.
/// The config space engine dispatches reads/writes within the
/// capability's range to these methods.
pub trait PciCapability: Send + Sync {
    /// Capability ID (PCI spec Table 7-1).
    fn cap_id(&self) -> u8;

    /// Byte offset in config space where this capability starts.
    fn offset(&self) -> u16;

    /// Length in bytes.
    fn length(&self) -> u16;

    /// Read a u32 at byte `offset` relative to capability start.
    fn read(&self, offset: u16) -> u32;

    /// Write a u32 at byte `offset` relative to capability start.
    fn write(&mut self, offset: u16, value: u32);

    /// Reset to power-on state.
    fn reset(&mut self);

    /// Human-readable name for debug.
    fn name(&self) -> &str;

    /// True if this is an extended capability (offset >= 0x100).
    fn is_extended(&self) -> bool {
        self.offset() >= 0x100
    }
}

// ── PciFunction ──────────────────────────────────────────────────────────────

/// A PCI function — the unit of device identity on the bus.
///
/// Every PCI device (VirtIO, accelerator, NIC, GPU) implements this.
/// The host bridge manages the Type 0 config space header (vendor/device
/// ID, BARs, command/status); the function handles device-specific
/// config and BAR MMIO.
pub trait PciFunction: Send + Sync {
    // ── Identity ──

    fn vendor_id(&self) -> u16;
    fn device_id(&self) -> u16;

    /// 24-bit: class << 16 | subclass << 8 | progif
    fn class_code(&self) -> u32;

    fn subsystem_vendor_id(&self) -> u16 { 0 }
    fn subsystem_id(&self) -> u16 { 0 }
    fn revision_id(&self) -> u8 { 0 }

    // ── BARs ──

    /// BAR declarations — size and type for each of 6 BARs.
    fn bars(&self) -> &[BarDecl; 6];

    // ── Capabilities ──

    /// Capabilities this function exposes (immutable).
    fn capabilities(&self) -> &[Box<dyn PciCapability>];

    /// Capabilities this function exposes (mutable).
    fn capabilities_mut(&mut self) -> &mut Vec<Box<dyn PciCapability>>;

    // ── BAR MMIO ──

    /// Read from a BAR-mapped MMIO region.
    /// `bar` is 0..5, `offset` is byte offset within that BAR.
    fn bar_read(&mut self, bar: u8, offset: u64, size: usize) -> u64;

    /// Write to a BAR-mapped MMIO region.
    fn bar_write(&mut self, bar: u8, offset: u64, size: usize, value: u64);

    // ── Device-specific config ──

    /// Device-specific config space read (offsets >= 0x40 not
    /// claimed by a capability).
    fn config_read(&self, _offset: u16) -> u32 { 0 }

    /// Device-specific config space write.
    fn config_write(&mut self, _offset: u16, _value: u32) {}

    // ── Lifecycle ──

    fn reset(&mut self);

    fn tick(&mut self, _cycles: u64) -> Vec<DeviceEvent> { vec![] }

    fn name(&self) -> &str;
}
```

Add `pub mod pci;` to `crates/helm-device/src/lib.rs` (after `pub mod platform;`).

**Step 4: Run test to verify it passes**

Run: `cargo test -p helm-device pci::traits`
Expected: 6 tests PASS

**Step 5: Commit**

```bash
git add crates/helm-device/src/pci/ crates/helm-device/src/tests/pci/ crates/helm-device/src/lib.rs crates/helm-device/src/tests/mod.rs
git commit -m "feat(pci): add PciCapability, PciFunction traits and BarDecl"
```

---

### Task 2: BDF Type

**Files:**
- Create: `crates/helm-device/src/pci/bdf.rs`
- Modify: `crates/helm-device/src/pci/mod.rs` (add `mod bdf;`)
- Test: `crates/helm-device/src/tests/pci/bdf.rs`

**Step 1: Write the failing test**

Create `crates/helm-device/src/tests/pci/bdf.rs`:

```rust
use crate::pci::Bdf;

#[test]
fn bdf_from_ecam_offset_bus0_dev0_fn0_reg0() {
    let (bdf, reg) = Bdf::from_ecam_offset(0x0000_0000);
    assert_eq!(bdf.bus, 0);
    assert_eq!(bdf.device, 0);
    assert_eq!(bdf.function, 0);
    assert_eq!(reg, 0);
}

#[test]
fn bdf_from_ecam_offset_bus0_dev1_fn0_reg4() {
    // device 1 = bits [19:15] = 1 << 15 = 0x8000, reg 4 = 0x004
    let (bdf, reg) = Bdf::from_ecam_offset(0x0000_8004);
    assert_eq!(bdf.bus, 0);
    assert_eq!(bdf.device, 1);
    assert_eq!(bdf.function, 0);
    assert_eq!(reg, 4);
}

#[test]
fn bdf_from_ecam_offset_bus1_dev2_fn3_regc() {
    // bus=1 (bit 20) | dev=2 (bits [19:15]) | fn=3 (bits [14:12]) | reg=0xC
    let offset: u64 = (1 << 20) | (2 << 15) | (3 << 12) | 0xC;
    let (bdf, reg) = Bdf::from_ecam_offset(offset);
    assert_eq!(bdf.bus, 1);
    assert_eq!(bdf.device, 2);
    assert_eq!(bdf.function, 3);
    assert_eq!(reg, 0xC);
}

#[test]
fn bdf_to_ecam_offset() {
    let bdf = Bdf { bus: 1, device: 2, function: 3 };
    let offset = bdf.ecam_offset(0x10);
    let (decoded, reg) = Bdf::from_ecam_offset(offset);
    assert_eq!(decoded, bdf);
    assert_eq!(reg, 0x10);
}

#[test]
fn bdf_display() {
    let bdf = Bdf { bus: 0, device: 31, function: 7 };
    assert_eq!(format!("{bdf}"), "00:1f.7");
}
```

Add `mod bdf;` to `crates/helm-device/src/tests/pci/mod.rs`.

**Step 2: Run test to verify it fails**

Run: `cargo test -p helm-device pci::bdf`
Expected: FAIL — `crate::pci::Bdf` not found

**Step 3: Write BDF implementation**

Create `crates/helm-device/src/pci/bdf.rs`:

```rust
//! PCI Bus:Device.Function address and ECAM offset decoding.

/// PCI Bus:Device.Function address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bdf {
    pub bus: u8,
    pub device: u8,     // 0..31
    pub function: u8,   // 0..7
}

impl Bdf {
    /// Decode a BDF and register offset from an ECAM byte offset.
    ///
    /// ECAM layout: `offset[27:20]=bus, [19:15]=dev, [14:12]=fn, [11:0]=reg`
    pub fn from_ecam_offset(offset: u64) -> (Self, u16) {
        let reg = (offset & 0xFFF) as u16;
        let function = ((offset >> 12) & 0x7) as u8;
        let device = ((offset >> 15) & 0x1F) as u8;
        let bus = ((offset >> 20) & 0xFF) as u8;
        (Bdf { bus, device, function }, reg)
    }

    /// Encode this BDF + register offset into an ECAM byte offset.
    pub fn ecam_offset(&self, reg: u16) -> u64 {
        ((self.bus as u64) << 20)
            | ((self.device as u64) << 15)
            | ((self.function as u64) << 12)
            | (reg as u64 & 0xFFF)
    }
}

impl std::fmt::Display for Bdf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02x}:{:02x}.{}", self.bus, self.device, self.function)
    }
}
```

Add to `crates/helm-device/src/pci/mod.rs`:

```rust
mod bdf;
pub use bdf::Bdf;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p helm-device pci::bdf`
Expected: 5 tests PASS

**Step 5: Commit**

```bash
git add crates/helm-device/src/pci/bdf.rs crates/helm-device/src/pci/mod.rs crates/helm-device/src/tests/pci/
git commit -m "feat(pci): add Bdf type with ECAM offset encoding/decoding"
```

---

### Task 3: PCI Config Space Engine

**Files:**
- Create: `crates/helm-device/src/pci/config.rs`
- Modify: `crates/helm-device/src/pci/mod.rs`
- Test: `crates/helm-device/src/tests/pci/config.rs`

**Step 1: Write the failing test**

Create `crates/helm-device/src/tests/pci/config.rs`:

```rust
use crate::pci::*;

#[test]
fn config_space_vendor_device_id_readonly() {
    let bars = [BarDecl::Mmio32 { size: 0x1000 }, BarDecl::Unused, BarDecl::Unused,
                BarDecl::Unused, BarDecl::Unused, BarDecl::Unused];
    let cfg = PciConfigSpace::new(0x1AF4, 0x1041, 0x020000, 0, &bars, &[]);

    // Read vendor/device ID at offset 0
    assert_eq!(cfg.read(0x00) & 0xFFFF, 0x1AF4);
    assert_eq!((cfg.read(0x00) >> 16) & 0xFFFF, 0x1041);

    // Attempt to overwrite vendor ID — should be ignored
    cfg.clone().write(0x00, 0xFFFF_FFFF);
    assert_eq!(cfg.read(0x00) & 0xFFFF, 0x1AF4);
}

#[test]
fn config_space_command_register_writable() {
    let bars = [BarDecl::Unused; 6];
    let mut cfg = PciConfigSpace::new(0x1234, 0x5678, 0x060000, 0, &bars, &[]);

    // Command register at offset 0x04 (lower 16 bits)
    cfg.write(0x04, 0x0007); // IO + MEM + BusMaster
    assert_eq!(cfg.read(0x04) & 0x7, 0x7);
}

#[test]
fn config_space_bar_sizing_protocol() {
    let bars = [BarDecl::Mmio32 { size: 0x1000 }, BarDecl::Unused, BarDecl::Unused,
                BarDecl::Unused, BarDecl::Unused, BarDecl::Unused];
    let mut cfg = PciConfigSpace::new(0x1234, 0x5678, 0x020000, 0, &bars, &[]);

    // BAR0 at offset 0x10
    // Step 1: write all-1s to BAR
    cfg.write(0x10, 0xFFFF_FFFF);
    // Step 2: read back — should show size mask (inverted size + 1)
    let readback = cfg.read(0x10);
    // For a 4KB BAR: mask = ~(0x1000 - 1) & 0xFFFF_FFF0 = 0xFFFF_F000
    assert_eq!(readback & 0xFFFF_F000, 0xFFFF_F000);
    // Bit 0 = 0 (memory space)
    assert_eq!(readback & 1, 0);
}

#[test]
fn config_space_bar_64bit() {
    let bars = [BarDecl::Mmio64 { size: 0x1_0000_0000 }, BarDecl::Unused, // consumed by 64-bit
                BarDecl::Unused, BarDecl::Unused, BarDecl::Unused, BarDecl::Unused];
    let mut cfg = PciConfigSpace::new(0x1234, 0x5678, 0x020000, 0, &bars, &[]);

    // BAR0 type field: bits [2:1] = 0b10 (64-bit)
    let bar0 = cfg.read(0x10);
    assert_eq!((bar0 >> 1) & 0x3, 0b10);

    // Sizing: write all-1s to both BAR0 and BAR1
    cfg.write(0x10, 0xFFFF_FFFF);
    cfg.write(0x14, 0xFFFF_FFFF);
    let lo = cfg.read(0x10);
    let hi = cfg.read(0x14);
    // 4GB BAR: low bits = 0xFFFF_FFF0 (preserving type), high = 0xFFFF_FFFF
    assert_eq!(lo & 0xFFFF_FFF0, 0xFFFF_FFF0);
    assert_eq!(hi, 0xFFFF_FFFF);
}

#[test]
fn config_space_class_code() {
    let bars = [BarDecl::Unused; 6];
    let cfg = PciConfigSpace::new(0x1234, 0x5678, 0x020000, 1, &bars, &[]);
    // Class code at offset 0x08: revision (low byte) + class (upper 3 bytes)
    let val = cfg.read(0x08);
    assert_eq!(val & 0xFF, 1); // revision
    assert_eq!(val >> 8, 0x020000 >> 0); // class_code occupies bits [31:8]
}

#[test]
fn config_space_header_type_is_type0() {
    let bars = [BarDecl::Unused; 6];
    let cfg = PciConfigSpace::new(0x1234, 0x5678, 0x020000, 0, &bars, &[]);
    // Header type at offset 0x0E (byte), within dword at 0x0C
    let val = cfg.read(0x0C);
    let header_type = (val >> 16) & 0xFF;
    assert_eq!(header_type, 0x00); // Type 0
}
```

Add `mod config;` to `crates/helm-device/src/tests/pci/mod.rs`.

**Step 2: Run test to verify it fails**

Run: `cargo test -p helm-device pci::config`
Expected: FAIL — `PciConfigSpace` not found

**Step 3: Write PciConfigSpace**

Create `crates/helm-device/src/pci/config.rs`:

```rust
//! PCI Type 0 config space engine (4KB PCIe extended).
//!
//! Manages the standard header, BAR sizing protocol, and capability
//! chain dispatch. Write masks prevent guest writes to read-only fields.

use super::traits::{BarDecl, PciCapability};

/// 4KB PCIe config space for one function.
pub struct PciConfigSpace {
    /// Raw backing store.
    data: [u8; 4096],
    /// Write mask per dword — 1 bits are writable by guest.
    write_mask: [u32; 1024],
    /// BAR sizes for the sizing protocol.
    bar_sizes: [u64; 6],
    /// Whether we're in the "sizing" state per BAR (wrote all-1s).
    bar_sizing: [bool; 6],
    /// Which BAR index is the upper half of a 64-bit BAR.
    bar_is_upper: [bool; 6],
}

impl Clone for PciConfigSpace {
    fn clone(&self) -> Self {
        Self {
            data: self.data,
            write_mask: self.write_mask,
            bar_sizes: self.bar_sizes,
            bar_sizing: self.bar_sizing,
            bar_is_upper: self.bar_is_upper,
        }
    }
}

impl PciConfigSpace {
    /// Build config space from PCI function identity.
    ///
    /// # Arguments
    /// * `vendor_id`, `device_id` — PCI identity
    /// * `class_code` — 24-bit class (class << 16 | subclass << 8 | progif)
    /// * `revision` — revision ID
    /// * `bars` — BAR declarations from PciFunction
    /// * `caps` — capability list (used to build the linked list)
    pub fn new(
        vendor_id: u16,
        device_id: u16,
        class_code: u32,
        revision: u8,
        bars: &[BarDecl; 6],
        caps: &[Box<dyn PciCapability>],
    ) -> Self {
        let mut s = Self {
            data: [0u8; 4096],
            write_mask: [0u32; 1024],
            bar_sizes: [0u64; 6],
            bar_sizing: [false; 6],
            bar_is_upper: [false; 6],
        };

        // Vendor ID (0x00) — read-only
        s.write_raw_u16(0x00, vendor_id);
        // Device ID (0x02) — read-only
        s.write_raw_u16(0x02, device_id);

        // Command (0x04) — writable: IO, Mem, BusMaster, INTx disable, etc.
        s.write_mask[1] = 0x0000_FFFF; // command is lower 16 bits of dword 1
        // Status (0x06) — mostly read-only, some W1C bits
        // For now allow W1C on bits [15:11]
        s.write_mask[1] |= 0xF800_0000;

        // Revision + Class Code (0x08) — read-only
        s.write_raw_u8(0x08, revision);
        s.write_raw_u8(0x09, (class_code & 0xFF) as u8);        // progif
        s.write_raw_u8(0x0A, ((class_code >> 8) & 0xFF) as u8); // subclass
        s.write_raw_u8(0x0B, ((class_code >> 16) & 0xFF) as u8); // class

        // Cache line size (0x0C) — writable
        s.write_mask[3] = 0x0000_00FF;
        // Latency timer (0x0D) — writable
        s.write_mask[3] |= 0x0000_FF00;
        // Header type (0x0E) — read-only, Type 0
        s.write_raw_u8(0x0E, 0x00);

        // Set up BARs
        for (i, bar) in bars.iter().enumerate() {
            s.bar_sizes[i] = bar.size();
            let bar_offset = 0x10 + (i as u16) * 4;

            match bar {
                BarDecl::Unused => {}
                BarDecl::Mmio32 { .. } => {
                    // Type bits [2:1] = 00 (32-bit), bit 0 = 0 (memory)
                    s.write_mask[4 + i] = 0xFFFF_FFF0; // address bits writable
                }
                BarDecl::Mmio64 { .. } => {
                    // Type bits [2:1] = 10 (64-bit), bit 0 = 0 (memory)
                    s.write_raw_u8(bar_offset as usize, 0x04); // bit 2 = 1 → type=64
                    s.write_mask[4 + i] = 0xFFFF_FFF0;
                    // Next BAR is upper 32 bits
                    if i + 1 < 6 {
                        s.bar_is_upper[i + 1] = true;
                        s.bar_sizes[i + 1] = bar.size();
                        s.write_mask[4 + i + 1] = 0xFFFF_FFFF;
                    }
                }
                BarDecl::Io { .. } => {
                    // Bit 0 = 1 (I/O space)
                    s.write_raw_u8(bar_offset as usize, 0x01);
                    s.write_mask[4 + i] = 0xFFFF_FFFC; // I/O address bits
                }
            }
        }

        // Capability pointer (0x34) — read-only, set if caps present
        if !caps.is_empty() {
            // Enable capabilities list bit in status register
            let status = s.read_raw_u16(0x06);
            s.write_raw_u16(0x06, status | (1 << 4)); // bit 4 = Capabilities List

            // Build linked list
            let mut prev_next_ptr_offset: Option<usize> = None;
            for cap in caps {
                let off = cap.offset() as usize;
                s.write_raw_u8(off, cap.cap_id());
                s.write_raw_u8(off + 1, 0); // next pointer (0 = end)

                if let Some(prev) = prev_next_ptr_offset {
                    s.write_raw_u8(prev, off as u8);
                } else {
                    // First capability — set capabilities pointer
                    s.write_raw_u8(0x34, off as u8);
                }
                prev_next_ptr_offset = Some(off + 1);
            }
        }

        // Interrupt line (0x3C) — writable
        s.write_mask[15] = 0x0000_00FF;

        s
    }

    /// Read a dword at the given byte offset (must be 4-aligned).
    pub fn read(&self, offset: u16) -> u32 {
        let off = (offset & 0xFFC) as usize;

        // BAR sizing protocol: if we're in sizing mode, return size mask
        let bar_idx = self.bar_index_for_offset(off);
        if let Some(idx) = bar_idx {
            if self.bar_sizing[idx] {
                return self.bar_size_mask(idx, off);
            }
        }

        self.read_raw_u32(off)
    }

    /// Write a dword at the given byte offset (must be 4-aligned).
    pub fn write(&mut self, offset: u16, value: u32) {
        let off = (offset & 0xFFC) as usize;
        let dword_idx = off / 4;

        // BAR sizing protocol
        if let Some(idx) = self.bar_index_for_offset(off) {
            if value == 0xFFFF_FFFF {
                self.bar_sizing[idx] = true;
                return;
            } else {
                self.bar_sizing[idx] = false;
            }
        }

        // Apply write mask
        if dword_idx < 1024 {
            let mask = self.write_mask[dword_idx];
            let old = self.read_raw_u32(off);
            let new = (old & !mask) | (value & mask);
            self.write_raw_u32(off, new);
        }
    }

    /// Get the current BAR address (as programmed by guest).
    pub fn bar_addr(&self, bar: usize) -> u64 {
        if bar >= 6 { return 0; }
        let off = 0x10 + bar * 4;
        let lo = self.read_raw_u32(off) as u64 & 0xFFFF_FFF0;
        if self.bar_is_upper[bar] {
            return 0; // this is an upper-half slot, not independently addressable
        }
        // Check if next BAR is the upper half of a 64-bit BAR
        if bar + 1 < 6 && self.bar_is_upper[bar + 1] {
            let hi = self.read_raw_u32(off + 4) as u64;
            return (hi << 32) | lo;
        }
        lo
    }

    // ── Internal helpers ──

    fn bar_index_for_offset(&self, off: usize) -> Option<usize> {
        if off >= 0x10 && off <= 0x24 {
            Some((off - 0x10) / 4)
        } else {
            None
        }
    }

    fn bar_size_mask(&self, idx: usize, off: usize) -> u32 {
        let size = self.bar_sizes[idx];
        if size == 0 { return 0; }

        if self.bar_is_upper[idx] {
            // Upper 32 bits of a 64-bit BAR
            let mask = !(size - 1);
            return (mask >> 32) as u32;
        }

        let mask = !(size - 1) as u32;
        // Preserve type bits in low nibble
        let type_bits = self.read_raw_u32(off) & 0x0F;
        (mask & 0xFFFF_FFF0) | type_bits
    }

    fn read_raw_u8(&self, off: usize) -> u8 {
        self.data[off]
    }

    fn write_raw_u8(&mut self, off: usize, val: u8) {
        self.data[off] = val;
    }

    fn read_raw_u16(&self, off: usize) -> u16 {
        u16::from_le_bytes([self.data[off], self.data[off + 1]])
    }

    fn write_raw_u16(&mut self, off: usize, val: u16) {
        let b = val.to_le_bytes();
        self.data[off] = b[0];
        self.data[off + 1] = b[1];
    }

    fn read_raw_u32(&self, off: usize) -> u32 {
        u32::from_le_bytes([
            self.data[off], self.data[off + 1],
            self.data[off + 2], self.data[off + 3],
        ])
    }

    fn write_raw_u32(&mut self, off: usize, val: u32) {
        let b = val.to_le_bytes();
        self.data[off..off + 4].copy_from_slice(&b);
    }
}
```

Add to `crates/helm-device/src/pci/mod.rs`:

```rust
mod config;
pub use config::PciConfigSpace;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p helm-device pci::config`
Expected: 6 tests PASS

**Step 5: Commit**

```bash
git add crates/helm-device/src/pci/config.rs crates/helm-device/src/pci/mod.rs crates/helm-device/src/tests/pci/
git commit -m "feat(pci): add PciConfigSpace engine with BAR sizing and write masks"
```

---

### Task 4: PCIe Capability

**Files:**
- Create: `crates/helm-device/src/pci/capability/mod.rs`
- Create: `crates/helm-device/src/pci/capability/pcie.rs`
- Test: `crates/helm-device/src/tests/pci/capability.rs`

**Step 1: Write the failing test**

Create `crates/helm-device/src/tests/pci/capability.rs`:

```rust
use crate::pci::capability::PcieCapability;
use crate::pci::PciCapability;

#[test]
fn pcie_cap_id() {
    let cap = PcieCapability::endpoint(0x40);
    assert_eq!(cap.cap_id(), 0x10);
}

#[test]
fn pcie_cap_offset_and_length() {
    let cap = PcieCapability::endpoint(0x40);
    assert_eq!(cap.offset(), 0x40);
    assert_eq!(cap.length(), 60); // PCIe cap is 60 bytes
}

#[test]
fn pcie_cap_device_capabilities() {
    let cap = PcieCapability::endpoint(0x40);
    // Device Capabilities at +0x04
    let dev_cap = cap.read(0x04);
    // Max payload size supported = 256 bytes (bits [2:0] = 1)
    assert_eq!(dev_cap & 0x7, 1);
}

#[test]
fn pcie_cap_link_status() {
    let cap = PcieCapability::endpoint(0x40);
    // Link Status at +0x12 (within dword at +0x10)
    let link = cap.read(0x10);
    let link_sta = (link >> 16) as u16;
    // Link speed = Gen3 (bits [3:0] = 3)
    assert_eq!(link_sta & 0xF, 3);
}

#[test]
fn pcie_cap_device_control_writable() {
    let mut cap = PcieCapability::endpoint(0x40);
    // Device Control at +0x08 (lower 16 bits of dword)
    cap.write(0x08, 0x0010); // Enable relaxed ordering
    let val = cap.read(0x08);
    assert_eq!(val & 0xFFFF, 0x0010);
}

#[test]
fn pcie_cap_reset() {
    let mut cap = PcieCapability::endpoint(0x40);
    cap.write(0x08, 0x0010);
    cap.reset();
    assert_eq!(cap.read(0x08) & 0xFFFF, 0); // device control zeroed
}

#[test]
fn pcie_cap_not_extended() {
    let cap = PcieCapability::endpoint(0x40);
    assert!(!cap.is_extended());
}
```

Add `mod capability;` to `crates/helm-device/src/tests/pci/mod.rs`.

**Step 2: Run test to verify it fails**

Run: `cargo test -p helm-device pci::capability`
Expected: FAIL — module not found

**Step 3: Write PCIe capability**

Create `crates/helm-device/src/pci/capability/mod.rs`:

```rust
//! PCI/PCIe capability implementations.

pub mod pcie;

pub use pcie::PcieCapability;
```

Create `crates/helm-device/src/pci/capability/pcie.rs`:

```rust
//! PCIe Capability (ID 0x10).
//!
//! Covers device/link/slot capabilities, control, and status registers
//! per PCIe Base Spec 5.0 Section 7.5.3.

use crate::pci::PciCapability;

/// PCIe device type (for PCIe Capabilities Register).
#[derive(Debug, Clone, Copy)]
pub enum PcieDeviceType {
    Endpoint = 0b0000,
    LegacyEndpoint = 0b0001,
    RootPort = 0b0100,
    UpstreamSwitch = 0b0101,
    DownstreamSwitch = 0b0110,
    RootComplex = 0b1001,
}

/// PCIe Capability structure (60 bytes).
pub struct PcieCapability {
    offset: u16,
    /// PCIe Capabilities Register (+0x02)
    pcie_cap: u16,
    /// Device Capabilities (+0x04)
    dev_cap: u32,
    /// Device Control (+0x08, lower 16)
    dev_ctl: u16,
    /// Device Status (+0x08, upper 16)
    dev_sta: u16,
    /// Link Capabilities (+0x0C)
    link_cap: u32,
    /// Link Control (+0x10, lower 16)
    link_ctl: u16,
    /// Link Status (+0x10, upper 16)
    link_sta: u16,
    /// Slot Capabilities (+0x14)
    slot_cap: u32,
    /// Slot Control (+0x18, lower 16)
    slot_ctl: u16,
    /// Slot Status (+0x18, upper 16)
    slot_sta: u16,
    /// Device Capabilities 2 (+0x24)
    dev_cap2: u32,
    /// Device Control 2 (+0x28, lower 16)
    dev_ctl2: u16,
    /// Link Capabilities 2 (+0x2C)
    link_cap2: u32,
    /// Link Control 2 (+0x30, lower 16)
    link_ctl2: u16,
    /// Link Status 2 (+0x30, upper 16)
    link_sta2: u16,
}

impl PcieCapability {
    /// Create a PCIe capability for an endpoint device.
    pub fn endpoint(offset: u16) -> Self {
        Self::new(offset, PcieDeviceType::Endpoint)
    }

    /// Create a PCIe capability for a root port.
    pub fn root_port(offset: u16) -> Self {
        Self::new(offset, PcieDeviceType::RootPort)
    }

    fn new(offset: u16, dev_type: PcieDeviceType) -> Self {
        // PCIe Capabilities Register: version=2, device type
        let pcie_cap = 0x0002 | ((dev_type as u16) << 4);

        // Device Capabilities: max payload 256B (1), phantom funcs=0
        let dev_cap = 0x0000_0001;

        // Link Capabilities: Gen3 (3), x1 width (1), ASPM L0s+L1
        let link_cap = 0x0001_0003 | (1 << 4); // max link width=1, speed=Gen3

        // Link Status: current speed=Gen3, width=x1
        let link_sta = 0x0013; // speed=3, width=1 in bits [9:4]

        Self {
            offset,
            pcie_cap,
            dev_cap,
            dev_ctl: 0,
            dev_sta: 0,
            link_cap,
            link_ctl: 0,
            link_sta,
            slot_cap: 0,
            slot_ctl: 0,
            slot_sta: 0,
            dev_cap2: 0,
            dev_ctl2: 0,
            link_cap2: 0,
            link_ctl2: 0,
            link_sta2: 0,
        }
    }
}

impl PciCapability for PcieCapability {
    fn cap_id(&self) -> u8 { 0x10 }
    fn offset(&self) -> u16 { self.offset }
    fn length(&self) -> u16 { 60 }

    fn read(&self, offset: u16) -> u32 {
        match offset {
            0x00 => (self.pcie_cap as u32) << 16 | 0x10, // cap ID + next=0 + pcie_cap
            0x04 => self.dev_cap,
            0x08 => (self.dev_sta as u32) << 16 | self.dev_ctl as u32,
            0x0C => self.link_cap,
            0x10 => (self.link_sta as u32) << 16 | self.link_ctl as u32,
            0x14 => self.slot_cap,
            0x18 => (self.slot_sta as u32) << 16 | self.slot_ctl as u32,
            0x24 => self.dev_cap2,
            0x28 => self.dev_ctl2 as u32,
            0x2C => self.link_cap2,
            0x30 => (self.link_sta2 as u32) << 16 | self.link_ctl2 as u32,
            _ => 0,
        }
    }

    fn write(&mut self, offset: u16, value: u32) {
        match offset {
            0x08 => {
                self.dev_ctl = value as u16;
                // Dev status bits are W1C
                self.dev_sta &= !((value >> 16) as u16);
            }
            0x10 => {
                self.link_ctl = value as u16;
            }
            0x18 => {
                self.slot_ctl = value as u16;
                self.slot_sta &= !((value >> 16) as u16);
            }
            0x28 => {
                self.dev_ctl2 = value as u16;
            }
            0x30 => {
                self.link_ctl2 = value as u16;
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.dev_ctl = 0;
        self.dev_sta = 0;
        self.link_ctl = 0;
        self.slot_ctl = 0;
        self.slot_sta = 0;
        self.dev_ctl2 = 0;
        self.link_ctl2 = 0;
        self.link_sta2 = 0;
    }

    fn name(&self) -> &str { "PCIe" }
}
```

Add to `crates/helm-device/src/pci/mod.rs`:

```rust
pub mod capability;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p helm-device pci::capability`
Expected: 7 tests PASS

**Step 5: Commit**

```bash
git add crates/helm-device/src/pci/capability/ crates/helm-device/src/pci/mod.rs crates/helm-device/src/tests/pci/
git commit -m "feat(pci): add PCIe capability (ID 0x10) with device/link/slot regs"
```

---

### Task 5: MSI-X Capability

**Files:**
- Create: `crates/helm-device/src/pci/capability/msix.rs`
- Modify: `crates/helm-device/src/pci/capability/mod.rs`
- Test: `crates/helm-device/src/tests/pci/msix.rs`

**Step 1: Write the failing test**

Create `crates/helm-device/src/tests/pci/msix.rs`:

```rust
use crate::pci::capability::MsixCapability;
use crate::pci::PciCapability;

#[test]
fn msix_cap_id() {
    let cap = MsixCapability::new(0x70, 4, 2, 0, 2, 0x1000);
    assert_eq!(cap.cap_id(), 0x11);
}

#[test]
fn msix_table_size() {
    let cap = MsixCapability::new(0x70, 4, 2, 0, 2, 0x1000);
    assert_eq!(cap.table_size(), 4);
}

#[test]
fn msix_enable_disable() {
    let mut cap = MsixCapability::new(0x70, 4, 2, 0, 2, 0x1000);
    assert!(!cap.is_enabled());
    // Message Control at +0x02 (within dword at +0x00): bit 15 = enable
    let val = cap.read(0x00);
    cap.write(0x00, val | (1 << 31)); // bit 31 of dword = bit 15 of upper 16
    assert!(cap.is_enabled());
}

#[test]
fn msix_vector_read_write() {
    let mut cap = MsixCapability::new(0x70, 4, 2, 0, 2, 0x1000);
    // Configure vector 0: addr=0x00000000_FEE00000, data=0x41
    cap.write_vector(0, 0xFEE0_0000, 0, 0x41);
    let v = cap.read_vector(0);
    assert_eq!(v.addr_lo, 0xFEE0_0000);
    assert_eq!(v.addr_hi, 0);
    assert_eq!(v.data, 0x41);
    assert!(!v.masked);
}

#[test]
fn msix_vector_mask() {
    let mut cap = MsixCapability::new(0x70, 2, 2, 0, 2, 0x1000);
    cap.write_vector(0, 0xFEE0_0000, 0, 0x41);
    cap.mask_vector(0, true);
    assert!(cap.read_vector(0).masked);
    cap.mask_vector(0, false);
    assert!(!cap.read_vector(0).masked);
}

#[test]
fn msix_fire_vector() {
    let mut cap = MsixCapability::new(0x70, 2, 2, 0, 2, 0x1000);
    cap.write_vector(0, 0xFEE0_0000, 0, 0x41);
    // Enable MSI-X
    let val = cap.read(0x00);
    cap.write(0x00, val | (1 << 31));
    // Fire — returns Some((addr, data))
    let result = cap.fire(0);
    assert!(result.is_some());
    let (addr, data) = result.unwrap();
    assert_eq!(addr, 0xFEE0_0000);
    assert_eq!(data, 0x41);
}

#[test]
fn msix_fire_masked_vector_sets_pending() {
    let mut cap = MsixCapability::new(0x70, 2, 2, 0, 2, 0x1000);
    cap.write_vector(0, 0xFEE0_0000, 0, 0x41);
    let val = cap.read(0x00);
    cap.write(0x00, val | (1 << 31)); // enable
    cap.mask_vector(0, true);
    let result = cap.fire(0);
    assert!(result.is_none()); // masked, not delivered
    assert!(cap.read_vector(0).pending);
}

#[test]
fn msix_reset_clears_all() {
    let mut cap = MsixCapability::new(0x70, 2, 2, 0, 2, 0x1000);
    cap.write_vector(0, 0xFEE0_0000, 0, 0x41);
    let val = cap.read(0x00);
    cap.write(0x00, val | (1 << 31));
    cap.reset();
    assert!(!cap.is_enabled());
    assert_eq!(cap.read_vector(0).data, 0);
}
```

Add `mod msix;` to `crates/helm-device/src/tests/pci/mod.rs`.

**Step 2: Run test to verify it fails**

Run: `cargo test -p helm-device pci::msix`
Expected: FAIL

**Step 3: Write MsixCapability**

Create `crates/helm-device/src/pci/capability/msix.rs`:

```rust
//! MSI-X Capability (ID 0x11).
//!
//! The MSI-X table and PBA live in BAR space. This struct tracks
//! per-vector state; the actual BAR reads/writes are forwarded by
//! the PciFunction implementation.

use crate::pci::PciCapability;

/// One MSI-X vector entry (16 bytes in the table).
#[derive(Debug, Clone, Default)]
pub struct MsixVector {
    pub addr_lo: u32,
    pub addr_hi: u32,
    pub data: u32,
    pub masked: bool,
    pub pending: bool,
}

/// MSI-X Capability.
pub struct MsixCapability {
    offset: u16,
    /// Number of vectors (1-based, stored as N-1 in hardware).
    table_size: u16,
    /// Which BAR holds the MSI-X table.
    table_bar: u8,
    /// Byte offset of the table within the BAR.
    table_offset: u32,
    /// Which BAR holds the PBA.
    pba_bar: u8,
    /// Byte offset of the PBA within the BAR.
    pba_offset: u32,
    /// MSI-X enabled.
    enabled: bool,
    /// Function mask (masks all vectors).
    function_mask: bool,
    /// Per-vector state.
    vectors: Vec<MsixVector>,
}

impl MsixCapability {
    /// Create a new MSI-X capability.
    ///
    /// # Arguments
    /// * `offset` — byte offset in config space
    /// * `num_vectors` — number of MSI-X vectors
    /// * `table_bar` — BAR index for the MSI-X table
    /// * `table_offset` — byte offset within the BAR
    /// * `pba_bar` — BAR index for the PBA
    /// * `pba_offset` — byte offset within the BAR
    pub fn new(
        offset: u16,
        num_vectors: u16,
        table_bar: u8,
        table_offset: u32,
        pba_bar: u8,
        pba_offset: u32,
    ) -> Self {
        Self {
            offset,
            table_size: num_vectors,
            table_bar,
            table_offset,
            pba_bar,
            pba_offset,
            enabled: false,
            function_mask: false,
            vectors: vec![MsixVector::default(); num_vectors as usize],
        }
    }

    pub fn table_size(&self) -> u16 { self.table_size }
    pub fn table_bar(&self) -> u8 { self.table_bar }
    pub fn table_offset(&self) -> u32 { self.table_offset }
    pub fn pba_bar(&self) -> u8 { self.pba_bar }
    pub fn pba_offset(&self) -> u32 { self.pba_offset }
    pub fn is_enabled(&self) -> bool { self.enabled }

    /// Write a vector entry.
    pub fn write_vector(&mut self, idx: u16, addr_lo: u32, addr_hi: u32, data: u32) {
        if let Some(v) = self.vectors.get_mut(idx as usize) {
            v.addr_lo = addr_lo;
            v.addr_hi = addr_hi;
            v.data = data;
        }
    }

    /// Read a vector entry.
    pub fn read_vector(&self, idx: u16) -> MsixVector {
        self.vectors.get(idx as usize).cloned().unwrap_or_default()
    }

    /// Mask or unmask a vector.
    pub fn mask_vector(&mut self, idx: u16, masked: bool) {
        if let Some(v) = self.vectors.get_mut(idx as usize) {
            v.masked = masked;
            // If unmasking and pending, the caller should re-fire
            if !masked && v.pending {
                v.pending = false;
            }
        }
    }

    /// Fire an MSI-X vector. Returns `Some((addr, data))` if the
    /// vector is enabled and not masked. If masked, sets pending.
    pub fn fire(&mut self, idx: u16) -> Option<(u64, u32)> {
        if !self.enabled { return None; }
        let v = self.vectors.get_mut(idx as usize)?;
        if v.masked || self.function_mask {
            v.pending = true;
            return None;
        }
        let addr = ((v.addr_hi as u64) << 32) | v.addr_lo as u64;
        Some((addr, v.data))
    }

    /// Handle a BAR read to the MSI-X table region.
    pub fn table_read(&self, offset: u64, _size: usize) -> u64 {
        let entry_idx = (offset / 16) as usize;
        let field = (offset % 16) as usize;
        if let Some(v) = self.vectors.get(entry_idx) {
            match field {
                0 => v.addr_lo as u64,
                4 => v.addr_hi as u64,
                8 => v.data as u64,
                12 => v.masked as u64,
                _ => 0,
            }
        } else {
            0
        }
    }

    /// Handle a BAR write to the MSI-X table region.
    pub fn table_write(&mut self, offset: u64, _size: usize, value: u64) {
        let entry_idx = (offset / 16) as usize;
        let field = (offset % 16) as usize;
        if let Some(v) = self.vectors.get_mut(entry_idx) {
            match field {
                0 => v.addr_lo = value as u32,
                4 => v.addr_hi = value as u32,
                8 => v.data = value as u32,
                12 => v.masked = (value & 1) != 0,
                _ => {}
            }
        }
    }

    /// Read the PBA (Pending Bit Array).
    pub fn pba_read(&self, offset: u64) -> u64 {
        let qword_idx = (offset / 8) as usize;
        let base_vec = qword_idx * 64;
        let mut bits: u64 = 0;
        for i in 0..64 {
            if let Some(v) = self.vectors.get(base_vec + i) {
                if v.pending {
                    bits |= 1 << i;
                }
            }
        }
        bits
    }
}

impl PciCapability for MsixCapability {
    fn cap_id(&self) -> u8 { 0x11 }
    fn offset(&self) -> u16 { self.offset }
    fn length(&self) -> u16 { 12 } // MSI-X cap is 12 bytes in config space

    fn read(&self, offset: u16) -> u32 {
        match offset {
            0x00 => {
                // Cap ID (byte 0) + next (byte 1) + Message Control (bytes 2-3)
                let msg_ctl = ((self.table_size - 1) as u32)
                    | if self.function_mask { 1 << 14 } else { 0 }
                    | if self.enabled { 1 << 15 } else { 0 };
                (msg_ctl << 16) | 0x11
            }
            0x04 => {
                // Table Offset / BIR
                self.table_offset | self.table_bar as u32
            }
            0x08 => {
                // PBA Offset / BIR
                self.pba_offset | self.pba_bar as u32
            }
            _ => 0,
        }
    }

    fn write(&mut self, offset: u16, value: u32) {
        match offset {
            0x00 => {
                // Message Control (upper 16 bits)
                let msg_ctl = (value >> 16) as u16;
                self.enabled = (msg_ctl & (1 << 15)) != 0;
                self.function_mask = (msg_ctl & (1 << 14)) != 0;
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.enabled = false;
        self.function_mask = false;
        for v in &mut self.vectors {
            *v = MsixVector::default();
        }
    }

    fn name(&self) -> &str { "MSI-X" }
}
```

Add to `crates/helm-device/src/pci/capability/mod.rs`:

```rust
pub mod msix;
pub use msix::{MsixCapability, MsixVector};
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p helm-device pci::msix`
Expected: 8 tests PASS

**Step 5: Commit**

```bash
git add crates/helm-device/src/pci/capability/ crates/helm-device/src/tests/pci/
git commit -m "feat(pci): add MSI-X capability with vector table and PBA"
```

---

### Task 6: PM, AER, ACS Capabilities

**Files:**
- Create: `crates/helm-device/src/pci/capability/pm.rs`
- Create: `crates/helm-device/src/pci/capability/aer.rs`
- Create: `crates/helm-device/src/pci/capability/acs.rs`
- Modify: `crates/helm-device/src/pci/capability/mod.rs`
- Test: `crates/helm-device/src/tests/pci/caps_pm_aer_acs.rs`

These three are smaller capabilities. Group them in one task.

**Step 1: Write the failing test**

Create `crates/helm-device/src/tests/pci/caps_pm_aer_acs.rs`:

```rust
use crate::pci::capability::{PmCapability, AerCapability, AcsCapability};
use crate::pci::PciCapability;

// ── PM ──

#[test]
fn pm_cap_id() {
    let cap = PmCapability::new(0x50);
    assert_eq!(cap.cap_id(), 0x01);
}

#[test]
fn pm_power_state_writable() {
    let mut cap = PmCapability::new(0x50);
    // PM CSR at +0x04: power state in bits [1:0]
    cap.write(0x04, 0x0003); // D3hot
    assert_eq!(cap.read(0x04) & 0x3, 3);
}

#[test]
fn pm_reset() {
    let mut cap = PmCapability::new(0x50);
    cap.write(0x04, 0x0003);
    cap.reset();
    assert_eq!(cap.read(0x04) & 0x3, 0); // D0
}

// ── AER ──

#[test]
fn aer_is_extended() {
    let cap = AerCapability::new(0x100);
    assert!(cap.is_extended());
    assert_eq!(cap.cap_id(), 0x01); // AER extended cap ID
}

#[test]
fn aer_uncorrectable_status_w1c() {
    let mut cap = AerCapability::new(0x100);
    // Inject an error
    cap.inject_uncorrectable(1 << 4); // Data Link Protocol Error
    assert_ne!(cap.read(0x04) & (1 << 4), 0);
    // W1C to clear
    cap.write(0x04, 1 << 4);
    assert_eq!(cap.read(0x04) & (1 << 4), 0);
}

#[test]
fn aer_uncorrectable_mask_writable() {
    let mut cap = AerCapability::new(0x100);
    cap.write(0x08, 0xFFFF_FFFF); // mask all
    assert_eq!(cap.read(0x08), 0xFFFF_FFFF);
}

// ── ACS ──

#[test]
fn acs_is_extended() {
    let cap = AcsCapability::new(0x140);
    assert!(cap.is_extended());
    assert_eq!(cap.cap_id(), 0x0D);
}

#[test]
fn acs_control_writable() {
    let mut cap = AcsCapability::new(0x140);
    // ACS Control at +0x06 (within dword at +0x04)
    cap.write(0x04, 0x001F_0000); // enable bits in upper 16
    let val = cap.read(0x04);
    assert_eq!((val >> 16) & 0x1F, 0x1F);
}
```

Add `mod caps_pm_aer_acs;` to `crates/helm-device/src/tests/pci/mod.rs`.

**Step 2: Run test to verify it fails**

Run: `cargo test -p helm-device pci::caps_pm_aer_acs`
Expected: FAIL

**Step 3: Write PM, AER, ACS capabilities**

Create `crates/helm-device/src/pci/capability/pm.rs`:

```rust
//! Power Management Capability (ID 0x01).

use crate::pci::PciCapability;

pub struct PmCapability {
    offset: u16,
    /// PM Capabilities register (+0x02).
    pm_cap: u16,
    /// PM Control/Status register (+0x04).
    pm_csr: u32,
}

impl PmCapability {
    pub fn new(offset: u16) -> Self {
        Self {
            offset,
            // PM cap: version 3, D1/D2 not supported, PME from D3hot
            pm_cap: 0x0003 | (1 << 11),
            pm_csr: 0,
        }
    }
}

impl PciCapability for PmCapability {
    fn cap_id(&self) -> u8 { 0x01 }
    fn offset(&self) -> u16 { self.offset }
    fn length(&self) -> u16 { 8 }

    fn read(&self, offset: u16) -> u32 {
        match offset {
            0x00 => (self.pm_cap as u32) << 16 | 0x01,
            0x04 => self.pm_csr,
            _ => 0,
        }
    }

    fn write(&mut self, offset: u16, value: u32) {
        if offset == 0x04 {
            // Power state bits [1:0] writable, PME_Status bit [15] W1C
            self.pm_csr = (self.pm_csr & !0x8003) | (value & 0x0003);
            if value & (1 << 15) != 0 {
                self.pm_csr &= !(1 << 15);
            }
        }
    }

    fn reset(&mut self) { self.pm_csr = 0; }
    fn name(&self) -> &str { "PM" }
}
```

Create `crates/helm-device/src/pci/capability/aer.rs`:

```rust
//! Advanced Error Reporting Extended Capability (ID 0x0001).

use crate::pci::PciCapability;

pub struct AerCapability {
    offset: u16,
    uncorrectable_status: u32,
    uncorrectable_mask: u32,
    uncorrectable_severity: u32,
    correctable_status: u32,
    correctable_mask: u32,
    cap_control: u32,
}

impl AerCapability {
    pub fn new(offset: u16) -> Self {
        Self {
            offset,
            uncorrectable_status: 0,
            uncorrectable_mask: 0,
            uncorrectable_severity: 0x0006_2030, // default severities per spec
            correctable_status: 0,
            correctable_mask: 0,
            cap_control: 0,
        }
    }

    /// Inject an uncorrectable error (set bits in status).
    pub fn inject_uncorrectable(&mut self, bits: u32) {
        self.uncorrectable_status |= bits;
    }

    /// Inject a correctable error.
    pub fn inject_correctable(&mut self, bits: u32) {
        self.correctable_status |= bits;
    }
}

impl PciCapability for AerCapability {
    fn cap_id(&self) -> u8 { 0x01 } // extended cap ID
    fn offset(&self) -> u16 { self.offset }
    fn length(&self) -> u16 { 48 }

    fn read(&self, offset: u16) -> u32 {
        match offset {
            0x00 => 0x0001 | (1 << 16), // cap ID=0x0001, version=1
            0x04 => self.uncorrectable_status,
            0x08 => self.uncorrectable_mask,
            0x0C => self.uncorrectable_severity,
            0x10 => self.correctable_status,
            0x14 => self.correctable_mask,
            0x18 => self.cap_control,
            _ => 0,
        }
    }

    fn write(&mut self, offset: u16, value: u32) {
        match offset {
            0x04 => self.uncorrectable_status &= !value, // W1C
            0x08 => self.uncorrectable_mask = value,
            0x0C => self.uncorrectable_severity = value,
            0x10 => self.correctable_status &= !value, // W1C
            0x14 => self.correctable_mask = value,
            0x18 => self.cap_control = value,
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.uncorrectable_status = 0;
        self.correctable_status = 0;
        self.uncorrectable_mask = 0;
        self.correctable_mask = 0;
        self.cap_control = 0;
    }

    fn name(&self) -> &str { "AER" }
}
```

Create `crates/helm-device/src/pci/capability/acs.rs`:

```rust
//! Access Control Services Extended Capability (ID 0x000D).

use crate::pci::PciCapability;

pub struct AcsCapability {
    offset: u16,
    /// ACS Capability (lower 16 of dword at +0x04).
    acs_cap: u16,
    /// ACS Control (upper 16 of dword at +0x04).
    acs_ctl: u16,
}

impl AcsCapability {
    pub fn new(offset: u16) -> Self {
        Self {
            offset,
            // Capability bits: SV, TB, RR, CR, UF (bits 0-4)
            acs_cap: 0x001F,
            acs_ctl: 0,
        }
    }
}

impl PciCapability for AcsCapability {
    fn cap_id(&self) -> u8 { 0x0D }
    fn offset(&self) -> u16 { self.offset }
    fn length(&self) -> u16 { 8 }

    fn read(&self, offset: u16) -> u32 {
        match offset {
            0x00 => 0x000D | (1 << 16), // cap ID=0x000D, version=1
            0x04 => (self.acs_ctl as u32) << 16 | self.acs_cap as u32,
            _ => 0,
        }
    }

    fn write(&mut self, offset: u16, value: u32) {
        if offset == 0x04 {
            self.acs_ctl = (value >> 16) as u16;
        }
    }

    fn reset(&mut self) { self.acs_ctl = 0; }
    fn name(&self) -> &str { "ACS" }
}
```

Add to `crates/helm-device/src/pci/capability/mod.rs`:

```rust
pub mod pm;
pub mod aer;
pub mod acs;

pub use pm::PmCapability;
pub use aer::AerCapability;
pub use acs::AcsCapability;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p helm-device pci::caps_pm_aer_acs`
Expected: 8 tests PASS

**Step 5: Commit**

```bash
git add crates/helm-device/src/pci/capability/ crates/helm-device/src/tests/pci/
git commit -m "feat(pci): add PM, AER, ACS capabilities"
```

---

### Task 7: PciBus

**Files:**
- Create: `crates/helm-device/src/pci/bus.rs`
- Modify: `crates/helm-device/src/pci/mod.rs`
- Test: `crates/helm-device/src/tests/pci/bus.rs`

**Step 1: Write the failing test**

Create `crates/helm-device/src/tests/pci/bus.rs` — tests for PciBus attach, config routing, and BAR MMIO dispatch. Use the DummyPciFunction from Task 1 tests (extract to a shared helper module).

Create `crates/helm-device/src/tests/pci/helpers.rs` (extract DummyPciFunction):

```rust
use crate::pci::*;
use crate::device::DeviceEvent;

pub struct DummyPciFunction {
    pub bars: [BarDecl; 6],
    pub caps: Vec<Box<dyn PciCapability>>,
    pub registers: [u64; 4],
}

impl DummyPciFunction {
    pub fn new() -> Self {
        Self {
            bars: [
                BarDecl::Mmio32 { size: 0x1000 },
                BarDecl::Unused, BarDecl::Unused,
                BarDecl::Unused, BarDecl::Unused, BarDecl::Unused,
            ],
            caps: Vec::new(),
            registers: [0; 4],
        }
    }
}

impl PciFunction for DummyPciFunction {
    fn vendor_id(&self) -> u16 { 0x1DE5 }
    fn device_id(&self) -> u16 { 0x0001 }
    fn class_code(&self) -> u32 { 0x020000 }
    fn bars(&self) -> &[BarDecl; 6] { &self.bars }
    fn capabilities(&self) -> &[Box<dyn PciCapability>] { &self.caps }
    fn capabilities_mut(&mut self) -> &mut Vec<Box<dyn PciCapability>> { &mut self.caps }

    fn bar_read(&mut self, _bar: u8, offset: u64, _size: usize) -> u64 {
        let idx = (offset / 8) as usize;
        self.registers.get(idx).copied().unwrap_or(0)
    }

    fn bar_write(&mut self, _bar: u8, offset: u64, _size: usize, value: u64) {
        let idx = (offset / 8) as usize;
        if let Some(r) = self.registers.get_mut(idx) { *r = value; }
    }

    fn reset(&mut self) { self.registers = [0; 4]; }
    fn name(&self) -> &str { "dummy-pci" }
}
```

Create `crates/helm-device/src/tests/pci/bus.rs`:

```rust
use crate::pci::*;
use super::helpers::DummyPciFunction;

#[test]
fn bus_attach_and_enumerate() {
    let mut bus = PciBus::new(0);
    bus.attach(0, 0, Box::new(DummyPciFunction::new()));
    bus.attach(1, 0, Box::new(DummyPciFunction::new()));
    let bdfs = bus.enumerate();
    assert_eq!(bdfs.len(), 2);
    assert_eq!(bdfs[0], Bdf { bus: 0, device: 0, function: 0 });
    assert_eq!(bdfs[1], Bdf { bus: 0, device: 1, function: 0 });
}

#[test]
fn bus_config_read_vendor_id() {
    let mut bus = PciBus::new(0);
    bus.attach(0, 0, Box::new(DummyPciFunction::new()));
    let bdf = Bdf { bus: 0, device: 0, function: 0 };
    let val = bus.config_read(bdf, 0x00);
    assert_eq!(val & 0xFFFF, 0x1DE5); // vendor ID
}

#[test]
fn bus_config_read_empty_slot_returns_ffff() {
    let bus = PciBus::new(0);
    let bdf = Bdf { bus: 0, device: 31, function: 0 };
    assert_eq!(bus.config_read(bdf, 0x00), 0xFFFF_FFFF);
}

#[test]
fn bus_config_write_then_read() {
    let mut bus = PciBus::new(0);
    bus.attach(0, 0, Box::new(DummyPciFunction::new()));
    let bdf = Bdf { bus: 0, device: 0, function: 0 };
    // Write command register
    bus.config_write(bdf, 0x04, 0x0006);
    let val = bus.config_read(bdf, 0x04);
    assert_eq!(val & 0x6, 0x6);
}

#[test]
fn bus_bar_read_write() {
    let mut bus = PciBus::new(0);
    bus.attach(0, 0, Box::new(DummyPciFunction::new()));
    // Allocate BAR0 at address 0x1000_0000
    bus.set_bar_mapping(Bdf { bus: 0, device: 0, function: 0 }, 0, 0x1000_0000, 0x1000);

    bus.bar_write(0x1000_0000, 8, 0xCAFE);
    let val = bus.bar_read(0x1000_0000, 8);
    assert_eq!(val, Some(0xCAFE));
}

#[test]
fn bus_bar_read_unmapped_returns_none() {
    let bus = PciBus::new(0);
    assert_eq!(bus.bar_read(0x2000_0000, 4), None);
}

#[test]
fn bus_reset_all() {
    let mut bus = PciBus::new(0);
    bus.attach(0, 0, Box::new(DummyPciFunction::new()));
    bus.set_bar_mapping(Bdf { bus: 0, device: 0, function: 0 }, 0, 0x1000_0000, 0x1000);
    bus.bar_write(0x1000_0000, 8, 42);
    bus.reset_all();
    assert_eq!(bus.bar_read(0x1000_0000, 8), Some(0)); // reset cleared it
}
```

Update `crates/helm-device/src/tests/pci/mod.rs`:

```rust
mod helpers;
mod traits;
mod bdf;
mod config;
mod capability;
mod msix;
mod caps_pm_aer_acs;
mod bus;
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p helm-device pci::bus`
Expected: FAIL — `PciBus` not found

**Step 3: Write PciBus**

Create `crates/helm-device/src/pci/bus.rs`:

```rust
//! PCI bus — topology manager with BDF-based device routing.

use std::collections::HashMap;
use super::bdf::Bdf;
use super::config::PciConfigSpace;
use super::traits::PciFunction;
use crate::device::DeviceEvent;

/// Resolved BAR mapping in platform address space.
#[derive(Debug, Clone)]
struct BarMapping {
    base: u64,
    size: u64,
    bar_index: u8,
    bdf: Bdf,
}

/// One populated PCI slot.
struct PciSlot {
    function: Box<dyn PciFunction>,
    config: PciConfigSpace,
}

/// PCI bus segment — routes config and BAR accesses by BDF.
pub struct PciBus {
    bus_number: u8,
    slots: HashMap<(u8, u8), PciSlot>,
    bar_mappings: Vec<BarMapping>,
}

impl PciBus {
    pub fn new(bus_number: u8) -> Self {
        Self {
            bus_number,
            slots: HashMap::new(),
            bar_mappings: Vec::new(),
        }
    }

    /// Attach a PCI function at (device, function).
    pub fn attach(&mut self, device: u8, function: u8, func: Box<dyn PciFunction>) {
        let config = PciConfigSpace::new(
            func.vendor_id(),
            func.device_id(),
            func.class_code(),
            func.revision_id(),
            func.bars(),
            func.capabilities(),
        );
        self.slots.insert((device, function), PciSlot { function: func, config });
    }

    /// Set a BAR's resolved address in platform space.
    pub fn set_bar_mapping(&mut self, bdf: Bdf, bar_index: u8, base: u64, size: u64) {
        // Remove any existing mapping for this BAR
        self.bar_mappings.retain(|m| !(m.bdf == bdf && m.bar_index == bar_index));
        self.bar_mappings.push(BarMapping { base, size, bar_index, bdf });
    }

    /// Read from PCI config space.
    pub fn config_read(&self, bdf: Bdf, offset: u16) -> u32 {
        if bdf.bus != self.bus_number {
            return 0xFFFF_FFFF;
        }
        match self.slots.get(&(bdf.device, bdf.function)) {
            Some(slot) => slot.config.read(offset),
            None => 0xFFFF_FFFF,
        }
    }

    /// Write to PCI config space.
    pub fn config_write(&mut self, bdf: Bdf, offset: u16, value: u32) {
        if bdf.bus != self.bus_number {
            return;
        }
        if let Some(slot) = self.slots.get_mut(&(bdf.device, bdf.function)) {
            slot.config.write(offset, value);
        }
    }

    /// Read from a BAR-mapped MMIO address.
    pub fn bar_read(&mut self, addr: u64, size: usize) -> Option<u64> {
        let mapping = self.bar_mappings.iter().find(|m| {
            addr >= m.base && addr < m.base + m.size
        })?.clone();

        let slot = self.slots.get_mut(&(mapping.bdf.device, mapping.bdf.function))?;
        let offset = addr - mapping.base;
        Some(slot.function.bar_read(mapping.bar_index, offset, size))
    }

    /// Write to a BAR-mapped MMIO address.
    pub fn bar_write(&mut self, addr: u64, size: usize, value: u64) -> bool {
        let mapping = match self.bar_mappings.iter().find(|m| {
            addr >= m.base && addr < m.base + m.size
        }) {
            Some(m) => m.clone(),
            None => return false,
        };

        if let Some(slot) = self.slots.get_mut(&(mapping.bdf.device, mapping.bdf.function)) {
            let offset = addr - mapping.base;
            slot.function.bar_write(mapping.bar_index, offset, size, value);
            true
        } else {
            false
        }
    }

    /// Enumerate all populated BDFs.
    pub fn enumerate(&self) -> Vec<Bdf> {
        let mut bdfs: Vec<Bdf> = self.slots.keys()
            .map(|&(d, f)| Bdf { bus: self.bus_number, device: d, function: f })
            .collect();
        bdfs.sort_by_key(|b| (b.device, b.function));
        bdfs
    }

    /// Tick all functions.
    pub fn tick_all(&mut self, cycles: u64) -> Vec<DeviceEvent> {
        let mut events = Vec::new();
        for slot in self.slots.values_mut() {
            events.extend(slot.function.tick(cycles));
        }
        events
    }

    /// Reset all functions.
    pub fn reset_all(&mut self) {
        for slot in self.slots.values_mut() {
            slot.function.reset();
        }
    }
}
```

Add to `crates/helm-device/src/pci/mod.rs`:

```rust
mod bus;
pub use bus::PciBus;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p helm-device pci::bus`
Expected: 7 tests PASS

**Step 5: Commit**

```bash
git add crates/helm-device/src/pci/bus.rs crates/helm-device/src/pci/mod.rs crates/helm-device/src/tests/pci/
git commit -m "feat(pci): add PciBus with BDF routing, config dispatch, BAR MMIO"
```

---

### Task 8: PCI Host Bridge (Device impl)

**Files:**
- Create: `crates/helm-device/src/pci/host.rs`
- Modify: `crates/helm-device/src/pci/mod.rs`
- Test: `crates/helm-device/src/tests/pci/host.rs`

**Step 1: Write the failing test**

Create `crates/helm-device/src/tests/pci/host.rs`:

```rust
use crate::pci::*;
use crate::device::Device;
use crate::transaction::Transaction;
use super::helpers::DummyPciFunction;

#[test]
fn host_bridge_ecam_read_vendor_id() {
    let mut host = PciHostBridge::new(
        0x3F00_0000,   // ecam_base
        0x0100_0000,   // ecam_size (16MB)
        0x1000_0000,   // mmio32_base
        0x2EFF_0000,   // mmio32_size
    );
    host.attach(0, 0, Box::new(DummyPciFunction::new()));

    // ECAM read: bus=0, dev=0, fn=0, offset=0x00
    let mut txn = Transaction::read(0x3F00_0000, 4);
    txn.offset = 0; // offset within ECAM region
    host.transact(&mut txn).unwrap();
    assert_eq!(txn.data_u32() & 0xFFFF, 0x1DE5);
}

#[test]
fn host_bridge_ecam_empty_slot_reads_ffff() {
    let mut host = PciHostBridge::new(
        0x3F00_0000, 0x0100_0000, 0x1000_0000, 0x2EFF_0000,
    );
    // dev=31, fn=0 — no device
    let ecam_off = (31u64 << 15);
    let mut txn = Transaction::read(0x3F00_0000 + ecam_off, 4);
    txn.offset = ecam_off;
    host.transact(&mut txn).unwrap();
    assert_eq!(txn.data_u32(), 0xFFFF_FFFF);
}

#[test]
fn host_bridge_bar_auto_allocation() {
    let mut host = PciHostBridge::new(
        0x3F00_0000, 0x0100_0000, 0x1000_0000, 0x2EFF_0000,
    );
    host.attach(0, 0, Box::new(DummyPciFunction::new())); // 4KB BAR0

    // BAR0 should be allocated at mmio32_base (0x1000_0000)
    let bar0_addr = host.bar_address(Bdf { bus: 0, device: 0, function: 0 }, 0);
    assert_eq!(bar0_addr, Some(0x1000_0000));
}

#[test]
fn host_bridge_mmio_read_write() {
    let mut host = PciHostBridge::new(
        0x3F00_0000, 0x0100_0000, 0x1000_0000, 0x2EFF_0000,
    );
    host.attach(0, 0, Box::new(DummyPciFunction::new()));

    let bar_base = host.bar_address(Bdf { bus: 0, device: 0, function: 0 }, 0).unwrap();

    // Write via MMIO window
    let mut txn = Transaction::write(bar_base, 8, 0xBEEF);
    txn.offset = bar_base - 0x1000_0000; // offset within MMIO region
    host.mmio_write(txn.offset, 8, 0xBEEF);

    // Read back
    let val = host.mmio_read(txn.offset, 8);
    assert_eq!(val, Some(0xBEEF));
}

#[test]
fn host_bridge_multiple_devices_bar_allocation() {
    let mut host = PciHostBridge::new(
        0x3F00_0000, 0x0100_0000, 0x1000_0000, 0x2EFF_0000,
    );
    host.attach(0, 0, Box::new(DummyPciFunction::new())); // 4KB BAR0
    host.attach(1, 0, Box::new(DummyPciFunction::new())); // 4KB BAR0

    let bar0 = host.bar_address(Bdf { bus: 0, device: 0, function: 0 }, 0).unwrap();
    let bar1 = host.bar_address(Bdf { bus: 0, device: 1, function: 0 }, 0).unwrap();

    // Second device should be allocated after first
    assert!(bar1 > bar0);
    assert!(bar1 >= bar0 + 0x1000); // at least 4KB apart
}

#[test]
fn host_bridge_is_device() {
    let host = PciHostBridge::new(
        0x3F00_0000, 0x0100_0000, 0x1000_0000, 0x2EFF_0000,
    );
    assert_eq!(host.name(), "pci-host-bridge");
    assert!(!host.regions().is_empty());
}
```

Add `mod host;` to `crates/helm-device/src/tests/pci/mod.rs`.

**Step 2: Run test to verify it fails**

Run: `cargo test -p helm-device pci::host`
Expected: FAIL

**Step 3: Write PciHostBridge**

Create `crates/helm-device/src/pci/host.rs`:

```rust
//! PCIe host bridge with ECAM config access.
//!
//! Implements `Device` — placed on the platform's system bus.
//! Two logical regions:
//!   1. ECAM config window (e.g. 0x3F00_0000, 16MB)
//!   2. PCI MMIO window (e.g. 0x1000_0000, ~768MB for 32-bit BARs)
//!
//! On `attach()`, auto-allocates BAR addresses from the MMIO pool.

use super::bdf::Bdf;
use super::bus::PciBus;
use super::traits::{BarDecl, PciFunction};
use crate::device::{Device, DeviceEvent};
use crate::region::{MemRegion, RegionKind};
use crate::transaction::Transaction;
use helm_core::HelmResult;

/// PCIe host bridge.
pub struct PciHostBridge {
    bus: PciBus,

    ecam_base: u64,
    ecam_size: u64,

    mmio32_base: u64,
    mmio32_size: u64,
    mmio32_next: u64,

    regions: Vec<MemRegion>,
}

impl PciHostBridge {
    pub fn new(ecam_base: u64, ecam_size: u64, mmio32_base: u64, mmio32_size: u64) -> Self {
        let regions = vec![
            MemRegion {
                name: "pci-ecam".to_string(),
                base: ecam_base,
                size: ecam_size,
                kind: RegionKind::Io,
                priority: 0,
            },
            MemRegion {
                name: "pci-mmio".to_string(),
                base: mmio32_base,
                size: mmio32_size,
                kind: RegionKind::Io,
                priority: 0,
            },
        ];

        Self {
            bus: PciBus::new(0),
            ecam_base,
            ecam_size,
            mmio32_base,
            mmio32_size,
            mmio32_next: mmio32_base,
            regions,
        }
    }

    /// Attach a PCI function and auto-allocate BARs.
    pub fn attach(&mut self, device: u8, function: u8, func: Box<dyn PciFunction>) {
        let bdf = Bdf { bus: 0, device, function };
        let bars = func.bars().clone();
        self.bus.attach(device, function, func);

        // Auto-allocate BARs
        for (i, bar) in bars.iter().enumerate() {
            let size = bar.size();
            if size == 0 { continue; }
            if bar.is_64bit() && i + 1 >= 6 { continue; }

            // Align to BAR size (power of 2)
            let align = size;
            self.mmio32_next = (self.mmio32_next + align - 1) & !(align - 1);
            let base = self.mmio32_next;
            self.mmio32_next += size;

            self.bus.set_bar_mapping(bdf, i as u8, base, size);
        }
    }

    /// Get the allocated BAR address for a function.
    pub fn bar_address(&self, bdf: Bdf, bar_index: u8) -> Option<u64> {
        // Walk the bus's internal bar_mappings
        self.bus.bar_address(bdf, bar_index)
    }

    /// MMIO read from the PCI MMIO window.
    pub fn mmio_read(&mut self, offset: u64, size: usize) -> Option<u64> {
        self.bus.bar_read(self.mmio32_base + offset, size)
    }

    /// MMIO write to the PCI MMIO window.
    pub fn mmio_write(&mut self, offset: u64, size: usize, value: u64) -> bool {
        self.bus.bar_write(self.mmio32_base + offset, size, value)
    }

    /// Enumerate all attached devices.
    pub fn enumerate(&self) -> Vec<Bdf> {
        self.bus.enumerate()
    }
}

impl Device for PciHostBridge {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        let addr = txn.addr;

        // Is this an ECAM access?
        if addr >= self.ecam_base && addr < self.ecam_base + self.ecam_size {
            let ecam_off = addr - self.ecam_base;
            let (bdf, reg) = Bdf::from_ecam_offset(ecam_off);

            if txn.is_write {
                let value = txn.data_u32();
                self.bus.config_write(bdf, reg, value);
            } else {
                let value = self.bus.config_read(bdf, reg);
                txn.set_data_u32(value);
            }
            txn.stall_cycles += 1;
            return Ok(());
        }

        // Is this a BAR MMIO access?
        if addr >= self.mmio32_base && addr < self.mmio32_base + self.mmio32_size {
            if txn.is_write {
                let value = txn.data_u64();
                self.bus.bar_write(addr, txn.size, value);
            } else {
                let value = self.bus.bar_read(addr, txn.size).unwrap_or(0);
                txn.set_data_u64(value);
            }
            txn.stall_cycles += 1;
            return Ok(());
        }

        Ok(())
    }

    fn regions(&self) -> &[MemRegion] {
        &self.regions
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.bus.reset_all();
        Ok(())
    }

    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        Ok(self.bus.tick_all(cycles))
    }

    fn name(&self) -> &str {
        "pci-host-bridge"
    }
}
```

This requires adding a `bar_address` method to `PciBus`. Add to `crates/helm-device/src/pci/bus.rs`:

```rust
/// Get the base address of a BAR mapping.
pub fn bar_address(&self, bdf: Bdf, bar_index: u8) -> Option<u64> {
    self.bar_mappings.iter()
        .find(|m| m.bdf == bdf && m.bar_index == bar_index)
        .map(|m| m.base)
}
```

Add to `crates/helm-device/src/pci/mod.rs`:

```rust
mod host;
pub use host::PciHostBridge;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p helm-device pci::host`
Expected: 6 tests PASS

**Step 5: Commit**

```bash
git add crates/helm-device/src/pci/ crates/helm-device/src/tests/pci/
git commit -m "feat(pci): add PciHostBridge with ECAM, BAR auto-allocation, Device impl"
```

---

### Task 9: VirtIO PCI Capability Structures

**Files:**
- Create: `crates/helm-device/src/pci/capability/virtio_pci_cap.rs`
- Modify: `crates/helm-device/src/pci/capability/mod.rs`
- Test: `crates/helm-device/src/tests/pci/virtio_pci_cap.rs`

Vendor-specific capabilities (ID 0x09) that point VirtIO drivers to their config regions within BARs.

Test, implement, commit following the same TDD pattern as Tasks 4-6. Five cap types: COMMON_CFG, NOTIFY_CFG, ISR_CFG, DEVICE_CFG, PCI_CFG.

---

### Task 10: VirtIO PCI Transport

**Files:**
- Create: `crates/helm-device/src/pci/transport.rs`
- Modify: `crates/helm-device/src/pci/mod.rs`
- Modify: `crates/helm-device/src/virtio/transport.rs` (add VirtioTransport impl to VirtioMmioTransport)
- Modify: `crates/helm-device/src/virtio/mod.rs` (re-export VirtioTransport)
- Test: `crates/helm-device/src/tests/pci/transport.rs`

Implements `PciFunction` + the new `VirtioTransport` trait. Wraps any existing `VirtioDeviceBackend`. BAR0 layout: common cfg (0x00-0x37), ISR (0x38-0x3B), notify (0x3C-0x7F), device cfg (0x80+). BAR4: MSI-X.

Tests mirror the existing VirtIO MMIO transport tests (`tests/virtio/transport.rs`) but via PCI BAR reads/writes instead of MMIO register offsets.

---

### Task 11: VirtioTransport Trait on VirtioMmioTransport

**Files:**
- Modify: `crates/helm-device/src/virtio/transport.rs`
- Create: `crates/helm-device/src/pci/virtio_transport_trait.rs` (trait definition)
- Test: existing tests should still pass

Add `impl VirtioTransport for VirtioMmioTransport` (trivial delegation to existing methods). Ensure `cargo test -p helm-device` still passes.

---

### Task 12: Accelerator PCI Function

**Files:**
- Create: `crates/helm-device/src/pci/accel.rs`
- Modify: `crates/helm-device/src/pci/mod.rs`
- Modify: `crates/helm-device/Cargo.toml` (add `helm-llvm` dependency)
- Test: `crates/helm-device/src/tests/pci/accel.rs`

Wraps `helm_llvm::Accelerator` as a `PciFunction`. BAR0: control/status regs, BAR2: scratchpad, BAR4: MSI-X.

---

### Task 13: PCI DTB Node Generation

**Files:**
- Create: `crates/helm-device/src/pci/fdt.rs`
- Modify: `crates/helm-device/src/fdt.rs` (add PCI host bridge node in `device_to_fdt_node`)
- Test: `crates/helm-device/src/tests/pci/fdt.rs`

Generates `pcie@ECAM_BASE` node with `compatible = "pci-host-ecam-generic"`, `device_type = "pci"`, `ranges`, `bus-range`, `#address-cells = 3`, `#size-cells = 2`, `msi-parent`.

---

### Task 14: Python Device Factory — PCI Host & Attach

**Files:**
- Modify: `crates/helm-python/src/platform.rs` (add `PciHost` and `PciFunc` to `DeviceInner`, add `attach` method, add `pci-host` to factory)
- Test: `cargo test -p helm-python` + Python integration test

Add `"pci-host"` to `create_device()`. `PyDeviceHandle` gains `attach(slot, device)` for PCI host handles.

---

### Task 15: Python Device Factory — VirtIO transport kwarg & accel-pci

**Files:**
- Modify: `crates/helm-python/src/platform.rs` (virtio types gain `transport` kwarg, add `accel-pci` type)
- Test: `cargo test -p helm-python`

When `transport="pci"`, virtio create_device returns `DeviceInner::PciFunc(VirtioPciTransport::new(...))`. New `"accel-pci"` type accepts `ir_file` / `ir` + FU config kwargs.

---

### Task 16: Remove old proto/pci.rs

**Files:**
- Delete: `crates/helm-device/src/proto/pci.rs`
- Modify: `crates/helm-device/src/proto/mod.rs` (remove `pub mod pci;` and re-export)
- Modify: `crates/helm-device/src/tests/proto_buses.rs` (remove old PCI tests)
- Modify: `crates/helm-device/src/lib.rs` (update re-exports if any)

The old `PciBus`/`PciDevice` from `proto/pci.rs` are superseded. Search for any remaining references with `cargo check --workspace` and fix.

---

### Task 17: Workspace Build & Full Test Suite

**Step 1:** Run `cargo check --workspace`
**Step 2:** Run `cargo test --workspace`
**Step 3:** Fix any compilation errors or test failures
**Step 4:** Run `cargo clippy --workspace` and fix warnings
**Step 5:** Final commit

```bash
git add -u
git commit -m "feat(pci): complete PCI subsystem with VirtIO PCI transport and accelerator"
```

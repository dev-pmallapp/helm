//! Core PCI types and traits.
//!
//! Defines the fundamental abstractions for PCI devices:
//! - [`BarDecl`] — static BAR layout declaration
//! - [`PciCapability`] — a single PCI capability structure
//! - [`PciFunction`] — a complete PCI function (endpoint device)

use crate::device::DeviceEvent;

// ── BAR declaration ──────────────────────────────────────────────────────────

/// Static declaration of a single Base Address Register.
///
/// A device declares its BAR layout at construction time. The
/// [`PciConfigSpace`](super::config::PciConfigSpace) engine uses these
/// declarations to initialise type bits and the BAR-sizing protocol.
///
/// # Examples
///
/// ```
/// use helm_device::pci::BarDecl;
///
/// let bar = BarDecl::Mmio32 { size: 0x1000 };
/// assert_eq!(bar.size(), 0x1000);
/// assert!(!bar.is_unused());
/// assert!(!bar.is_64bit());
/// assert!(!bar.is_io());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarDecl {
    /// BAR is not implemented.
    Unused,
    /// 32-bit memory-mapped BAR.
    Mmio32 { size: u64 },
    /// 64-bit memory-mapped BAR (occupies two consecutive BAR slots).
    Mmio64 { size: u64 },
    /// I/O-space BAR.
    Io { size: u32 },
}

impl BarDecl {
    /// Returns the size of this BAR in bytes.
    ///
    /// Returns `0` for [`BarDecl::Unused`].
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::BarDecl;
    /// assert_eq!(BarDecl::Unused.size(), 0);
    /// assert_eq!(BarDecl::Mmio32 { size: 0x4000 }.size(), 0x4000);
    /// assert_eq!(BarDecl::Mmio64 { size: 0x10_0000 }.size(), 0x10_0000);
    /// assert_eq!(BarDecl::Io { size: 0x100 }.size(), 0x100);
    /// ```
    #[must_use]
    pub fn size(self) -> u64 {
        match self {
            Self::Unused => 0,
            Self::Mmio32 { size } => size,
            Self::Mmio64 { size } => size,
            Self::Io { size } => u64::from(size),
        }
    }

    /// Returns `true` if this BAR is unused.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::BarDecl;
    /// assert!(BarDecl::Unused.is_unused());
    /// assert!(!BarDecl::Mmio32 { size: 0x1000 }.is_unused());
    /// ```
    #[must_use]
    pub fn is_unused(self) -> bool {
        matches!(self, Self::Unused)
    }

    /// Returns `true` if this is a 64-bit memory BAR.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::BarDecl;
    /// assert!(BarDecl::Mmio64 { size: 0x1000 }.is_64bit());
    /// assert!(!BarDecl::Mmio32 { size: 0x1000 }.is_64bit());
    /// ```
    #[must_use]
    pub fn is_64bit(self) -> bool {
        matches!(self, Self::Mmio64 { .. })
    }

    /// Returns `true` if this is an I/O-space BAR.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::BarDecl;
    /// assert!(BarDecl::Io { size: 0x100 }.is_io());
    /// assert!(!BarDecl::Mmio32 { size: 0x1000 }.is_io());
    /// ```
    #[must_use]
    pub fn is_io(self) -> bool {
        matches!(self, Self::Io { .. })
    }
}

// ── PCI capability trait ─────────────────────────────────────────────────────

/// A single PCI capability structure.
///
/// PCI capabilities form a linked list in config space starting at the
/// capability pointer (offset 0x34). Standard capabilities start at
/// offsets below 0x100; extended capabilities (PCIe) start at 0x100.
pub trait PciCapability: Send + Sync {
    /// Capability ID (e.g. 0x05 = MSI, 0x10 = PCIe).
    fn cap_id(&self) -> u8;

    /// Absolute offset of this capability in config space.
    fn offset(&self) -> u16;

    /// Length of the capability structure in bytes.
    fn length(&self) -> u16;

    /// Read a 32-bit word at `offset` bytes from the start of this capability.
    fn read(&self, offset: u16) -> u32;

    /// Write a 32-bit word at `offset` bytes from the start of this capability.
    fn write(&mut self, offset: u16, value: u32);

    /// Reset capability state to power-on defaults.
    fn reset(&mut self);

    /// Human-readable capability name (e.g. `"MSI"`, `"PCIe"`).
    fn name(&self) -> &str;

    /// Returns `true` for PCIe extended capabilities (offset >= 0x100).
    ///
    /// The default implementation derives this from [`offset`](Self::offset).
    fn is_extended(&self) -> bool {
        self.offset() >= 0x100
    }
}

// ── PCI function trait ───────────────────────────────────────────────────────

/// A complete PCI function (type 0 endpoint device).
///
/// Implementors declare their identity, BAR layout, and capabilities.
/// The [`PciConfigSpace`](super::config::PciConfigSpace) engine uses this
/// information to build the standard type-0 config space header.
///
/// # Default implementations
///
/// - `subsystem_vendor_id()` → `0`
/// - `subsystem_id()` → `0`
/// - `revision_id()` → `0`
/// - `config_read()` → `0`
/// - `config_write()` → no-op
/// - `tick()` → empty event list
pub trait PciFunction: Send + Sync {
    // ── Identity fields ──────────────────────────────────────────────────

    /// PCI vendor identifier (16-bit).
    fn vendor_id(&self) -> u16;

    /// PCI device identifier (16-bit).
    fn device_id(&self) -> u16;

    /// 24-bit class code: [23:16] = base, [15:8] = sub, [7:0] = prog-if.
    fn class_code(&self) -> u32;

    /// Subsystem vendor ID (default 0).
    fn subsystem_vendor_id(&self) -> u16 {
        0
    }

    /// Subsystem device ID (default 0).
    fn subsystem_id(&self) -> u16 {
        0
    }

    /// Revision identifier (default 0).
    fn revision_id(&self) -> u8 {
        0
    }

    // ── BAR and capability layout ────────────────────────────────────────

    /// Static BAR declarations for all 6 BAR slots.
    fn bars(&self) -> &[BarDecl; 6];

    /// Capabilities attached to this function.
    fn capabilities(&self) -> &[Box<dyn PciCapability>];

    /// Mutable access to the capability list.
    fn capabilities_mut(&mut self) -> &mut Vec<Box<dyn PciCapability>>;

    // ── BAR access ───────────────────────────────────────────────────────

    /// Read from a BAR's memory region.
    ///
    /// `bar` is the BAR index (0–5), `offset` is the byte offset within the
    /// BAR window, `size` is the access size in bytes (1, 2, 4, or 8).
    fn bar_read(&self, bar: u8, offset: u64, size: usize) -> u64;

    /// Write to a BAR's memory region.
    fn bar_write(&mut self, bar: u8, offset: u64, size: usize, value: u64);

    // ── Device-specific config space extension ───────────────────────────

    /// Read from device-specific config space (offsets >= 0x40).
    ///
    /// The default implementation returns 0 for all offsets.
    fn config_read(&self, offset: u16) -> u32 {
        let _ = offset;
        0
    }

    /// Write to device-specific config space.
    ///
    /// The default implementation ignores all writes.
    fn config_write(&mut self, offset: u16, value: u32) {
        let _ = (offset, value);
    }

    // ── Lifecycle ────────────────────────────────────────────────────────

    /// Reset function to power-on state.
    fn reset(&mut self);

    /// Called periodically during simulation.
    ///
    /// Returns any events (IRQ assertions, DMA completions) the engine
    /// should route. The default returns an empty list.
    fn tick(&mut self, _cycles: u64) -> Vec<DeviceEvent> {
        vec![]
    }

    /// Human-readable function name.
    fn name(&self) -> &str;
}

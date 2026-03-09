//! PCI bus — topology manager with BDF-based device routing.
//!
//! [`PciBus`] owns all PCI functions attached to a single bus number and
//! dispatches config-space and BAR MMIO accesses by BDF address.
//!
//! # Architecture
//!
//! ```text
//! PciBus (bus_number)
//!   ├── slots: HashMap<(device, function), PciSlot>
//!   │     ├── function: Box<dyn PciFunction>
//!   │     └── config:   PciConfigSpace
//!   └── bar_mappings: Vec<BarMapping>   ← resolved platform addresses
//! ```
//!
//! BAR mappings are set by the host bridge after it allocates address space.
//! MMIO reads/writes use the mapping table to find the owning slot.

use std::collections::HashMap;

use crate::device::DeviceEvent;
use crate::pci::{Bdf, PciConfigSpace, PciFunction};

// ── Internal types ────────────────────────────────────────────────────────────

/// Resolved platform address for one BAR of one function.
#[derive(Debug, Clone)]
struct BarMapping {
    /// Absolute base address in platform MMIO space.
    base: u64,
    /// Size of the BAR window in bytes.
    size: u64,
    /// BAR index within the function (0–5).
    bar_index: u8,
    /// BDF of the owning function.
    bdf: Bdf,
}

/// One slot in the bus topology: a function plus its config-space engine.
struct PciSlot {
    function: Box<dyn PciFunction>,
    config: PciConfigSpace,
}

// ── PciBus ────────────────────────────────────────────────────────────────────

/// PCI bus — topology manager with BDF-based device routing.
///
/// Manages all PCI functions attached to a single bus number. Provides
/// config-space access (read/write) and BAR MMIO routing.
///
/// # Examples
///
/// ```
/// use helm_device::pci::{PciBus, Bdf};
///
/// let bus = PciBus::new(0);
/// assert!(bus.enumerate().is_empty());
/// ```
pub struct PciBus {
    bus_number: u8,
    /// `(device, function)` → slot.
    slots: HashMap<(u8, u8), PciSlot>,
    bar_mappings: Vec<BarMapping>,
}

impl PciBus {
    /// Create an empty bus with the given bus number.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::PciBus;
    ///
    /// let bus = PciBus::new(0);
    /// assert!(bus.enumerate().is_empty());
    /// ```
    #[must_use]
    pub fn new(bus_number: u8) -> Self {
        Self {
            bus_number,
            slots: HashMap::new(),
            bar_mappings: Vec::new(),
        }
    }

    /// Attach a PCI function to `(device, function)` on this bus.
    ///
    /// Builds a [`PciConfigSpace`] from the function's identity and BAR
    /// declarations. If a slot is already occupied it is replaced.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::{PciBus, Bdf};
    /// // (requires a concrete PciFunction impl — see integration tests)
    /// ```
    pub fn attach(&mut self, device: u8, function: u8, func: Box<dyn PciFunction>) {
        let config = PciConfigSpace::new(
            func.vendor_id(),
            func.device_id(),
            func.class_code(),
            func.revision_id(),
            func.bars(),
            func.capabilities(),
        );
        let slot = PciSlot { function: func, config };
        self.slots.insert((device & 0x1F, function & 0x07), slot);
    }

    /// Set the resolved platform address for one BAR of a function.
    ///
    /// This is called by the host bridge after it allocates address space.
    /// Replaces any existing mapping for the same `(bdf, bar_index)` pair.
    pub fn set_bar_mapping(&mut self, bdf: Bdf, bar_index: u8, base: u64, size: u64) {
        // Remove any existing mapping for this BDF+BAR.
        self.bar_mappings
            .retain(|m| !(m.bdf == bdf && m.bar_index == bar_index));
        if size > 0 {
            self.bar_mappings.push(BarMapping { base, size, bar_index, bdf });
        }
    }

    /// Return the base address of a BAR mapping, or `None` if not mapped.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::{PciBus, Bdf};
    ///
    /// let mut bus = PciBus::new(0);
    /// let bdf = Bdf::new(0, 1, 0);
    /// assert!(bus.bar_address(bdf, 0).is_none());
    /// ```
    #[must_use]
    pub fn bar_address(&self, bdf: Bdf, bar_index: u8) -> Option<u64> {
        self.bar_mappings
            .iter()
            .find(|m| m.bdf == bdf && m.bar_index == bar_index)
            .map(|m| m.base)
    }

    /// Read a 32-bit dword from the config space of the function at `bdf`.
    ///
    /// Returns `0xFFFF_FFFF` for empty slots (standard PCI absent-device
    /// response).
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::{PciBus, Bdf};
    ///
    /// let bus = PciBus::new(0);
    /// let bdf = Bdf::new(0, 5, 0);
    /// assert_eq!(bus.config_read(bdf, 0), 0xFFFF_FFFF);
    /// ```
    #[must_use]
    pub fn config_read(&self, bdf: Bdf, offset: u16) -> u32 {
        if bdf.bus != self.bus_number {
            return 0xFFFF_FFFF;
        }
        match self.slots.get(&(bdf.device, bdf.function)) {
            Some(slot) => slot.config.read(offset),
            None => 0xFFFF_FFFF,
        }
    }

    /// Write a 32-bit dword to the config space of the function at `bdf`.
    ///
    /// Silently ignored for empty slots.
    pub fn config_write(&mut self, bdf: Bdf, offset: u16, value: u32) {
        if bdf.bus != self.bus_number {
            return;
        }
        if let Some(slot) = self.slots.get_mut(&(bdf.device, bdf.function)) {
            slot.config.write(offset, value);
            // Mirror device-specific config writes to the function.
            if offset >= 0x40 {
                slot.function.config_write(offset, value);
            }
        }
    }

    /// Read from a BAR MMIO region by absolute address.
    ///
    /// Returns `None` if no mapped BAR contains `addr`.
    pub fn bar_read(&mut self, addr: u64, size: usize) -> Option<u64> {
        // Find matching mapping — clone needed fields to avoid borrow conflict.
        let (bdf, bar_index, offset) = self
            .bar_mappings
            .iter()
            .find_map(|m| {
                if addr >= m.base && addr < m.base + m.size {
                    Some((m.bdf, m.bar_index, addr - m.base))
                } else {
                    None
                }
            })?;

        let slot = self.slots.get_mut(&(bdf.device, bdf.function))?;
        Some(slot.function.bar_read(bar_index, offset, size))
    }

    /// Write to a BAR MMIO region by absolute address.
    ///
    /// Returns `true` if a mapped BAR was found and the write was dispatched,
    /// `false` otherwise.
    pub fn bar_write(&mut self, addr: u64, size: usize, value: u64) -> bool {
        let found = self
            .bar_mappings
            .iter()
            .find_map(|m| {
                if addr >= m.base && addr < m.base + m.size {
                    Some((m.bdf, m.bar_index, addr - m.base))
                } else {
                    None
                }
            });

        let Some((bdf, bar_index, offset)) = found else {
            return false;
        };

        if let Some(slot) = self.slots.get_mut(&(bdf.device, bdf.function)) {
            slot.function.bar_write(bar_index, offset, size, value);
            true
        } else {
            false
        }
    }

    /// Return all populated BDFs on this bus, sorted by `(device, function)`.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::{PciBus, Bdf};
    ///
    /// let bus = PciBus::new(0);
    /// assert!(bus.enumerate().is_empty());
    /// ```
    #[must_use]
    pub fn enumerate(&self) -> Vec<Bdf> {
        let mut keys: Vec<(u8, u8)> = self.slots.keys().copied().collect();
        keys.sort_unstable();
        keys.into_iter()
            .map(|(device, function)| Bdf {
                bus: self.bus_number,
                device,
                function,
            })
            .collect()
    }

    /// Tick all attached functions and collect emitted events.
    pub fn tick_all(&mut self, cycles: u64) -> Vec<DeviceEvent> {
        let mut events = Vec::new();
        for slot in self.slots.values_mut() {
            events.extend(slot.function.tick(cycles));
        }
        events
    }

    /// Reset all attached functions and their config-space engines.
    pub fn reset_all(&mut self) {
        for slot in self.slots.values_mut() {
            slot.function.reset();
        }
        // Clear BAR mappings so the host bridge can re-allocate.
        self.bar_mappings.clear();
    }
}
